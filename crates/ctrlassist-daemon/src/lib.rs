//! CtrlAssist's daemon: the control side.
//!
//! It owns the input thread and serves the control plane on a Unix socket
//! in the user's runtime directory. So far it answers the version check
//! and publishes its own state; pads, profiles and setups follow.

pub mod logging;
pub mod service;

use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use ctrlassist_device::InputThread;
use ctrlassist_protocol::endpoint::daemon;
use ctrlassist_protocol::{VERSION, bootstrap, keys, payload, rpc};
use tokio::task::JoinHandle;
use tracing::{info, warn};
use zenoh::Session;
use zenoh::bytes::Encoding;
use zenoh::liveliness::LivelinessToken;
use zenoh_ext::{AdvancedPublisher, AdvancedPublisherBuilderExt, CacheConfig, MissDetectionConfig};

/// Heartbeat period for sample-miss recovery on cached publishers: a
/// heartbeat goes out only in a period after a publish, so an idle key
/// costs a subscriber nothing.
const MISS_HEARTBEAT: Duration = Duration::from_millis(500);

/// A running daemon.
#[must_use = "dropping the daemon withdraws its liveliness token and stops the input thread, which removes every virtual pad"]
pub struct Daemon {
    // Declared in the order they are torn down: the token first, so a
    // client sees the daemon gone before what it served goes.
    alive: LivelinessToken,
    state: AdvancedPublisher<'static>,
    serving: JoinHandle<()>,
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

/// Answers the daemon's own rpcs until the session closes.
async fn serve(
    queryable: zenoh::query::Queryable<zenoh::handlers::FifoChannelHandler<zenoh::query::Query>>,
) {
    while let Ok(query) = queryable.recv_async().await {
        match daemon::Incoming::receive(query).await {
            Some(((), daemon::Incoming::Info((), responder))) => {
                let info = rpc::ServiceInfo {
                    version: VERSION.to_string(),
                    keys: vec![keys::daemon()],
                };
                responder.ok(&info).await;
            }
            // Refused already, with a typed error.
            None => {}
        }
    }
}

impl Daemon {
    /// Starts the daemon on `socket`, around an input thread that is
    /// already running: that thread is created before any async runtime
    /// exists, so the caller makes it.
    ///
    /// # Errors
    ///
    /// Fails if the socket's directory can't be made, another daemon
    /// holds the socket, or zenoh refuses a declaration.
    pub async fn start(socket: &Path, input: InputThread) -> Result<Self> {
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

        let queryable = session
            .declare_queryable(daemon::Incoming::wild())
            .await
            .map_err(|error| anyhow!("declaring the daemon's rpcs: {error}"))?;
        let serving = tokio::spawn(serve(queryable));

        // State is published from a cache, so a client that connects
        // later still receives it.
        let state = session
            .declare_publisher(keys::daemon())
            .encoding(Encoding::APPLICATION_JSON)
            .cache(CacheConfig::default().max_samples(1))
            .sample_miss_detection(
                MissDetectionConfig::default().sporadic_heartbeat(MISS_HEARTBEAT),
            )
            .publisher_detection()
            .await
            .map_err(|error| anyhow!("declaring the daemon's state: {error}"))?;
        let realtime = input.is_realtime();
        let now = payload::Daemon {
            version: VERSION.to_string(),
            realtime,
        };
        let bytes = serde_json::to_vec(&now).context("encoding the daemon's state")?;
        state
            .put(bytes)
            .await
            .map_err(|error| anyhow!("publishing the daemon's state: {error}"))?;
        if !realtime {
            warn!("real-time scheduling was refused; input latency under load is not dependable");
        }

        // The token is what clients take as "the daemon is there", so it
        // comes last: everything it serves is declared by the time a
        // client can see it alive.
        let alive = session
            .liveliness()
            .declare_token(keys::daemon_alive())
            .await
            .map_err(|error| anyhow!("declaring the daemon's liveliness: {error}"))?;
        info!(socket = %socket.display(), version = VERSION, realtime, "the daemon is up");
        Ok(Self {
            alive,
            state,
            serving,
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
            state,
            serving,
            session,
            input,
        } = self;
        if let Err(error) = alive.undeclare().await {
            warn!(%error, "withdrawing the daemon's liveliness failed");
        }
        drop(state);
        serving.abort();
        if let Err(error) = session.close().await {
            warn!(%error, "closing the control plane failed");
        }
        // Joins the thread, off the runtime's workers.
        let _ = tokio::task::spawn_blocking(move || drop(input)).await;
        info!("the daemon has stopped");
    }
}
