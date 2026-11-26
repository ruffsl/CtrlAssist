//! D-Bus surface for the GUI binary.
//!
//! Two interfaces are served at `/io/github/ctrlassist/Gui` on the user bus
//! under the well-known name `io.github.ctrlassist.Gui`:
//!
//! - `io.github.ctrlassist.Gui1` — version + config_path introspection,
//!   as the daemon's and the tray's `info` rpcs say theirs, so external
//!   tooling can ask each part the same way.
//! - `org.freedesktop.Application` — single-instance activation per
//!   the freedesktop Desktop Entry D-Bus spec. Launchers (KRunner,
//!   the Plasma app menu, GNOME Shell, the tray's "Settings"
//!   button) call `Activate(platform_data)` to either auto-spawn
//!   the GUI (via the session `.service` file) or raise the
//!   existing window. We implement the three required methods —
//!   Activate, Open, ActivateAction — even though only Activate
//!   carries semantic weight today; Open and ActivateAction fall
//!   through to Activate so launchers that prefer them still work.
//!
//! The bus name is claimed *before* the Slint window is created.
//! On conflict — another GUI instance is running — the new process
//! proxies an Activate call to the existing owner and exits without
//! flashing a half-built window.

use anyhow::{Context, Result};
use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use tracing::{info, warn};
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::interface;
use zbus::names::WellKnownName;
use zbus::zvariant::{OwnedValue, Value};

use crate::AppWindow;

pub(crate) const GUI_BUS_NAME: &str = crate::APP_ID;
const GUI_OBJECT_PATH: &str = "/io/github/ctrlassist/Gui";
const APPLICATION_INTERFACE: &str = "org.freedesktop.Application";

/// Result of attempting to claim the GUI's well-known bus name.
///
/// The caller dispatches on this: [`Primary`](Self::Primary)
/// proceeds with normal startup (interfaces are already
/// registered by [`try_claim_and_serve`]); [`Proxied`](Self::Proxied)
/// means another instance already owns the name and was handed
/// the activation request — the caller should return from `run()`.
pub(crate) enum ClaimResult {
    /// This process won the bus name. Hold onto the connection for
    /// the lifetime of the GUI — dropping it tears down the name
    /// claim and the object server.
    Primary(zbus::Connection),
    /// Another instance owns the name; an `Activate` call has
    /// already been proxied to it. Exit cleanly.
    Proxied,
    /// Another instance owns the name and the `Activate` handed to it
    /// failed: it is hung, or isn't a window that answers. There is a
    /// bus, so this is not the case of none; a second window isn't
    /// opened beside the one that holds the name.
    Unanswered(anyhow::Error),
}

/// Connect to the user bus, register the `Gui1` and
/// `org.freedesktop.Application` interfaces, *then* claim
/// `io.github.ctrlassist.Gui` with `DO_NOT_QUEUE` semantics.
///
/// **Order matters.** KDE's KIO launcher (`DBusActivationRunner`
/// in `kio/src/gui/dbusactivationrunner.cpp`) calls
/// `org.freedesktop.Application.Activate` on our bus name as part
/// of cold-spawn. `dbus-daemon` queues that call waiting for the
/// well-known name to be claimed. If we claim the name first and
/// register the interfaces later, the queued call is delivered
/// during the gap — and `zbus` drops it (per its own warning,
/// "Method calls arriving before interfaces are registered may
/// be lost"). The launcher times out at 25 s waiting for a reply,
/// surfacing as "Did not receive a reply" / "Remote peer
/// disconnected" notifications. Registering interfaces first
/// ensures the queued call hits a ready handler the moment the
/// name is claimed.
///
/// The returned [`Activation`] handle is the cross-thread wiring
/// between the D-Bus interface methods (which run on zbus worker
/// tasks) and the Slint main thread:
///
/// - `cold_spawn_token` carries the first inbound Activate's
///   `activation-token` to the `BackendSelector::with_winit_window_
///   attributes_hook` closure (read at `AppWindow::new()`).
/// - `window` is populated by the caller after `AppWindow::new()`
///   returns; subsequent re-activation handlers read it to call
///   `winit::Window::request_user_attention` for the taskbar
///   pulse.
///
/// In the `Proxied` path (another instance owns the name), the
/// registered interfaces sit on a connection we're about to drop
/// — unused but cheap. The activation handle is still returned
/// but never used.
pub(crate) async fn try_claim_and_serve(
    config_path: Option<PathBuf>,
) -> Result<(ClaimResult, Activation)> {
    let connection = zbus::Connection::session()
        .await
        .context("Failed to connect to user D-Bus")?;

    claim_on_connection(connection, config_path).await
}

