//! The About pane: its rows, one per part of CtrlAssist, and what its
//! links open.
//!
//! The rows are made from what the window already shows of the daemon
//! (its health and the hiding service's, its release, where it keeps
//! the user's files) and the release of the `ctrlassist` on PATH, so
//! they can never disagree with the sidebar's dots.

use std::path::Path;

use ctrlassist::DaemonClient;
use slint::{ComponentHandle, Model as _, ModelRc, SharedString, VecModel};
use tracing::warn;

use crate::toast::show_toast;
use crate::{AboutComponentRow, AppWindow, Clipboard, ServiceHealth, diagnostic, on_ui};

/// This window's release, shown in the header's chip, and the one each
/// part's health is judged against.
pub(crate) const GUI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// The project's repository, from the workspace's package metadata.
/// The brand link opens it; Report issue opens its `/issues`, the list
/// and not `/new`, so a report already made is seen before another is.
pub(crate) const REPO_URL: &str = env!("CARGO_PKG_REPOSITORY");

// Cargo sets it to an empty string when the manifest declares no
// repository, which would turn every link here into a file path.
const _: () = assert!(
    !REPO_URL.is_empty(),
    "ctrlassist-gui-app's manifest must declare `repository`"
);

/// The third-party license bundle as this build had it: what
/// `build.rs` copied from the tree, for a build with no installed copy
/// to open.
const NOTICES_HTML: &str = include_str!(concat!(env!("OUT_DIR"), "/THIRD-PARTY-LICENSES.html"));

/// Where the packages install the bundle.
const NOTICES_HTML_FHS: &str = "/usr/share/doc/ctrlassist/THIRD-PARTY-LICENSES.html";

/// An installed copy of the bundle, if there is one: at the usual
/// path, or under the prefix this binary runs from
/// (`<prefix>/bin/ctrlassist-gui`, `<prefix>/share/doc/ctrlassist`),
/// which covers a Nix store path and an install under `/usr/local`.
fn installed_notices_path() -> Option<std::path::PathBuf> {
    let fhs = std::path::PathBuf::from(NOTICES_HTML_FHS);
    if fhs.exists() {
        return Some(fhs);
    }
    let exe = std::env::current_exe().ok()?;
    let prefix = exe.parent()?.parent()?;
    let beside = prefix.join("share/doc/ctrlassist/THIRD-PARTY-LICENSES.html");
    beside.exists().then_some(beside)
}

/// Opens the bundle in the user's browser: the installed copy, or the
/// embedded one written to a file made for it.
fn open_notices_html() -> anyhow::Result<()> {
    if let Some(installed) = installed_notices_path() {
        webbrowser::open(&installed.to_string_lossy())?;
        return Ok(());
    }
    // Made with a random name and exclusively, so nothing can be put in
    // its place first; kept, since the browser reads it after this
    // returns. The system clears its temporary directory in its own
    // time.
    use std::io::Write as _;
    let mut file = tempfile::Builder::new()
        .prefix("ctrlassist-third-party-licenses-")
        .suffix(".html")
        .tempfile()?;
    file.write_all(NOTICES_HTML.as_bytes())?;
    let (_, path) = file.keep()?;
    webbrowser::open(&path.to_string_lossy())?;
    Ok(())
}

/// What the rows are made from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parts {
    /// The daemon's health, as the sidebar shows it.
    pub daemon: ServiceHealth,
    /// Its release, once it has said.
    pub daemon_version: String,
    /// Where it keeps the user's files; empty when it hasn't said.
    pub daemon_config: String,
    /// Whether the kernel gave its input thread real-time scheduling.
    pub daemon_realtime: bool,
    /// The hiding service's health, as the daemon finds it.
    pub hider: ServiceHealth,
    /// The tray's health, as the daemon reports it.
    pub tray: ServiceHealth,
    /// Its release, when one is connected.
    pub tray_version: String,
    /// The release of the `ctrlassist` on PATH; empty for none found.
    pub cli_version: String,
}

impl Parts {
    /// Reads them off the window.
    #[must_use]
    pub fn of(app: &AppWindow) -> Self {
        Self {
            daemon: app.get_daemon_health(),
            daemon_version: app.get_daemon_version().to_string(),
            daemon_config: app.get_daemon_config().to_string(),
            daemon_realtime: app.get_daemon_realtime(),
            hider: app.get_hider_health(),
            tray: app.get_tray_health(),
            tray_version: app.get_tray_version().to_string(),
            cli_version: app.get_cli_version().to_string(),
        }
    }
}

/// A row's cells that differ by part.
struct Cells {
    version: String,
    health: ServiceHealth,
    chip: String,
    tip: String,
    config_path: String,
}

