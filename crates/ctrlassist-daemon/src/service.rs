//! `service daemon`: the process becomes the daemon.

use std::path::PathBuf;

use anyhow::{Context, Result};
use ctrlassist_device::InputThread;
use tokio::signal::unix::{SignalKind, signal};
use tracing::info;

use crate::{Daemon, Paths};

/// The daemon's arguments.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct Args {
    /// The socket to listen on, in place of the one in the user's runtime
    /// directory.
    #[arg(long)]
    pub socket: Option<PathBuf>,
    /// The directory for what the daemon remembers between runs, in place
    /// of the one in the user's state directory.
    #[arg(long, value_name = "DIRECTORY")]
    pub state_dir: Option<PathBuf>,
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
    let defaults = Paths::default_for_user();
    let paths = Paths {
        socket: args.socket.unwrap_or(defaults.socket),
        state: args.state_dir.unwrap_or(defaults.state),
    };
    let daemon = Daemon::start(&paths, input).await?;

    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    tokio::select! {
        _ = interrupt.recv() => info!("interrupted"),
        _ = terminate.recv() => info!("asked to stop"),
    }
    daemon.stop().await;
    Ok(())
}
