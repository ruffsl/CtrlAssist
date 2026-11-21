//! Request and reply bodies for the rpcs: shared by the daemon and its
//! clients so the wire shape exists in exactly one place.

use serde::{Deserialize, Serialize};

/// Reply to an rpc that only acknowledges: `{}` on the wire.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ack {}

/// Reply to the daemon's `info` rpc: the handshake a client runs before
/// anything else.
///
/// This shape is frozen: a client of any release must still read it.
/// Fields may be added, never changed or removed; an added field carries
/// `#[serde(default)]`, so a newer client still reads an older daemon's
/// reply, and unknown fields are never denied, so an older client reads a
/// newer one's. `version` is the daemon's [`crate::VERSION`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ServiceInfo {
    /// The daemon's release; clients check it with [`crate::compatible`].
    pub version: String,
    /// The keys the daemon publishes from a cache right now, so a client
    /// knows when its history has arrived: once it holds a sample for
    /// each one its subscriptions match. Empty from a daemon that
    /// publishes none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keys: Vec<String>,
}
