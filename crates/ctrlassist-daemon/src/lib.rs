//! CtrlAssist's daemon: the control side.
//!
//! It owns the input thread and serves the control plane on a Unix socket
//! in the user's runtime directory. So far it answers the version check
//! and publishes its own state; pads, profiles and setups follow.

mod blacklist;
mod book;
mod copied;
pub mod durable;
mod hotplug;
pub mod logging;
mod profiles;
mod reactor;
pub mod service;
mod setups;
mod store;

use std::fs::OpenOptions;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use anyhow::{Context, Result, anyhow};
use ctrlassist_device::{Database, InputThread, Report};
use ctrlassist_protocol::profile::Identity;
use ctrlassist_protocol::{VERSION, bootstrap, keys, payload};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tracing::{info, warn};
use zenoh::Session;
use zenoh::liveliness::LivelinessToken;

use blacklist::Blacklist;
use book::Book;
use hotplug::Watch;
use profiles::Profiles;
use reactor::{Reactor, Rpcs};
use setups::Hiding;
use store::Store;

/// Where a daemon keeps what it needs.
#[derive(Clone, Debug)]
pub struct Paths {
    /// The socket it listens on.
    pub socket: PathBuf,
    /// The directory for what it remembers between runs.
    pub state: PathBuf,
    /// The directory for what the user had it keep: the profiles.
    pub config: PathBuf,
    /// The hiding service's socket.
    pub hider: PathBuf,
    /// Steam's `config.vdf` files, one per way Steam may be installed:
    /// where the hiding service isn't there, a setup's pads are added to
    /// the controller blacklist in each that exists. None leaves Steam's
    /// files alone.
    pub steam: Vec<PathBuf>,
    /// Whether to take the tests' fake pads for pads to play with. A
    /// daemon in use doesn't: a test run beside it would fill its list
    /// of pads and start its profiles.
    pub test_pads: bool,
}

/// The directory `variable` names, if it is set to an absolute path, or
/// `fallback` under the user's home.
fn user_directory(variable: &str, fallback: &str) -> PathBuf {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(fallback)))
        .unwrap_or_else(std::env::temp_dir)
}

impl Paths {
    /// The paths a daemon uses when it isn't given any: the socket in the
    /// user's runtime directory; `$XDG_STATE_HOME/ctrlassist`, or
    /// `~/.local/state/ctrlassist` where that isn't set; and
    /// `$XDG_CONFIG_HOME/ctrlassist`, or `~/.config/ctrlassist`; and the
    /// hiding service's socket where its package puts it.
    #[must_use]
    pub fn default_for_user() -> Self {
        Self {
            socket: bootstrap::default_socket_path(),
            state: user_directory("XDG_STATE_HOME", ".local/state").join("ctrlassist"),
            config: user_directory("XDG_CONFIG_HOME", ".config").join("ctrlassist"),
            hider: PathBuf::from(ctrlassist_hider::SOCKET_PATH),
            steam: {
                let data = user_directory("XDG_DATA_HOME", ".local/share");
                let config = "Steam/config/config.vdf";
                // As installed by a distribution, and as a Flatpak.
                let flatpak = ".var/app/com.valvesoftware.Steam/.local/share";
                vec![data.join(config), user_directory("", flatpak).join(config)]
            },
            test_pads: false,
        }
    }
}

/// A running daemon.
#[must_use = "dropping the daemon withdraws its liveliness token and stops the input thread, which removes every virtual pad"]
pub struct Daemon {
    /// What the daemon has in Steam's controller blacklist, taken out
    /// as it stops.
    blacklist: Arc<std::sync::Mutex<Blacklist>>,
    // Declared in the order they are torn down: the token first, so a
    // client sees the daemon gone before what it served goes.
    alive: LivelinessToken,
    stop: oneshot::Sender<()>,
    reactor: JoinHandle<()>,
    watch: Watch,
    session: Session,
    input: Arc<InputThread>,
}

impl std::fmt::Debug for Daemon {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Daemon")
            .field("realtime", &self.input.is_realtime())
            .finish_non_exhaustive()
    }
}

/// The controller database: the snapshot built in, under the user's own
/// `gamecontrollerdb.txt` in the configuration directory, if there is
/// one.
fn database(config: &Path) -> Database {
    let bundled = Database::bundled();
    let path = config.join("gamecontrollerdb.txt");
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let own = Database::parse(&text);
            info!(path = %path.display(), layouts = own.len(), skipped = own.skipped(), "the user's controller database is laid over the built-in one");
            bundled.overlaid(own)
        }
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                warn!(path = %path.display(), %error, "the user's controller database can't be read; using the built-in one alone");
            }
            bundled
        }
    }
}

