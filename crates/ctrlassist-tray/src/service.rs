//! `ctrlassist service tray`: the tray's command line, run from the
//! `ctrlassist` binary.

use std::path::PathBuf;

/// The tray's arguments.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct Args {
    /// The daemon's socket, in place of the one in the user's runtime
    /// directory.
    #[arg(long)]
    pub socket: Option<PathBuf>,
}

/// Runs the tray until it is told to stop, on a runtime of its own.
///
/// # Errors
///
/// Fails if the runtime can't be built or the tray can't reach the
/// session bus.
pub fn main(args: Args) -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async {
            // journald when run as a service, stderr at a terminal;
            // RUST_LOG overrides these defaults.
            ctrlassist_daemon::logging::init(
                "ctrlassist-tray",
                &["ctrlassist_tray=info", "zbus=warn", "ksni=warn"],
            )?;
            crate::run(crate::TrayOptions {
                socket: args.socket.unwrap_or_else(ctrlassist::default_socket_path),
            })
            .await
        })
}
