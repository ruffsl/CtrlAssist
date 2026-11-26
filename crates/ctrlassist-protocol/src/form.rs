//! A preset's fields, described once: what a form shows for each, which
//! presets it belongs to, and how a form's values become a profile and
//! come back from one.
//!
//! The daemon serves the description, and a client renders it without
//! knowing the presets itself: a preset or a field added here appears
//! in every client's form. The profile a form writes is still checked
//! by the daemon like any other; this describes, it doesn't decide.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::profile::{Hide, Identity, NO_BUTTON, Preset, Rumble, SCHEMA, button_names};

/// One choice of a field that offers some.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// The choice as a profile writes it.
    pub value: String,
    /// The choice as a person reads it.
    pub label: String,
    /// A sentence on what choosing it does; empty when the label says it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// What a run of choices has in common, for a form that can say it
    /// once above them, as a menu can: "Assist holds". Empty for a
    /// choice that stands by itself.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub caption: String,
    /// The choice as it reads under its caption, which says the rest:
    /// "Right bumper". Empty where there is no caption.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub brief: String,
}

/// What a field holds, and so what a form shows for it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[non_exhaustive]
pub enum Kind {
    /// One of a few choices.
    Choice {
        /// The choices, in the order to list them.
        choices: Vec<Choice>,
    },
    /// A pad, by what it is known by.
    Pad,
    /// A whole number in a range.
    Number {
        /// The least it may be.
        min: u32,
        /// The most it may be.
        max: u32,
    },
    /// On or off.
    Switch,
}

/// One field of a preset.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Field {
    /// The profile's key for it.
    pub key: String,
    /// Its name as a person reads it.
    pub label: String,
    /// A sentence on what it is for.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub help: String,
    /// What it holds.
    #[serde(flatten)]
    pub kind: Kind,
    /// The presets it belongs to, as a profile writes them; empty for a
    /// field of every preset.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presets: Vec<String>,
    /// What leaving it out of a profile means, as a profile would write
    /// it; empty for a field that must be given.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default: String,
    /// The card a form shows it in: [`INPUTS`], [`OUTPUTS`] or
    /// [`ARRANGEMENT`]; empty for one a form places by itself, as
    /// whether the profile is managed.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
}

/// The group of the fields that name the profile's pads.
pub const INPUTS: &str = "inputs";
/// The group of the fields about its virtual pads.
pub const OUTPUTS: &str = "outputs";
/// The group of the fields that say how the two are arranged.
pub const ARRANGEMENT: &str = "preset";

/// The card a field belongs in, by its key.
fn group_of(field: &Field) -> &'static str {
    match (&field.kind, field.key.as_str()) {
        (Kind::Pad, _) => INPUTS,
        (_, "identity" | "outputs") => OUTPUTS,
        (_, "managed") => "",
        _ => ARRANGEMENT,
    }
}

impl Field {
    /// Whether the field belongs to `preset`.
    #[must_use]
    pub fn applies_to(&self, preset: &str) -> bool {
        self.presets.is_empty() || self.presets.iter().any(|name| name == preset)
    }
}

/// The key of the field that picks the preset.
pub const PRESET: &str = "mode";

fn choice(value: &str, label: &str, description: &str) -> Choice {
    Choice {
        value: value.to_string(),
        label: label.to_string(),
        description: description.to_string(),
        caption: String::new(),
        brief: String::new(),
    }
}

/// The presets of two pads, and of one, as a profile writes them.
const TWO_PADS: [&str; 3] = ["priority", "average", "toggle"];
/// Those of two pads where the motion sensors are the primary's unless
/// the assist takes them.
const MOTION_TAKEN: [&str; 2] = ["priority", "average"];
const ONE_PAD: [&str; 2] = ["unicast", "multicast"];

fn names(presets: &[&str]) -> Vec<String> {
    presets.iter().map(|name| (*name).to_string()).collect()
}

/// A button's name as a person reads it: `left-bumper` as `Left bumper`.
fn button_label(name: &str) -> String {
    let mut label = name.replace('-', " ");
    if let Some(first) = label.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    label
}

/// Where a back button is, for a paddle's name, as SDL numbers them;
/// nothing for any other button, whose name says where it is.
fn paddle_place(name: &str) -> &'static str {
    match name {
        "paddle-1" => "The upper right back button, as on a Steam Controller.",
        "paddle-2" => "The upper left back button, as on a Steam Controller.",
        "paddle-3" => "The lower right back button, as on a Steam Controller.",
        "paddle-4" => "The lower left back button, as on a Steam Controller.",
        _ => "",
    }
}