/// Why a version chip is the color it is, for a part judged by its
/// release against this window's.
fn release_tip(health: ServiceHealth, theirs: &str) -> String {
    match health {
        ServiceHealth::Offline => "Not running".to_string(),
        ServiceHealth::Ok if theirs.is_empty() => "Release not read yet".to_string(),
        ServiceHealth::Ok => "Same release as this window".to_string(),
        ServiceHealth::Skew => format!(
            "Runs {theirs}, this window {GUI_VERSION}: compatible, only the patch release differs"
        ),
        _ => format!(
            "Runs {theirs}, this window {GUI_VERSION}: the two can't talk. Restart it or upgrade so they match"
        ),
    }
}

/// The About pane's rows: the daemon, the hiding service, the tray,
/// this window and the command line, in that order.
#[must_use]
pub fn rows(parts: &Parts) -> Vec<AboutComponentRow> {
    let online = |health: ServiceHealth| health != ServiceHealth::Offline;
    let daemon = Cells {
        version: match (online(parts.daemon), parts.daemon_version.as_str()) {
            (false, _) => String::new(),
            (true, "") => "—".to_string(),
            (true, version) => version.to_string(),
        },
        health: parts.daemon,
        chip: match (online(parts.daemon), parts.daemon_version.as_str()) {
            (false, _) => "offline".to_string(),
            (true, "") => "—".to_string(),
            (true, version) => format!("v{version}"),
        },
        tip: release_tip(parts.daemon, &parts.daemon_version),
        config_path: parts.daemon_config.clone(),
    };
    // The hiding service has no release a client can read and no files
    // of the user's: it is there, hiding pads, or it isn't.
    let hider = Cells {
        version: String::new(),
        health: parts.hider,
        chip: match (online(parts.daemon), online(parts.hider)) {
            (false, _) => "unknown".to_string(),
            (true, true) => "hiding".to_string(),
            (true, false) => "not running".to_string(),
        },
        tip: match (online(parts.daemon), online(parts.hider)) {
            (false, _) => "Only the daemon can say, and it isn't running".to_string(),
            (true, true) => "A setup's pads are hidden from games".to_string(),
            (true, false) => {
                "A setup's pads are only grabbed: a program that reads a pad some other way, as Steam reads a PlayStation pad, still sees it".to_string()
            }
        },
        config_path: String::new(),
    };
    // The tray is the daemon's to report: one client can't see another.
    let tray = Cells {
        version: parts.tray_version.clone(),
        health: parts.tray,
        chip: match (online(parts.daemon), online(parts.tray)) {
            (false, _) => "unknown".to_string(),
            (true, false) => "offline".to_string(),
            (true, true) => format!("v{}", parts.tray_version),
        },
        tip: match (online(parts.daemon), online(parts.tray)) {
            (false, _) => "Only the daemon can say, and it isn't running".to_string(),
            (true, _) => release_tip(parts.tray, &parts.tray_version),
        },
        config_path: String::new(),
    };
    let window = Cells {
        version: GUI_VERSION.to_string(),
        health: ServiceHealth::Ok,
        chip: format!("v{GUI_VERSION}"),
        tip: "This window's release".to_string(),
        config_path: crate::couch::default_config_path().display().to_string(),
    };
    // The command never talks to the window; both ship in one package,
    // so another release means the one on PATH is another install's.
    let cli_health = match parts.cli_version.as_str() {
        "" => ServiceHealth::Offline,
        GUI_VERSION => ServiceHealth::Ok,
        theirs if ctrlassist_protocol::compatible(GUI_VERSION, theirs) => ServiceHealth::Skew,
        _ => ServiceHealth::Incompatible,
    };
    let cli = Cells {
        version: parts.cli_version.clone(),
        health: cli_health,
        chip: match parts.cli_version.as_str() {
            "" => "not found".to_string(),
            version => format!("v{version}"),
        },
        tip: match cli_health {
            ServiceHealth::Offline => "ctrlassist not found on PATH".to_string(),
            ServiceHealth::Ok => "Same release as this window".to_string(),
            _ => format!(
                "From a different install: {}, this window {GUI_VERSION}",
                parts.cli_version
            ),
        },
        config_path: String::new(),
    };
    let realtime = if online(parts.daemon) && !parts.daemon_realtime {
        " Without real-time scheduling, which the kernel refused it: latency under load isn't dependable."
    } else {
        ""
    };
    [
        (
            "daemon",
            "Daemon",
            ctrlassist_protocol::keys::daemon_alive(),
            format!("Reads the pads, runs the setups and keeps the profiles.{realtime}"),
            daemon,
        ),
        (
            "hider",
            "Hiding",
            String::new(),
            "Runs as root to make a setup's pads absent to games.".to_string(),
            hider,
        ),
        (
            "tray",
            "Tray",
            String::new(),
            "The tray icon, its menu and the notifications.".to_string(),
            tray,
        ),
        (
            "gui",
            "GUI",
            crate::APP_ID.to_string(),
            "This window.".to_string(),
            window,
        ),
        (
            "cli",
            "Command line",
            // Nothing in parens: the row then has no folder button, and
            // the command has no files of its own to show.
            String::new(),
            "Everything here, from a terminal or a script.".to_string(),
            cli,
        ),
    ]
    .into_iter()
    .map(
        |(name, friendly, identity, description, cells)| AboutComponentRow {
            name: SharedString::from(name),
            friendly_name: SharedString::from(friendly),
            bus_address: SharedString::from(if identity.is_empty() {
                String::new()
            } else {
                format!("({identity})")
            }),
            description: SharedString::from(description),
            version: SharedString::from(cells.version),
            online: cells.health != ServiceHealth::Offline,
            health: cells.health,
            chip: SharedString::from(cells.chip),
            tip: SharedString::from(cells.tip),
            config_path: SharedString::from(cells.config_path),
        },
    )
    .collect()
}

