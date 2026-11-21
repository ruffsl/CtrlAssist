//! The reactor: the one task that owns what the daemon knows and serves.
//!
//! Pads connecting and leaving, and every rpc, arrive here and are
//! handled one at a time, so nothing it owns needs a lock. What it knows
//! goes out as state, each key from a cache so a client that connects
//! later still receives it.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ctrlassist_core::Level;
use ctrlassist_device::{Database, InputThread, Pad, Report};
use ctrlassist_protocol::endpoint::{daemon, pad, profile, setup};
use ctrlassist_protocol::error::{ErrorCode, WireError};
use ctrlassist_protocol::serve::Responder;
use ctrlassist_protocol::{VERSION, keys, rpc};
use serde::Serialize;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use tracing::{info, warn};
use zenoh::Session;
use zenoh::bytes::Encoding;
use zenoh::handlers::FifoChannelHandler;
use zenoh::query::{Query, Queryable};
use zenoh_ext::{AdvancedPublisher, AdvancedPublisherBuilderExt, CacheConfig, MissDetectionConfig};

use crate::book::Book;
use crate::hotplug::Seen;
use crate::profiles::Profiles;
use crate::setups::{Done, Hiding, Setups};
use crate::store::Store;

/// Heartbeat period for sample-miss recovery on cached publishers: a
/// heartbeat goes out only in a period after a publish, so an idle key
/// costs a subscriber nothing.
const MISS_HEARTBEAT: Duration = Duration::from_millis(500);

/// How hard `identify` runs a pad's motors: plainly felt, short of full.
const BUZZ: Level = Level {
    strong: 0xa000,
    weak: 0xa000,
};

type Rpc = Queryable<FifoChannelHandler<Query>>;

/// The rpc families the reactor answers, one queryable each.
pub(crate) struct Rpcs {
    pub(crate) daemon: Rpc,
    pub(crate) pad: Rpc,
    pub(crate) profile: Rpc,
    pub(crate) setup: Rpc,
}

impl Rpcs {
    /// Declares every family on `session`.
    pub(crate) async fn declare(session: &Session) -> zenoh::Result<Self> {
        Ok(Self {
            daemon: session.declare_queryable(daemon::Incoming::wild()).await?,
            pad: session.declare_queryable(pad::Incoming::wild()).await?,
            profile: session.declare_queryable(profile::Incoming::wild()).await?,
            setup: session.declare_queryable(setup::Incoming::wild()).await?,
        })
    }
}

/// What the reactor owns.
pub(crate) struct Reactor {
    session: Session,
    pub(crate) book: Book,
    store: Store,
    pub(crate) profiles: Profiles,
    pub(crate) setups: Setups,
    /// The input thread, which runs the setups.
    pub(crate) input: Arc<InputThread>,
    /// The layouts of pads whose drivers don't present a gamepad's.
    pub(crate) database: Arc<Database>,
    /// How a setup's pads are kept from games.
    pub(crate) hiding: Arc<Hiding>,
    /// Blocking work under way for the setups, each piece coming back as
    /// what it did.
    pub(crate) work: JoinSet<Done>,
    /// One cached publisher per key published now.
    publishers: BTreeMap<String, AdvancedPublisher<'static>>,
    /// The buzzes under way; each ends itself.
    buzzes: JoinSet<()>,
}

impl std::fmt::Debug for Reactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reactor")
            .field("book", &self.book)
            .finish_non_exhaustive()
    }
}

/// Starts a pad buzzing and keeps it buzzing for `millis`. The reply
/// goes once the motors run, or with why they don't.
async fn buzz(node: PathBuf, millis: u32, responder: Responder<pad::Identify>) {
    let started = tokio::task::spawn_blocking(move || {
        let file = OpenOptions::new().read(true).write(true).open(&node)?;
        let mut pad = Pad::from_fd(OwnedFd::from(file))?;
        pad.rumble(BUZZ)?;
        Ok::<_, ctrlassist_device::PadError>(pad)
    })
    .await;
    let pad = match started {
        Ok(Ok(pad)) => pad,
        Ok(Err(error)) => {
            let refusal =
                WireError::refusal(ErrorCode::PadRefused, format!("the pad refused: {error}"));
            return responder.err(&refusal).await;
        }
        Err(error) => {
            return responder.err(&WireError::internal(error.to_string())).await;
        }
    };
    responder.ok(&rpc::Ack {}).await;
    // The caller's own duration. Dropping the pad stops its motors, here
    // or wherever this task is dropped.
    tokio::time::sleep(Duration::from_millis(u64::from(millis))).await;
    let _ = tokio::task::spawn_blocking(move || drop(pad)).await;
}

