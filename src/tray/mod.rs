use ashpd::is_sandboxed;
use futures_util::TryFutureExt;
use ksni::TrayMethods;
use std::error::Error;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::watch;

pub mod app;
pub mod config;
pub mod state;

pub use app::CtrlAssistTray;

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

    // Create a separate shutdown channel for Ctrl+C or SIGTERM
    let (ctrlc_tx, mut ctrlc_rx) = watch::channel(false);

    tokio::spawn(async move {
        // Wait for either SIGINT or SIGTERM
        let mut sigint =
            signal(SignalKind::interrupt()).expect("Failed to register SIGINT handler");
        let mut sigterm =
            signal(SignalKind::terminate()).expect("Failed to register SIGTERM handler");
        tokio::select! {
            _ = sigint.recv() => {
                let _ = ctrlc_tx.send(true);
            }
            _ = sigterm.recv() => {
                let _ = ctrlc_tx.send(true);
            }
        }
    });

    println!("CtrlAssist system tray started");
    println!("Configure and control the mux from your system tray");
    println!("Press Ctrl+C to exit");

    // Wait for either shutdown signal or Ctrl+C/SIGTERM
    tokio::select! {
        _ = shutdown_rx.changed() => {
            // Exit button clicked, tray handled shutdown
        }
        _ = ctrlc_rx.changed() => {
            // Ctrl+C or SIGTERM, handle shutdown here
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