/// A preset's fields, in the order a form shows them.
#[must_use]
pub fn fields() -> Vec<Field> {
    let mut fields = described();
    for field in &mut fields {
        field.group = group_of(field).to_string();
    }
    fields
}

/// The fields, each still to be told its card.
fn described() -> Vec<Field> {
    let pad = |key: &str, label: &str, help: &str, presets: &[&str]| Field {
        key: key.to_string(),
        label: label.to_string(),
        help: help.to_string(),
        kind: Kind::Pad,
        presets: names(presets),
        default: String::new(),
        group: String::new(),
    };
    vec![
        Field {
            key: PRESET.to_string(),
            label: "Mode".to_string(),
            help: "How the pads and the virtual pads are arranged.".to_string(),
            kind: Kind::Choice {
                choices: vec![
                    choice(
                        Preset::Priority.as_str(),
                        "Priority",
                        "Two pads as one. The assist takes over whatever it touches.",
                    ),
                    choice(
                        Preset::Average.as_str(),
                        "Average",
                        "Two pads as one. Sticks and triggers blend.",
                    ),
                    choice(
                        Preset::Toggle.as_str(),
                        "Toggle",
                        "Two pads as one, one at a time. The Mode button hands over.",
                    ),
                    choice(
                        Preset::Unicast.as_str(),
                        "Unicast",
                        "One pad as several virtual pads, one at a time.",
                    ),
                    choice(
                        Preset::Multicast.as_str(),
                        "Multicast",
                        "One pad as several virtual pads, all at once.",
                    ),
                ],
            },
            presets: Vec::new(),
            default: String::new(),
            group: String::new(),
        },
        pad("primary", "Primary", "The player's pad.", &TWO_PADS),
        pad(
            "assist",
            "Assist",
            "The helper's pad, which outranks the primary.",
            &TWO_PADS,
        ),
        pad("pad", "Pad", "The pad that is shared out.", &ONE_PAD),
        Field {
            key: "outputs".to_string(),
            label: "Outputs".to_string(),
            help: "How many virtual pads to make.".to_string(),
            kind: Kind::Number { min: 1, max: 8 },
            presets: names(&ONE_PAD),
            default: "2".to_string(),
            group: String::new(),
        },
        Field {
            key: "identity".to_string(),
            label: "Identity".to_string(),
            help: "What the virtual pad says it is to games.".to_string(),
            kind: Kind::Choice {
                choices: vec![
                    choice(
                        Identity::Generic.as_str(),
                        "Generic",
                        "A plain gamepad that games map from its buttons.",
                    ),
                    choice(
                        Identity::Xbox.as_str(),
                        "Xbox",
                        "An Xbox Series controller, which most games recognize outright.",
                    ),
                    choice(
                        Identity::DualSense.as_str(),
                        "DualSense",
                        "A PlayStation 5 controller, with PlayStation prompts where a game has them.",
                    ),
                ],
            },
            presets: Vec::new(),
            default: Identity::default().as_str().to_string(),
            group: String::new(),
        },
        Field {
            key: "rumble".to_string(),
            label: "Rumble".to_string(),
            help: "Who feels what a game rumbles.".to_string(),
            kind: Kind::Choice {
                choices: vec![
                    choice(Rumble::Both.as_str(), "Both pads", ""),
                    choice(Rumble::Primary.as_str(), "Primary only", ""),
                    choice(Rumble::Assist.as_str(), "Assist only", ""),
                    choice(Rumble::None.as_str(), "Neither", ""),
                ],
            },
            presets: names(&TWO_PADS),
            default: Rumble::default().as_str().to_string(),
            group: String::new(),
        },
        Field {
            key: "assist-gyro".to_string(),
            label: "Gyro".to_string(),
            help: "Whose motion sensors the virtual pad carries: the primary's, or the assist's while the assist holds a button of their pad, which the game then doesn't see. Only a DualSense identity carries motion."
                .to_string(),
            kind: Kind::Choice {
                // Each says whose motion the game gets, so a line that shows
                // the choice alone still reads.
                choices: std::iter::once(choice(NO_BUTTON, "Primary only", ""))
                    .chain(button_names().map(|name| {
                        let button = button_label(name);
                        Choice {
                            caption: "Assist holds".to_string(),
                            brief: button.clone(),
                            ..choice(name, &format!("Assist · {button}"), paddle_place(name))
                        }
                    }))
                    .collect(),
            },
            presets: names(&MOTION_TAKEN),
            default: NO_BUTTON.to_string(),
            group: String::new(),
        },
        Field {
            key: "hide".to_string(),
            label: "Visibility".to_string(),
            help: "How the pads behind the virtual pad are kept from games while this runs."
                .to_string(),
            kind: Kind::Choice {
                choices: vec![
                    choice(
                        Hide::Full.as_str(),
                        "Hidden",
                        "Games don't see them: the hiding service takes them away, or they are grabbed where it isn't there, as a Steam Controller always is.",
                    ),
                    choice(
                        Hide::Grab.as_str(),
                        "Grabbed",
                        "Most games don't see them. A program that reads pads another way, as Steam does, still does.",
                    ),
                    choice(
                        Hide::None.as_str(),
                        "Visible",
                        "Games see them beside the virtual pad, and may take what is pressed twice.",
                    ),
                ],
            },
            presets: Vec::new(),
            default: Hide::default().as_str().to_string(),
            group: String::new(),
        },
        Field {
            key: "managed".to_string(),
            label: "Managed".to_string(),
            help: "The daemon starts it whenever every pad it names is connected. Unmanaged, it is started by hand."
                .to_string(),
            kind: Kind::Switch,
            presets: Vec::new(),
            default: "false".to_string(),
            group: String::new(),
        },
    ]
}

