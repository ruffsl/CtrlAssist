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
    /// The hiding service's release, while the daemon finds it there,
    /// asked on every connection: the daemon is its only client, so only
    /// the daemon can say. None while it isn't there.
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
    /// it some other way, as Steam reads a PlayStation pad or a Steam
    /// Controller, still sees it. A Steam Controller in a setup is held
    /// so whichever this is: the hiding service doesn't take it.
    Grab,
    /// The daemon has just started and is still finding out whether the
    /// hiding service is there, which it does without holding up its
    /// start. A setup that starts meanwhile waits, as it starts, for the
    /// answer, and is then hidden or grabbed as for either of the others.
    Looking,
}

impl Hiding {
    /// The mechanism as it travels on the wire, and as it is shown.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Service => "service",
            Self::Grab => "grab",
            Self::Looking => "looking",
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
    /// Whether it can rumble now, and so be buzzed by `identify`; absent
    /// when the daemon couldn't tell. A [Steam
    /// Controller](Self::is_steam_controller) is rumbled through its raw
    /// node while a setup holds it, and so can then, whatever its kernel
    /// driver has; free, it can only where its driver has force feedback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rumble: Option<bool>,
    /// The bus it is connected on now, as the kernel numbers them: 3 is
    /// USB, 5 Bluetooth. Absent while it isn't connected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bus: Option<u16>,
}

impl Pad {
    /// Whether it is a Steam Controller (2026), on any of its transports:
    /// a pad the daemon reads through its raw node, holds rather than
    /// hides, and can't leave visible to games in a setup.
    #[must_use]
    pub const fn is_steam_controller(&self) -> bool {
        is_steam_controller(self.vendor, self.product)
    }
}

/// Whether `vendor` and `product` are a Steam Controller's (2026): on a
/// cable, over Bluetooth, behind the Puck, or behind a Steam Machine's
/// receiver. The device crate has the same list, which the daemon's
/// tests hold this one to.
#[must_use]
pub const fn is_steam_controller(vendor: u16, product: u16) -> bool {
    vendor == 0x28de && matches!(product, 0x1302..=0x1305)
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
    /// The pads it still wants chosen, by their keys, where that is all
    /// it lacks: it isn't wrong, and it can't run yet.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unset: Vec<String>,
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
    /// Which of the daemon's starts this is: a number the daemon gives
    /// each setup it starts afresh, never twice while it runs, and that
    /// stays the same through a change the setup takes as it runs. A
    /// setup seen with another number than the last seen under its name
    /// was stopped and started again since, and has new virtual pads. 0
    /// from a daemon too old to say.
    #[serde(default)]
    pub start: u32,
    /// Where it is in its life.
    pub phase: Phase,
    /// Why it failed, for a person to read; or, while it is held, why a
    /// pad of it was refused as it came back. Absent otherwise.
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
        for hiding in [Hiding::Service, Hiding::Grab, Hiding::Looking] {
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
    fn a_setup_from_a_daemon_that_does_not_number_its_starts_reads_as_start_0() {
        let older = r#"{"name":"couch","phase":"running","sources":[],"sinks":[]}"#;
        let setup: Setup = serde_json::from_str(older).unwrap();
        assert_eq!(setup.start, 0);
        let numbered = Setup { start: 7, ..setup };
        let text = serde_json::to_string(&numbered).unwrap();
        assert!(text.contains(r#""start":7"#), "{text}");
        assert_eq!(serde_json::from_str::<Setup>(&text).unwrap(), numbered);
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
