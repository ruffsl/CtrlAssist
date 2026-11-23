//! Following one service's lifetime: the same way for every mirror.
//!
//! The daemon's lifetime is its liveliness token's presence. While the
//! token is present, the mirror holds one subscription per key expression
//! it mirrors, each replaying the daemon's caches as history; when the
//! token goes, the subscriptions go first, then the mirror's state.
//!
//! The subscriptions can't be kept across a lifetime: zenoh-ext's
//! advanced subscriber keeps each publisher's last delivered sequence
//! number and drops replayed samples at or below it, so a mirror that
//! cleared would never get an unchanged value back. A token that returns,
//! a restarted daemon's or the same one's after its links dropped (the
//! PC woke with this process frozen past the link lease), gets fresh
//! subscriptions that hydrate from the caches again.
//!
//! Within a lifetime, recovery retransmits a sample lost on the way from
//! the publisher's cache, once the publisher's heartbeat shows the gap.
//!
//! A lifetime starts hydrating: the history arrives after the
//! subscriptions are declared, and zenoh-ext doesn't say when it's all
//! in. The daemon's `info` reply lists the keys it publishes from a
//! cache, so the mirror is hydrated once it holds a sample for each one
//! its subscriptions match, or at a deadline, since keys can change
//! between the reply and the history.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ctrlassist_protocol::caller::{Caller, RpcError};
use ctrlassist_protocol::endpoint::{daemon, tray};
use ctrlassist_protocol::keys;
use tokio::sync::{Notify, broadcast, mpsc};
use tokio::task::JoinHandle;
use tracing::{debug, warn};
use zenoh::Session;
use zenoh::handlers::FifoChannelHandler;
use zenoh::key_expr::KeyExpr;
use zenoh::pubsub::Subscriber;
use zenoh::sample::{Sample, SampleKind};
use zenoh_ext::{AdvancedSubscriber, AdvancedSubscriberBuilderExt, HistoryConfig, RecoveryConfig};

use crate::handshake::Handshake;
use crate::{EVENT_CAPACITY, Error, Result, Service, ServiceInfo, StateEvent};

/// The first wait before a failed subscription is retried, doubling up
/// to [`SUBSCRIBE_RETRY_MAX`].
const SUBSCRIBE_RETRY_FIRST: Duration = Duration::from_millis(100);
const SUBSCRIBE_RETRY_MAX: Duration = Duration::from_secs(5);

/// The first wait before a failed release check is retried, doubling up
/// to [`CHECK_RETRY_MAX`].
const CHECK_RETRY_FIRST: Duration = Duration::from_secs(1);

/// The longest wait between release-check retries.
const CHECK_RETRY_MAX: Duration = Duration::from_secs(30);

/// How long a lifetime hydrates at most, from its token appearing: past
/// it, the mirror counts as hydrated with whatever arrived.
const HYDRATE_DEADLINE: Duration = Duration::from_secs(5);

/// One service's mirrored state: what [`LifetimeMirror`] keeps current.
pub(crate) trait Mirror: Default + std::fmt::Debug + Send + 'static {
    /// The service whose keys this mirrors.
    const SERVICE: Service;

    /// The key expressions to subscribe.
    fn keys() -> Vec<String>;

    /// Apply one sample; the returned event (if any) is what a watcher
    /// should broadcast.
    fn apply(&mut self, sample: &Sample) -> Option<StateEvent>;

    /// Drop everything, the daemon having gone; one event per thing that
    /// was showing.
    fn clear(&mut self) -> Vec<StateEvent>;
}

#[derive(Debug, Default)]
struct Tracked<M> {
    mirror: M,
    /// Whether the daemon's token is present and every subscription is
    /// declared.
    alive: bool,
    /// Whether this lifetime's history has arrived.
    hydrated: bool,
    /// The keys a sample has arrived on this lifetime.
    seen: HashSet<String>,
}

/// A mirror that follows the daemon's lifetime: see the module docs.
#[derive(Debug)]
pub(crate) struct LifetimeMirror<M> {
    tracked: Arc<Mutex<Tracked<M>>>,
    events: broadcast::Sender<StateEvent>,
    handshake: Arc<Handshake>,
    /// Woken whenever a sample arrives, for the hydration wait.
    arrived: Arc<Notify>,
    /// The client copies' hold on the follow task, which ends it once the
    /// last copy drops and so releases the session it holds. The task's
    /// own copies have none, or it would keep itself alive.
    owner: Option<Arc<FollowOwner>>,
}

