//! Request and reply bodies for the rpcs: shared by the daemon and its
//! clients so the wire shape exists in exactly one place.

use serde::{Deserialize, Serialize};

/// Reply to an rpc that only acknowledges: `{}` on the wire.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ack {}

/// Names a pad: by the id it is known by across reconnects, by that id's
/// key segment, by a bare address, or as `model:<vendor>:<product>` when
/// only one known pad is of that model.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PadRequest {
    /// The pad.
    pub pad: String,
}

/// Asks a pad to buzz, so a person can tell which one it is.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct IdentifyRequest {
    /// The pad, named as in a [`PadRequest`].
    pub pad: String,
    /// How long to buzz, in milliseconds: from 1 to [`IDENTIFY_MAX_MS`].
    pub millis: u32,
}

/// The longest buzz `identify` accepts, in milliseconds.
pub const IDENTIFY_MAX_MS: u32 = 5_000;

/// A profile's text, to be checked.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ValidateRequest {
    /// The profile, as TOML.
    pub toml: String,
}

/// Reply to `validate`: what is wrong with the profile, or nothing.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Validation {
    /// What is wrong, and where; absent for a profile the daemon would
    /// keep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<crate::payload::Problem>,
}

/// A profile to keep, under a name: a new one, or in place of the one
/// that has the name.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PutProfileRequest {
    /// The profile's name: lowercase letters, digits and hyphens.
    pub name: String,
    /// The profile, as TOML.
    pub toml: String,
}

/// Reply to `schema`: a preset's fields, for a form.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct PresetForm {
    /// The fields, in the order a form shows them.
    pub fields: Vec<crate::form::Field>,
}

/// Names a profile.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ProfileRequest {
    /// The profile's name.
    pub name: String,
}

/// Names a setup, which goes by the name of the profile it runs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SetupRequest {
    /// The setup's name.
    pub name: String,
}

/// Asks a running setup to take up its profile as it is kept now.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct UpdateRequest {
    /// The setup's name.
    pub name: String,
    /// Whether to do it, or only to say what doing it would cost.
    pub apply: bool,
}

/// Reply to `update`: what the change costs, or cost.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Updated {
    /// Whether the setup's virtual pads are made anew, which a game
    /// sees as its pad disconnecting and another connecting.
    pub disruptive: bool,
}

/// Asks a node of a setup to select another input or output.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SelectRequest {
    /// The setup's name.
    pub name: String,
    /// The node's name in the profile.
    pub node: String,
    /// What to select: one of the node's choices by name, or by its
    /// place among them, counted from 0. Absent, the node moves to its
    /// next choice, as its switch button would move it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice: Option<String>,
}

/// Reply to `reload`: what the daemon found changed in its files, and
/// what it is doing about it.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Reloaded {
    /// The profiles that are new or whose text changed, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<String>,
    /// The profiles whose files are gone, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
    /// Of the changed profiles, those with something wrong with them,
    /// which are kept and listed but can't be run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wrong: Vec<crate::payload::Profile>,
    /// The running setups being brought up to their changed profiles:
    /// as they run where the change allows, by a restart otherwise.
    /// They are still changing when this reply is sent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub updating: Vec<String>,
}

/// The tray's notification switches, one for each kind of thing it says.
/// All are on until one is turned off.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct NotificationSettings {
    /// Say when a setup starts or stops.
    pub setup_started: bool,
    /// Say when something is wrong: a setup held for a missing pad, or
    /// failed; a setup starting with pads that are only grabbed or can't
    /// rumble; the daemon stopping, and coming back.
    pub setup_trouble: bool,
    /// Say when a pad connects or leaves.
    pub pad_connection: bool,
    /// Say when the daemon refuses something asked from the tray's menu.
    pub refused: bool,
}

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            setup_started: true,
            setup_trouble: true,
            pad_connection: true,
            refused: true,
        }
    }
}

impl NotificationSettings {
    /// These switches with those `patch` sets changed.
    #[must_use]
    pub fn merged_with(self, patch: &NotificationSettingsPatch) -> Self {
        Self {
            setup_started: patch.setup_started.unwrap_or(self.setup_started),
            setup_trouble: patch.setup_trouble.unwrap_or(self.setup_trouble),
            pad_connection: patch.pad_connection.unwrap_or(self.pad_connection),
            refused: patch.refused.unwrap_or(self.refused),
        }
    }
}

/// Body of the tray's `update`: only the switches being changed.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NotificationSettingsPatch {
    /// See [`NotificationSettings::setup_started`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_started: Option<bool>,
    /// See [`NotificationSettings::setup_trouble`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_trouble: Option<bool>,
    /// See [`NotificationSettings::pad_connection`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pad_connection: Option<bool>,
    /// See [`NotificationSettings::refused`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<bool>,
}

/// Reply to the tray's `reload`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReloadReport {
    /// Whether the file said something other than what the tray held, so
    /// the reload changed a switch.
    pub changed: bool,
}

/// Reply to a service's `info` rpc: the handshake a client runs before
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
    /// The file the service keeps its settings in, from one that has
    /// one: the tray's. The daemon says where its files are in its own
    /// state.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub config: String,
}