async fn claim_on_connection(
    connection: zbus::Connection,
    config_path: Option<PathBuf>,
) -> Result<(ClaimResult, Activation)> {
    let gui_service = GuiService::new(config_path);
    let (app_service, activation) = ApplicationService::new();

    connection
        .object_server()
        .at(GUI_OBJECT_PATH, gui_service)
        .await
        .context("Failed to serve Gui1 interface")?;
    connection
        .object_server()
        .at(GUI_OBJECT_PATH, app_service)
        .await
        .context("Failed to serve org.freedesktop.Application interface")?;

    let name = WellKnownName::try_from(GUI_BUS_NAME).expect("static bus name is well-formed");
    let reply = match connection
        .request_name_with_flags(name, RequestNameFlags::DoNotQueue.into())
        .await
    {
        Ok(reply) => reply,
        Err(zbus::Error::NameTaken) => RequestNameReply::Exists,
        Err(error) => return Err(error).context("Failed to request io.github.ctrlassist.Gui"),
    };

    match reply {
        RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => {
            info!(
                bus_name = GUI_BUS_NAME,
                path = GUI_OBJECT_PATH,
                "GUI D-Bus interfaces registered and bus name claimed"
            );
            Ok((ClaimResult::Primary(connection), activation))
        }
        RequestNameReply::Exists | RequestNameReply::InQueue => {
            info!(
                bus_name = GUI_BUS_NAME,
                "Another ctrlassist-gui instance is running; proxying Activate and exiting"
            );
            match proxy_activate(&connection).await {
                Ok(()) => Ok((ClaimResult::Proxied, activation)),
                Err(error) => Ok((ClaimResult::Unanswered(error), activation)),
            }
        }
    }
}

/// Gives up the window's name, and the connection that claimed it, as
/// the window closes: a launch after this claims the name itself and
/// opens a window, where one handed over to this process would find no
/// window to raise.
pub(crate) async fn release(connection: zbus::Connection) {
    let name = WellKnownName::try_from(GUI_BUS_NAME).expect("static bus name is well-formed");
    if let Err(error) = connection.release_name(name).await {
        warn!(%error, "the window's name on the session bus wasn't released");
    }
}

/// Hand off to the existing GUI owner: build a proxy and call
/// `Activate(a{sv})` with empty `platform_data`. The receiving
/// instance dispatches through [`ApplicationService::raise`],
/// which pulses the taskbar via `request_user_attention` — no
/// token from us needed, since the existing instance owns the
/// surface that issues the xdg-activation request.
async fn proxy_activate(connection: &zbus::Connection) -> Result<()> {
    let proxy = zbus::Proxy::new(
        connection,
        GUI_BUS_NAME,
        GUI_OBJECT_PATH,
        APPLICATION_INTERFACE,
    )
    .await
    .context("Failed to build Application proxy")?;

    let platform_data: HashMap<String, Value<'_>> = HashMap::new();
    let _: () = proxy
        .call("Activate", &(platform_data,))
        .await
        .context("Failed to call Activate on existing GUI instance")?;
    Ok(())
}

// ----------------------------------------------------------------
// io.github.ctrlassist.Gui1 — the window's release and file, as the
// daemon's and the tray's `info` rpcs say theirs.
// ----------------------------------------------------------------

pub(crate) struct GuiService {
    config_path: Option<PathBuf>,
}

impl GuiService {
    pub(crate) fn new(config_path: Option<PathBuf>) -> Self {
        Self { config_path }
    }
}

#[interface(name = "io.github.ctrlassist.Gui1")]
impl GuiService {
    /// Compile-time version of the running GUI binary.
    #[zbus(property(emits_changed_signal = "const"))]
    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    /// Where the window keeps its preferences, `gui.toml`; empty when
    /// it wasn't given one.
    #[zbus(property(emits_changed_signal = "const"))]
    fn config_path(&self) -> String {
        self.config_path
            .as_ref()
            .and_then(|p| p.to_str())
            .unwrap_or_default()
            .to_string()
    }
}

