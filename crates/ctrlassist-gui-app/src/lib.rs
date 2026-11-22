//! ctrlassist-gui: the settings window, in Slint.
//!
//! The UI itself is compiled in `ctrlassist-gui`; this crate drives it.
//! So far that is the shell: a sidebar and its panes, empty, steered by
//! keys alone, with couch mode for a TV across the room.

pub mod about;
mod couch;
pub mod diagnostic;
mod file_manager;
mod icon_theme;
mod icons;
pub mod keys;
mod models;
pub mod nav;
pub mod pads;
pub mod profiles;
pub mod setups;
pub mod state;
mod toast;

use anyhow::{Context, Result};
use clap::Parser;
use slint::{ComponentHandle, SharedString};
use tracing::{debug, info, warn};

pub use ctrlassist_gui_ui::*;

/// Marshal a UI-touching closure onto the Slint event loop. Clones the
/// weak ref internally so callers can pass it by reference; if the
/// `AppWindow` has been dropped by the time the closure runs, it is
/// silently skipped.
pub(crate) fn on_ui<F>(weak: &slint::Weak<AppWindow>, f: F)
where
    F: FnOnce(AppWindow) + Send + 'static,
{
    let weak = weak.clone();
    slint::invoke_from_event_loop(move || {
        if let Some(app) = weak.upgrade() {
            f(app);
        }
    })
    .ok();
}

/// The application id: the Wayland app id, and the name of the desktop
/// entry that launches the window.
pub const APP_ID: &str = "io.github.ctrlassist.Gui";

#[derive(Parser, Debug)]
#[command(name = "ctrlassist-gui", version, about)]
struct Args {
    /// The daemon's socket (default: $XDG_RUNTIME_DIR/ctrlassist/zenoh.sock).
    #[arg(long)]
    socket: Option<std::path::PathBuf>,
}

