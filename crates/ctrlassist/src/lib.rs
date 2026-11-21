#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

mod error;
mod handshake;
mod lifetime;
mod mirror;

use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ctrlassist_protocol::bootstrap;
/// The socket a client connects to when it isn't given one: the daemon's,
/// in the user's runtime directory.
pub use ctrlassist_protocol::bootstrap::default_socket_path;
use ctrlassist_protocol::caller::Caller;
use ctrlassist_protocol::endpoint::daemon;
pub use ctrlassist_protocol::error::ErrorCode;
pub use ctrlassist_protocol::payload::Daemon as DaemonState;
pub use ctrlassist_protocol::rpc::ServiceInfo;
pub use error::{BoxError, Error, Result};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, warn};

use handshake::Handshake;
use lifetime::LifetimeMirror;
use mirror::DaemonMirror;

/// Change notifications from the client's mirror and the daemon's
/// liveliness.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StateEvent {
    /// The daemon's own state changed in the mirror, or left it because
    /// the daemon went away. Re-read [`DaemonClient::daemon_state`].
    DaemonStateChanged,
    /// The daemon's liveliness token appeared: it's running and serving
    /// rpcs. Can repeat for one lifetime, so treat it idempotently.
    DaemonAppeared,
    /// The daemon's liveliness token went away (stopped, crashed, or the
    /// link dropped). Its state has already left the mirror, announced
    /// before this event, and [`DaemonClient::service_info`] has cleared.
    DaemonDisappeared,
    /// Re-read everything: the watcher's periodic tick
    /// ([`StateWatcherConfig::fallback_poll_interval`]), also sent when
    /// the watcher fell behind and dropped events.
    FallbackPoll,
    /// The daemon's release check settled for its current lifetime, so
    /// [`DaemonClient::service_info`] now answers. Sent once per daemon
    /// lifetime, after [`Self::DaemonAppeared`], whatever the verdict; a
    /// watcher built after the check settled gets it on start, and may
    /// then see it twice.
    ReleaseChecked,
    /// The daemon's cached state has arrived in the mirror for the
    /// current lifetime, so an empty read now means empty rather than not
    /// yet known: until then, show the daemon as loading. Sent once per
    /// daemon lifetime, after [`Self::DaemonAppeared`]; a watcher built
    /// later reads [`DaemonClient::is_hydrated`] instead.
    Hydrated,
    /// The daemon runs a release this client can't talk to: every rpc to
    /// it fails until it's restarted on a matching release. Sent once per
    /// daemon lifetime, right after its [`Self::ReleaseChecked`]; a
    /// watcher built while the mismatch is already known gets it on
    /// start, and may then see it twice.
    VersionMismatch {
        /// The protocol release this SDK speaks.
        ours: String,
        /// The daemon's release.
        theirs: String,
    },
}

const EVENT_CAPACITY: usize = 256;

/// Client for the daemon: mirror reads, rpc writes.
#[derive(Debug, Clone)]
pub struct DaemonClient {
    caller: Arc<Caller>,
    mirror: LifetimeMirror<DaemonMirror>,
    events: broadcast::Sender<StateEvent>,
    handshake: Arc<Handshake>,
}

impl DaemonClient {
    /// Connect to the daemon socket and start mirroring.
    ///
    /// This succeeds whether or not the daemon is running (see the crate
    /// docs).
    ///
    /// # Errors
    ///
    /// Fails only with [`Error::Transport`], when the local session or
    /// its subscriptions can't be set up.
    pub async fn connect(socket: &Path) -> Result<Self> {
        let session = zenoh::open(
            bootstrap::client_config_resilient(socket)
                .map_err(|e| Error::transport("build the client config", e))?,
        )
        .await
        .map_err(|e| Error::transport("connect to the daemon socket", e))?;
        let caller = Arc::new(Caller::new(session.clone()));
        let mirror = LifetimeMirror::<DaemonMirror>::start(&session, &caller).await?;
        Ok(Self {
            caller,
            events: mirror.events().clone(),
            handshake: Arc::clone(mirror.handshake()),
            mirror,
        })
    }

    /// Whether the daemon's liveliness token is present: a mirror read.
    /// `true` doesn't promise the next rpc succeeds (the daemon may be
    /// stopping, or run another release).
    #[must_use]
    pub fn is_available(&self) -> bool {
        self.mirror.is_available()
    }