// ----------------------------------------------------------------
// org.freedesktop.Application — Desktop Entry D-Bus activation.
// ----------------------------------------------------------------
//
// Spec: https://specifications.freedesktop.org/desktop-entry/latest/dbus.html
//
// Activate / Open / ActivateAction all extract the optional
// `platform_data["activation-token"]` and dispatch through
// `ApplicationService::raise`, which routes on whether the
// AppWindow exists yet (cold-spawn vs re-activation). Full design
// rationale lives in docs/kde/gui-application-activation.md.

/// Cross-thread state shared between the zbus interface handlers
/// (worker threads) and the Slint main thread. Cloneable — both
/// sides hold their own [`Arc`]-cloned handles.
#[derive(Clone)]
pub(crate) struct Activation {
    /// First-Activate `activation-token`, captured by the handler
    /// before the AppWindow exists. Read once by the
    /// `BackendSelector::with_winit_window_attributes_hook`
    /// closure at `AppWindow::new()` time, applied to the
    /// surface via `WindowAttributesExtStartupNotify::
    /// with_activation_token`. Cleared by the hook on `take()`.
    pub(crate) cold_spawn_token: Arc<Mutex<Option<String>>>,
    /// `AppWindow` weak reference, set by the caller once
    /// `AppWindow::new()` returns. Read by `raise()` on re-
    /// activations to post a `request_user_attention` via
    /// `slint::invoke_from_event_loop`. Empty until set —
    /// re-activations arriving before this point fall into
    /// the cold-spawn branch (token stashed for the hook).
    pub(crate) window: Arc<OnceLock<slint::Weak<AppWindow>>>,
}

impl Activation {
    fn new() -> Self {
        Self {
            cold_spawn_token: Arc::new(Mutex::new(None)),
            window: Arc::new(OnceLock::new()),
        }
    }
}

pub(crate) struct ApplicationService {
    activation: Activation,
}

impl ApplicationService {
    pub(crate) fn new() -> (Self, Activation) {
        let activation = Activation::new();
        (
            Self {
                activation: activation.clone(),
            },
            activation,
        )
    }
}

#[interface(name = "org.freedesktop.Application")]
impl ApplicationService {
    fn activate(&self, platform_data: HashMap<String, OwnedValue>) {
        let token = extract_activation_token(&platform_data);
        info!(
            method = "Activate",
            token_present = token.is_some(),
            "Application call received"
        );
        self.raise(token);
    }

    /// We register no MIME types and don't handle URIs today;
    /// treat as a plain activation so launchers that prefer
    /// `Open` still bring the window forward.
    fn open(&self, _uris: Vec<String>, platform_data: HashMap<String, OwnedValue>) {
        let token = extract_activation_token(&platform_data);
        info!(
            method = "Open",
            token_present = token.is_some(),
            "Application call received"
        );
        self.raise(token);
    }

    /// The `.desktop` entry declares no `Actions` yet; required by
    /// the spec to implement the method.
    fn activate_action(
        &self,
        action_name: String,
        _parameter: Vec<OwnedValue>,
        platform_data: HashMap<String, OwnedValue>,
    ) {
        let token = extract_activation_token(&platform_data);
        info!(
            method = "ActivateAction",
            action = %action_name,
            token_present = token.is_some(),
            "Application call received"
        );
        self.raise(token);
    }
}

impl ApplicationService {
    fn raise(&self, activation_token: Option<String>) {
        // Dispatch on AppWindow existence. Full design rationale,
        // KWin FSP mapping, and the `gdbus` smoke-test caveat are
        // in docs/kde/gui-application-activation.md.
        //
        // - **Re-activation path** (window exists): post to the
        //   Slint event loop and call
        //   `request_user_attention(Informational)` via
        //   `WinitWindowAccessor::with_winit_window`. The
        //   `activation_token` from `platform_data` is ignored —
        //   winit issues its own `Attention`-flagged xdg-
        //   activation-v1 token bound to our surface.
        //
        // - **Cold-spawn path** (window not yet constructed):
        //   stash the inbound `activation-token` in
        //   `Activation::cold_spawn_token`. The
        //   `BackendSelector::with_winit_window_attributes_hook`
        //   in `lib.rs` reads it at `AppWindow::new()` time and
        //   applies it via `with_activation_token`. Multiple
        //   cold-spawn Activates overwrite the slot; most recent
        //   wins at hook time.
        if let Some(weak) = self.activation.window.get() {
            let weak = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = weak.upgrade() {
                    use slint::winit_030::winit::window::UserAttentionType;
                    app.window().with_winit_window(|w| {
                        w.request_user_attention(Some(UserAttentionType::Informational));
                    });
                    // A remote user re-opening the window sees where
                    // they are: focus the active pane's sidebar item,
                    // unless something already has focus.
                    if app.global::<crate::Couch>().get_active() {
                        app.invoke_focus_on_reactivation();
                    }
                }
            });
            return;
        }
        if let Some(token) = activation_token
            && let Ok(mut guard) = self.activation.cold_spawn_token.lock()
        {
            *guard = Some(token);
        }
    }
}

