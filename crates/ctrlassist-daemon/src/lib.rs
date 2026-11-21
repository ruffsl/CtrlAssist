//! CtrlAssist's daemon: the control side.
//!
//! It owns the input thread and serves the control plane on a Unix socket
//! in the user's runtime directory. So far it answers the version check
//! and publishes its own state; pads, profiles and setups follow.

mod book;
mod hotplug;
pub mod logging;
mod reactor;
pub mod service;
mod store;

use std::fs::OpenOptions;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use ctrlassist_device::InputThread;
use ctrlassist_protocol::endpoint::{daemon, pad};
use ctrlassist_protocol::profile::Identity;
use ctrlassist_protocol::{VERSION, bootstrap, keys, payload};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{info, warn};
use zenoh::Session;
use zenoh::liveliness::LivelinessToken;

use book::Book;
use hotplug::Watch;
use reactor::Reactor;
use store::Store;

/// Where a daemon keeps what it needs.
#[derive(Clone, Debug)]
pub struct Paths {
    /// The socket it listens on.
    pub socket: PathBuf,
    /// The directory for what it remembers between runs.
    pub state: PathBuf,
}

impl Paths {
    /// The paths a daemon uses when it isn't given any: the socket in the
    /// user's runtime directory, and `$XDG_STATE_HOME/ctrlassist`, or
    /// `~/.local/state/ctrlassist` where that isn't set.
    #[must_use]
    pub fn default_for_user() -> Self {
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state"))
            })
            .unwrap_or_else(std::env::temp_dir);
        Self {
            socket: bootstrap::default_socket_path(),
            state: state.join("ctrlassist"),
        }
    }
}

/// A running daemon.
#[must_use = "dropping the daemon withdraws its liveliness token and stops the input thread, which removes every virtual pad"]
pub struct Daemon {
    // Declared in the order they are torn down: the token first, so a
    // client sees the daemon gone before what it served goes.
    alive: LivelinessToken,
    stop: oneshot::Sender<()>,
    reactor: JoinHandle<()>,
    watch: Watch,
    session: Session,
    input: InputThread,
}

impl std::fmt::Debug for Daemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Daemon")
            .field("realtime", &self.input.is_realtime())
            .finish_non_exhaustive()
    }
}

/// What the device crate calls an identity a profile names.
fn device_identity(identity: Identity) -> ctrlassist_device::Identity {
    match identity {
        Identity::Generic => ctrlassist_device::Identity::Generic,
        Identity::Xbox => ctrlassist_device::Identity::Xbox,
    }
}

impl Daemon {
    /// Starts the daemon at `paths`, around an input thread that is
    /// already running: that thread is created before any async runtime
    /// exists, so the caller makes it.
    ///
    /// # Errors
    ///
    /// Fails if the socket's directory can't be made, another daemon
    /// holds the socket, udev can't be watched, or zenoh refuses a
    /// declaration.
    pub async fn start(paths: &Paths, input: InputThread) -> Result<Self> {
        let socket: &Path = &paths.socket;
        if let Some(directory) = socket.parent() {
            // The user's alone: who can reach the socket is who can open
            // this directory.
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(directory)
                .with_context(|| format!("creating {}", directory.display()))?;
        }
        let session = zenoh::open(bootstrap::peer_config(socket)?)
            .await
            .map_err(|error| {
                anyhow!("opening the control plane on {}: {error}", socket.display())
            })?;

        let daemon_rpcs = session
            .declare_queryable(daemon::Incoming::wild())
            .await
            .map_err(|error| anyhow!("declaring the daemon's rpcs: {error}"))?;
        let pad_rpcs = session
            .declare_queryable(pad::Incoming::wild())
            .await
            .map_err(|error| anyhow!("declaring the pad rpcs: {error}"))?;

        // The pads remembered, then the pads connected now.
        let (store, known) = Store::open(&paths.state);
        let (changes, seen) = mpsc::unbounded_channel();
        let state_dir = paths.state.clone();
        let (watch, now) = tokio::task::spawn_blocking(move || Watch::start(changes))
            .await
            .context("starting the hotplug watch")?
            .context("watching udev for pads")?;
        let mut reactor = Reactor::new(session.clone(), Book::new(known), store);

        let realtime = input.is_realtime();
        let state = payload::Daemon {
            version: VERSION.to_string(),
            realtime,
        };
        // Whether this process may make a virtual pad at all: every
        // identity so far is made through uinput.
        let uinput = OpenOptions::new().write(true).open("/dev/uinput").is_ok();
        let published = async {
            reactor.publish(keys::daemon(), &state).await?;
            for identity in Identity::ALL {
                let device = device_identity(identity);
                let (vendor, product) = device.model();
                let body = payload::Identity {
                    id: identity.as_str().to_string(),
                    name: device.name().to_string(),
                    vendor,
                    product,
                    available: uinput,
                };
                reactor
                    .publish(keys::identity(identity.as_str()), &body)
                    .await?;
            }
            reactor.publish_pads().await
        };
        published
            .await
            .map_err(|error| anyhow!("publishing the daemon's state: {error}"))?;
        for pad in now {
            reactor.saw(pad).await;
        }
        if !realtime {
            warn!("real-time scheduling was refused; input latency under load is not dependable");
        }
        if !uinput {
            warn!("/dev/uinput can't be opened; no virtual pad can be made until it can");
        }

        let (stop, stopped) = oneshot::channel();
        let reactor = tokio::spawn(reactor.run(daemon_rpcs, pad_rpcs, seen, stopped));

        // The token is what clients take as "the daemon is there", so it
        // comes last: everything it serves is declared by the time a
        // client can see it alive.
        let alive = session
            .liveliness()
            .declare_token(keys::daemon_alive())
            .await
            .map_err(|error| anyhow!("declaring the daemon's liveliness: {error}"))?;
        info!(socket = %socket.display(), state = %state_dir.display(), version = VERSION, realtime, "the daemon is up");
        Ok(Self {
            alive,
            stop,
            reactor,
            watch,
            session,
            input,
        })
    }

    /// Whether the input thread was granted real-time scheduling.
    #[must_use]
    pub fn is_realtime(&self) -> bool {
        self.input.is_realtime()
    }

    /// Stops the daemon: clients see it gone, then the control plane
    /// closes, then the input thread stops and its virtual pads go.
    pub async fn stop(self) {
        let Self {
            alive,
            stop,
            reactor,
            watch,
            session,
            input,
        } = self;
        if let Err(error) = alive.undeclare().await {
            warn!(%error, "withdrawing the daemon's liveliness failed");
        }
        // The reactor ends when told, or when its sender is gone.
        drop(stop);
        let _ = reactor.await;
        if let Err(error) = session.close().await {
            warn!(%error, "closing the control plane failed");
        }
        // Joins both threads, off the runtime's workers.
        let _ = tokio::task::spawn_blocking(move || {
            drop(watch);
            drop(input);
        })
        .await;
        info!("the daemon has stopped");
    }
}
