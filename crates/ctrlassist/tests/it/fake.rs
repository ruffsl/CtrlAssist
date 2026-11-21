//! A stand-in daemon built from the protocol's `serve` dispatch, as a
//! third party might mock one: it listens on a socket, holds its
//! liveliness token, answers `info` with a chosen release, and publishes
//! whatever keys a test puts, through cached publishers configured like
//! the real daemon's, listing them in its `info` reply.

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ctrlassist_protocol::endpoint::daemon;
use ctrlassist_protocol::keys;
use serde_json::Value;
use tokio::task::JoinHandle;
use zenoh::Session;
use zenoh::liveliness::LivelinessToken;
use zenoh_ext::{AdvancedPublisher, AdvancedPublisherBuilderExt, CacheConfig, MissDetectionConfig};

/// A running fake daemon. Its publishers live as long as it does.
pub(crate) struct FakeDaemon {
    session: Session,
    token: LivelinessToken,
    serve: JoinHandle<()>,
    publishers: HashMap<String, AdvancedPublisher<'static>>,
    /// The keys the `info` reply lists.
    listed: Arc<Mutex<BTreeSet<String>>>,
}

/// Answer `info` on `session` with `release` and the `listed` keys.
pub(crate) async fn answer_info(
    session: &Session,
    release: &str,
    listed: &Arc<Mutex<BTreeSet<String>>>,
) -> JoinHandle<()> {
    let queryable = session
        .declare_queryable(keys::daemon_rpc_wild())
        .await
        .unwrap();
    let release = release.to_string();
    let listed = Arc::clone(listed);
    tokio::spawn(async move {
        while let Ok(query) = queryable.recv_async().await {
            if let Some(((), daemon::Incoming::Info((), responder))) =
                daemon::Incoming::receive(query).await
            {
                let info = ctrlassist::ServiceInfo {
                    version: release.clone(),
                    keys: listed.lock().unwrap().iter().cloned().collect(),
                };
                responder.ok(&info).await;
            }
        }
    })
}

impl FakeDaemon {
    /// Serve on `socket`, reporting `release`.
    pub(crate) async fn start(socket: &Path, release: &str) -> Self {
        let session = zenoh::open(ctrlassist_protocol::bootstrap::peer_config(socket).unwrap())
            .await
            .unwrap();
        let listed = Arc::new(Mutex::new(BTreeSet::new()));
        let serve = answer_info(&session, release, &listed).await;
        let token = session
            .liveliness()
            .declare_token(keys::daemon_alive())
            .await
            .unwrap();
        Self {
            session,
            token,
            serve,
            publishers: HashMap::new(),
            listed,
        }
    }

    /// How many peers this daemon's session has a link to.
    pub(crate) async fn peers(&self) -> usize {
        self.session.info().peers_zid().await.count()
    }

    /// Publish `body` on `key`, declaring its publisher on first use.
    pub(crate) async fn put(&mut self, key: &str, body: &Value) {
        if !self.publishers.contains_key(key) {
            let publisher = self
                .session
                .declare_publisher(key.to_string())
                .cache(CacheConfig::default().max_samples(1))
                .sample_miss_detection(
                    MissDetectionConfig::default().sporadic_heartbeat(Duration::from_millis(500)),
                )
                .publisher_detection()
                .await
                .unwrap();
            self.publishers.insert(key.to_string(), publisher);
            self.listed.lock().unwrap().insert(key.to_string());
        }
        self.publishers[key]
            .put(serde_json::to_vec(body).unwrap())
            .await
            .unwrap();
    }

    /// Delete `key` through its publisher, then drop the publisher, as
    /// the real daemon does.
    pub(crate) async fn delete(&mut self, key: &str) {
        let publisher = self.publishers.remove(key).unwrap();
        self.listed.lock().unwrap().remove(key);
        publisher.delete().await.unwrap();
    }

    /// List `key` in the `info` reply without publishing it: a key the
    /// daemon published, then deleted after the reply.
    pub(crate) fn list_unpublished(&self, key: &str) {
        self.listed.lock().unwrap().insert(key.to_string());
    }

    /// Stop: the token, the rpcs and the publishers go with the session.
    pub(crate) async fn stop(self) {
        self.serve.abort();
        drop(self.publishers);
        drop(self.token);
        self.session.close().await.unwrap();
    }
}