/// What the device crate calls an identity a profile names.
pub(crate) fn device_identity(identity: Identity) -> ctrlassist_device::Identity {
    match identity {
        Identity::Generic => ctrlassist_device::Identity::Generic,
        Identity::Xbox => ctrlassist_device::Identity::Xbox,
        Identity::DualSense => ctrlassist_device::Identity::DualSense,
    }
}

impl Daemon {
    /// Starts the daemon at `paths`, around an input thread that is
    /// already running, and what it reports: that thread is created
    /// before any async runtime exists, so the caller makes it.
    ///
    /// # Errors
    ///
    /// Fails if the socket's directory can't be made, another daemon
    /// holds the socket, udev can't be watched, or zenoh refuses a
    /// declaration.
    pub async fn start(
        paths: &Paths,
        input: InputThread,
        reports: Receiver<Report>,
    ) -> Result<Self> {
        let input = Arc::new(input);
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

        let rpcs = Rpcs::declare(&session)
            .await
            .map_err(|error| anyhow!("declaring the daemon's rpcs: {error}"))?;

        // The pads remembered, then the pads connected now.
        let (store, known) = Store::open(&paths.state);
        let (changes, seen) = mpsc::unbounded_channel();
        let state_dir = paths.state.clone();
        let (watch, now) = tokio::task::spawn_blocking(move || Watch::start(changes))
            .await
            .context("starting the hotplug watch")?
            .context("watching udev for pads")?;
        let profiles = Profiles::open(&paths.config.join("profiles"));
        let hider = paths.hider.clone();
        let hiding = tokio::task::spawn_blocking(move || Hiding::find(&hider))
            .await
            .context("looking for the hiding service")?;
        let mechanism = hiding.mechanism();
        let mut reactor = Reactor::new(
            session.clone(),
            Book::new(known),
            store,
            profiles,
            Arc::clone(&input),
            database(&paths.config),
            hiding,
            copied::Copied::open(&paths.state),
        );
        reactor.test_pads = paths.test_pads;
        let (steam, state) = (paths.steam.clone(), paths.state.clone());
        let blacklist = tokio::task::spawn_blocking(move || Blacklist::open(steam, &state))
            .await
            .context("reading what was left in Steam's controller blacklist")?;
        let blacklist = Arc::new(std::sync::Mutex::new(blacklist));
        reactor.blacklist = Arc::clone(&blacklist);

        // The input thread's reports wait on a thread of their own, which
        // ends with the input thread.
        let (reported, reports_in) = mpsc::unbounded_channel();
        std::thread::Builder::new()
            .name("reports".into())
            .spawn(move || {
                for report in reports {
                    if reported.send(report).is_err() {
                        return;
                    }
                }
            })
            .context("starting the thread that carries the input thread's reports")?;

        // What games ask of the virtual pads and what the pads are told
        // is said only where it would be logged.
        input.tap(tracing::enabled!(tracing::Level::DEBUG));
        let realtime = input.is_realtime();
        let state = payload::Daemon {
            version: VERSION.to_string(),
            realtime,
            hiding: mechanism,
            config: paths.config.display().to_string(),
            tray: None,
        };
        // Whether this process may make a virtual pad at all: every
        // identity needs uinput, for the pad itself or to read it back.
        let uinput = OpenOptions::new().write(true).open("/dev/uinput").is_ok();
        let published = async {
            reactor.publish_daemon(state.clone()).await?;
            reactor.publish_identities().await?;
            reactor.publish_pads().await?;
            reactor.publish_profiles().await
        };
        published
            .await
            .map_err(|error| anyhow!("publishing the daemon's state: {error}"))?;
        for pad in now {
            reactor.saw(pad).await;
        }
        // Profiles that start by themselves, now that every pad already
        // connected is known.
        reactor.start_what_can(None).await;
        if !realtime {
            warn!("real-time scheduling was refused; input latency under load is not dependable");
        }
        if !uinput {
            warn!("/dev/uinput can't be opened; no virtual pad can be made until it can");
        }

        let (stop, stopped) = oneshot::channel();
        let reactor = tokio::spawn(reactor.run(rpcs, seen, reports_in, stopped));

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
            blacklist,
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
            blacklist,
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
        // Joins both threads, off the runtime's workers. The reactor's
        // hold on the input thread went with the reactor.
        let _ = tokio::task::spawn_blocking(move || {
            drop(watch);
            drop(input);
            // Nothing runs any more: Steam is left as it was found.
            if let Ok(mut blacklist) = blacklist.lock() {
                blacklist.clear();
            }
        })
        .await;
        info!("the daemon has stopped");
    }
}
