//! Quick start: two pads, a mode and a few choices, made into a profile.
//!
//! The choices become one kept profile, `quick`, written in the full form
//! so that who feels rumble can be said. The same profile read back is
//! where the choices come from when the tray starts, so nothing of them
//! is stored anywhere else.

use ctrlassist_protocol::profile::{Feedback, Identity, MuxMode, Node, Profile, Sink, Source};

/// The name of the profile quick start writes and runs.
pub const NAME: &str = "quick";

/// Who feels what a game rumbles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rumble {
    /// Both pads.
    #[default]
    Both,
    /// The primary alone.
    Primary,
    /// The assist alone.
    Assist,
}

impl Rumble {
    /// Every choice, in the order the menu lists them.
    pub const ALL: [Self; 3] = [Self::Both, Self::Primary, Self::Assist];

    /// The choice as the menu words it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Both => "both pads",
            Self::Primary => "primary only",
            Self::Assist => "assist only",
        }
    }
}

/// The modes quick start offers, in the order the menu lists them.
pub const MODES: [MuxMode; 3] = [MuxMode::Priority, MuxMode::Average, MuxMode::Toggle];

/// A mode as the menu and a profile word it.
#[must_use]
pub const fn mode_label(mode: MuxMode) -> &'static str {
    match mode {
        MuxMode::Priority => "priority",
        MuxMode::Average => "average",
        MuxMode::Toggle => "toggle",
    }
}

/// What quick start is set to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quick {
    /// The primary pad, by the id it is known by.
    pub primary: Option<String>,
    /// The assist pad.
    pub assist: Option<String>,
    /// How the two are merged.
    pub mode: MuxMode,
    /// What the virtual pad presents itself as.
    pub identity: Identity,
    /// Whether the profile starts by itself once both pads are connected.
    pub auto_start: bool,
    /// Who feels rumble.
    pub rumble: Rumble,
}

impl Default for Quick {
    fn default() -> Self {
        Self {
            primary: None,
            assist: None,
            mode: MuxMode::Priority,
            identity: Identity::Generic,
            auto_start: false,
            rumble: Rumble::Both,
        }
    }
}

impl Quick {
    /// The choices as a profile's TOML; nothing until two different pads
    /// are chosen.
    #[must_use]
    pub fn profile(&self) -> Option<String> {
        let (primary, assist) = (self.primary.as_ref()?, self.assist.as_ref()?);
        if primary == assist {
            return None;
        }
        let source = |pad: &String| Source {
            pad: pad.clone(),
            deadzone: None,
        };
        let feedback = match self.rumble {
            // A mux's own default: whoever reaches the output.
            Rumble::Both => None,
            Rumble::Primary => Some(Feedback::Inputs(vec!["primary".to_string()])),
            Rumble::Assist => Some(Feedback::Inputs(vec!["assist".to_string()])),
        };
        let mut profile = Profile {
            auto_start: self.auto_start,
            ..Profile::default()
        };
        profile.schema = ctrlassist_protocol::profile::SCHEMA;
        profile
            .source
            .insert("primary".to_string(), source(primary));
        profile.source.insert("assist".to_string(), source(assist));
        profile.node.insert(
            "mux".to_string(),
            Node {
                mux: Some(self.mode),
                // The later input outranks the earlier one.
                inputs: vec!["primary".to_string(), "assist".to_string()],
                feedback,
                ..Node::default()
            },
        );
        profile.sink.insert(
            "virtual".to_string(),
            Sink {
                input: "mux".to_string(),
                identity: self.identity,
            },
        );
        profile.to_toml().ok()
    }

    /// Whether running these in place of `kept` takes a restart: other
    /// pads or another identity. The rest a running setup takes as it
    /// runs.
    #[must_use]
    pub fn restarts(&self, kept: &Self) -> bool {
        self.primary != kept.primary || self.assist != kept.assist || self.identity != kept.identity
    }

