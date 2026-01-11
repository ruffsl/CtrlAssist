use futures_util::TryFutureExt;

pub mod app;
pub mod config;
pub mod state;

pub use app::CtrlAssistTray;

use ashpd::is_sandboxed;
use ksni::TrayMethods;
use std::error::Error;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

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

    // Set up async shutdown channel (shared for Ctrl+C and tray Exit)
    let (tx, shutdown_rx) = oneshot::channel::<()>();
    let shutdown_tx: Arc<Mutex<Option<tokio::sync::oneshot::Sender<()>>>> =
        Arc::new(Mutex::new(Some(tx)));

    // Set up Ctrl+C handler
    let shutdown_tx_ctrlc = Arc::clone(&shutdown_tx);
    ctrlc::set_handler(move || {
        println!("\nShutting down gracefully...");
        if let Some(tx) = shutdown_tx_ctrlc.lock().unwrap().take() {
            let _ = tx.send(());
        }
    })?;

    // Store handle and shutdown sender in tray
    let handle_clone = handle.clone();
    let shutdown_tx_tray = Arc::clone(&shutdown_tx);
    handle
        .update(|tray: &mut CtrlAssistTray| {
            tray.tray_handle = Some(handle_clone);
            tray.shutdown_tx = Some(Box::new(move || {
                if let Some(tx) = shutdown_tx_tray.lock().unwrap().take() {
                    let _ = tx.send(());
                }
            }));
        })
        .await;

    println!("CtrlAssist system tray started");
    println!("Configure and control the mux from your system tray");
    println!("Press Ctrl+C to exit");

    // Wait for shutdown signal (from either Ctrl+C or Exit button)
    let _ = shutdown_rx.await;

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
