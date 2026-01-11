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
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to listen for Ctrl+C");
        println!("\nShutting down gracefully...");
        let _ = ctrlc_tx.send(true);
    });

    println!("CtrlAssist system tray started");
    println!("Configure and control the mux from your system tray");
    println!("Press Ctrl+C to exit");

    // Wait for either shutdown signal or Ctrl+C
    tokio::select! {
        _ = shutdown_rx.changed() => {
            // Exit button was clicked
        }
        _ = ctrlc_rx.changed() => {
            // Ctrl+C was pressed, trigger shutdown through the tray
            handle.update(|tray: &mut CtrlAssistTray| {
                tray.shutdown();
            }).await;
        }
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
