//! The profile: a TOML file that says which pads feed which virtual pads,
//! and how.
//!
//! A profile is either a preset, whose graph is implied by its name, or
//! the full form, which names sources, nodes and sinks. Both become a
//! [`Plan`]: a graph for the core to build, and what each of its sources
//! and sinks is to be.
//!
//! A profile is a user's file and is checked as one. Every problem is
//! reported with where it is, in the file's own terms.

use std::collections::BTreeMap;

use ctrlassist_core::{
    Control, DemuxConfig, DemuxFeedback, DemuxMode, Graph, GraphSpec, Input, Kind, Layout,
    MAX_INPUTS, MAX_NODES, MAX_OUTPUTS, MAX_SINKS, MAX_SOURCES, Mode, MuxConfig, MuxFeedback,
    NodeId, NodeSpec, Rule, SourceId, SourceSpec, Switch, gamepad,
};
use serde::{Deserialize, Serialize};

/// The schema version this build reads and writes.
pub const SCHEMA: u32 = 1;

/// The deadzone a source has unless it says otherwise.
const DEADZONE: f32 = 0.1;

/// A profile as written.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Profile {
    /// The schema version the file was written for.
    #[serde(default = "schema", skip_serializing_if = "is_current")]
    pub schema: u32,
    /// Whether the daemon manages when this profile runs: it starts a
    /// managed one once the daemon is up and its pads are connected.
    /// One that isn't is started by hand.
    #[serde(default, skip_serializing_if = "is_false")]
    pub managed: bool,

    /// The preset, for a profile in the short form: the mode its pads
    /// and virtual pads are arranged in, which the file writes as `mode`.
    #[serde(default, rename = "mode", skip_serializing_if = "Option::is_none")]
    pub preset: Option<Preset>,
    /// A mux preset's first pad.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<String>,
    /// A mux preset's second pad, which outranks the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assist: Option<String>,
    /// A demux preset's pad.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pad: Option<String>,
    /// How many virtual pads a demux preset makes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<usize>,
    /// What a preset's virtual pads present themselves as.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// Who feels what a game rumbles, for a mux preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rumble: Option<Rumble>,
    /// How the setup's pads are kept from games.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hide: Option<Hide>,

    /// The pads, by name, for a profile in the full form.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub source: BTreeMap<String, Source>,
    /// The nodes, by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub node: BTreeMap<String, Node>,
    /// The virtual pads, by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sink: BTreeMap<String, Sink>,
}

fn schema() -> u32 {
    SCHEMA
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "the signature serde calls"
)]
fn is_current(version: &u32) -> bool {
    *version == SCHEMA
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "the signature serde calls"
)]
fn is_false(value: &bool) -> bool {
    !*value
}

/// Who feels what a game rumbles, in a preset of two pads.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rumble {
    /// Both pads: whoever reaches the virtual pad.
    #[default]
    Both,
    /// The primary alone.
    Primary,
    /// The assist alone.
    Assist,
    /// Neither: what a game rumbles is felt by nobody.
    None,
}

impl Rumble {
    /// Every choice, in the order a menu lists them.
    pub const ALL: [Self; 4] = [Self::Both, Self::Primary, Self::Assist, Self::None];

    /// The choice as a profile writes it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Both => "both",
            Self::Primary => "primary",
            Self::Assist => "assist",
            Self::None => "none",
        }
    }
}

/// How a setup's pads are kept from games while it runs, so that a game
/// reads the virtual pad and not the pads behind it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Hide {
    /// As well as can be: taken away by the hiding service, or grabbed
    /// where that isn't there.
    #[default]
    Full,
    /// Grabbed only, which hides a pad from most games and not from a
    /// program that reads it some other way.
    Grab,
    /// Not at all: games see the pads beside the virtual pad.
    None,
}

impl Hide {
    /// Every choice, in the order a menu lists them.
    pub const ALL: [Self; 3] = [Self::Full, Self::Grab, Self::None];

    /// The choice as a profile writes it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Grab => "grab",
            Self::None => "none",
        }
    }
}

/// The name of the profile a daemon always keeps: made where there is
/// none, never deleted. It is the one the tray is quickest to.
pub const DEFAULT: &str = "default";

/// The default profile as a daemon makes it: the first preset, with its
/// pads still to choose.
pub const DEFAULT_TOML: &str = "mode = \"priority\"\n";