/// Aborts the follow task when dropped.
#[derive(Debug)]
struct FollowOwner(tokio::task::AbortHandle);

impl Drop for FollowOwner {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl<M> Clone for LifetimeMirror<M> {
    fn clone(&self) -> Self {
        Self {
            tracked: Arc::clone(&self.tracked),
            events: self.events.clone(),
            handshake: Arc::clone(&self.handshake),
            arrived: Arc::clone(&self.arrived),
            owner: self.owner.clone(),
        }
    }
}

/// One lifetime's tasks: the setup that subscribes (retrying until it
/// can), checks the service's release and waits for its history, and a
/// task per subscription.
struct Lifetime {
    setup: JoinHandle<()>,
    pumps: Pumps,
}

impl Lifetime {
    /// Stop every task and wait for each, so no sample is still being
    /// applied once this returns.
    async fn end(mut self) {
        self.setup.abort();
        // Once it has stopped, it registers no more pumps.
        let _ = (&mut self.setup).await;
        let mut pumps = self.pumps.take();
        // All stopped before any is waited for: dropped between two waits
        // (its follow task aborted), the rest are stopped all the same.
        for pump in &pumps {
            pump.0.abort();
        }
        for pump in &mut pumps {
            let _ = (&mut pump.0).await;
        }
    }
}

/// A lifetime dropped without [`Lifetime::end`] (its follow task aborted)
/// still stops its tasks, so their subscriptions and session go too. A
/// pump its setup task registers after this, while finishing what it
/// was doing when aborted, is stopped as the setup task lets go of the
/// list.
impl Drop for Lifetime {
    fn drop(&mut self) {
        self.setup.abort();
        drop(self.pumps.take());
    }
}

/// The pumps of one lifetime, each registered as it is spawned, so none
/// is left running when the lifetime ends or is dropped partway through
/// subscribing.
#[derive(Clone, Default)]
struct Pumps(Arc<Mutex<Vec<Pump>>>);

impl Pumps {
    fn register(&self, pump: JoinHandle<()>) {
        self.lock().push(Pump(pump));
    }

    /// Every pump registered so far, leaving none.
    fn take(&self) -> Vec<Pump> {
        std::mem::take(&mut *self.lock())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Pump>> {
        self.0
            .lock()
            .expect("nothing panics while the pump list is held")
    }
}

/// A pump's task, stopped when this is dropped: by the list going with
/// the last of its lifetime's tasks, or by a retry letting go of it.
struct Pump(JoinHandle<()>);

impl Drop for Pump {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl<M: Mirror> LifetimeMirror<M> {
    /// Follow `M`'s service on `session`: its liveliness from history on,
    /// then a lifetime per token.
    pub(crate) async fn start(session: &Session, caller: &Arc<Caller>) -> Result<Self> {
        let liveliness = session
            .liveliness()
            .declare_subscriber(alive_key(M::SERVICE))
            .history(true)
            .await
            .map_err(|e| Error::transport("subscribe to the service's liveliness", e))?;
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let mut this = Self {
            tracked: Arc::new(Mutex::new(Tracked::default())),
            events,
            handshake: Arc::new(Handshake::new(M::SERVICE)),
            arrived: Arc::new(Notify::new()),
            owner: None,
        };
        let follow = tokio::spawn(this.clone().follow(
            liveliness,
            session.clone(),
            Arc::clone(caller),
        ));
        this.owner = Some(Arc::new(FollowOwner(follow.abort_handle())));
        Ok(this)
    }

    /// Read the mirror.
    pub(crate) fn read<R>(&self, read: impl FnOnce(&M) -> R) -> R {
        read(&self.lock().mirror)
    }

    /// Whether the daemon's token is present and its keys subscribed.
    pub(crate) fn is_available(&self) -> bool {
        self.lock().alive
    }

    /// Whether this lifetime's history has arrived; `false` while the
    /// daemon is down.
    pub(crate) fn is_hydrated(&self) -> bool {
        self.lock().hydrated
    }

    /// Where the mirror's events, and the release check's, go out.
    pub(crate) fn events(&self) -> &broadcast::Sender<StateEvent> {
        &self.events
    }