impl Reactor {
    pub(crate) fn new(
        session: Session,
        book: Book,
        store: Store,
        profiles: Profiles,
        input: Arc<InputThread>,
        database: Database,
        hiding: Hiding,
    ) -> Self {
        Self {
            session,
            book,
            store,
            profiles,
            setups: Setups::default(),
            input,
            database: Arc::new(database),
            hiding: Arc::new(hiding),
            work: JoinSet::new(),
            publishers: BTreeMap::new(),
            buzzes: JoinSet::new(),
        }
    }

    /// Publishes `body` on `key`, from a cache.
    pub(crate) async fn publish(
        &mut self,
        key: String,
        body: &impl Serialize,
    ) -> zenoh::Result<()> {
        let bytes = serde_json::to_vec(body)?;
        if !self.publishers.contains_key(&key) {
            let publisher = self
                .session
                .declare_publisher(key.clone())
                .encoding(Encoding::APPLICATION_JSON)
                .cache(CacheConfig::default().max_samples(1))
                .sample_miss_detection(
                    MissDetectionConfig::default().sporadic_heartbeat(MISS_HEARTBEAT),
                )
                .publisher_detection()
                .await?;
            self.publishers.insert(key.clone(), publisher);
        }
        self.publishers[&key].put(bytes).await
    }

    /// Takes `key` back: subscribers see it deleted.
    pub(crate) async fn unpublish(&mut self, key: &str) {
        if let Some(publisher) = self.publishers.remove(key)
            && let Err(error) = publisher.delete().await
        {
            warn!(key, %error, "withdrawing state failed");
        }
    }

    /// Publishes every pad in the book.
    pub(crate) async fn publish_pads(&mut self) -> zenoh::Result<()> {
        let ids: Vec<String> = self.book.ids().map(str::to_string).collect();
        for id in ids {
            self.publish_pad(&id).await?;
        }
        Ok(())
    }

    async fn publish_pad(&mut self, id: &str) -> zenoh::Result<()> {
        match self.book.pad(id) {
            Some(pad) => self.publish(keys::pad(&keys::pad_id(id)), &pad).await,
            None => Ok(()),
        }
    }

    /// Publishes every profile kept.
    pub(crate) async fn publish_profiles(&mut self) -> zenoh::Result<()> {
        let names: Vec<String> = self.profiles.names().map(str::to_string).collect();
        for name in names {
            self.publish_profile(&name).await?;
        }
        Ok(())
    }

    async fn publish_profile(&mut self, name: &str) -> zenoh::Result<()> {
        match self.profiles.profile(name) {
            Some(profile) => self.publish(keys::profile(name), &profile).await,
            None => Ok(()),
        }
    }

    async fn profile_rpc(&mut self, query: Query) {
        match profile::Incoming::receive(query).await {
            Some(((), profile::Incoming::Validate(request, responder))) => {
                let validation = rpc::Validation {
                    problem: Profiles::problem(&request.toml),
                };
                responder.ok(&validation).await;
            }
            Some(((), profile::Incoming::Put(request, responder))) => {
                match self.profiles.put(&request.name, &request.toml) {
                    Ok(()) => {
                        info!(profile = request.name, "a profile was put");
                        if let Err(error) = self.publish_profile(&request.name).await {
                            warn!(profile = request.name, %error, "publishing a profile failed");
                        }
                        responder.ok(&rpc::Ack {}).await;
                    }
                    Err(refusal) => responder.err(&refusal).await,
                }
            }
            Some(((), profile::Incoming::Delete(request, responder))) => {
                match self.profiles.delete(&request.name) {
                    Ok(()) => {
                        info!(profile = request.name, "a profile was deleted");
                        self.unpublish(&keys::profile(&request.name)).await;
                        responder.ok(&rpc::Ack {}).await;
                    }
                    Err(refusal) => responder.err(&refusal).await,
                }
            }
            // Refused already, with a typed error.
            None => {}
        }
    }

    /// Publishes a pad's state; a failure is logged.
    pub(crate) async fn publish_pad_logged(&mut self, id: &str) {
        if let Err(error) = self.publish_pad(id).await {
            warn!(pad = id, %error, "publishing a pad's state failed");
        }
    }

    fn save(&self) {
        if let Err(error) = self.store.save(&self.book.known()) {
            warn!(%error, "the known pads couldn't be saved; they are kept until the daemon stops");
        }
    }