/// The profiles whose graph is implied by a name.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Preset {
    /// Two pads as one; the assist overrides what it touches.
    Priority,
    /// Two pads as one; sticks and triggers blend.
    Average,
    /// Two pads as one, one at a time, handed over by a button.
    Toggle,
    /// One pad as several, one at a time.
    Unicast,
    /// One pad as several, all at once.
    Multicast,
}

impl Preset {
    /// Every preset, in the order they are listed to a person.
    pub const ALL: [Self; 5] = [
        Self::Priority,
        Self::Average,
        Self::Toggle,
        Self::Unicast,
        Self::Multicast,
    ];

    /// The preset's name, as a profile writes it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Priority => "priority",
            Self::Average => "average",
            Self::Toggle => "toggle",
            Self::Unicast => "unicast",
            Self::Multicast => "multicast",
        }
    }
}

/// What a virtual pad presents itself as.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Identity {
    /// A pad that claims to be nothing else.
    #[default]
    Generic,
    /// An Xbox pad.
    Xbox,
    /// A DualSense, made from one that has been connected.
    #[serde(rename = "dualsense")]
    DualSense,
}

impl Identity {
    /// Every identity, in the order they are listed to a person.
    pub const ALL: [Self; 3] = [Self::Generic, Self::Xbox, Self::DualSense];

    /// The identity's name, as a profile writes it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::Xbox => "xbox",
            Self::DualSense => "dualsense",
        }
    }
}

/// A pad in the full form.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Source {
    /// Which pad; see [`PadRef`].
    pub pad: String,
    /// Sticks resting inside this are read as at rest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadzone: Option<f32>,
}

/// A node in the full form: a mux or a demux, by which key it has.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Node {
    /// The mode, for a mux.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mux: Option<MuxMode>,
    /// The mode, for a demux.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub demux: Option<Fanout>,
    /// A mux's inputs. Later inputs outrank earlier ones.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<String>,
    /// A demux's input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    /// How many outputs a demux has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<usize>,
    /// A mux input becomes active on a group at or above this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engage: Option<f32>,
    /// An active mux input stops being active on a group below this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<f32>,
    /// A mux's rule for a kind of group, where it differs from its mode's.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rules: BTreeMap<GroupKind, RuleName>,
    /// Where feedback goes: `"active"`, `"none"`, or for a mux a list of
    /// its inputs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<Feedback>,
    /// The button that switches the node: a button's name, or `false` for
    /// none. A mux may say whose button it is with a table.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switch: Option<SwitchSpec>,
}

/// A mux's modes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MuxMode {
    /// The later input overrides what it touches.
    Priority,
    /// Sticks and triggers blend.
    Average,
    /// One input at a time.
    Toggle,
}

/// A demux's modes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Fanout {
    /// One output at a time.
    Unicast,
    /// Every output at once.
    Multicast,
}

/// The kinds of group a rule can be set for.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GroupKind {
    /// Sticks.
    Stick,
    /// Triggers.
    Trigger,
    /// Buttons.
    Button,
    /// The directional pad.
    Hat,
}

/// How a group's output is chosen from a mux's inputs.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuleName {
    /// The highest active input owns the whole group.
    Owned,
    /// Each control takes the input furthest from rest.
    Largest,
    /// Axes are the mean of the active inputs.
    Blended,
    /// The selected input owns the group.
    Switched,
}

/// Where a node's feedback goes.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Feedback {
    /// `"active"` or `"none"`.
    Named(FeedbackName),
    /// A mux's inputs, by the names they have in `inputs`.
    Inputs(Vec<String>),
}

/// The named feedback settings.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FeedbackName {
    /// To whatever currently reaches the output.
    Active,
    /// Nowhere.
    None,
}

/// What switches a node.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SwitchSpec {
    /// `false`: no button; only a client can switch it.
    Off(bool),
    /// A button, on any input.
    Button(String),
    /// A button on one of a mux's inputs.
    On {
        /// The button.
        button: String,
        /// The input whose button it is.
        input: String,
    },
}

/// A virtual pad in the full form.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Sink {
    /// The source or node output it follows.
    pub input: String,
    /// What it presents itself as.
    #[serde(default)]
    pub identity: Identity,
}