    /// The daemon's release check, cleared with each lifetime.
    pub(crate) fn handshake(&self) -> &Arc<Handshake> {
        &self.handshake
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Tracked<M>> {
        self.tracked
            .lock()
            .expect("nothing panics while the mirror lock is held")
    }

    async fn follow(
        self,
        liveliness: Subscriber<FifoChannelHandler<Sample>>,
        session: Session,
        caller: Arc<Caller>,
    ) {
        let mut lifetime: Option<Lifetime> = None;
        while let Ok(sample) = liveliness.recv_async().await {
            if sample.kind() == SampleKind::Put {
                if lifetime.is_none() {
                    lifetime = Some(self.begin(&session, &caller));
                }
                continue;
            }
            if let Some(lifetime) = lifetime.take() {
                lifetime.end().await;
            }
            let removed = {
                let mut tracked = self.lock();
                tracked.alive = false;
                tracked.hydrated = false;
                tracked.seen.clear();
                tracked.mirror.clear()
            };
            for event in removed {
                let _ = self.events.send(event);
            }
            self.handshake.disappeared(&self.events);
        }
    }

    /// Subscribe, then report the daemon available: a client that waits
    /// for it finds the subscriptions in place, their history replaying
    /// as it arrives. Then check its release, and wait for the history
    /// to arrive.
    fn begin(&self, session: &Session, caller: &Arc<Caller>) -> Lifetime {
        let deadline = tokio::time::Instant::now() + HYDRATE_DEADLINE;
        let pumps = Pumps::default();
        let setup = tokio::spawn({
            let this = self.clone();
            let session = session.clone();
            let caller = Arc::clone(caller);
            let pumps = pumps.clone();
            async move {
                this.subscribe(&session, &pumps).await;
                // Before announcing, so the release check's settling can't
                // be missed.
                let events = this.events.subscribe();
                this.lock().alive = true;
                let lifetime = this.handshake.appeared(&this.events);
                tokio::join!(
                    this.check(lifetime, &caller),
                    this.hydrate(events, deadline)
                );
            }
        });
        Lifetime { setup, pumps }
    }

    /// Check the service's release at once, so its release (and a
    /// mismatch) reaches a watcher without waiting for a call, retrying
    /// a check that got no answer, backing off, until one settles or the
    /// lifetime ends. Runs on the lifetime's setup task, so it ends with
    /// the lifetime, and with the client.
    async fn check(&self, lifetime: u64, caller: &Arc<Caller>) {
        let mut backoff = CHECK_RETRY_FIRST;
        loop {
            let checked = self
                .handshake
                .ensure(&self.events, || async {
                    service_info(Arc::clone(caller), M::SERVICE)
                        .await
                        .map_err(|e| Error::from_rpc(M::SERVICE, e))
                })
                .await;
            // Settled, a mismatch included; or the service went away, and
            // its next lifetime gets its own check.
            if self.handshake.info().is_some() || self.handshake.lifetime() != lifetime {
                return;
            }
            if let Err(e) = checked {
                debug!(service = %M::SERVICE, error = %e, ?backoff, "release check failed; retrying");
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(CHECK_RETRY_MAX);
        }
    }

    /// Wait for the release check's `info` reply, then for a sample on each
    /// cached key it lists that this mirror subscribes; at the deadline,
    /// stop waiting. A mismatched daemon lists nothing this client can
    /// read, so it's hydrated at once.
    async fn hydrate(
        &self,
        mut events: broadcast::Receiver<StateEvent>,
        deadline: tokio::time::Instant,
    ) {
        let info = loop {
            if let Some(info) = self.handshake.info() {
                break Some(info);
            }
            tokio::select! {
                // Any event, a lag included, is a cue to look again.
                _ = events.recv() => {}
                () = tokio::time::sleep_until(deadline) => break None,
            }
        };
        let expected: Vec<String> = match info {
            Some(info) if self.handshake.known_mismatch().is_none() => {
                let subscribed: Vec<KeyExpr<'static>> = M::keys()
                    .into_iter()
                    .filter_map(|key| KeyExpr::try_from(key).ok())
                    .collect();
                info.keys
                    .into_iter()
                    .filter(|key| {
                        KeyExpr::try_from(key.as_str())
                            .is_ok_and(|key| subscribed.iter().any(|expr| expr.intersects(&key)))
                    })
                    .collect()
            }
            _ => Vec::new(),
        };
        loop {
            let arrived = self.arrived.notified();
            if expected.iter().all(|key| self.lock().seen.contains(key)) {
                break;
            }
            tokio::select! {
                () = arrived => {}
                () = tokio::time::sleep_until(deadline) => break,
            }
        }
        self.lock().hydrated = true;
        let _ = self.events.send(StateEvent::Hydrated(M::SERVICE));
    }

    /// Subscribes to every key `M` mirrors, from history on and with
    /// recovery, retrying until all are declared, and registers in
    /// `pumps` each task that reads one as soon as it is spawned.
    ///
    /// What a subscription delivers is queued without bound. Its history
    /// arrives while it is still being declared, before there is
    /// anything to read it from, and zenoh's own queue holds 256 samples
    /// and stops the session's reader when full: a daemon with more
    /// state under one key than that stopped the declaration for good,
    /// and the client never found the daemon alive. What a daemon
    /// publishes is its state, which is what bounds the queue.
    async fn subscribe(&self, session: &Session, pumps: &Pumps) {
        let mut backoff = SUBSCRIBE_RETRY_FIRST;
        loop {
            let mut failed = None;
            for key in M::keys() {
                let (samples, arriving) = mpsc::unbounded_channel();
                let declared = session
                    .declare_subscriber(key)
                    .callback(move |sample| {
                        // Nobody is reading once the pump has gone.
                        let _ = samples.send(sample);
                    })
                    .history(HistoryConfig::default().detect_late_publishers())
                    .recovery(RecoveryConfig::default().heartbeat())
                    .await;
                match declared {
                    Ok(subscriber) => {
                        pumps.register(tokio::spawn(self.clone().pump(subscriber, arriving)));
                    }
                    Err(e) => {
                        failed = Some(e);
                        break;
                    }
                }
            }
            let Some(error) = failed else {
                return;
            };
            warn!(service = %M::SERVICE, %error, ?backoff, "couldn't subscribe to the service's keys; retrying");
            // Each takes its subscription with it.
            drop(pumps.take());
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(SUBSCRIBE_RETRY_MAX);
        }
    }

    /// Applies what a subscription delivers, for as long as it lives:
    /// the subscription is held here, and goes with this task.
    async fn pump(
        self,
        subscriber: AdvancedSubscriber<()>,
        mut arriving: mpsc::UnboundedReceiver<Sample>,
    ) {
        let _subscriber = subscriber;
        while let Some(sample) = arriving.recv().await {
            let event = {
                let mut tracked = self.lock();
                tracked.seen.insert(sample.key_expr().to_string());
                tracked.mirror.apply(&sample)
            };
            self.arrived.notify_one();
            if let Some(event) = event {
                let _ = self.events.send(event);
            }
        }
    }
}

fn alive_key(service: Service) -> String {
    match service {
        Service::Daemon => keys::daemon_alive(),
        Service::Tray => keys::tray_alive(),
    }
}

async fn service_info(
    caller: Arc<Caller>,
    service: Service,
) -> std::result::Result<ServiceInfo, RpcError> {
    match service {
        Service::Daemon => caller.call::<daemon::Info>(&(), &()).await,
        Service::Tray => caller.call::<tray::Info>(&(), &()).await,
    }
}

#[cfg(test)]
mod tests {
    use tokio::sync::oneshot;

    use super::*;

    /// A pump that holds `alive` for as long as its task lives.
    fn pump(alive: oneshot::Sender<()>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let _alive = alive;
            std::future::pending::<()>().await;
        })
    }