    /// Whether the daemon's cached state has arrived in the mirror for
    /// its current lifetime ([`StateEvent::Hydrated`]): while it's up and
    /// this is `false`, an empty read means not yet known. A mirror read.
    #[must_use]
    pub fn is_hydrated(&self) -> bool {
        self.mirror.is_hydrated()
    }

    /// What this lifetime's release check learned: the daemon's release.
    /// `None` while the daemon is down and until the check settles
    /// ([`StateEvent::ReleaseChecked`]). Ungated, so it reports a
    /// mismatched release too. A mirror read; [`Self::version`] asks the
    /// daemon instead.
    #[must_use]
    pub fn service_info(&self) -> Option<ServiceInfo> {
        self.handshake.info()
    }

    /// The daemon's own state, as last published: a mirror read, no rpc.
    /// `None` until it arrives, and while the daemon is down.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::VersionMismatch`] when the daemon is known to
    /// run another release.
    pub fn daemon_state(&self) -> Result<Option<DaemonState>> {
        self.refuse_known_mismatch()?;
        Ok(self.mirror.read(|mirror| mirror.daemon.clone()))
    }

    /// The daemon's release. Ungated, so it reports what's running even
    /// across a mismatch.
    ///
    /// # Errors
    ///
    /// Fails as any rpc can, except with [`Error::VersionMismatch`].
    pub async fn version(&self) -> Result<String> {
        Ok(self.info().await?.version)
    }

    /// Check that the daemon is there and runs a release this client can
    /// talk to: the gate every other rpc passes first, on its own.
    ///
    /// # Errors
    ///
    /// Fails as any rpc can, with [`Error::VersionMismatch`] for a daemon
    /// on another release line.
    pub async fn check_release(&self) -> Result<()> {
        self.handshake.ensure(&self.events, || self.info()).await
    }

    async fn info(&self) -> Result<ServiceInfo> {
        self.caller
            .call::<daemon::Info>(&(), &())
            .await
            .map_err(Error::from_rpc)
    }

    fn subscribe_events(&self) -> broadcast::Receiver<StateEvent> {
        self.events.subscribe()
    }

    /// Fail when the daemon is already known to run another release: the
    /// gate the mirror reads share, which doesn't ask.
    fn refuse_known_mismatch(&self) -> Result<()> {
        match self.handshake.known_mismatch() {
            Some(mismatch) => Err(mismatch),
            None => Ok(()),
        }
    }
}