    /// `kept` with the choices of these that a running setup takes as it
    /// runs: the mode, who feels rumble, and whether it starts by itself.
    #[must_use]
    pub fn onto(&self, kept: Self) -> Self {
        Self {
            mode: self.mode,
            rumble: self.rumble,
            auto_start: self.auto_start,
            ..kept
        }
    }

    /// The choices a kept `quick` profile was written from; nothing for
    /// text that quick start didn't write.
    #[must_use]
    pub fn from_profile(toml: &str) -> Option<Self> {
        let profile = Profile::from_toml(toml).ok()?;
        let node = profile.node.get("mux")?;
        let rumble = match &node.feedback {
            None => Rumble::Both,
            Some(Feedback::Inputs(inputs)) if inputs == &["primary"] => Rumble::Primary,
            Some(Feedback::Inputs(inputs)) if inputs == &["assist"] => Rumble::Assist,
            Some(_) => return None,
        };
        Some(Self {
            primary: Some(profile.source.get("primary")?.pad.clone()),
            assist: Some(profile.source.get("assist")?.pad.clone()),
            mode: node.mux?,
            identity: profile.sink.get("virtual")?.identity,
            auto_start: profile.auto_start,
            rumble,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chosen() -> Quick {
        Quick {
            primary: Some("address:0c:27:56:4f:bd:14".to_string()),
            assist: Some("serial:045e:0b12:3039".to_string()),
            ..Quick::default()
        }
    }

    #[test]
    fn every_choice_makes_a_profile_the_daemon_would_keep_and_reads_back() {
        for mode in MODES {
            for rumble in Rumble::ALL {
                for (identity, auto_start) in [(Identity::Generic, false), (Identity::Xbox, true)] {
                    let quick = Quick {
                        mode,
                        rumble,
                        identity,
                        auto_start,
                        ..chosen()
                    };
                    let toml = quick.profile().unwrap();
                    let plan = Profile::check(&toml).unwrap_or_else(|problem| {
                        unreachable!("{quick:?} made a wrong profile: {problem}\n{toml}")
                    });
                    assert_eq!(plan.auto_start, auto_start);
                    assert_eq!(plan.sinks[0].identity, identity);
                    assert_eq!(Quick::from_profile(&toml), Some(quick));
                }
            }
        }
    }

    #[test]
    fn only_other_pads_or_another_identity_take_a_restart() {
        let kept = chosen();
        let live = Quick {
            mode: MuxMode::Toggle,
            rumble: Rumble::Assist,
            auto_start: true,
            ..chosen()
        };
        assert!(!live.restarts(&kept));
        let other_pad = Quick {
            primary: Some("address:other".to_string()),
            ..live.clone()
        };
        let other_identity = Quick {
            identity: Identity::Xbox,
            ..live.clone()
        };
        assert!(other_pad.restarts(&kept) && other_identity.restarts(&kept));
        // What is taken as it runs goes onto what is kept; the rest waits.
        assert_eq!(other_pad.onto(kept.clone()), live);
        assert_eq!(other_identity.onto(kept), live);
    }

    #[test]
    fn there_is_no_profile_until_two_different_pads_are_chosen() {
        assert_eq!(Quick::default().profile(), None);
        let one = Quick {
            assist: None,
            ..chosen()
        };
        assert_eq!(one.profile(), None);
        let same = Quick {
            assist: chosen().primary,
            ..chosen()
        };
        assert_eq!(same.profile(), None);
    }

    #[test]
    fn a_profile_quick_start_did_not_write_gives_no_choices() {
        let preset = "preset = \"priority\"\nprimary = \"0c:27:56:4f:bd:14\"\nassist = \"model:054c:09cc\"\n";
        assert_eq!(Quick::from_profile(preset), None);
        assert_eq!(Quick::from_profile("not toml ="), None);
        let odd = chosen()
            .profile()
            .unwrap()
            .replace("inputs = ", "feedback = \"none\"\ninputs = ");
        assert_eq!(Quick::from_profile(&odd), None);
        assert_eq!(mode_label(MuxMode::Toggle), "toggle");
        assert_eq!(Rumble::Assist.label(), "assist only");
    }
}