/// Which pad a source is.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PadRef {
    /// One pad, by the name it is known by across reconnects, such as
    /// `address:0c:27:56:4f:bd:14`.
    Exact(String),
    /// Any pad of a model, by vendor and product.
    Model(u16, u16),
}

impl PadRef {
    /// Reads a pad reference: a bare address, one of the `address:`,
    /// `serial:`, `port:` or `model:` names a pad is known by, or
    /// `model:<vendor>:<product>` for any pad of that model.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let hex = |text: &str| {
            (text.len() == 4)
                .then(|| u16::from_str_radix(text, 16).ok())
                .flatten()
        };
        let is_address = |text: &str| {
            text.len() == 17
                && text.bytes().enumerate().all(|(index, byte)| {
                    if index % 3 == 2 {
                        byte == b':'
                    } else {
                        byte.is_ascii_hexdigit()
                    }
                })
        };
        if is_address(text) {
            return Some(Self::Exact(format!(
                "address:{}",
                text.to_ascii_lowercase()
            )));
        }
        let (kind, rest) = text.split_once(':')?;
        if rest.is_empty() {
            return None;
        }
        match kind {
            "address" => is_address(rest).then(|| Self::Exact(text.to_ascii_lowercase())),
            "model" => match rest.split(':').collect::<Vec<_>>().as_slice() {
                [vendor, product] => Some(Self::Model(hex(vendor)?, hex(product)?)),
                [vendor, product, _name, ..] => (hex(vendor).is_some() && hex(product).is_some())
                    .then(|| Self::Exact(text.to_string())),
                _ => None,
            },
            "serial" | "port" => {
                let mut parts = rest.splitn(3, ':');
                let numbers = hex(parts.next()?).is_some() && hex(parts.next()?).is_some();
                (numbers && parts.next().is_some_and(|last| !last.is_empty()))
                    .then(|| Self::Exact(text.to_string()))
            }
            _ => None,
        }
    }

    /// Whether a connected pad is the one meant, given the name it is
    /// known by and its vendor and product.
    #[must_use]
    pub fn matches(&self, id: &str, vendor: u16, product: u16) -> bool {
        match self {
            Self::Exact(exact) => exact == id,
            Self::Model(want_vendor, want_product) => {
                (*want_vendor, *want_product) == (vendor, product)
            }
        }
    }
}

/// What is wrong with a profile, and where.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProfileError {
    /// Where, in the file's own terms, such as `node.second.inputs[1]`.
    /// Empty for a problem with the file as a whole.
    pub at: String,
    /// What.
    pub problem: String,
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.at.is_empty() {
            f.write_str(&self.problem)
        } else {
            write!(f, "{}: {}", self.at, self.problem)
        }
    }
}

impl std::error::Error for ProfileError {}

fn error<T>(at: impl Into<String>, problem: impl Into<String>) -> Result<T, ProfileError> {
    Err(ProfileError {
        at: at.into(),
        problem: problem.into(),
    })
}

/// A source in a plan.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PlannedSource {
    /// Its name in the profile.
    pub name: String,
    /// Which pad it is.
    pub pad: PadRef,
}

/// A virtual pad in a plan.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PlannedSink {
    /// Its name in the profile.
    pub name: String,
    /// What it presents itself as.
    pub identity: Identity,
}

/// A profile worked out: the graph, and what each numbered source, node
/// and sink in it is.
#[derive(Clone, PartialEq, Debug)]
pub struct Plan {
    /// The graph, which is known to build.
    pub graph: GraphSpec,
    /// The pads, by source number.
    pub sources: Vec<PlannedSource>,
    /// The nodes' names, by node number.
    pub nodes: Vec<String>,
    /// The virtual pads, by sink number.
    pub sinks: Vec<PlannedSink>,
    /// Whether to start it without being asked.
    pub managed: bool,
    /// How its pads are kept from games.
    pub hide: Hide,
}

/// Button names, as profiles write them.
const BUTTONS: [(&str, Control); 15] = [
    ("south", gamepad::SOUTH),
    ("east", gamepad::EAST),
    ("north", gamepad::NORTH),
    ("west", gamepad::WEST),
    ("left-bumper", gamepad::LEFT_BUMPER),
    ("right-bumper", gamepad::RIGHT_BUMPER),
    ("select", gamepad::SELECT),
    ("start", gamepad::START),
    ("mode", gamepad::MODE),
    ("left-thumb", gamepad::LEFT_THUMB),
    ("right-thumb", gamepad::RIGHT_THUMB),
    ("dpad-up", gamepad::DPAD_UP),
    ("dpad-down", gamepad::DPAD_DOWN),
    ("dpad-left", gamepad::DPAD_LEFT),
    ("dpad-right", gamepad::DPAD_RIGHT),
];

