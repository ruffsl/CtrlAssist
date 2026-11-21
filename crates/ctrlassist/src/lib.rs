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
use ctrlassist_protocol::endpoint::{Unary, daemon, pad, profile, setup};
pub use ctrlassist_protocol::error::ErrorCode;
pub use ctrlassist_protocol::payload::{
    Daemon as DaemonState, Identity, Pad, Phase, Problem, Profile, Setup, SetupSink, SetupSource,
};
use ctrlassist_protocol::rpc;
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
    /// A pad's state changed in the mirror: it connected or left, the
    /// daemon came to know it or forgot it, or it left the mirror because
    /// the daemon went away. Re-read [`DaemonClient::list_pads`].
    PadChanged {
        /// The pad, by the id it is known by across reconnects.
        pad: String,
    },
    /// A profile's state changed in the mirror: it was put or deleted, or
    /// it left the mirror because the daemon went away. Re-read
    /// [`DaemonClient::list_profiles`].
    ProfileChanged {
        /// The profile's name.
        profile: String,
    },
    /// A setup's state changed in the mirror: it was started or stopped,
    /// it moved between phases, a pad of its came or went, or it left the
    /// mirror because the daemon went away. Re-read
    /// [`DaemonClient::list_setups`].
    SetupChanged {
        /// The setup's name: the profile it runs.
        setup: String,
    },
    /// What a virtual pad can present itself as changed, or left the
    /// mirror because the daemon went away. Re-read
    /// [`DaemonClient::list_identities`].
    IdentitiesChanged,
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
        Self::with_session(session).await
    }

    /// Start mirroring over a zenoh `session` the caller already holds on
    /// the daemon's socket, rather than opening one. [`Self::connect`]
    /// opens a session that keeps dialing for as long as the client
    /// lives; a command that runs once and wants to fail at once when no
    /// daemon is listening opens its own, with
    /// `ctrlassist_protocol::bootstrap::client_config`, and hands it over.
    ///
    /// # Errors
    ///
    /// Fails only with [`Error::Transport`].
    pub async fn with_session(session: zenoh::Session) -> Result<Self> {
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

    /// The pads the daemon knows, connected or remembered, in the order
    /// of their ids: a mirror read, no rpc.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::VersionMismatch`] when the daemon is known to
    /// run another release.
    pub fn list_pads(&self) -> Result<Vec<Pad>> {
        self.refuse_known_mismatch()?;
        let mut pads: Vec<Pad> = self
            .mirror
            .read(|mirror| mirror.pads.values().cloned().collect());
        pads.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(pads)
    }

    /// What a virtual pad can present itself as: a mirror read, no rpc.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::VersionMismatch`] when the daemon is known to
    /// run another release.
    pub fn list_identities(&self) -> Result<Vec<Identity>> {
        self.refuse_known_mismatch()?;
        Ok(self
            .mirror
            .read(|mirror| mirror.identities.values().cloned().collect()))
    }

    /// The profiles the daemon keeps, in the order of their names: a
    /// mirror read, no rpc. One with a [`Profile::problem`] is kept but
    /// can't be run until it is put right.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::VersionMismatch`] when the daemon is known to
    /// run another release.
    pub fn list_profiles(&self) -> Result<Vec<Profile>> {
        self.refuse_known_mismatch()?;
        Ok(self
            .mirror
            .read(|mirror| mirror.profiles.values().cloned().collect()))
    }

    /// The profile kept under `name`, if one is: a mirror read, no rpc.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::VersionMismatch`] when the daemon is known to
    /// run another release.
    pub fn profile(&self, name: &str) -> Result<Option<Profile>> {
        self.refuse_known_mismatch()?;
        Ok(self
            .mirror
            .read(|mirror| mirror.profiles.get(name).cloned()))
    }

    /// Have the daemon check a profile's TOML without keeping it: what is
    /// wrong with it and where, or `None` for one it would keep.
    ///
    /// # Errors
    ///
    /// Fails as any rpc can.
    pub async fn validate_profile(&self, toml: &str) -> Result<Option<Problem>> {
        let request = rpc::ValidateRequest {
            toml: toml.to_string(),
        };
        Ok(self.call::<profile::Validate>(&request).await?.problem)
    }

    /// Keep `toml` as the profile `name`: a new one, or in place of the
    /// one that has the name.
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::InvalidProfile`], whose message says
    /// where the profile is wrong and what, or with
    /// [`ErrorCode::BadRequest`] for a name that isn't 1 to 32 lowercase
    /// letters, digits and hyphens; and fails as any rpc can.
    pub async fn put_profile(&self, name: &str, toml: &str) -> Result<()> {
        let request = rpc::PutProfileRequest {
            name: name.to_string(),
            toml: toml.to_string(),
        };
        self.call::<profile::Put>(&request).await.map(|_| ())
    }

    /// Stop keeping the profile `name`.
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::UnknownProfile`]; and fails as any rpc
    /// can.
    pub async fn delete_profile(&self, name: &str) -> Result<()> {
        let request = rpc::ProfileRequest {
            name: name.to_string(),
        };
        self.call::<profile::Delete>(&request).await.map(|_| ())
    }

    /// The setups that exist, in the order of their names: a mirror read,
    /// no rpc.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::VersionMismatch`] when the daemon is known to
    /// run another release.
    pub fn list_setups(&self) -> Result<Vec<Setup>> {
        self.refuse_known_mismatch()?;
        Ok(self
            .mirror
            .read(|mirror| mirror.setups.values().cloned().collect()))
    }

    /// The setup called `name`, if there is one: a mirror read, no rpc.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::VersionMismatch`] when the daemon is known to
    /// run another release.
    pub fn setup(&self, name: &str) -> Result<Option<Setup>> {
        self.refuse_known_mismatch()?;
        Ok(self.mirror.read(|mirror| mirror.setups.get(name).cloned()))
    }

    /// Run the kept profile `name` as a setup of the same name. Returns
    /// once its virtual pads exist; it is then running, or held if a pad
    /// it needs isn't connected, which its state says.
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::UnknownProfile`],
    /// [`ErrorCode::InvalidProfile`], [`ErrorCode::SetupExists`],
    /// [`ErrorCode::PadInUse`], or [`ErrorCode::SetupFailed`] with why;
    /// and fails as any rpc can.
    pub async fn start_setup(&self, name: &str) -> Result<()> {
        let request = rpc::SetupRequest {
            name: name.to_string(),
        };
        self.call::<setup::Start>(&request).await.map(|_| ())
    }

    /// Stop the setup `name`. Returns once its virtual pads are gone.
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::UnknownSetup`], or with
    /// [`ErrorCode::SetupBusy`] while it is starting or stopping; and
    /// fails as any rpc can.
    pub async fn stop_setup(&self, name: &str) -> Result<()> {
        let request = rpc::SetupRequest {
            name: name.to_string(),
        };
        self.call::<setup::Stop>(&request).await.map(|_| ())
    }

    /// Buzz a pad for `duration`, so a person can tell which one it is.
    /// `pad` is its id, a bare address, or `model:<vendor>:<product>`
    /// when one known pad is of that model. Returns once the buzz has
    /// started.
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::UnknownPad`], [`ErrorCode::AmbiguousPad`],
    /// [`ErrorCode::PadDisconnected`], [`ErrorCode::PadWithoutRumble`] or
    /// [`ErrorCode::PadRefused`], or with [`ErrorCode::BadRequest`] for a
    /// duration of nothing or of more than five seconds; and fails as any
    /// rpc can.
    pub async fn identify_pad(&self, pad: &str, duration: Duration) -> Result<()> {
        let request = rpc::IdentifyRequest {
            pad: pad.to_string(),
            // Longer than the daemon accepts either way.
            millis: u32::try_from(duration.as_millis()).unwrap_or(u32::MAX),
        };
        self.call::<pad::Identify>(&request).await.map(|_| ())
    }

    /// Stop remembering a pad that isn't connected. `pad` names it as for
    /// [`Self::identify_pad`].
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::UnknownPad`], [`ErrorCode::AmbiguousPad`]
    /// or [`ErrorCode::PadConnected`]; and fails as any rpc can.
    pub async fn forget_pad(&self, pad: &str) -> Result<()> {
        let request = rpc::PadRequest {
            pad: pad.to_string(),
        };
        self.call::<pad::Forget>(&request).await.map(|_| ())
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

    /// Call a daemon rpc, once the daemon's release is known to match.
    async fn call<R: Unary<Target = ()>>(&self, request: &R::Request) -> Result<R::Response> {
        self.check_release().await?;
        self.caller
            .call::<R>(&(), request)
            .await
            .map_err(Error::from_rpc)
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
