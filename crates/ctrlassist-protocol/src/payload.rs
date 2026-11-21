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