fn button(at: &str, name: &str) -> Result<Control, ProfileError> {
    match BUTTONS.iter().find(|(known, _)| *known == name) {
        Some((_, control)) => Ok(*control),
        None => {
            let known: Vec<&str> = BUTTONS.iter().map(|(name, _)| *name).collect();
            error(
                at,
                format!(
                    "no button named `{name}`; the buttons are {}",
                    known.join(", ")
                ),
            )
        }
    }
}

/// A name a source, node or sink may have: what can be written as a bare
/// TOML key and can't be mistaken for a node's numbered output.
fn check_name(at: &str, name: &str) -> Result<(), ProfileError> {
    let ok = !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if ok {
        Ok(())
    } else {
        error(
            at,
            "a name is 1 to 32 lowercase letters, digits and hyphens",
        )
    }
}

/// Whether `name` can name a profile: what a source, node or sink may be
/// called, so 1 to 32 lowercase letters, digits and hyphens. A profile's
/// name is its file's and its key's, so nothing else is let through.
#[must_use]
pub fn is_name(name: &str) -> bool {
    check_name("", name).is_ok()
}

impl Profile {
    /// Reads a profile from TOML and works it out, as a daemon does
    /// before it keeps one.
    ///
    /// # Errors
    ///
    /// Fails as [`from_toml`](Self::from_toml) and [`plan`](Self::plan)
    /// do.
    pub fn check(text: &str) -> Result<Plan, ProfileError> {
        Self::from_toml(text)?.plan()
    }

    /// The pads a preset still wants chosen, by their keys, where that
    /// is all `text` lacks to be a profile: it can be kept, and shown
    /// with its pads to choose, but not run. Empty for a profile that
    /// lacks nothing, and for one that is wrong some other way.
    #[must_use]
    pub fn unchosen(text: &str) -> Vec<String> {
        let Ok(mut table) = text.parse::<toml::Table>() else {
            return Vec::new();
        };
        let Some(preset) = table.get(crate::form::PRESET).and_then(toml::Value::as_str) else {
            return Vec::new();
        };
        let wanted: Vec<String> = crate::form::fields()
            .into_iter()
            .filter(|field| field.kind == crate::form::Kind::Pad && field.applies_to(preset))
            .map(|field| field.key)
            .filter(|key| !table.contains_key(key))
            .collect();
        // With a pad of its own in each place, is it a profile?
        for (at, key) in wanted.iter().enumerate() {
            let stand_in = format!("model:0000:{:04x}", at + 1);
            table.insert(key.clone(), toml::Value::String(stand_in));
        }
        if wanted.is_empty() || Self::check(&table.to_string()).is_err() {
            return Vec::new();
        }
        wanted
    }

    /// Reads a profile from TOML.
    ///
    /// # Errors
    ///
    /// Fails if the text isn't TOML, has a key the schema doesn't, or was
    /// written for a newer schema. What the profile then means is checked
    /// by [`plan`](Self::plan).
    pub fn from_toml(text: &str) -> Result<Self, ProfileError> {
        let profile: Self = toml::from_str(text).map_err(|error| {
            // The line, where the parser knows it; its message already
            // names the key.
            let line = error.span().map(|span| {
                text.as_bytes()
                    .iter()
                    .take(span.start)
                    .filter(|byte| **byte == b'\n')
                    .count()
                    + 1
            });
            ProfileError {
                at: line.map_or(String::new(), |line| format!("line {line}")),
                problem: error.message().to_string(),
            }
        })?;
        if profile.schema > SCHEMA {
            return error(
                "schema",
                format!(
                    "this profile is for schema {}, and this version reads up to {SCHEMA}",
                    profile.schema
                ),
            );
        }
        if profile.schema == 0 {
            return error("schema", "the schema starts at 1");
        }
        Ok(profile)
    }

    /// Writes the profile as TOML.
    ///
    /// # Errors
    ///
    /// Fails if a value has no TOML form, which a number that isn't one
    /// doesn't.
    pub fn to_toml(&self) -> Result<String, ProfileError> {
        toml::to_string(self).map_err(|error| ProfileError {
            at: String::new(),
            problem: error.to_string(),
        })
    }

