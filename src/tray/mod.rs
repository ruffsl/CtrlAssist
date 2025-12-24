mod app;
mod config;
mod state;

pub use app::CtrlAssistTray;
pub use config::TrayConfig;
pub use state::{ControllerInfo, MuxStatus, TrayState};

use ksni::TrayMethods;
use std::error::Error;

pub fn run_tray() -> Result<(), Box<dyn Error>> {
    let tray = CtrlAssistTray::new()?;
    let handle = tray.spawn().map_err(|e| format!("Failed to spawn tray: {}", e))?;

    println!("CtrlAssist system tray started");
    println!("Configure and control the mux from your system tray");
    println!("Press Ctrl+C to exit");

    // Run forever
    std::thread::park();
    
    Ok(())
}
