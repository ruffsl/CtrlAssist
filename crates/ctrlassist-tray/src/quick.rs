//! Quick start: two pads, a preset and a few choices, made into a
//! profile.
//!
//! The choices are a preset form's values, over the fields the daemon
//! serves: what there is to choose, what each choice is called and what
//! a profile writes for it all come from there, as they do for the
//! window's form. The choices become one kept profile, `quick`, and the
//! same profile read back is where they come from when the tray starts,
//! so nothing of them is stored anywhere else.

use ctrlassist::form::{self, Choice, Field, Kind, PRESET, Values};

/// The name of the profile quick start writes and runs.
pub const NAME: &str = "quick";

/// How many pads a preset of quick start's merges.
const PADS: usize = 2;

/// The fields of `preset` that hold a pad.
fn pad_fields<'a>(fields: &'a [Field], preset: &'a str) -> impl Iterator<Item = &'a Field> {
    fields
        .iter()
        .filter(move |field| field.kind == Kind::Pad && field.applies_to(preset))
}

/// The presets quick start offers, of those the daemon serves: the ones
/// of two pads, in the order they are listed.
#[must_use]
pub fn presets(fields: &[Field]) -> Vec<&Choice> {
    let Some(Kind::Choice { choices }) = fields
        .iter()
        .find(|field| field.key == PRESET)
        .map(|field| &field.kind)
    else {
        return Vec::new();
    };
    choices
        .iter()
        .filter(|choice| pad_fields(fields, &choice.value).count() == PADS)
        .collect()
}

/// What quick start is set to: a value for each field that was picked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Quick {
    /// The picks, by field key, as a profile writes them.
    pub values: Values,
}

impl Quick {
    /// Takes a pick.
    pub fn set(&mut self, key: &str, value: &str) {
        self.values.insert(key.to_string(), value.to_string());
    }

    /// The preset picked, or the first one offered while none is.
    #[must_use]
    pub fn preset<'a>(&'a self, fields: &'a [Field]) -> &'a str {
        self.values.get(PRESET).map_or_else(
            || presets(fields).first().map_or("", |first| &first.value),
            String::as_str,
        )
    }

    /// What `field` stands at: the pick, or what leaving it out means.
    #[must_use]
    pub fn value<'a>(&'a self, field: &'a Field) -> &'a str {
        self.values
            .get(&field.key)
            .map_or(field.default.as_str(), String::as_str)
    }

    /// The pads picked for the preset's pad fields, each with its
    /// field's key; a field with none picked isn't among them.
    #[must_use]
    pub fn pads<'a>(&'a self, fields: &'a [Field]) -> Vec<(&'a str, &'a str)> {
        pad_fields(fields, self.preset(fields))
            .filter_map(|field| Some((field.key.as_str(), self.values.get(&field.key)?.as_str())))
            .collect()
    }

    /// The picks as a profile's TOML, complete or not: what two sets of
    /// picks are compared by, since it leaves out what is at its default
    /// and what the preset has no use for.
    #[must_use]
    pub fn text(&self, fields: &[Field]) -> String {
        let mut values = self.values.clone();
        values.insert(PRESET.to_string(), self.preset(fields).to_string());
        form::to_toml(fields, &values)
    }

    /// The picks as a profile's TOML; nothing until the fields are known
    /// and two different pads are picked.
    #[must_use]
    pub fn profile(&self, fields: &[Field]) -> Option<String> {
        let pads = self.pads(fields);
        let [(_, one), (_, other)] = pads.as_slice() else {
            return None;
        };
        (one != other).then(|| self.text(fields))
    }

    /// The picks a kept `quick` profile holds: any preset quick start
    /// offers reads back. Nothing for one it doesn't, such as a profile
    /// in the full form or a preset of one pad, which the menu couldn't
    /// show.
    #[must_use]
    pub fn from_profile(fields: &[Field], toml: &str) -> Option<Self> {
        let values = form::from_toml(fields, toml)?;
        let preset = values.get(PRESET)?;
        presets(fields)
            .iter()
            .any(|offered| offered.value == *preset)
            .then_some(Self { values })
    }
}

#[cfg(test)]
mod tests {
    use ctrlassist_protocol::profile::{Identity, Profile};

    use super::*;