/// A form's values, by field key, each as a profile would write it:
/// a choice's value, a pad's id, a number in digits, `true` or `false`.
pub type Values = BTreeMap<String, String>;

/// The profile a form's values make, as TOML. A field that doesn't
/// belong to the chosen preset is left out, as is one at its default,
/// and one with no value: what is missing that a preset needs is for
/// the daemon's check to say.
#[must_use]
pub fn to_toml(fields: &[Field], values: &Values) -> String {
    let preset = values.get(PRESET).map(String::as_str).unwrap_or_default();
    let mut text = String::new();
    for field in fields {
        let Some(value) = values.get(&field.key).filter(|value| !value.is_empty()) else {
            continue;
        };
        if !field.applies_to(preset) || (field.key != PRESET && *value == field.default) {
            continue;
        }
        // A number and the two words stand bare; anything else, under any
        // kind, is written as a string, which the check then refuses where
        // a string doesn't belong. So no value can add to the TOML.
        let quoted = || toml::Value::String(value.clone()).to_string();
        let written = match (&field.kind, value.as_str()) {
            (Kind::Number { .. }, _) => value
                .parse::<u32>()
                .map_or_else(|_| quoted(), |number| number.to_string()),
            (Kind::Switch, "true" | "false") => value.clone(),
            _ => quoted(),
        };
        text.push_str(&format!("{} = {written}\n", field.key));
    }
    text
}

/// The values a form shows for a profile, or nothing for a profile a
/// form can't show: one that isn't TOML, isn't a preset, holds a key
/// that isn't a field (a profile in the full form does), or is for a
/// schema other than this version's. What a form writes carries no
/// schema, so showing a newer profile's values and writing them back
/// would quietly make it this version's.
#[must_use]
pub fn from_toml(fields: &[Field], toml: &str) -> Option<Values> {
    let table: toml::Table = toml.parse().ok()?;
    table.get(PRESET)?;
    let mut values = Values::new();
    for (key, value) in &table {
        if key == "schema" {
            if value.as_integer() != Some(i64::from(SCHEMA)) {
                return None;
            }
            continue;
        }
        fields.iter().find(|field| field.key == *key)?;
        let text = match value {
            toml::Value::String(text) => text.clone(),
            toml::Value::Integer(number) => number.to_string(),
            toml::Value::Boolean(flag) => flag.to_string(),
            _ => return None,
        };
        values.insert(key.clone(), text);
    }
    // What the profile leaves out stands at its default.
    let preset = values.get(PRESET).cloned().unwrap_or_default();
    for field in fields {
        if field.applies_to(&preset) && !field.default.is_empty() {
            values
                .entry(field.key.clone())
                .or_insert_with(|| field.default.clone());
        }
    }
    Some(values)
}
