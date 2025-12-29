use futures_util::TryFutureExt;
mod app;
mod config;
mod state;

pub use app::CtrlAssistTray;

use ashpd::is_sandboxed;
use ksni::TrayMethods;
use std::error::Error;

pub async fn run_tray() -> Result<(), Box<dyn Error>> {
    let tray = CtrlAssistTray::new()?;

    // Use ashpd for robust sandbox detection
    let is_sandboxed = is_sandboxed().await;

    let handle_result = if is_sandboxed {
        tray.disable_dbus_name(true)
            .spawn()
            .map_err(|e| format!("Failed to spawn tray (sandbox workaround): {}", e))
            .await
    } else {
        tray.spawn()
            .map_err(|e| format!("Failed to spawn tray: {}", e))
            .await
    };

    handle_result?;

    println!("CtrlAssist system tray started");
    println!("Configure and control the mux from your system tray");
    println!("Press Ctrl+C to exit");

    // Run forever
    std::thread::park();

    Ok(())
}
