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
    /// How the pads of a running setup are kept from games.
    pub hiding: Hiding,
    /// The directory it keeps the user's files in, the profiles among
    /// them, for a client that offers to show it. Empty from a daemon
    /// that doesn't say.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub config: String,
    /// The hiding service's release, while it is there and says: the
    /// daemon is its only client, so only the daemon can. None with no
    /// service, or from one too old to be asked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hider_version: Option<String>,
}

/// How the pads of a running setup are kept from games.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Hiding {
    /// The hiding service makes them absent: nothing but the daemon can
    /// open them.
    Service,
    /// The hiding service isn't there, so each is grabbed: programs that
    /// read its event or joystick node hear nothing, but one that reads
    /// it some other way, as Steam reads a PlayStation pad, still sees it.
    Grab,
}

impl Hiding {
    /// The mechanism as it travels on the wire, and as it is shown.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Service => "service",
            Self::Grab => "grab",
        }
    }
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
    /// The bus it is connected on now, as the kernel numbers them: 3 is
    /// USB, 5 Bluetooth. Absent while it isn't connected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bus: Option<u16>,
}

/// A profile the daemon keeps. Its key is
/// [`keys::profile`](crate::keys::profile) of `name`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    /// The profile's name.
    pub name: String,
    /// The profile, as the TOML it was written as.
    pub toml: String,
    /// What is wrong with it, if anything: a profile the daemon accepted
    /// has nothing wrong, but a file on disk may have been changed since,
    /// or written for a later release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<Problem>,
}

/// What is wrong with a profile, and where.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// Where, in the file's own terms, such as `node.second.inputs[1]`
    /// or `line 3`. Empty for a problem with the file as a whole.
    pub at: String,
    /// What.
    pub message: String,
}

impl From<crate::profile::ProfileError> for Problem {
    fn from(error: crate::profile::ProfileError) -> Self {
        Self {
            at: error.at,
            message: error.problem,
        }
    }
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.at.is_empty() {
            f.write_str(&self.message)
        } else {
            write!(f, "{}: {}", self.at, self.message)
        }
    }
}

/// A setup: a profile being run. Its key is
/// [`keys::setup`](crate::keys::setup) of `name`, published from when it
/// is asked for until it is stopped.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Setup {
    /// The setup's name: the profile it runs.
    pub name: String,
    /// Where it is in its life.
    pub phase: Phase,
    /// Why it failed, for a person to read; absent in any other phase.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Its pads, in the order of their names in the profile.
    pub sources: Vec<SetupSource>,
    /// Its virtual pads, in the order of their names in the profile.
    pub sinks: Vec<SetupSink>,
    /// Its nodes that select one input or one output, and which each has
    /// selected. A node that blends its inputs, or sends to every output,
    /// isn't listed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<SetupNode>,
    /// Whether the profile as it is kept now is another than the one
    /// this setup runs, or is gone: an `update` would change something,
    /// or be refused for want of a profile.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stale: bool,
}

/// A node of a setup that selects: a mux that follows its switch, or a
/// demux that sends to one output at a time.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SetupNode {
    /// Its name in the profile.
    pub name: String,
    /// What it selects between, in order: a mux's inputs by the names the
    /// profile gives them, a demux's outputs by number.
    pub choices: Vec<String>,
    /// Which of them is selected, as a place in `choices`.
    pub selected: usize,
}

/// Where a setup is in its life.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Phase {
    /// Its pads are being opened and its virtual pads made.
    Starting,
    /// Every pad it needs is there.
    Running,
    /// A pad it needs is missing: the virtual pads stay, the missing pad
    /// reads as at rest, and the rest carries on.
    Held,
    /// It couldn't start, or stopped working; `reason` says why. Its
    /// virtual pads are gone. It stays listed until it is stopped.
    Failed,
}

impl Phase {
    /// The phase as it travels on the wire, and as it is shown.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Held => "held",
            Self::Failed => "failed",
        }
    }
}

/// One of a setup's pads.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SetupSource {
    /// Its name in the profile.
    pub name: String,
    /// The pad it is, by the id the pad is known by; absent while no pad
    /// that fits the profile has been found.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pad: Option<String>,
    /// Whether that pad is there now.
    pub present: bool,
}

/// One of a setup's virtual pads.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct SetupSink {
    /// Its name in the profile.
    pub name: String,
    /// The identity it presents, by name.
    pub identity: String,
    /// Its event node, once it is made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::Profile as Schema;

    #[test]
    fn a_hiding_mechanism_travels_as_the_word_it_is_shown_as() {
        for hiding in [Hiding::Service, Hiding::Grab] {
            assert_eq!(serde_json::to_value(hiding).unwrap(), hiding.as_str());
        }
    }

    #[test]
    fn a_phase_travels_as_the_word_it_is_shown_as() {
        for phase in [Phase::Starting, Phase::Running, Phase::Held, Phase::Failed] {
            assert_eq!(serde_json::to_value(phase).unwrap(), phase.as_str());
        }
    }

    #[test]
    fn a_problem_reads_as_where_then_what() {
        let nowhere = Problem {
            at: String::new(),
            message: "a preset and a full form don't mix".to_string(),
        };
        assert_eq!(nowhere.to_string(), "a preset and a full form don't mix");
        let located: Problem = Schema::check("preset = 1").unwrap_err().into();
        assert_eq!(located.at, "line 1");
        assert!(located.to_string().starts_with("line 1: "), "{located}");
    }
}
