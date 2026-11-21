//! What the daemon publishes as state.

use serde::{Deserialize, Serialize};

/// The daemon's own state, published on [`keys::daemon`](crate::keys::daemon).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Daemon {
    /// The daemon's release.
    pub version: String,
    /// Whether the kernel granted the input thread real-time scheduling.
    /// Without it, latency under load is not dependable.
    pub realtime: bool,
}
