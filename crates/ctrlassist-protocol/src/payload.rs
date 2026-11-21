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

/// A pad the daemon knows: one connected now, or one it remembers. Its
/// key is [`keys::pad`](crate::keys::pad) of
/// [`keys::pad_id`](crate::keys::pad_id) of `id`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Pad {
    /// What the pad is known by across reconnects, as a profile names
    /// it: `address:…`, `serial:…`, `port:…` or `model:…`.
    pub id: String,
    /// The pad's own name.
    pub name: String,
    /// Its vendor number.
    pub vendor: u16,
    /// Its product number.
    pub product: u16,
    /// Whether it is connected now.
    pub connected: bool,
    /// Whether it is hidden from games.
    pub hidden: bool,
    /// The setup that uses it, if one does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup: Option<String>,
    /// Whether it can rumble; absent when the daemon couldn't tell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rumble: Option<bool>,
}

/// What a virtual pad can present itself as. Its key is
/// [`keys::identity`](crate::keys::identity) of `id`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    /// The identity's name, as a profile writes it.
    pub id: String,
    /// The name a game sees.
    pub name: String,
    /// The vendor number a game sees.
    pub vendor: u16,
    /// The product number a game sees.
    pub product: u16,
    /// Whether a virtual pad with this identity can be made now.
    pub available: bool,
}
