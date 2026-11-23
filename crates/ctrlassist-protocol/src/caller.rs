//! Matching-gated rpc calls — the client half of the querier-matching
//! law (pinned in `tests/querier_matching.rs`). A [`Caller`] holds one
//! declared querier per exact rpc key; every call awaits the querier's
//! matching status before firing, so the zero-matching-queryables
//! instant-no-reply race cannot happen: matching ⇒ routable. A
//! service absent past the deadline fails fast as
//! [`RpcError::Unavailable`], and the liveliness token stays a
//! *presence display* signal only — rpc readiness is the querier's job.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use zenoh::Session;
use zenoh::bytes::Encoding;
use zenoh::query::Querier;

use crate::endpoint::Unary;
use crate::error::WireError;

/// Per-call timeout once a matched queryable holds the query, for every
/// rpc but an [`untimed`](crate::endpoint::Unary::UNTIMED) one.
pub const RPC_TIMEOUT: Duration = Duration::from_secs(5);

/// What an untimed rpc's query is declared with, zenoh having no query
/// without a time limit: the longest a query's limit can be on the wire,
/// which carries it as a count of milliseconds, and which zenoh and tokio
/// take as never. Its caller ends the wait by the service going instead.
const UNLIMITED: Duration = Duration::from_millis(u64::MAX);

/// How long a call waits for a matching queryable to become routable
/// before failing [`RpcError::Unavailable`]. Route propagation on a
/// fresh link completes in tens of milliseconds; the margin covers a
/// service that is mid-start.
pub const MATCHING_DEADLINE: Duration = Duration::from_secs(2);

/// How an rpc call ended, when it didn't end in a typed reply.
#[derive(Debug)]
#[non_exhaustive]
pub enum RpcError {
    /// The service refused the request.
    Service(WireError),
    /// No service matched the key within [`MATCHING_DEADLINE`].
    Unavailable {
        /// The rpc key no queryable matched.
        key: String,
    },
    /// A service held the query but didn't reply within [`RPC_TIMEOUT`].
    Timeout {
        /// The rpc key the query was sent on.
        key: String,
    },
    /// The query closed without a reply after matching confirmed (the
    /// service dropped mid-call). For a write, the outcome is unknown.
    NoReply {
        /// The rpc key the query was sent on.
        key: String,
    },
    /// The local zenoh session failed the call.
    Transport {
        /// The rpc key the call addressed.
        key: String,
        /// The zenoh failure: declaring the querier, tracking its
        /// matching status, or sending the query.
        source: zenoh::Error,
    },
    /// A reply that isn't the rpc's response type, or an error payload
    /// that isn't a [`WireError`].
    Undecodable {
        /// The rpc key the reply came from.
        key: String,
        /// The JSON decode failure.
        source: serde_json::Error,
    },
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Service(error) => write!(f, "{} ({})", error.message, error.code),
            Self::Unavailable { key } => {
                write!(f, "no rpc service for {key} within {MATCHING_DEADLINE:?}")
            }
            Self::Timeout { key } => write!(f, "rpc {key}: timed out"),
            Self::NoReply { key } => write!(f, "rpc {key}: no reply"),
            Self::Transport { key, .. } => write!(f, "rpc {key}: transport failure"),
            Self::Undecodable { key, .. } => write!(f, "rpc {key}: undecodable reply"),
        }
    }
}