fn extract_activation_token(platform_data: &HashMap<String, OwnedValue>) -> Option<String> {
    let value = platform_data.get("activation-token")?.clone();
    String::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_tokens_require_a_string_and_absent_tokens_preserve_the_slot() {
        let (service, activation) = ApplicationService::new();
        service.raise(Some("kept".into()));
        service.activate(HashMap::new());
        service.activate(HashMap::from([(
            "activation-token".into(),
            OwnedValue::from(42u32),
        )]));
        assert_eq!(
            activation.cold_spawn_token.lock().unwrap().as_deref(),
            Some("kept")
        );
    }

    /// A private `dbus-daemon`, killed when dropped.
    struct PrivateBus {
        child: std::process::Child,
        address: String,
    }

    impl Drop for PrivateBus {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// A bus of the test's own; none where `dbus-daemon` isn't on PATH,
    /// which the dev shell doesn't carry.
    fn private_bus() -> Option<PrivateBus> {
        use std::io::BufRead as _;
        let mut child = std::process::Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--print-address=1"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let mut address = String::new();
        let stdout = child.stdout.take()?;
        std::io::BufReader::new(stdout)
            .read_line(&mut address)
            .ok()?;
        Some(PrivateBus {
            child,
            address: address.trim().to_string(),
        })
    }

    /// The name held by something that doesn't answer an `Activate` is
    /// a window that is there, not a missing bus: the launch ends, where
    /// it used to open a second window.
    #[tokio::test]
    async fn a_name_held_by_one_that_doesnt_answer_is_not_a_missing_bus() {
        let Some(bus) = private_bus() else {
            return;
        };
        let connect = || async {
            zbus::connection::Builder::address(bus.address.as_str())
                .expect("dbus-daemon printed an address")
                .build()
                .await
                .expect("the private bus takes a connection")
        };
        // The name, with an object at the window's path that has no
        // `org.freedesktop.Application`: an `Activate` fails at once.
        let holder = connect().await;
        holder
            .object_server()
            .at(GUI_OBJECT_PATH, GuiService::new(None))
            .await
            .expect("the object is served");
        holder
            .request_name(GUI_BUS_NAME)
            .await
            .expect("the name is free on a bus of the test's own");

        let (claimed, _) = claim_on_connection(connect().await, None)
            .await
            .expect("a bus is there");
        assert!(matches!(claimed, ClaimResult::Unanswered(_)));
    }

    /// A window that is closing gives up its name before it waits for
    /// anything, and a launch after that claims the name and opens a
    /// window of its own.
    #[tokio::test]
    async fn a_window_that_has_released_its_name_leaves_it_to_the_next_launch() {
        let Some(bus) = private_bus() else {
            return;
        };
        let connect = || async {
            zbus::connection::Builder::address(bus.address.as_str())
                .expect("dbus-daemon printed an address")
                .build()
                .await
                .expect("the private bus takes a connection")
        };
        let (first, _) = claim_on_connection(connect().await, None)
            .await
            .expect("a bus is there");
        let ClaimResult::Primary(first) = first else {
            unreachable!("the name is free on a bus of the test's own");
        };
        // Kept alive, as a window still waiting on its writers keeps the
        // process and its connection: the name is gone all the same.
        let kept = first.clone();
        release(first).await;
        let (next, _) = claim_on_connection(connect().await, None)
            .await
            .expect("a bus is there");
        assert!(matches!(next, ClaimResult::Primary(_)));
        drop(kept);
    }
}
