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
    /// Run a kept profile: its pads become its virtual pads.
    Start {
        /// The profile's name.
        profile: String,
    },
    /// Stop a setup: its virtual pads go.
    Stop {
        /// The setup's name: the profile it runs.
        setup: String,
    },
    /// Have a running setup take up its profile as it is kept now.
    Update {
        /// The setup's name.
        setup: String,
        /// Only say what the update would cost; change nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Have a node of a setup select another input or output.
    Select {
        /// The setup's name.
        setup: String,
        /// The node's name in the profile; `setups` lists the ones that
        /// select.
        node: String,
        /// What to select: one of the node's choices as `setups` lists
        /// them, or its place among them counted from 0. Left out, the
        /// node moves to its next choice, as its switch button would.
        choice: Option<String>,
    },
    /// List the setups that exist, with their pads and virtual pads.
    Setups {
        /// Print a JSON array, one object per setup.
        #[arg(long)]
        json: bool,
    },
    /// List, show, check, keep and delete profiles.
    #[command(subcommand)]
    Profile(Profile),
    /// Become one of CtrlAssist's long-running processes.
    #[command(subcommand)]
    Service(Service),
}

/// What to do with profiles.
#[derive(Subcommand, Debug)]
pub enum Profile {
    /// List the profiles the daemon keeps.
    List {
        /// Print a JSON array, one object per profile.
        #[arg(long)]
        json: bool,
    },
    /// Print a profile as the TOML it was written as.
    Show {
        /// The profile's name.
        name: String,
    },
    /// Check a profile file, and say where it is wrong. Nothing is kept.
    Validate {
        /// The profile, as a TOML file.
        file: PathBuf,
    },
    /// Have the daemon keep a profile file: a new profile, or in place
    /// of the one that has the name.
    Put {
        /// The profile, as a TOML file.
        file: PathBuf,
        /// The profile's name, in place of the file's name without its
        /// `.toml`: lowercase letters, digits and hyphens.
        #[arg(long)]
        name: Option<String>,
    },
    /// Have the daemon stop keeping a profile.
    Delete {
        /// The profile's name.
        name: String,
    },
}

/// The long-running processes.
#[derive(Subcommand, Debug)]
pub enum Service {
    /// The daemon: owns the pads and virtual pads, and serves the clients.
    Daemon(ctrlassist_daemon::service::Args),
}
