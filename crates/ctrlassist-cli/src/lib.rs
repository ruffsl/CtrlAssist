//! The `ctrlassist` command's argument tree, as a library so that the
//! documentation and completions can be generated from it. The sources
//! of completion values live here too, because the derive attributes
//! that name them do.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Parser, Subcommand};
use clap_complete::engine::{ArgValueCandidates, CompletionCandidate};
use ctrlassist::{DaemonClient, StateEvent, StateWatcher, StateWatcherConfig};

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
        #[arg(add = pad_completions())]
        pad: String,
        /// How long to buzz, in milliseconds.
        #[arg(long, value_name = "MS", default_value_t = 600)]
        millis: u32,
    },
    /// Stop remembering a pad that isn't connected.
    Forget {
        /// The pad, named as for `identify`.
        #[arg(add = pad_completions())]
        pad: String,
    },
    /// Run a kept profile: its pads become its virtual pads.
    Start {
        /// The profile's name.
        #[arg(add = profile_completions())]
        profile: String,
    },
    /// Stop a setup: its virtual pads go.
    Stop {
        /// The setup's name: the profile it runs.
        #[arg(add = setup_completions())]
        setup: String,
    },
    /// Have a running setup take up its profile as it is kept now.
    Update {
        /// The setup's name.
        #[arg(add = setup_completions())]
        setup: String,
        /// Only say what the update would cost; change nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Have a node of a setup select another input or output.
    Select {
        /// The setup's name.
        #[arg(add = setup_completions())]
        setup: String,
        /// The node's name in the profile; `setups` lists the ones that
        /// select.
        node: String,
        /// What to select: one of the node's choices as `setups` lists
        /// them, or its place among them counted from 0. Left out, the
        /// node moves to its next choice, as its switch button would.
        choice: Option<String>,
    },
    /// Have the daemon read its files again, for edits made by hand,
    /// and bring the running setups up to them.
    Reload,
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
        #[arg(add = profile_completions())]
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
        #[arg(add = profile_completions())]
        name: String,
    },
}

/// The long-running processes.
#[derive(Subcommand, Debug)]
pub enum Service {
    /// The daemon: owns the pads and virtual pads, and serves the clients.
    Daemon(ctrlassist_daemon::service::Args),
    /// The system tray: what is running, start and stop, quick start,
    /// and notifications.
    Tray(ctrlassist_tray::service::Args),
}

/// Hard ceiling on the daemon round trip a `<TAB>` triggers, so a wedged
/// daemon can't hang the shell. An absent socket fails fast well under
/// this; the budget only bites when the daemon is reachable but silent.
/// TisVlay's, with its completions.
const COMPLETION_BUDGET: Duration = Duration::from_millis(600);

// Value-candidate sources, one per kind of argument. Each is wrapped in
// its own helper so a churn in the unstable completion API is a single
// edit, not one per call site. They use the default socket because
// completers run before clap parses the arguments, so `--socket` isn't
// known to them.

/// The pads the daemon knows, by id.
fn pad_completions() -> ArgValueCandidates {
    ArgValueCandidates::new(|| candidates(&default_socket(), pads))
}

/// The profiles the daemon keeps, by name.
fn profile_completions() -> ArgValueCandidates {
    ArgValueCandidates::new(|| candidates(&default_socket(), profiles))
}

/// The setups that exist, by name.
fn setup_completions() -> ArgValueCandidates {
    ArgValueCandidates::new(|| candidates(&default_socket(), setups))
}

fn default_socket() -> PathBuf {
    ctrlassist::default_socket_path()
}

fn pads(client: &DaemonClient) -> Vec<CompletionCandidate> {
    let pads = client.list_pads().unwrap_or_default();
    pads.into_iter()
        .map(|pad| {
            let about = if pad.connected {
                pad.name
            } else {
                format!("{} (not connected)", pad.name)
            };
            CompletionCandidate::new(pad.id).help(Some(printable(&about).into()))
        })
        .collect()
}

fn profiles(client: &DaemonClient) -> Vec<CompletionCandidate> {
    let profiles = client.list_profiles().unwrap_or_default();
    profiles
        .into_iter()
        .map(|profile| {
            let candidate = CompletionCandidate::new(profile.name);
            match profile.problem {
                Some(problem) => candidate.help(Some(printable(&problem.to_string()).into())),
                None => candidate,
            }
        })
        .collect()
}

fn setups(client: &DaemonClient) -> Vec<CompletionCandidate> {
    let setups = client.list_setups().unwrap_or_default();
    setups
        .into_iter()
        .map(|setup| CompletionCandidate::new(setup.name).help(Some(setup.phase.as_str().into())))
        .collect()
}

/// A pad's or a profile's own text, made safe to show: it is whatever
/// its author wrote, and a terminal obeys control characters.
#[must_use]
pub fn printable(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                '?'
            } else {
                character
            }
        })
        .collect()
}

/// Reads candidates from the daemon on `socket`, on a runtime of its own:
/// completers run before any runtime exists. Any failure (no daemon, a
/// wedged daemon, the budget spent) yields no candidates rather than
/// holding the prompt.
fn candidates(
    socket: &Path,
    read: fn(&DaemonClient) -> Vec<CompletionCandidate>,
) -> Vec<CompletionCandidate> {
    blocking(async {
        let client = hydrated(socket).await?;
        Some(read(&client))
    })
}

fn blocking(
    query: impl Future<Output = Option<Vec<CompletionCandidate>>>,
) -> Vec<CompletionCandidate> {
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    else {
        return Vec::new();
    };
    runtime
        .block_on(async { tokio::time::timeout(COMPLETION_BUDGET, query).await })
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// A client whose mirror holds all the daemon on `socket` has published,
/// or nothing if no daemon listens there or it goes away.
async fn hydrated(socket: &Path) -> Option<DaemonClient> {
    // A session that doesn't keep dialing, so no daemon is known at once.
    let config = ctrlassist_protocol::bootstrap::client_config(socket).ok()?;
    let session = zenoh::open(config).await.ok()?;
    let client = DaemonClient::with_session(session).await.ok()?;
    let mut config = StateWatcherConfig::default();
    config.fallback_poll_interval = Duration::ZERO;
    let mut watcher = StateWatcher::new(&client, None, config);
    while !client.is_hydrated() {
        if matches!(
            watcher.next().await,
            Some(StateEvent::DaemonDisappeared) | None
        ) {
            return None;
        }
    }
    Some(client)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    /// clap's self-check: catches conflicting argument ids, and exercises
    /// the `#[arg(add = …)]` value-candidate wiring for well-formedness.
    #[test]
    fn the_argument_tree_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    /// A socket that can never connect stands in for "no daemon": the
    /// link fails fast, so completion yields nothing without approaching
    /// the budget, and the prompt is never held.
    #[test]
    fn candidates_are_none_and_prompt_without_a_daemon() {
        let dead = Path::new("/nonexistent/ctrlassist/completion.sock");
        let start = Instant::now();
        for read in [pads, profiles, setups] {
            assert!(candidates(dead, read).is_empty());
        }
        assert!(
            start.elapsed() < COMPLETION_BUDGET,
            "completion should fail fast on an unreachable daemon"
        );
    }

    #[test]
    fn text_from_a_pad_cannot_drive_the_terminal() {
        assert_eq!(printable("a\u{1b}[31mb\nc"), "a?[31mb?c");
    }
}
