use futures_util::TryFutureExt;

pub mod app;
pub mod config;
pub mod state;

pub use app::CtrlAssistTray;

use ashpd::is_sandboxed;
use ksni::TrayMethods;
use std::error::Error;
use tokio::sync::watch;

pub async fn run_tray() -> Result<(), Box<dyn Error>> {
    let (tray, mut shutdown_rx) = CtrlAssistTray::new()?;

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

    // Create a separate shutdown channel for Ctrl+C
    let (ctrlc_tx, mut ctrlc_rx) = watch::channel(false);

    tokio::spawn(async move {
        // Wait for Ctrl+C signal
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                let _ = ctrlc_tx.send(true);
            }
            Err(e) => {
                eprintln!("Failed to listen for Ctrl+C: {}", e);
            }
        }
    });

    println!("CtrlAssist system tray started");
    println!("Configure and control the mux from your system tray");
    println!("Press Ctrl+C to exit");

    // Wait for either shutdown signal or Ctrl+C
    tokio::select! {
        _ = shutdown_rx.changed() => {
            // Exit button was clicked
            handle.update(|tray: &mut CtrlAssistTray| {
                tray.stop_operation();
            }).await;
        }
        _ = ctrlc_rx.changed() => {
            // Ctrl+C was pressed
            handle.update(|tray: &mut CtrlAssistTray| {
                tray.shutdown();
            }).await;
        }
    }

    println!("\nShutting down tray...");
    handle.shutdown().await;

    println!("Cleanup complete, exiting.");
    Ok(())
}
