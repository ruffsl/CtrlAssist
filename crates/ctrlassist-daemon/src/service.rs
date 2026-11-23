//! `service daemon`: the process becomes the daemon.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use std::sync::mpsc::Receiver;

use ctrlassist_device::{InputThread, Report};
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
    /// The directory the profiles are kept under, in place of the one in
    /// the user's configuration directory.
    #[arg(long, value_name = "DIRECTORY")]
    pub config_dir: Option<PathBuf>,
    /// The hiding service's socket, in place of the installed one.
    #[arg(long, value_name = "SOCKET")]
    pub hider_socket: Option<PathBuf>,
    /// Take the tests' fake pads for pads to play with, which a daemon
    /// in use doesn't. Also asked for by setting `CTRLASSIST_TEST_PADS`.
    #[arg(long, hide = true)]
    pub test_pads: bool,
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
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(args, input, reports))
}

async fn run(args: Args, input: InputThread, reports: Receiver<Report>) -> Result<()> {
    crate::logging::init(
        "ctrlassist-daemon",
        &["ctrlassist_daemon=info", "ctrlassist_protocol=info"],
    )?;
    let defaults = Paths::default_for_user();
    let paths = Paths {
        socket: args.socket.unwrap_or(defaults.socket),
        state: args.state_dir.unwrap_or(defaults.state),
        config: args.config_dir.unwrap_or(defaults.config),
        hider: args.hider_socket.unwrap_or(defaults.hider),
        test_pads: args.test_pads || std::env::var_os("CTRLASSIST_TEST_PADS").is_some(),
        steam: defaults.steam,
    };
    // A daemon that takes the tests' pads isn't one in use, and leaves
    // Steam's files alone.
    let paths = Paths {
        steam: if paths.test_pads {
            Vec::new()
        } else {
            paths.steam
        },
        ..paths
    };
    let mut daemon = Daemon::start(&paths, input, reports).await?;

    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;
    let ended = tokio::select! {
        _ = interrupt.recv() => { info!("interrupted"); false }
        _ = terminate.recv() => { info!("asked to stop"); false }
        () = daemon.ended() => true,
    };
    daemon.stop().await;
    // Exiting with an error has the service manager start it afresh.
    if ended {
        bail!("the daemon stopped serving, and has stopped");
    }
    Ok(())
}