    /// Works the profile out into a graph and what its parts are.
    ///
    /// # Errors
    ///
    /// Fails, saying where, if the profile mixes the two forms, leaves
    /// out something its form needs, names something that isn't there, or
    /// describes a graph that can't be built.
    pub fn plan(&self) -> Result<Plan, ProfileError> {
        let full = self.clone().expanded()?;
        let layout = Layout::gamepad();
        for (at, count, most, what) in [
            ("source", full.source.len(), MAX_SOURCES, "pads"),
            ("node", full.node.len(), MAX_NODES, "nodes"),
            ("sink", full.sink.len(), MAX_SINKS, "virtual pads"),
        ] {
            if count > most {
                return error(
                    at,
                    format!("a profile has at most {most} {what}, and this one has {count}"),
                );
            }
        }

        let mut sources = Vec::new();
        let mut specs = Vec::new();
        for (name, source) in &full.source {
            let at = format!("source.{name}");
            check_name(&at, name)?;
            let Some(pad) = PadRef::parse(&source.pad) else {
                return error(
                    format!("{at}.pad"),
                    format!(
                        "`{}` isn't a pad: write its address, the name `ctrlassist pads` gives it, or `model:<vendor>:<product>`",
                        source.pad
                    ),
                );
            };
            // One pad is one source: a setup takes each pad once. Two
            // sources of one model are two pads of it.
            let twice = matches!(pad, PadRef::Exact(_))
                .then(|| {
                    sources
                        .iter()
                        .find(|other: &&PlannedSource| other.pad == pad)
                })
                .flatten();
            if let Some(other) = twice {
                return error(
                    format!("{at}.pad"),
                    format!(
                        "`{}` is the pad `{}` has; each needs its own",
                        source.pad, other.name
                    ),
                );
            }
            let deadzone = source.deadzone.unwrap_or(DEADZONE);
            if !(deadzone.is_finite() && deadzone >= 0.0) {
                return error(format!("{at}.deadzone"), "a deadzone is a number from 0");
            }
            sources.push(PlannedSource {
                name: name.clone(),
                pad,
            });
            specs.push(SourceSpec {
                deadzone,
                layout: layout.clone(),
            });
        }
        let node_names: Vec<String> = full.node.keys().cloned().collect();
        for name in &node_names {
            let at = format!("node.{name}");
            check_name(&at, name)?;
            if full.source.contains_key(name) {
                return error(
                    at,
                    format!(
                        "`{name}` is a source's name too; a reference to it would be ambiguous"
                    ),
                );
            }
        }

        // What a reference such as `helper` or `share.1` means.
        let resolve = |at: &str, reference: &str| -> Result<Input, ProfileError> {
            let (name, output) = match reference.rsplit_once('.') {
                Some((name, output)) => match output.parse::<usize>() {
                    Ok(output) => (name, Some(output)),
                    Err(_) => {
                        return error(
                            at,
                            format!("`{reference}`: after the dot comes an output's number"),
                        );
                    }
                },
                None => (reference, None),
            };
            if let Some(index) = full.source.keys().position(|source| source == name) {
                return match output {
                    None => Ok(Input::Source(SourceId(index))),
                    Some(_) => error(
                        at,
                        format!("`{name}` is a source, which has no numbered outputs"),
                    ),
                };
            }
            if let Some(index) = node_names.iter().position(|node| node == name) {
                let outputs = full.node[name].outputs.unwrap_or(1).max(1);
                let output = output.unwrap_or(0);
                return if full.node[name].demux.is_some() && output >= outputs {
                    error(
                        at,
                        format!("`{name}` has {outputs} outputs, numbered from 0"),
                    )
                } else if full.node[name].demux.is_none() && output > 0 {
                    error(at, format!("`{name}` is a mux, which has one output"))
                } else {
                    Ok(Input::Node(NodeId(index), output))
                };
            }
            error(at, format!("no source or node named `{name}`"))
        };

        let mut nodes = Vec::new();
        for (name, node) in &full.node {
            let at = format!("node.{name}");
            nodes.push(match (node.mux, node.demux) {
                (Some(mode), None) => mux(&at, node, mode, &layout, &resolve)?,
                (None, Some(mode)) => demux(&at, node, mode, &resolve)?,
                (Some(_), Some(_)) => return error(at, "a node is a mux or a demux, not both"),
                (None, None) => return error(at, "a node needs `mux` or `demux`"),
            });
        }

        let mut sinks = Vec::new();
        let mut follows = Vec::new();
        for (name, sink) in &full.sink {
            let at = format!("sink.{name}");
            check_name(&at, name)?;
            follows.push(resolve(&format!("{at}.input"), &sink.input)?);
            sinks.push(PlannedSink {
                name: name.clone(),
                identity: sink.identity,
            });
        }
        if sinks.is_empty() {
            return error("sink", "a profile needs at least one virtual pad");
        }
        if sources.is_empty() {
            return error("source", "a profile needs at least one pad");
        }

        let graph = GraphSpec {
            sources: specs,
            nodes,
            sinks: follows,
        };
        // The core has the last word: cycles, and anything missed above.
        if let Err(refused) = Graph::build(graph.clone()) {
            return error("", format!("the graph can't be built: {refused}"));
        }
        Ok(Plan {
            graph,
            sources,
            nodes: node_names,
            sinks,
            managed: full.managed,
            hide: full.hide.unwrap_or_default(),
        })
    }

