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
    /// List the pads the daemon knows: those connected, and those it
    /// remembers.
    Pads {
        /// Print a JSON array, one object per pad.
        #[arg(long)]
        json: bool,
    },
    /// List what a virtual pad can present itself as.
    Identities {
        /// Print a JSON array, one object per identity.
        #[arg(long)]
        json: bool,
    },
    /// Buzz a pad, to tell which one it is.
    Identify {
        /// The pad: its id as `pads` prints it, a bare address, or
        /// `model:<vendor>:<product>` when one known pad is of that model.
        pad: String,
        /// How long to buzz, in milliseconds.
        #[arg(long, value_name = "MS", default_value_t = 600)]
        millis: u32,
    },
    /// Stop remembering a pad that isn't connected.
    Forget {
        /// The pad, named as for `identify`.
        pad: String,
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