/// Makes the rows afresh from what the window shows now.
pub fn refresh(app: &AppWindow) {
    app.set_about_components(ModelRc::new(VecModel::from(rows(&Parts::of(app)))));
}

/// The release of the `ctrlassist` on PATH, from `ctrlassist --version`
/// (`ctrlassist 0.5.0`); `None` when there is none, it fails, or says
/// something else.
async fn probe_cli_version() -> Option<String> {
    let output = tokio::process::Command::new("ctrlassist")
        .arg("--version")
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let said = String::from_utf8(output.stdout).ok()?;
    said.split_whitespace().nth(1).map(str::to_string)
}

fn open(what: &str, url: &str) {
    if let Err(error) = webbrowser::open(url) {
        warn!(%error, url, "couldn't open {what}");
    }
}

/// The diagnostic bundle of this moment: the pane's rows, what the
/// mirror holds of the daemon, and facts of this machine.
fn diagnostic_bundle(app: &AppWindow, daemon: &DaemonClient) -> String {
    let parts = app
        .get_about_components()
        .iter()
        .map(|row| diagnostic::Part {
            name: row.name.to_string(),
            version: row.version.to_string(),
            running: row.online,
            chip: row.chip.to_string(),
        })
        .collect();
    diagnostic::bundle(&diagnostic::Inputs {
        parts,
        daemon: daemon.daemon_state().ok().flatten(),
        pads: daemon.list_pads().unwrap_or_default(),
        setups: daemon.list_setups().unwrap_or_default(),
        profiles: daemon.list_profiles().unwrap_or_default(),
        machine: diagnostic::Machine::read(),
    })
}

/// Binds the clipboard every Copy button goes through: the text onto
/// the Wayland clipboard, then a toast naming what was copied.
fn wire_clipboard(app: &AppWindow) {
    let weak = app.as_weak();
    app.global::<Clipboard>().on_copy(move |value, label| {
        use wl_clipboard_rs::copy::{MimeType, Options, Source};
        let source = Source::Bytes(value.as_bytes().to_vec().into_boxed_slice());
        if let Err(error) = Options::new().copy(source, MimeType::Text) {
            warn!(%error, "the clipboard couldn't be written");
            return;
        }
        let message = if label.is_empty() {
            "Copied to clipboard.".to_string()
        } else {
            format!("Copied {label}.")
        };
        show_toast(weak.clone(), &message);
    });
}

/// Sets the pane's fixed parts, binds its links and buttons, and looks
/// for the command line once.
pub(crate) fn wire(app: &AppWindow, daemon: &DaemonClient) {
    wire_clipboard(app);
    let (weak, client) = (app.as_weak(), daemon.clone());
    app.on_copy_diagnostic_clicked(move || {
        if let Some(app) = weak.upgrade() {
            let bundle = diagnostic_bundle(&app, &client);
            app.global::<Clipboard>().invoke_copy(
                SharedString::from(bundle),
                SharedString::from("diagnostic info"),
            );
        }
    });
    app.set_gui_version(SharedString::from(GUI_VERSION));
    refresh(app);
    app.on_view_source_clicked(|| open("the repository", REPO_URL));
    app.on_view_release_clicked(|| {
        open(
            "the release notes",
            &format!("{REPO_URL}/releases/tag/v{GUI_VERSION}"),
        );
    });
    app.on_report_issue_clicked(|| open("the issue tracker", &format!("{REPO_URL}/issues")));
    app.on_view_notices_clicked(|| {
        if let Err(error) = open_notices_html() {
            warn!(%error, "couldn't open the third-party notices");
        }
    });
    app.on_open_config_folder(|path| {
        let path = path.to_string();
        tokio::spawn(async move {
            let revealed = match zbus::Connection::session().await {
                Ok(bus) => crate::file_manager::reveal(&bus, Path::new(&path)).await,
                Err(error) => Err(error.into()),
            };
            if let Err(error) = revealed {
                warn!(%error, path, "couldn't show the folder");
            }
        });
    });
    let weak = app.as_weak();
    tokio::spawn(async move {
        let version = probe_cli_version().await.unwrap_or_default();
        on_ui(&weak, move |app| {
            app.set_cli_version(SharedString::from(version));
            refresh(&app);
        });
    });
}
