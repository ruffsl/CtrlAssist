//! ctrlassist-gui: the settings window, in Slint.
//!
//! The UI itself is compiled in `ctrlassist-gui`; this crate drives it.
//! So far that is the shell: a sidebar and its panes, empty.

use anyhow::{Context, Result};
use clap::Parser;
use slint::ComponentHandle;
use tracing::info;

pub use ctrlassist_gui_ui::*;

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
    wire_navigation(&app);
    app.run()?;
    info!("ctrlassist-gui shutting down");
    Ok(())
}

/// Pane changes go through `navigate`, the one place they are made.
pub fn wire_navigation(app: &AppWindow) {
    let weak = app.as_weak();
    app.on_navigate(move |pane| {
        if let Some(app) = weak.upgrade() {
            app.set_active_pane(pane);
        }
    });
}