    /// The profile in the full form: a preset written out as the sources,
    /// node and sinks it stands for.
    fn expanded(mut self) -> Result<Self, ProfileError> {
        let has_full = !(self.source.is_empty() && self.node.is_empty() && self.sink.is_empty());
        let Some(preset) = self.preset else {
            for (key, present) in [
                ("primary", self.primary.is_some()),
                ("assist", self.assist.is_some()),
                ("pad", self.pad.is_some()),
                ("outputs", self.outputs.is_some()),
                ("identity", self.identity.is_some()),
                ("rumble", self.rumble.is_some()),
            ] {
                if present {
                    return error(
                        key,
                        "this key belongs to a preset, and the profile names none",
                    );
                }
            }
            return Ok(self);
        };
        if has_full {
            return error(
                "mode",
                "a profile is a preset or names its own sources, nodes and sinks, not both",
            );
        }
        let identity = self.identity.unwrap_or_default();
        let source = |pad: String| Source {
            pad,
            deadzone: None,
        };
        let sink = |input: String| Sink { input, identity };
        let mode = match preset {
            Preset::Priority => Ok(MuxMode::Priority),
            Preset::Average => Ok(MuxMode::Average),
            Preset::Toggle => Ok(MuxMode::Toggle),
            Preset::Unicast => Err(Fanout::Unicast),
            Preset::Multicast => Err(Fanout::Multicast),
        };
        match mode {
            Ok(mode) => {
                for (key, present) in [
                    ("pad", self.pad.is_some()),
                    ("outputs", self.outputs.is_some()),
                ] {
                    if present {
                        return error(key, "this key belongs to the unicast and multicast presets");
                    }
                }
                let (Some(primary), Some(assist)) = (self.primary.take(), self.assist.take())
                else {
                    return error("mode", "this preset needs `primary` and `assist`");
                };
                self.source.insert("primary".to_string(), source(primary));
                self.source.insert("assist".to_string(), source(assist));
                let feedback = match self.rumble.take().unwrap_or_default() {
                    // A mux's own default: whoever reaches the output.
                    Rumble::Both => None,
                    Rumble::Primary => Some(Feedback::Inputs(vec!["primary".to_string()])),
                    Rumble::Assist => Some(Feedback::Inputs(vec!["assist".to_string()])),
                    Rumble::None => Some(Feedback::Named(FeedbackName::None)),
                };
                let node = Node {
                    mux: Some(mode),
                    inputs: vec!["primary".to_string(), "assist".to_string()],
                    feedback,
                    ..Node::default()
                };
                self.node.insert("mux".to_string(), node);
                self.sink
                    .insert("virtual".to_string(), sink("mux".to_string()));
            }
            Err(mode) => {
                for (key, present) in [
                    ("primary", self.primary.is_some()),
                    ("assist", self.assist.is_some()),
                    ("rumble", self.rumble.is_some()),
                ] {
                    if present {
                        return error(
                            key,
                            "this key belongs to the priority, average and toggle presets",
                        );
                    }
                }
                let Some(pad) = self.pad.take() else {
                    return error("mode", "this preset needs `pad`");
                };
                let outputs = self.outputs.take().unwrap_or(2);
                if !(1..=8).contains(&outputs) {
                    return error("outputs", "between 1 and 8 virtual pads");
                }
                self.source.insert("pad".to_string(), source(pad));
                let node = Node {
                    demux: Some(mode),
                    input: Some("pad".to_string()),
                    outputs: Some(outputs),
                    ..Node::default()
                };
                self.node.insert("demux".to_string(), node);
                for output in 0..outputs {
                    self.sink
                        .insert(format!("virtual-{output}"), sink(format!("demux.{output}")));
                }
            }
        }
        self.preset = None;
        self.identity = None;
        Ok(self)
    }
}