/// Process entry point. The `ctrlassist-gui` binary's `main()` is a thin
/// shim that calls this; the integration tests reach the types behind
/// this library crate directly.
pub fn run() -> Result<()> {
    ctrlassist_daemon::logging::init("ctrlassist-gui", &["ctrlassist_gui=info", "slint=warn"])?;

    let args = Args::parse();
    let socket = args
        .socket
        .unwrap_or_else(ctrlassist_protocol::bootstrap::default_socket_path);
    info!(socket = %socket.display(), "ctrlassist-gui starting");

    // Slint's event loop owns the main thread once `run()` is called. A
    // tokio runtime backs the work beside it; the guard lets a Slint
    // callback on the main thread spawn onto it.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("couldn't build the tokio runtime")?;
    let _enter = rt.enter();

    // Couch mode: the GUI's own preference file, read before the window
    // exists since it decides how the window opens.
    let gui_config_path = couch::default_config_path();
    let couch_prefs = couch::load(&gui_config_path);
    let couch_override = std::env::var(couch::ENV_OVERRIDE).ok();
    let session_signals = couch::SessionSignals::from_env();
    let couch_active = couch::active(
        couch_prefs.mode,
        couch_override.as_deref(),
        &session_signals,
    );
    info!(
        couch = couch_active,
        mode = couch_prefs.mode.as_str(),
        "couch mode decided"
    );

    // Selected explicitly so `set_xdg_app_id` has a backend to register
    // the id against; it also starts Slint's MCP server when the binary
    // is built with `--features slint/mcp` and `SLINT_MCP_PORT` is set
    // (`just gui-start`; inert otherwise).
    slint::BackendSelector::new()
        .select()
        .context("couldn't initialize the Slint backend")?;
    // Before the window shows: winit reads the app id as it builds the
    // first window, and the compositor matches the window to the
    // desktop entry by it.
    slint::set_xdg_app_id(APP_ID).context("couldn't set the Wayland app id")?;

    let app = AppWindow::new()?;
    let nav = nav::wire(&app);
    icons::load(&app);

    // The daemon: connected to whether or not it is there yet, and
    // followed from then on.
    let daemon = rt
        .block_on(ctrlassist::DaemonClient::connect(&socket))
        .context("couldn't open a session for the daemon's socket")?;
    state::wire_reload(&app, &daemon);
    about::wire(&app, &daemon);
    pads::wire(&app, &daemon, &nav);
    setups::wire(&app, &daemon, &nav);
    profiles::wire(&app, &daemon, &nav);
    rt.spawn(state::follow(daemon, app.as_weak()));

    keys::bind_key_router(&app);
    // Escape on the sidebar with nothing to go back to, in couch mode:
    // the window closes and the process ends, as Bigscreen's apps do.
    app.global::<Nav>().on_close(|| {
        if let Err(e) = slint::quit_event_loop() {
            warn!(error = %e, "close requested with no event loop");
        }
    });

    // Couch mode's window side. The `Couch` global carries the decision
    // and the preference to the panes; the Settings pane's edits come
    // back through its callbacks and are saved and applied at once.
    // The scale is relative: every compositor scale event is multiplied
    // by the preference and re-dispatched, and the window's first size
    // puts it on for the scale the window was created with, which
    // bypasses the hook. The raw value is kept for later changes, and
    // read from the live window once there is one.
    let compositor_scale = std::rc::Rc::new(std::cell::Cell::new(1.0f32));
    // The file is hand-editable: its scale is held to the Settings
    // control's bounds.
    let couch_prefs = {
        let couch = app.global::<Couch>();
        couch_prefs.sanitized(couch.get_scale_min_percent(), couch.get_scale_max_percent())
    };
    debug!(scale = couch_prefs.scale, "couch scale");
    let prefs = std::rc::Rc::new(std::cell::Cell::new(couch_prefs));
    // One writer, so a held arrow's run of scale changes lands in order
    // and the exit below can wait for the last one.
    let writer = std::rc::Rc::new(couch::PrefsWriter::spawn(gui_config_path, couch_prefs));
    {
        let couch = app.global::<Couch>();
        couch.set_active(couch_active);
        couch.set_mode(couch_prefs.mode.as_str().into());
        couch.set_scale_percent((couch_prefs.scale * 100.0).round() as i32);

        let app_weak = app.as_weak();
        let prefs_for_mode = prefs.clone();
        let writer_for_mode = writer.clone();
        let scale_for_mode = compositor_scale.clone();
        let signals = session_signals;
        let env_override = couch_override;
        couch.on_mode_changed(move |text: SharedString| {
            let Some(mode) = couch::CouchMode::parse(text.as_str()) else {
                return;
            };
            let mut current = prefs_for_mode.get();
            current.mode = mode;
            prefs_for_mode.set(current);
            writer_for_mode.send(current);
            if let Some(app) = app_weak.upgrade() {
                let active = couch::active(mode, env_override.as_deref(), &signals);
                let couch = app.global::<Couch>();
                couch.set_mode(mode.as_str().into());
                couch.set_active(active);
                apply_scale(
                    &app,
                    compositor_scale_now(&app, &scale_for_mode),
                    active.then_some(current.scale),
                );
            }
        });

        let app_weak = app.as_weak();
        let scale_for_pref = compositor_scale.clone();
        let writer_for_pref = writer.clone();
        couch.on_scale_changed(move |percent| {
            let mut current = prefs.get();
            current.scale = percent as f32 / 100.0;
            prefs.set(current);
            writer_for_pref.send(current);
            if let Some(app) = app_weak.upgrade() {
                let couch = app.global::<Couch>();
                couch.set_scale_percent(percent);
                apply_scale(
                    &app,
                    compositor_scale_now(&app, &scale_for_pref),
                    couch.get_active().then_some(current.scale),
                );
            }
        });
    }

    // Mouse back/forward buttons → history navigation, handled at the
    // winit layer so the side buttons work anywhere in the window
    // without a full-window TouchArea swallowing clicks. Gated on
    // `nav-enabled`, matching the Alt+Left/Right FocusScope. The same hook
    // sees the compositor's scale events for couch mode's relative
    // scale.
    {
        use slint::winit_030::winit::event::{ElementState, MouseButton, WindowEvent};
        use slint::winit_030::{EventResult, WinitWindowAccessor};

        let app_weak = app.as_weak();
        let back = nav.back.clone();
        let forward = nav.forward.clone();
        app.window()
            .on_winit_window_event(move |_window, event| match event {
                WindowEvent::MouseInput {
                    state: ElementState::Pressed,
                    button,
                    ..
                } => {
                    if let Some(app) = app_weak.upgrade()
                        && app.get_nav_enabled()
                    {
                        match button {
                            MouseButton::Back => back(&app),
                            MouseButton::Forward => forward(&app),
                            _ => {}
                        }
                    }
                    EventResult::Propagate
                }
                WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                    compositor_scale.set(*scale_factor as f32);
                    match app_weak.upgrade() {
                        Some(app) if app.global::<Couch>().get_active() => {
                            let couch = app.global::<Couch>();
                            let factor = couch.get_scale_percent() as f32 / 100.0;
                            apply_scale(&app, compositor_scale.get(), Some(factor));
                            EventResult::PreventDefault
                        }
                        _ => EventResult::Propagate,
                    }
                }
                // Slint creates the window when the loop starts and
                // sets its scale straight from winit, past this hook, so
                // the first size is where couch mode's scale goes on
                // top; a compositor that then sends a scale event of its
                // own (Wayland) takes the arm above as well. Idempotent:
                // a later resize finds the scale in place.
                WindowEvent::Resized(_) => {
                    if let Some(app) = app_weak.upgrade()
                        && app.global::<Couch>().get_active()
                    {
                        // The hook's `window` is Slint's, whose scale is
                        // the one already applied: the compositor's own
                        // comes from the winit window underneath.
                        let compositor = compositor_scale_now(&app, &compositor_scale);
                        let factor = app.global::<Couch>().get_scale_percent() as f32 / 100.0;
                        let wanted = couch::window_scale(compositor, Some(factor));
                        if couch::scale_is_stale(app.window().scale_factor(), wanted) {
                            apply_scale(&app, compositor, Some(factor));
                        }
                    }
                    EventResult::Propagate
                }
                // A key the backend would drop (a gamepad's Share, L2 or
                // Square; a remote's Exit or Info): dispatch the key it
                // means instead.
                WindowEvent::KeyboardInput { event, .. }
                    if !event.repeat && keys::remap_dropped_key(&event.logical_key).is_some() =>
                {
                    if let Some(app) = app_weak.upgrade()
                        && let Some(key) = keys::remap_dropped_key(&event.logical_key)
                    {
                        let text: SharedString = key.into();
                        if event.state == ElementState::Pressed {
                            watch_focus_after(&app);
                            app.window()
                                .dispatch_event(slint::platform::WindowEvent::KeyPressed { text });
                        } else {
                            app.window()
                                .dispatch_event(slint::platform::WindowEvent::KeyReleased { text });
                        }
                    }
                    EventResult::PreventDefault
                }
                // A navigation key with nothing focused reaches no
                // handler; check after the dispatch and restore focus.
                WindowEvent::KeyboardInput { event, .. }
                    if event.state == ElementState::Pressed
                        && !event.repeat
                        && is_navigation_key(&event.logical_key) =>
                {
                    if let Some(app) = app_weak.upgrade() {
                        watch_focus_after(&app);
                    }
                    EventResult::Propagate
                }
                _ => EventResult::Propagate,
            });
    }

    // Open maximized in couch mode, with focus on the active sidebar
    // item so the ring shows from the first frame. The scale goes on
    // once the window exists (the hook's `Resized` arm): there is none
    // before the loop runs, and a scale set now would be replaced by
    // the compositor's at creation.
    if couch_active {
        app.window().set_maximized(true);
    }
    app.show()?;
    if couch_active {
        app.invoke_focus_active_sidebar_item();
    }

    // Slint's event loop blocks the main thread; tokio's workers keep
    // driving tasks. `rt` drops at the end of this function, aborting
    // any that remain.
    app.run()?;

    // A preference changed just before the window closed may still be
    // on its way to the disk: wait for the writer to reach the last
    // change, briefly.
    if !rt.block_on(writer.flush(std::time::Duration::from_secs(2))) {
        warn!("couch preference write didn't finish before exit");
    }

    info!("ctrlassist-gui shutting down");
    Ok(())
}