    /// Takes in what the hotplug watch saw.
    pub(crate) async fn saw(&mut self, seen: Seen) {
        let arrived = matches!(seen, Seen::Arrived(..));
        let (changed, remembered): (Vec<String>, bool) = match seen {
            Seen::Arrived(found, rumble) => {
                let changed = self.book.arrived(&found, rumble);
                let remembered = changed.iter().any(|changed| changed.remembered);
                if let Some(pad) = changed.last() {
                    info!(
                        pad = pad.id,
                        name = found.identifiers.name,
                        "a pad connected"
                    );
                }
                (
                    changed.into_iter().map(|changed| changed.id).collect(),
                    remembered,
                )
            }
            Seen::Left(node) => {
                let left: Vec<String> = self.book.left(&node).into_iter().collect();
                if let Some(pad) = left.first() {
                    info!(pad, "a pad disconnected");
                }
                (left, false)
            }
        };
        if remembered {
            self.save();
        }
        for id in &changed {
            self.publish_pad_logged(id).await;
        }
        // The last is the pad that connected, if one did.
        if arrived && let Some(id) = changed.last() {
            self.pad_arrived(id).await;
        }
    }

    async fn daemon_rpc(&self, query: Query) {
        match daemon::Incoming::receive(query).await {
            Some(((), daemon::Incoming::Info((), responder))) => {
                let info = rpc::ServiceInfo {
                    version: VERSION.to_string(),
                    keys: self.publishers.keys().cloned().collect(),
                };
                responder.ok(&info).await;
            }
            // Refused already, with a typed error.
            None => {}
        }
    }

    /// The node to buzz for an `identify`, or why not.
    fn buzzable(&self, request: &rpc::IdentifyRequest) -> Result<PathBuf, WireError> {
        if !(1..=rpc::IDENTIFY_MAX_MS).contains(&request.millis) {
            return Err(WireError::bad_request(format!(
                "a buzz lasts from 1 to {} ms, not {}",
                rpc::IDENTIFY_MAX_MS,
                request.millis
            )));
        }
        let id = self.book.resolve(&request.pad)?;
        let node = self.book.node(id).ok_or_else(|| {
            WireError::refusal(ErrorCode::PadDisconnected, format!("{id} isn't connected"))
        })?;
        if self.book.rumble(id) == Some(false) {
            return Err(WireError::refusal(
                ErrorCode::PadWithoutRumble,
                format!("{id} has no rumble to buzz with"),
            ));
        }
        Ok(node.to_path_buf())
    }

    async fn pad_rpc(&mut self, query: Query) {
        match pad::Incoming::receive(query).await {
            Some(((), pad::Incoming::Identify(request, responder))) => {
                match self.buzzable(&request) {
                    Ok(node) => {
                        self.buzzes.spawn(buzz(node, request.millis, responder));
                    }
                    Err(refusal) => responder.err(&refusal).await,
                }
            }
            Some(((), pad::Incoming::Forget(request, responder))) => {
                match self.book.forget(&request.pad) {
                    Ok(id) => {
                        self.save();
                        self.unpublish(&keys::pad(&keys::pad_id(&id))).await;
                        info!(pad = id, "a pad was forgotten");
                        responder.ok(&rpc::Ack {}).await;
                    }
                    Err(refusal) => responder.err(&refusal).await,
                }
            }
            // Refused already, with a typed error.
            None => {}
        }
    }

    /// Runs until told to stop. What it published goes with it.
    pub(crate) async fn run(
        mut self,
        rpcs: Rpcs,
        mut seen: UnboundedReceiver<Seen>,
        mut reports: UnboundedReceiver<Report>,
        mut stop: oneshot::Receiver<()>,
    ) {
        loop {
            tokio::select! {
                _ = &mut stop => break,
                Ok(query) = rpcs.daemon.recv_async() => self.daemon_rpc(query).await,
                Ok(query) = rpcs.pad.recv_async() => self.pad_rpc(query).await,
                Ok(query) = rpcs.profile.recv_async() => self.profile_rpc(query).await,
                Ok(query) = rpcs.setup.recv_async() => self.setup_rpc(query).await,
                Some(report) = reports.recv() => self.reported(report).await,
                Some(Ok(done)) = self.work.join_next() => self.done(done).await,
                Some(seen) = seen.recv() => self.saw(seen).await,
                // A finished buzz has nothing to say.
                Some(_) = self.buzzes.join_next() => {}
            }
        }
        // Pads still buzzing stop as their tasks are dropped.
        self.buzzes.shutdown().await;
        // What is being made or dropped finishes; the setups themselves
        // go with the input thread.
        while self.work.join_next().await.is_some() {}
    }
}
