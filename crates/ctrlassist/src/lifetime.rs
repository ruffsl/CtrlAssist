//! Following the daemon's lifetime.
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
use ctrlassist_protocol::endpoint::daemon;
use ctrlassist_protocol::keys;
use tokio::sync::{Notify, broadcast};
use tokio::task::JoinHandle;
use tracing::warn;
use zenoh::Session;
use zenoh::handlers::FifoChannelHandler;
use zenoh::key_expr::KeyExpr;
use zenoh::pubsub::Subscriber;
use zenoh::sample::{Sample, SampleKind};
use zenoh_ext::{AdvancedSubscriber, AdvancedSubscriberBuilderExt, HistoryConfig, RecoveryConfig};

use crate::handshake::Handshake;
use crate::{EVENT_CAPACITY, Error, Result, ServiceInfo, StateEvent, on_liveliness};

/// The first wait before a failed subscription is retried, doubling up
/// to [`SUBSCRIBE_RETRY_MAX`].
const SUBSCRIBE_RETRY_FIRST: Duration = Duration::from_millis(100);
const SUBSCRIBE_RETRY_MAX: Duration = Duration::from_secs(5);

/// How long a lifetime hydrates at most, from its token appearing: past
/// it, the mirror counts as hydrated with whatever arrived.
const HYDRATE_DEADLINE: Duration = Duration::from_secs(5);

/// The daemon's mirrored state: what [`LifetimeMirror`] keeps current.
pub(crate) trait Mirror: Default + std::fmt::Debug + Send + 'static {
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
/// can), and a task per subscription.
struct Lifetime {
    setup: JoinHandle<()>,
    pumps: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl Lifetime {
    /// Stop every task and wait for each, so no sample is still being
    /// applied once this returns.
    async fn end(mut self) {
        self.setup.abort();
        let _ = (&mut self.setup).await;
        let pumps = std::mem::take(
            &mut *self
                .pumps
                .lock()
                .expect("nothing panics while the pump list is held"),
        );
        for pump in pumps {
            pump.abort();
            let _ = pump.await;
        }
    }
}

/// A lifetime dropped without [`Lifetime::end`] (its follow task aborted)
/// still stops its tasks, so their subscriptions and session go too.
impl Drop for Lifetime {
    fn drop(&mut self) {
        self.setup.abort();
        for pump in self
            .pumps
            .lock()
            .expect("nothing panics while the pump list is held")
            .iter()
        {
            pump.abort();
        }
    }
}

impl<M: Mirror> LifetimeMirror<M> {
    /// Follow the daemon on `session`: its liveliness from history on,
    /// then a lifetime per token.
    pub(crate) async fn start(session: &Session, caller: &Arc<Caller>) -> Result<Self> {
        let liveliness = session
            .liveliness()
            .declare_subscriber(keys::daemon_alive())
            .history(true)
            .await
            .map_err(|e| Error::transport("subscribe to the daemon's liveliness", e))?;
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let mut this = Self {
            tracked: Arc::new(Mutex::new(Tracked::default())),
            events,
            handshake: Arc::new(Handshake::default()),
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
            on_liveliness(
                false,
                StateEvent::DaemonDisappeared,
                &self.handshake,
                &self.events,
                &caller,
                service_info,
            );
        }
    }

    /// Subscribe, then report the daemon available: a client that waits
    /// for it finds the subscriptions in place, their history replaying
    /// as it arrives. Then wait for the history to arrive.
    fn begin(&self, session: &Session, caller: &Arc<Caller>) -> Lifetime {
        let deadline = tokio::time::Instant::now() + HYDRATE_DEADLINE;
        let pumps = Arc::new(Mutex::new(Vec::new()));
        let setup = tokio::spawn({
            let this = self.clone();
            let session = session.clone();
            let caller = Arc::clone(caller);
            let pumps = Arc::clone(&pumps);
            async move {
                for subscriber in subscribe::<M>(&session).await {
                    let pump = tokio::spawn(this.clone().pump(subscriber));
                    pumps
                        .lock()
                        .expect("nothing panics while the pump list is held")
                        .push(pump);
                }
                // Before announcing, so the release check's settling can't
                // be missed.
                let events = this.events.subscribe();
                this.lock().alive = true;
                on_liveliness(
                    true,
                    StateEvent::DaemonAppeared,
                    &this.handshake,
                    &this.events,
                    &caller,
                    service_info,
                );
                this.hydrate(events, deadline).await;
            }
        });
        Lifetime { setup, pumps }
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
        let _ = self.events.send(StateEvent::Hydrated);
    }

    async fn pump(self, subscriber: AdvancedSubscriber<FifoChannelHandler<Sample>>) {
        while let Ok(sample) = subscriber.recv_async().await {
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

/// Subscribe to every key `M` mirrors, from history on and with recovery,
/// retrying until all are declared.
async fn subscribe<M: Mirror>(
    session: &Session,
) -> Vec<AdvancedSubscriber<FifoChannelHandler<Sample>>> {
    let mut backoff = SUBSCRIBE_RETRY_FIRST;
    loop {
        let mut subscribers = Vec::new();
        let mut failed = None;
        for key in M::keys() {
            let declared = session
                .declare_subscriber(key)
                .history(HistoryConfig::default().detect_late_publishers())
                .recovery(RecoveryConfig::default().heartbeat())
                .await;
            match declared {
                Ok(subscriber) => subscribers.push(subscriber),
                Err(e) => {
                    failed = Some(e);
                    break;
                }
            }
        }
        let Some(error) = failed else {
            return subscribers;
        };
        warn!(%error, ?backoff, "couldn't subscribe to the daemon's keys; retrying");
        drop(subscribers);
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(SUBSCRIBE_RETRY_MAX);
    }
}

async fn service_info(caller: Arc<Caller>) -> std::result::Result<ServiceInfo, RpcError> {
    caller.call::<daemon::Info>(&(), &()).await
}