/// Note the shell's key count now and, once the key about to be
/// dispatched has run, restore focus if the shell never saw it.
fn watch_focus_after(app: &AppWindow) {
    let seen = keys::key_seen(app);
    let weak = app.as_weak();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        if let Some(app) = weak.upgrade()
            && keys::recover_if_unhandled(&app, seen)
        {
            info!("focus was lost; restored to its column");
        }
    });
}

/// The keys a remote navigates with, whose arrival with no focus means
/// focus was lost.
fn is_navigation_key(key: &slint::winit_030::winit::keyboard::Key) -> bool {
    use slint::winit_030::winit::keyboard::{Key, NamedKey};
    matches!(
        key,
        Key::Named(
            NamedKey::ArrowUp
                | NamedKey::ArrowDown
                | NamedKey::ArrowLeft
                | NamedKey::ArrowRight
                | NamedKey::Tab
                | NamedKey::Enter
                | NamedKey::Escape
        )
    )
}

/// The compositor's own scale for the window: what winit reports once
/// the window exists (Slint creates it when the event loop starts),
/// else the last scale event seen.
fn compositor_scale_now(app: &AppWindow, last: &std::cell::Cell<f32>) -> f32 {
    use slint::winit_030::WinitWindowAccessor;
    if let Some(scale) = app.window().with_winit_window(|w| w.scale_factor() as f32) {
        last.set(scale);
    }
    last.get()
}

/// Set the window's scale to the compositor's times couch mode's
/// factor, or back to the compositor's own when the factor is `None`.
fn apply_scale(app: &AppWindow, compositor: f32, factor: Option<f32>) {
    let scale_factor = couch::window_scale(compositor, factor);
    app.window()
        .dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor });
}