/// Announce the daemon's liveliness edge (`announce`) and keep the
/// client's release check in step: a daemon that went away loses its
/// release before the announcement, so a reader acting on it sees none,
/// and its next lifetime gets a fresh check. One that appeared is checked
/// at once, after the announcement, so its release (and a mismatch)
/// reaches the watcher without waiting for a call. A failed check is
/// retried, backing off, for as long as that lifetime lasts. The check
/// runs on its own task, so a slow `info` never holds up the next
/// liveliness edge.
fn on_liveliness<F, Fut>(
    alive: bool,
    announce: StateEvent,
    handshake: &Arc<Handshake>,
    events: &broadcast::Sender<StateEvent>,
    caller: &Arc<Caller>,
    info: F,
) where
    F: Fn(Arc<Caller>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = std::result::Result<ServiceInfo, ctrlassist_protocol::caller::RpcError>>
        + Send,
{
    if !alive {
        handshake.reset();
        let _ = events.send(announce);
        return;
    }
    // Read before spawning, on the liveliness task: a `reset` for the
    // daemon going away can only come after it.
    let lifetime = handshake.lifetime();
    let _ = events.send(announce);
    let handshake = Arc::clone(handshake);
    let events = events.clone();
    let caller = Arc::clone(caller);
    tokio::spawn(async move {
        let mut backoff = CHECK_RETRY_FIRST;
        loop {
            let caller = Arc::clone(&caller);
            let checked = handshake
                .ensure(&events, || async {
                    info(caller).await.map_err(Error::from_rpc)
                })
                .await;
            // Settled, a mismatch included; or the daemon went away, and
            // its next lifetime gets its own check.
            if handshake.info().is_some() || handshake.lifetime() != lifetime {
                return;
            }
            if let Err(e) = checked {
                debug!(error = %e, ?backoff, "release check failed; retrying");
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(CHECK_RETRY_MAX);
        }
    });
}

/// The first wait before a failed release check is retried, doubling up
/// to [`CHECK_RETRY_MAX`].
const CHECK_RETRY_FIRST: Duration = Duration::from_secs(1);

/// The longest wait between release-check retries.
const CHECK_RETRY_MAX: Duration = Duration::from_secs(30);

/// Watcher tuning: mirror deltas forward directly, so the one knob is
/// the fallback poll interval.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct StateWatcherConfig {
    /// How often the watcher emits [`StateEvent::FallbackPoll`], a
    /// safety net for a missed change. Zero turns the poll off.
    pub fallback_poll_interval: Duration,
}

impl Default for StateWatcherConfig {
    fn default() -> Self {
        Self {
            fallback_poll_interval: Duration::from_secs(300),
        }
    }
}

/// The client's change stream: mirror deltas, liveliness edges and a
/// fallback poll tick.
#[derive(Debug)]
pub struct StateWatcher {
    rx: mpsc::UnboundedReceiver<StateEvent>,
}

impl StateWatcher {
    /// Watch `daemon`'s events. A daemon already alive when the watcher
    /// is built gets a [`StateEvent::DaemonAppeared`], so a consumer
    /// driven by events alone still learns of it. Must run inside the
    /// tokio runtime.
    #[must_use]
    pub fn new(daemon: &DaemonClient, config: StateWatcherConfig) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        forward(daemon.subscribe_events(), tx.clone());
        // Synthesize `DaemonAppeared` for a daemon already alive, and the
        // `ReleaseChecked` / `VersionMismatch` of a check already
        // settled: all fired into the client's broadcast before this
        // watcher subscribed (a tokio broadcast drops events sent with no
        // receivers), so consumers relying on edges alone would never
        // learn of them. A level-triggered emit after subscription closes
        // the window; a check finishing in between can announce twice,
        // and consumers treat repeats idempotently.
        {
            let tx = tx.clone();
            let daemon = daemon.clone();
            tokio::spawn(async move {
                if daemon.is_available() {
                    let _ = tx.send(StateEvent::DaemonAppeared);
                }
                for event in daemon.handshake.settled_events() {
                    let _ = tx.send(event);
                }
            });
        }
        if config.fallback_poll_interval.is_zero() {
            return Self { rx };
        }
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(config.fallback_poll_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await; // immediate first tick is not a poll
            loop {
                interval.tick().await;
                if tx.send(StateEvent::FallbackPoll).is_err() {
                    return;
                }
            }
        });
        Self { rx }
    }

    /// The next event, waiting for one. `None` only once the watcher's
    /// tasks have stopped, which happens when the runtime shuts down.
    pub async fn next(&mut self) -> Option<StateEvent> {
        self.rx.recv().await
    }

    /// The next event if one is already queued, without waiting. Draining
    /// these after [`Self::next`] lets a consumer act once on the state a
    /// burst of events leaves, rather than once per event.
    pub fn try_next(&mut self) -> Option<StateEvent> {
        self.rx.try_recv().ok()
    }
}

fn forward(mut rx: broadcast::Receiver<StateEvent>, tx: mpsc::UnboundedSender<StateEvent>) {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if tx.send(event).is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    warn!(missed, "state events lagged; forcing a refresh");
                    if tx.send(StateEvent::FallbackPoll).is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A watcher that fell behind is told to re-read everything, and its
    /// forwarding ends with either side of it.
    #[tokio::test]
    async fn a_lagging_watcher_is_told_to_refresh() {
        let (events, behind) = broadcast::channel(2);
        for _ in 0..5 {
            events.send(StateEvent::DaemonStateChanged).unwrap();
        }
        let (tx, mut rx) = mpsc::unbounded_channel();
        forward(behind, tx);
        assert_eq!(rx.recv().await, Some(StateEvent::FallbackPoll));
        assert_eq!(rx.recv().await, Some(StateEvent::DaemonStateChanged));
        assert_eq!(rx.recv().await, Some(StateEvent::DaemonStateChanged));
        // The client gone, the stream ends.
        drop(events);
        assert_eq!(rx.recv().await, None);

        // The watcher gone, the forwarding stops at the next event, lagged
        // or not.
        for lagged in [false, true] {
            let (events, behind) = broadcast::channel(2);
            let (tx, rx) = mpsc::unbounded_channel();
            drop(rx);
            for _ in 0..if lagged { 5 } else { 1 } {
                events.send(StateEvent::DaemonAppeared).unwrap();
            }
            forward(behind, tx.clone());
            tx.closed().await;
            while events.receiver_count() != 0 {
                tokio::task::yield_now().await;
            }
        }
    }
}
