//! Quick start: two pads, a mode and a few choices, made into a profile.
//!
//! The choices become one kept profile, `quick`, written as a preset. The
//! same profile read back is where the choices come from when the tray
//! starts, so nothing of them is stored anywhere else.

pub use ctrlassist_protocol::profile::Rumble;
use ctrlassist_protocol::profile::{Identity, MuxMode, Preset, Profile};

/// The name of the profile quick start writes and runs.
pub const NAME: &str = "quick";

/// Who feels rumble, as the menu words the choice.
#[must_use]
pub const fn rumble_label(rumble: Rumble) -> &'static str {
    match rumble {
        Rumble::Both => "both pads",
        Rumble::Primary => "primary only",
        Rumble::Assist => "assist only",
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
        let profile = Profile {
            schema: ctrlassist_protocol::profile::SCHEMA,
            auto_start: self.auto_start,
            preset: Some(match self.mode {
                MuxMode::Priority => Preset::Priority,
                MuxMode::Average => Preset::Average,
                MuxMode::Toggle => Preset::Toggle,
            }),
            primary: Some(primary.clone()),
            assist: Some(assist.clone()),
            // What a preset means by leaving a key out is left out.
            identity: (self.identity != Identity::default()).then_some(self.identity),
            rumble: (self.rumble != Rumble::default()).then_some(self.rumble),
            ..Profile::default()
        };
        profile.to_toml().ok()
    }

    /// The choices a kept `quick` profile holds: any preset of two pads
    /// reads back. Nothing for one that isn't, such as a profile in the
    /// full form or a preset of one pad, which the menu couldn't show.
    #[must_use]
    pub fn from_profile(toml: &str) -> Option<Self> {
        let profile = Profile::from_toml(toml).ok()?;
        let mode = match profile.preset? {
            Preset::Priority => MuxMode::Priority,
            Preset::Average => MuxMode::Average,
            Preset::Toggle => MuxMode::Toggle,
            Preset::Unicast | Preset::Multicast => return None,
        };
        Some(Self {
            primary: Some(profile.primary?),
            assist: Some(profile.assist?),
            mode,
            identity: profile.identity.unwrap_or_default(),
            auto_start: profile.auto_start,
            rumble: profile.rumble.unwrap_or_default(),
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
    fn a_preset_of_two_pads_gives_its_choices_and_nothing_else_gives_any() {
        let preset = "preset = \"average\"\nprimary = \"0c:27:56:4f:bd:14\"\nassist = \"model:054c:09cc\"\nrumble = \"assist\"\n";
        let read = Quick::from_profile(preset).unwrap();
        assert_eq!((read.mode, read.rumble), (MuxMode::Average, Rumble::Assist));
        assert_eq!(read.identity, Identity::Generic);
        assert_eq!(read.assist.as_deref(), Some("model:054c:09cc"));
        // What it writes is the short form.
        let written = chosen().profile().unwrap();
        assert!(written.starts_with("preset = \"priority\"\n"), "{written}");
        assert!(!written.contains("[node"), "{written}");

        assert_eq!(Quick::from_profile("not toml ="), None);
        let one_pad = "preset = \"unicast\"\npad = \"model:054c:09cc\"\n";
        assert_eq!(Quick::from_profile(one_pad), None);
        let full = "[source.primary]\npad = \"model:0001:0001\"\n[source.assist]\npad = \"model:0002:0002\"\n[node.mux]\nmux = \"priority\"\ninputs = [\"primary\", \"assist\"]\n[sink.virtual]\ninput = \"mux\"\n";
        assert_eq!(Quick::from_profile(full), None);
        assert_eq!(mode_label(MuxMode::Toggle), "toggle");
        assert_eq!(rumble_label(Rumble::Assist), "assist only");
    }
}
