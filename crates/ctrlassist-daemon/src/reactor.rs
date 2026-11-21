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
use std::time::Duration;

use ctrlassist_core::Level;
use ctrlassist_device::Pad;
use ctrlassist_protocol::endpoint::{daemon, pad};
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

type Rpcs = Queryable<FifoChannelHandler<Query>>;

/// What the reactor owns.
pub(crate) struct Reactor {
    session: Session,
    book: Book,
    store: Store,
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
                WireError::pad(ErrorCode::PadRefused, format!("the pad refused: {error}"));
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
    pub(crate) fn new(session: Session, book: Book, store: Store) -> Self {
        Self {
            session,
            book,
            store,
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
    async fn unpublish(&mut self, key: &str) {
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

    fn save(&self) {
        if let Err(error) = self.store.save(&self.book.known()) {
            warn!(%error, "the known pads couldn't be saved; they are kept until the daemon stops");
        }
    }

    /// Takes in what the hotplug watch saw.
    pub(crate) async fn saw(&mut self, seen: Seen) {
        let (changed, remembered) = match seen {
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
        for id in changed {
            if let Err(error) = self.publish_pad(&id).await {
                warn!(pad = id, %error, "publishing a pad's state failed");
            }
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
            WireError::pad(ErrorCode::PadDisconnected, format!("{id} isn't connected"))
        })?;
        if self.book.rumble(id) == Some(false) {
            return Err(WireError::pad(
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
        daemon_rpcs: Rpcs,
        pad_rpcs: Rpcs,
        mut seen: UnboundedReceiver<Seen>,
        mut stop: oneshot::Receiver<()>,
    ) {
        loop {
            tokio::select! {
                _ = &mut stop => break,
                Ok(query) = daemon_rpcs.recv_async() => self.daemon_rpc(query).await,
                Ok(query) = pad_rpcs.recv_async() => self.pad_rpc(query).await,
                Some(seen) = seen.recv() => self.saw(seen).await,
                // A finished buzz has nothing to say.
                Some(_) = self.buzzes.join_next() => {}
            }
        }
        // Pads still buzzing stop as their tasks are dropped.
        self.buzzes.shutdown().await;
    }
}