impl std::error::Error for RpcError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport { source, .. } => Some(source.as_ref()),
            Self::Undecodable { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Matching-gated rpc caller over one zenoh session. Queriers are cached
/// per key, so repeat calls skip declaration and their matching status
/// is already resolved.
///
/// The cache is never evicted: a long-lived client keeps a querier for
/// each rpc it has called. That's a few dozen, each cheap.
#[derive(Debug)]
pub struct Caller {
    session: Session,
    queriers: Mutex<HashMap<String, Arc<Querier<'static>>>>,
}

impl Caller {
    /// A caller over `session`. Declares nothing up front: each key's
    /// querier is declared on its first call.
    pub fn new(session: Session) -> Self {
        Self {
            session,
            queriers: Mutex::new(HashMap::new()),
        }
    }

    /// One rpc, one typed reply. Matching-gated; re-issued once when
    /// the reply channel closes empty only if `R::REISSUE` (reads) —
    /// a write that minted state before a lost reply must surface as
    /// an error rather than mint twice.
    ///
    /// An [`untimed`](Unary::UNTIMED) rpc's call waits for its reply with
    /// no time limit: until the service answers, drops the query, or the
    /// link to it goes. A caller that must not outwait a service that is
    /// still there but going drops the call when the service's
    /// liveliness token goes, as the SDK's clients do.
    pub async fn call<R: Unary>(
        &self,
        target: &R::Target,
        request: &R::Request,
    ) -> Result<R::Response, RpcError> {
        let key = R::key(target);
        let body = encode(request);
        let timeout = if R::UNTIMED { UNLIMITED } else { RPC_TIMEOUT };
        let querier = self.querier(&key, timeout).await?;
        self.await_matching(&querier).await?;
        let bytes = match issue(&querier, &body).await {
            Err(RpcError::NoReply { .. }) if R::REISSUE => {
                self.await_matching(&querier).await?;
                issue(&querier, &body).await?
            }
            outcome => outcome?,
        };
        decode(&key, &bytes)
    }

    /// The cached single-reply querier for `key`, whose queries wait
    /// `timeout` for a reply. A key is one rpc's, so its timeout is
    /// always the same.
    async fn querier(
        &self,
        key: &str,
        timeout: Duration,
    ) -> Result<Arc<Querier<'static>>, RpcError> {
        if let Some(querier) = self
            .queriers
            .lock()
            .expect("no code panics while the querier cache lock is held, so it is never poisoned")
            .get(key)
        {
            return Ok(Arc::clone(querier));
        }
        let declared = Arc::new(
            self.session
                .declare_querier(key.to_string())
                .timeout(timeout)
                .await
                .map_err(|source| transport(key, source))?,
        );
        // A concurrent first call may have raced us; keep whichever
        // landed first so the loser's querier undeclares on drop.
        Ok(Arc::clone(
            self.queriers
                .lock()
                .expect(
                    "no code panics while the querier cache lock is held, so it is never poisoned",
                )
                .entry(key.to_string())
                .or_insert(declared),
        ))
    }

    /// Wait (bounded) until the querier has a matching routable
    /// queryable — the fact liveliness tokens cannot provide, since
    /// token arrival races queryable-route propagation.
    async fn await_matching(&self, querier: &Querier<'static>) -> Result<(), RpcError> {
        let key = querier.key_expr().as_str();
        if status(querier).await?.matching() {
            return Ok(());
        }
        let listener = querier
            .matching_listener()
            .await
            .map_err(|source| transport(key, source))?;
        // Re-check after subscribing: the flip may have landed
        // between the status read and the listener declaration.
        if status(querier).await?.matching() {
            return Ok(());
        }
        tokio::time::timeout(MATCHING_DEADLINE, async {
            while let Ok(status) = listener.recv_async().await {
                if status.matching() {
                    return Ok(());
                }
            }
            Err(transport(key, "matching listener closed".into()))
        })
        .await
        .map_err(|_| RpcError::Unavailable {
            key: key.to_string(),
        })?
    }
}

async fn status(querier: &Querier<'static>) -> Result<zenoh::matching::MatchingStatus, RpcError> {
    querier
        .matching_status()
        .await
        .map_err(|source| transport(querier.key_expr().as_str(), source))
}

/// Issue one query and take its first reply's payload.
async fn issue(querier: &Querier<'static>, body: &[u8]) -> Result<Vec<u8>, RpcError> {
    let key = querier.key_expr().as_str().to_string();
    let replies = querier
        .get()
        .payload(body.to_vec())
        .encoding(Encoding::APPLICATION_JSON)
        .await
        .map_err(|source| transport(&key, source))?;
    match replies.recv_async().await {
        Ok(reply) => reply_bytes(&key, reply),
        // Empty channel close: the query finalized with zero replies.
        // With matching confirmed this is the rare dropped-mid-call
        // case, not the routing race.
        Err(_) => Err(RpcError::NoReply { key }),
    }
}

fn transport(key: &str, source: zenoh::Error) -> RpcError {
    RpcError::Transport {
        key: key.to_string(),
        source,
    }
}

fn encode<T: Serialize + ?Sized>(body: &T) -> Vec<u8> {
    serde_json::to_vec(body)
        .expect("rpc bodies are structs and string-keyed maps, which always encode as JSON")
}

fn decode<T: DeserializeOwned>(key: &str, bytes: &[u8]) -> Result<T, RpcError> {
    serde_json::from_slice(bytes).map_err(|source| RpcError::Undecodable {
        key: key.to_string(),
        source,
    })
}

/// A reply's payload, or the call's failure: the service's
/// [`WireError`], or zenoh's synthetic timeout reply.
fn reply_bytes(key: &str, reply: zenoh::query::Reply) -> Result<Vec<u8>, RpcError> {
    match reply.result() {
        Ok(sample) => Ok(sample.payload().to_bytes().into_owned()),
        Err(err) => {
            let bytes = err.payload().to_bytes();
            match serde_json::from_slice::<WireError>(&bytes) {
                Ok(error) => Err(RpcError::Service(error)),
                // Zenoh's query timeout arrives as a synthetic
                // ReplyError("Timeout"), not a WireError — keep "service
                // slow" distinguishable from a genuine service error. The
                // whole payload, so a newer service's refusal that merely
                // mentions a timeout stays undecodable rather than read as
                // one.
                Err(_) if *bytes == *b"Timeout" => Err(RpcError::Timeout {
                    key: key.to_string(),
                }),
                Err(source) => Err(RpcError::Undecodable {
                    key: key.to_string(),
                    source,
                }),
            }
        }
    }
}
