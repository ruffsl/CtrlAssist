//! The `ctrlassist` command's argument tree, as a library so that the
//! documentation and completions can be generated from it.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// Controller assist: let several game controllers act as one, or one as
/// several.
#[derive(Parser, Debug)]
#[command(name = "ctrlassist", version, about)]
pub struct Cli {
    /// The daemon's socket, in place of the one in the user's runtime
    /// directory.
    #[arg(long, global = true)]
    pub socket: Option<PathBuf>,

    /// What to do.
    #[command(subcommand)]
    pub command: Command,
}

/// The verbs.
#[derive(Subcommand, Debug)]
pub enum Command {
    /// Say whether the daemon is running and which release it is.
    Status {
        /// Print one JSON object, and exit with status 1 if the daemon is
        /// missing or incompatible.
        #[arg(long)]
        json: bool,
    },
    /// Become one of CtrlAssist's long-running processes.
    #[command(subcommand)]
    Service(Service),
}

/// The long-running processes.
#[derive(Subcommand, Debug)]
pub enum Service {
    /// The daemon: owns the pads and virtual pads, and serves the clients.
    Daemon(ctrlassist_daemon::service::Args),
}