type Resolve<'a> = &'a dyn Fn(&str, &str) -> Result<Input, ProfileError>;

fn mux(
    at: &str,
    node: &Node,
    mode: MuxMode,
    layout: &Layout,
    resolve: Resolve<'_>,
) -> Result<NodeSpec, ProfileError> {
    for (key, present) in [
        ("input", node.input.is_some()),
        ("outputs", node.outputs.is_some()),
    ] {
        if present {
            return error(format!("{at}.{key}"), "this key belongs to a demux");
        }
    }
    if node.inputs.is_empty() {
        return error(format!("{at}.inputs"), "a mux needs inputs");
    }
    if node.inputs.len() > MAX_INPUTS {
        return error(
            format!("{at}.inputs"),
            format!("a mux has at most {MAX_INPUTS} inputs"),
        );
    }
    let mut inputs = Vec::new();
    for (index, reference) in node.inputs.iter().enumerate() {
        inputs.push(resolve(&format!("{at}.inputs[{index}]"), reference)?);
    }
    let position = |key: &str, name: &str| match node.inputs.iter().position(|input| input == name)
    {
        Some(position) => Ok(position),
        None => error(
            format!("{at}.{key}"),
            format!("`{name}` isn't one of this mux's inputs"),
        ),
    };

    let mode = match mode {
        MuxMode::Priority => Mode::Priority,
        MuxMode::Average => Mode::Average,
        MuxMode::Toggle => Mode::Toggle,
    };
    let mut config = MuxConfig::new(mode, layout);
    let rule = |kind: Kind| {
        let key = match kind {
            Kind::Stick => GroupKind::Stick,
            Kind::Trigger => GroupKind::Trigger,
            Kind::Button => GroupKind::Button,
            Kind::Hat => GroupKind::Hat,
        };
        node.rules.get(&key).map_or_else(
            || mode.rule(kind),
            |rule| match rule {
                RuleName::Owned => Rule::Owned,
                RuleName::Largest => Rule::Largest,
                RuleName::Blended => Rule::Blended,
                RuleName::Switched => Rule::Switched,
            },
        )
    };
    config.rules = layout
        .groups()
        .iter()
        .map(|(group, kind)| (*group, rule(*kind)))
        .collect();
    config.ungrouped_buttons = rule(Kind::Button);
    config.ungrouped_axes = rule(Kind::Stick);
    if let Some(engage) = node.engage {
        config.engage = engage;
    }
    if let Some(release) = node.release {
        config.release = release;
    }
    thresholds(at, node, &config)?;
    config.feedback = match &node.feedback {
        None | Some(Feedback::Named(FeedbackName::Active)) => MuxFeedback::Active,
        Some(Feedback::Named(FeedbackName::None)) => MuxFeedback::None,
        Some(Feedback::Inputs(names)) => {
            let mut mask = 0u32;
            for name in names {
                mask |= 1u32
                    .checked_shl(position("feedback", name)? as u32)
                    .unwrap_or(0);
            }
            MuxFeedback::Inputs(mask)
        }
    };
    let selects = config.selects();
    config.switch = match &node.switch {
        // A toggle whose `rules` leave nothing switched doesn't take
        // the toggle's own switch: it would only keep the button from
        // the game.
        None if !selects => None,
        // A toggle's own switch is on its second input, and a toggle of
        // one input has none: its only input's, then.
        None if inputs.len() == 1 => config.switch.map(|switch| Switch {
            input: Some(0),
            ..switch
        }),
        None => config.switch,
        Some(SwitchSpec::Off(false)) => None,
        Some(SwitchSpec::Off(true)) => {
            return error(
                format!("{at}.switch"),
                "name the button, or write `false` for none",
            );
        }
        Some(SwitchSpec::Button(name)) => Some(Switch {
            input: None,
            button: button(&format!("{at}.switch"), name)?,
        }),
        Some(SwitchSpec::On {
            button: name,
            input,
        }) => Some(Switch {
            input: Some(position("switch.input", input)?),
            button: button(&format!("{at}.switch.button"), name)?,
        }),
    };
    if config.switch.is_some() && !selects {
        return error(
            format!("{at}.switch"),
            "this mux never selects an input, since none of its rules is `switched`, so a switch would only keep its button from the game: leave `switch` out, or make a rule `switched`",
        );
    }
    Ok(NodeSpec::Mux { inputs, config })
}

