use futures_util::TryFutureExt;
mod app;
mod config;
mod state;

pub use app::CtrlAssistTray;

use ksni::TrayMethods;
use std::error::Error;

pub async fn run_tray() -> Result<(), Box<dyn Error>> {
    let tray = CtrlAssistTray::new()?;
    let handle = tray
        .spawn()
        .map_err(|e| format!("Failed to spawn tray: {}", e))
        .await?;

    println!("CtrlAssist system tray started");
    println!("Configure and control the mux from your system tray");
    println!("Press Ctrl+C to exit");

    // Run forever
    std::thread::park();

    Ok(())
}
