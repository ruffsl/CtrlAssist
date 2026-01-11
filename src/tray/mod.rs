use futures_util::TryFutureExt;

pub mod app;
pub mod config;
pub mod state;

pub use app::CtrlAssistTray;

use ashpd::is_sandboxed;
use ksni::TrayMethods;
use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub async fn run_tray() -> Result<(), Box<dyn Error>> {
    let tray = CtrlAssistTray::new()?;

    let is_sandboxed = is_sandboxed().await;

    let handle = if is_sandboxed {
        tray.disable_dbus_name(true)
            .spawn()
            .map_err(|e| format!("Failed to spawn tray (sandbox workaround): {}", e))
            .await?
    } else {
        tray.spawn()
            .map_err(|e| format!("Failed to spawn tray: {}", e))
            .await?
    };

    // Set up Ctrl+C handler
    let shutdown_signal = Arc::new(AtomicBool::new(false));
    let shutdown_signal_ctrlc = Arc::clone(&shutdown_signal);

    ctrlc::set_handler(move || {
        println!("\nShutting down gracefully...");
        shutdown_signal_ctrlc.store(true, Ordering::SeqCst);
    })?;

    // Store handle and shutdown signal in tray
    let handle_clone = handle.clone();
    let shutdown_signal_clone = Arc::clone(&shutdown_signal);
    handle
        .update(|tray: &mut CtrlAssistTray| {
            tray.tray_handle = Some(handle_clone);
            tray.shutdown_signal = Some(shutdown_signal_clone);
        })
        .await;

    println!("CtrlAssist system tray started");
    println!("Configure and control the mux from your system tray");
    println!("Press Ctrl+C to exit");

    // Wait for shutdown signal (from either Ctrl+C or Exit button)
    while !shutdown_signal.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Perform cleanup
    println!("Stopping operations...");
    handle
        .update(|tray: &mut CtrlAssistTray| {
            tray.stop_operation();
        })
        .await;

    println!("Shutting down tray...");
    handle.shutdown().await;

    println!("Cleanup complete, exiting.");
    Ok(())
}