/// Checks a mux's thresholds, as given or as defaulted, as the core
/// would, so that a refusal says which key is wrong.
fn thresholds(at: &str, node: &Node, config: &MuxConfig) -> Result<(), ProfileError> {
    let (engage, release) = (config.engage, config.release);
    // Written so that a value that isn't a number fails.
    let between = |value: f32| value > 0.0 && value <= 1.0;
    if !between(engage) {
        return error(
            format!("{at}.engage"),
            "`engage` is more than 0 and at most 1: how far a control moves, from rest to all the way, before an input takes hold of it",
        );
    }
    if !between(release) {
        return error(
            format!("{at}.release"),
            "`release` is more than 0 and at most 1: at 0, an input that took hold of a control would never let go",
        );
    }
    if release > engage {
        // The one written, where only one is: the other is the default.
        let key = if node.release.is_some() {
            "release"
        } else {
            "engage"
        };
        return error(
            format!("{at}.{key}"),
            format!(
                "`release` ({release}) is at most `engage` ({engage}): an input lets go below where it took hold"
            ),
        );
    }
    Ok(())
}

fn demux(
    at: &str,
    node: &Node,
    mode: Fanout,
    resolve: Resolve<'_>,
) -> Result<NodeSpec, ProfileError> {
    for (key, present) in [
        ("inputs", !node.inputs.is_empty()),
        ("engage", node.engage.is_some()),
        ("release", node.release.is_some()),
        ("rules", !node.rules.is_empty()),
    ] {
        if present {
            return error(format!("{at}.{key}"), "this key belongs to a mux");
        }
    }
    let Some(input) = &node.input else {
        return error(format!("{at}.input"), "a demux needs an input");
    };
    let Some(outputs) = node.outputs else {
        return error(
            format!("{at}.outputs"),
            "a demux needs to know how many outputs it has",
        );
    };
    if !(1..=MAX_OUTPUTS).contains(&outputs) {
        return error(
            format!("{at}.outputs"),
            format!("a demux has between 1 and {MAX_OUTPUTS} outputs"),
        );
    }
    let mode = match mode {
        Fanout::Unicast => DemuxMode::Unicast,
        Fanout::Multicast => DemuxMode::Multicast,
    };
    let mut config = DemuxConfig::new(mode, outputs);
    config.feedback = match &node.feedback {
        None | Some(Feedback::Named(FeedbackName::Active)) => DemuxFeedback::Active,
        Some(Feedback::Named(FeedbackName::None)) => DemuxFeedback::None,
        Some(Feedback::Inputs(_)) => {
            return error(
                format!("{at}.feedback"),
                "a demux's feedback is \"active\" or \"none\"",
            );
        }
    };
    config.switch = match &node.switch {
        None => config.switch,
        Some(SwitchSpec::Off(false)) => None,
        Some(SwitchSpec::Off(true)) => {
            return error(
                format!("{at}.switch"),
                "name the button, or write `false` for none",
            );
        }
        Some(SwitchSpec::Button(_) | SwitchSpec::On { .. }) if mode == DemuxMode::Multicast => {
            return error(
                format!("{at}.switch"),
                "a multicast demux sends to every output and never selects one, so a switch would only keep its button from the games",
            );
        }
        Some(SwitchSpec::Button(name)) => Some(button(&format!("{at}.switch"), name)?),
        Some(SwitchSpec::On { .. }) => {
            return error(
                format!("{at}.switch"),
                "a demux has one input, so its switch is just the button's name",
            );
        }
    };
    Ok(NodeSpec::Demux {
        input: resolve(&format!("{at}.input"), input)?,
        config,
    })
}