    /// A lifetime dropped while its setup task is busy declaring (zenoh
    /// declares without yielding, so an abort can't stop it there) still
    /// stops the pumps that task goes on to spawn.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_lifetime_dropped_while_subscribing_stops_its_pumps() {
        let pumps = Pumps::default();
        let (alive, gone) = oneshot::channel();
        let (busy, declaring) = oneshot::channel();
        let setup = tokio::spawn({
            let pumps = pumps.clone();
            async move {
                let _ = busy.send(());
                std::thread::sleep(Duration::from_millis(200));
                pumps.register(pump(alive));
                std::future::pending::<()>().await;
            }
        });
        declaring.await.unwrap();
        drop(Lifetime { setup, pumps });
        let stopped = tokio::time::timeout(Duration::from_secs(10), gone).await;
        assert!(
            matches!(stopped, Ok(Err(_))),
            "the pump outlived its lifetime"
        );
    }

    /// Ending a lifetime stops every pump, and waits for each.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ending_a_lifetime_stops_every_pump() {
        let pumps = Pumps::default();
        let mut gone = Vec::new();
        for _ in 0..3 {
            let (alive, went) = oneshot::channel();
            pumps.register(pump(alive));
            gone.push(went);
        }
        let setup = tokio::spawn(std::future::pending::<()>());
        Lifetime { setup, pumps }.end().await;
        for mut went in gone {
            assert!(
                matches!(went.try_recv(), Err(oneshot::error::TryRecvError::Closed)),
                "a pump was still running once the lifetime ended"
            );
        }
    }
}
