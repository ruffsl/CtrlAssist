//! `service daemon`: the process becomes the daemon.

use std::path::PathBuf;

use anyhow::{Context, Result};
use ctrlassist_device::InputThread;
use tokio::signal::unix::{SignalKind, signal};
use tracing::info;

use crate::Daemon;

/// The daemon's arguments.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct Args {
    /// The socket to listen on, in place of the one in the user's runtime
    /// directory.
    #[arg(long)]
    pub socket: Option<PathBuf>,
}

/// Runs the daemon until it is told to stop, on a runtime of its own.
///
/// # Errors
///
/// Fails if the input thread, the runtime or the daemon can't start.
pub fn main(args: Args) -> Result<()> {
    // Before the runtime: the input thread shares nothing with it but
    // its queues, and asks for real-time scheduling for itself alone.
    let (input, reports) = InputThread::spawn().context("starting the input thread")?;
    // Nothing runs a setup yet, so nothing is reported; the receiver is
    // held so the thread's reports have somewhere to go.
    let _reports = reports;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(args, input))
}

async fn run(args: Args, input: InputThread) -> Result<()> {
    crate::logging::init(
        "ctrlassist-daemon",
        &["ctrlassist_daemon=info", "ctrlassist_protocol=info"],
    )?;
    let socket = args
        .socket
        .unwrap_or_else(ctrlassist_protocol::bootstrap::default_socket_path);
    let daemon = Daemon::start(&socket, input).await?;

    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        _ = interrupt.recv() => info!("interrupted"),
        _ = terminate.recv() => info!("asked to stop"),
    }
    daemon.stop().await;
    Ok(())
}