    fn chosen() -> Quick {
        let mut quick = Quick::default();
        quick.set("primary", "address:0c:27:56:4f:bd:14");
        quick.set("assist", "serial:045e:0b12:3039");
        quick
    }

    fn choices_of<'a>(fields: &'a [Field], key: &str) -> Vec<&'a str> {
        let field = fields.iter().find(|field| field.key == key).unwrap();
        let Kind::Choice { choices } = &field.kind else {
            unreachable!("{key} is a choice");
        };
        choices.iter().map(|choice| choice.value.as_str()).collect()
    }

    #[test]
    fn quick_start_offers_the_presets_of_two_pads() {
        let fields = form::fields();
        let offered: Vec<&str> = presets(&fields)
            .iter()
            .map(|preset| preset.value.as_str())
            .collect();
        assert_eq!(offered, ["priority", "average", "toggle"]);
        assert!(presets(&[]).is_empty());
        assert_eq!(Quick::default().preset(&fields), "priority");
        assert_eq!(Quick::default().preset(&[]), "");
    }

    #[test]
    fn every_choice_makes_a_profile_the_daemon_would_keep_and_reads_back() {
        let fields = form::fields();
        for preset in presets(&fields) {
            for rumble in choices_of(&fields, "rumble") {
                for (identity, auto_start) in [(Identity::Generic, false), (Identity::Xbox, true)] {
                    let mut quick = chosen();
                    quick.set(PRESET, &preset.value);
                    quick.set("rumble", rumble);
                    quick.set("identity", identity.as_str());
                    quick.set("auto-start", &auto_start.to_string());
                    let toml = quick.profile(&fields).unwrap();
                    let plan = Profile::check(&toml).unwrap_or_else(|problem| {
                        unreachable!("{quick:?} made a wrong profile: {problem}\n{toml}")
                    });
                    assert_eq!(plan.auto_start, auto_start);
                    assert_eq!(plan.sinks[0].identity, identity);
                    let back = Quick::from_profile(&fields, &toml).unwrap();
                    assert_eq!(back.text(&fields), toml);
                }
            }
        }
    }

    #[test]
    fn there_is_no_profile_until_two_different_pads_are_chosen() {
        let fields = form::fields();
        assert_eq!(Quick::default().profile(&fields), None);
        let mut one = chosen();
        one.values.remove("assist");
        assert_eq!(one.profile(&fields), None);
        let mut same = chosen();
        same.set("assist", "address:0c:27:56:4f:bd:14");
        assert_eq!(same.profile(&fields), None);
        // Nor before the daemon has said what the fields are.
        assert_eq!(chosen().profile(&[]), None);
        assert!(chosen().profile(&fields).is_some());
    }

    #[test]
    fn a_preset_of_two_pads_gives_its_choices_and_nothing_else_gives_any() {
        let fields = form::fields();
        let preset = "preset = \"average\"\nprimary = \"0c:27:56:4f:bd:14\"\nassist = \"model:054c:09cc\"\nrumble = \"assist\"\n";
        let read = Quick::from_profile(&fields, preset).unwrap();
        assert_eq!(read.preset(&fields), "average");
        assert_eq!(read.values["rumble"], "assist");
        // What the profile leaves out stands at its default.
        assert_eq!(read.values["identity"], "generic");
        assert_eq!(
            read.pads(&fields),
            [
                ("primary", "0c:27:56:4f:bd:14"),
                ("assist", "model:054c:09cc")
            ]
        );
        // What it writes is the short form, and the same picks compare
        // equal however they were come by.
        let written = chosen().profile(&fields).unwrap();
        assert!(written.starts_with("preset = \"priority\"\n"), "{written}");
        assert!(!written.contains("[node"), "{written}");
        assert!(!written.contains("identity"), "{written}");
        assert_eq!(read.text(&fields), preset);

        assert_eq!(Quick::from_profile(&fields, "not toml ="), None);
        let one_pad = "preset = \"unicast\"\npad = \"model:054c:09cc\"\n";
        assert_eq!(Quick::from_profile(&fields, one_pad), None);
        let full = "[source.primary]\npad = \"model:0001:0001\"\n[source.assist]\npad = \"model:0002:0002\"\n[node.mux]\nmux = \"priority\"\ninputs = [\"primary\", \"assist\"]\n[sink.virtual]\ninput = \"mux\"\n";
        assert_eq!(Quick::from_profile(&fields, full), None);
    }
}
