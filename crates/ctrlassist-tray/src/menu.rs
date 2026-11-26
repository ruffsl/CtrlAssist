//! The tray's menu as plain data, made from a view. Nothing here knows
//! the desktop: the tray turns these entries into its toolkit's items,
//! and tests read them as an outline.
//!
//! A profile's menu is the window's profile detail in small: what to do
//! with it, who has control while it runs, and for a profile in the
//! preset form its fields, each a submenu by its kind. What there is to
//! choose, what each choice is called and what a profile writes for it
//! all come from the fields the daemon serves, as they do for the
//! window's form.

use std::collections::BTreeMap;

use ctrlassist::form::{self, Field, Kind, PRESET, Values};
use ctrlassist::{Hiding, Pad, Phase, Profile, Setup};
use ctrlassist_protocol::profile::{DEFAULT, PadRef};
use ctrlassist_protocol::rpc;

use crate::state::View;

/// What activating an entry asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Run a kept profile.
    Start(String),
    /// Stop a setup.
    Stop(String),
    /// Have a setup's node select one of its choices.
    Select {
        /// The setup.
        setup: String,
        /// The node.
        node: String,
        /// The choice, by name.
        choice: String,
    },
    /// Buzz a pad.
    Identify(String),
    /// Stop remembering a pad.
    Forget(String),
    /// Set one field of a profile in the preset form: it is kept so,
    /// and a setup that runs it takes the change at once.
    Set {
        /// The profile.
        profile: String,
        /// The field's key.
        key: String,
        /// Its new value, as a profile writes it.
        value: String,
    },
    /// Open the settings window, or raise it if it is open.
    Settings,
    /// End the tray.
    Quit,
}

impl Command {
    /// Sets the field `key` of `profile` to `value`.
    #[must_use]
    pub fn set(profile: &str, key: &str, value: &str) -> Self {
        Self::Set {
            profile: profile.to_string(),
            key: key.to_string(),
            value: value.to_string(),
        }
    }
}

/// A row's icon: names in a desktop's icon theme, the first it has
/// being the one shown, in its symbolic form. Empty for none.
pub type Icon = &'static [&'static str];

/// The menu's icons. The choices follow 0.4.0's tray where it had one.
pub mod icon {
    use super::Icon;

    /// No icon.
    pub const NONE: Icon = &[];
    /// A pad, and the tray itself. Adwaita has the first name's
    /// symbolic form and Breeze the second's.
    pub const PAD: Icon = &["input-gaming", "input-gamepad"];
    /// Start, and a profile that runs.
    pub const START: Icon = &["media-playback-start"];
    /// Stop, and a profile that doesn't run.
    pub const STOP: Icon = &["media-playback-stop"];
    /// A setup held for a pad.
    pub const HELD: Icon = &["media-playback-pause"];
    /// A setup that failed.
    pub const FAILED: Icon = &["dialog-error"];
    /// A profile that is wrong.
    pub const WRONG: Icon = &["dialog-warning"];
    /// A setup starting.
    pub const STARTING: Icon = &["view-refresh"];
    /// Which pad has control.
    pub const CONTROL: Icon = &["swap-panels", "object-flip-horizontal"];
    /// A profile's mode.
    pub const MODE: Icon = &["media-playlist-shuffle"];
    /// Rumble, and a buzz.
    pub const RUMBLE: Icon = &["notification-active", "preferences-desktop-notification"];
    /// Whether the real pads are hidden.
    pub const HIDE: Icon = &["view-visible"];
    /// What a virtual pad presents itself as.
    pub const IDENTITY: Icon = &["edit-copy"];
    /// Whether the daemon starts a profile itself.
    pub const MANAGED: Icon = &["system-run", "media-playlist-repeat"];
    /// Forget a pad.
    pub const FORGET: Icon = &["edit-delete", "list-remove"];
    /// The daemon's status.
    pub const STATUS: Icon = &["dialog-information", "help-about"];
    /// The settings window.
    pub const SETTINGS: Icon = &["settings-configure", "configure", "preferences-system"];
    /// End the tray.
    pub const QUIT: Icon = &["application-exit"];
}

/// One of a radio group's options.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// What it says.
    pub label: String,
    /// Whether it can be picked.
    pub enabled: bool,
    /// What picking it asks for.
    pub command: Command,
}

/// One entry of a menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A line between groups.
    Separator,
    /// A row: something to activate, or with no command, something said.
    Item {
        /// Its icon.
        icon: Icon,
        /// What it says.
        label: String,
        /// What activating it asks for.
        command: Option<Command>,
    },

    /// A group of which one is picked.
    Radio {
        /// The options.
        choices: Vec<Choice>,
        /// Which is picked, if one is.
        selected: Option<usize>,
    },
    /// A row that opens a menu of its own.
    Sub {
        /// Its icon.
        icon: Icon,
        /// What it says.
        label: String,
        /// Its entries.
        entries: Vec<Self>,
    },
}

fn said(icon: Icon, label: impl Into<String>) -> Entry {
    Entry::Item {
        icon,
        label: label.into(),
        command: None,
    }
}

fn action(icon: Icon, label: impl Into<String>, command: Command) -> Entry {
    Entry::Item {
        icon,
        label: label.into(),
        command: Some(command),
    }
}

fn sub(icon: Icon, label: impl Into<String>, entries: Vec<Entry>) -> Entry {
    Entry::Sub {
        icon,
        label: label.into(),
        entries,
    }
}

/// A name as a row says it: `primary` as "Primary".
pub(crate) fn capitalized(name: &str) -> String {
    let mut letters = name.chars();
    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}

/// The most of a pad's name a row of a profile's menu or a line of the
/// tooltip shows, in characters: a long name widens every row of the
/// menu, and a desktop wraps a tooltip's long lines.
const NAME_MAX: usize = 28;

/// A pad's name cut to fit, with "…" for what was cut.
pub(crate) fn short(name: &str) -> String {
    if name.chars().count() <= NAME_MAX {
        return name.to_string();
    }
    let kept: String = name.chars().take(NAME_MAX - 1).collect();
    format!("{}…", kept.trim_end())
}

/// The name a game sees a virtual pad of `identity` under.
pub(crate) fn virtual_name(view: &View, identity: &str) -> String {
    view.identities
        .iter()
        .find(|known| known.id == identity)
        .map_or_else(|| capitalized(identity), |known| known.name.clone())
}

/// What a running setup's nodes select between: who has control.
fn control_entries(setup: &Setup, entries: &mut Vec<Entry>) {
    for node in &setup.nodes {
        // One node needs no name, and of several each is called by its
        // own, as the window has them.
        let label = if setup.nodes.len() == 1 {
            "Has control".to_string()
        } else {
            node.name.clone()
        };
        let choices = node
            .choices
            .iter()
            .map(|choice| {
                // A mux doesn't hand over to a pad that isn't there.
                let absent = setup
                    .sources
                    .iter()
                    .any(|source| source.name == *choice && !source.present);
                Choice {
                    label: choice.clone(),
                    enabled: !absent,
                    command: Command::Select {
                        setup: setup.name.clone(),
                        node: node.name.clone(),
                        choice: choice.clone(),
                    },
                }
            })
            .collect();
        entries.push(Entry::Separator);
        entries.push(sub(
            icon::CONTROL,
            label,
            vec![Entry::Radio {
                choices,
                selected: Some(node.selected),
            }],
        ));
    }
}

/// What a running setup is made of, said and not chosen: for a profile
/// no form can show.
fn part_entries(view: &View, setup: &Setup, entries: &mut Vec<Entry>) {
    for source in &setup.sources {
        let pad = match (&source.pad, source.present) {
            (Some(pad), true) => short(view.pad_name(pad)),
            (Some(pad), false) => format!("{} (disconnected)", short(view.pad_name(pad))),
            (None, _) => "waiting for a pad that fits".to_string(),
        };
        entries.push(said(
            icon::PAD,
            format!("{}: {pad}", capitalized(&source.name)),
        ));
    }
    for sink in &setup.sinks {
        let made = if sink.node.is_some() {
            ""
        } else {
            " (not made)"
        };
        entries.push(said(
            icon::PAD,
            format!(
                "{}: {}{made}",
                capitalized(&sink.name),
                short(&virtual_name(view, &sink.identity))
            ),
        ));
    }
}

/// A setup's phase as the window says it, with the icon for it.
pub(crate) fn phase_label(phase: Phase) -> (&'static str, Icon) {
    match phase {
        Phase::Running => ("Running", icon::START),
        Phase::Held => ("Held", icon::HELD),
        Phase::Failed => ("Failed", icon::FAILED),
        _ => ("Starting", icon::STARTING),
    }
}

/// What a profile's value for a pad field names: a pad by its id as it
/// stands, or by any of the ways a profile names one.
fn wanted(view: &View, value: &str) -> Option<PadRef> {
    if view.pad(value).is_some() {
        return Some(PadRef::Exact(value.to_string()));
    }
    PadRef::parse(value)
}

/// The known pad each pad field of the profile `name` takes, by its key:
/// one named by its id that pad; one named by model, while a setup runs
/// the profile, the pad that setup has for it unless another part names
/// that pad by its id, and otherwise the pad a start would take now, as
/// [`crate::tip::bind`] has the daemon bind them. With `fresh`, as a
/// start would take them whether or not a setup runs the profile, its
/// own pads counting as free. A part with no pad to take isn't there.
fn pads_taken<'a>(
    view: &'a View,
    name: &str,
    values: &Values,
    fresh: bool,
) -> BTreeMap<&'a str, &'a Pad> {
    let preset = values.get(PRESET).map(String::as_str).unwrap_or_default();
    // In the order of their keys, as a start takes a profile's pads.
    let mut parts: Vec<(&str, PadRef)> = view
        .fields
        .iter()
        .filter(|field| field.kind == Kind::Pad && field.applies_to(preset))
        .filter_map(|field| Some((field.key.as_str(), wanted(view, values.get(&field.key)?)?)))
        .collect();
    parts.sort_by_key(|(key, _)| *key);
    let refs: Vec<&PadRef> = parts.iter().map(|(_, wanted)| wanted).collect();
    let bound = crate::tip::bind(view, &refs, fresh.then_some(name));
    let named = crate::tip::named_exactly(&refs);
    let setup = view.setup(name).filter(|_| !fresh);
    parts
        .iter()
        .zip(bound)
        .filter_map(|((key, wanted), bound)| {
            let pad = match wanted {
                PadRef::Exact(_) => bound,
                PadRef::Model(..) => setup
                    .and_then(|setup| {
                        let source = setup.sources.iter().find(|source| source.name == *key)?;
                        view.pad(source.pad.as_deref()?)
                    })
                    .filter(|pad| {
                        crate::tip::names(wanted, pad) && !named.contains(&pad.id.as_str())
                    })
                    .or(bound),
            };
            Some((*key, pad?))
        })
        .collect()
}

/// The known pad each pad field of the profile `name` means, by its key:
/// the pad it takes, and for one named by model that would take none, a
/// pad of the model that is connected, or any that is known.
fn pads_meant<'a>(view: &'a View, name: &str, values: &Values) -> BTreeMap<&'a str, &'a Pad> {
    let mut meant = pads_taken(view, name, values, false);
    let preset = values.get(PRESET).map(String::as_str).unwrap_or_default();
    for field in &view.fields {
        if field.kind != Kind::Pad
            || !field.applies_to(preset)
            || meant.contains_key(field.key.as_str())
        {
            continue;
        }
        let Some(wanted @ PadRef::Model(..)) =
            values.get(&field.key).and_then(|value| wanted(view, value))
        else {
            continue;
        };
        let mut fitting = view
            .pads
            .iter()
            .filter(|pad| crate::tip::names(&wanted, pad));
        let first = fitting.clone().next();
        if let Some(pad) = fitting.find(|pad| pad.connected).or(first) {
            meant.insert(field.key.as_str(), pad);
        }
    }
    meant
}

/// Works a pick at the profile `name` into its values. A pad picked for
/// one part that another part names by its id changes places with it:
/// the other part takes the pad this one had, or is left to choose.
/// Without that, two pads could never change parts, each being the
/// other's and so not to be picked. A part named by model whose pad is
/// the one picked keeps its model when a start would give it another pad
/// of the model, and otherwise takes the pad this one had; with none, it
/// keeps its model, never being left to choose by a pick. The window's
/// form does the same, and `swap_cases.txt` holds the cases both meet.
pub(crate) fn pick(view: &View, name: &str, values: &mut Values, key: &str, value: &str) {
    let is_pad = |key: &str| {
        view.fields
            .iter()
            .any(|field| field.key == key && field.kind == Kind::Pad)
    };
    let had = values.get(key).cloned();
    let picked = wanted(view, value).and_then(|wanted| match wanted {
        PadRef::Exact(id) => view.pad(&id),
        PadRef::Model(..) => None,
    });
    if is_pad(key)
        && let Some(picked) = picked
    {
        let others: Vec<String> = pads_taken(view, name, values, false)
            .into_iter()
            .filter(|(other, pad)| *other != key && pad.id == picked.id)
            .map(|(other, _)| other.to_string())
            .collect();
        let mut then = values.clone();
        then.insert(key.to_string(), value.to_string());
        let taken_then = pads_taken(view, name, &then, true);
        for other in others {
            let by_model = values
                .get(&other)
                .and_then(|value| wanted(view, value))
                .is_some_and(|wanted| matches!(wanted, PadRef::Model(..)));
            if by_model {
                let elsewhere = taken_then
                    .get(other.as_str())
                    .is_some_and(|pad| pad.id != picked.id);
                if !elsewhere && let Some(had) = &had {
                    values.insert(other, had.clone());
                }
                continue;
            }
            match &had {
                Some(had) => values.insert(other, had.clone()),
                None => values.remove(&other),
            };
        }
    }
    values.insert(key.to_string(), value.to_string());
}

/// A pad field of the profile `name`: the known pads to pick from, with
/// the one picked. A pad can't be picked that isn't connected or is in
/// another profile's setup. One that another part of this profile has
/// can: the two change places, as [`pick`] has it.
fn pad_entry(view: &View, name: &str, values: &Values, field: &Field) -> Entry {
    let picked = values.get(&field.key).map(String::as_str);
    let pads = pads_meant(view, name, values);
    let meant = pads.get(field.key.as_str()).copied();
    // The part a pick of `pad` would move, by its label.
    let moves = |pad: &Pad| {
        let mut then = values.clone();
        pick(view, name, &mut then, &field.key, &pad.id);
        view.fields
            .iter()
            .find(|other| {
                other.kind == Kind::Pad
                    && other.key != field.key
                    && values.get(&other.key) != then.get(&other.key)
            })
            .map(|other| other.label.clone())
    };
    let choices: Vec<Choice> = view
        .pads
        .iter()
        .map(|pad| {
            let (enabled, label) = match (pad.connected, pad.setup.as_deref()) {
                (false, _) => (false, format!("{} (disconnected)", pad.name)),
                (true, Some(setup)) if setup != name => {
                    (false, format!("{} (in {setup})", pad.name))
                }
                (true, _) => match moves(pad) {
                    Some(part) => (true, format!("{} (swap with {part})", pad.name)),
                    None => (true, pad.name.clone()),
                },
            };
            Choice {
                label,
                enabled,
                command: Command::set(name, &field.key, &pad.id),
            }
        })
        .collect();
    let selected = meant.and_then(|meant| view.pads.iter().position(|pad| pad.id == meant.id));
    // A pad the profile names that isn't known goes by what it wrote.
    // Cut short on the profile's own menu, where a long name widens
    // every row; the pads to pick from, under it, are named in full.
    let label = match (meant, picked) {
        (Some(pad), _) if !pad.connected => format!("{} (disconnected)", short(&pad.name)),
        (Some(pad), _) => short(&pad.name),
        (None, Some(value)) => short(value),
        (None, None) => "not chosen".to_string(),
    };
    sub(
        icon::PAD,
        format!("{}: {label}", field.label),
        vec![Entry::Radio { choices, selected }],
    )
}

/// A field of choices: each by its label, with the one it stands at.
fn choice_entry(
    view: &View,
    name: &str,
    values: &Values,
    field: &Field,
    choices: &[form::Choice],
) -> Entry {
    let stands_at = values
        .get(&field.key)
        .map_or(field.default.as_str(), String::as_str);
    let selected = choices.iter().position(|choice| choice.value == stands_at);
    let label = selected.map_or(stands_at, |at| choices[at].label.as_str());
    // Choices that share a caption are a group under it, each by what
    // the caption leaves to say; the rest are a group by their labels.
    let mut entries = Vec::new();
    for run in choices.chunk_by(|one, next| one.caption == next.caption) {
        let caption = run.first().map_or("", |choice| choice.caption.as_str());
        if !caption.is_empty() {
            entries.push(said(icon::NONE, caption));
        }
        entries.push(Entry::Radio {
            choices: run
                .iter()
                .map(|choice| Choice {
                    label: if choice.brief.is_empty() {
                        choice.label.clone()
                    } else {
                        choice.brief.clone()
                    },
                    // An identity the daemon says it can't make now
                    // can't be picked.
                    enabled: field.key != "identity"
                        || view
                            .identities
                            .iter()
                            .find(|known| known.id == choice.value)
                            .is_none_or(|known| known.available),
                    command: Command::set(name, &field.key, &choice.value),
                })
                .collect(),
            selected: run.iter().position(|choice| choice.value == stands_at),
        });
    }
    let icon = match field.key.as_str() {
        PRESET => icon::MODE,
        "rumble" => icon::RUMBLE,
        "hide" => icon::HIDE,
        "identity" => icon::IDENTITY,
        _ => icon::NONE,
    };
    sub(icon, format!("{}: {label}", field.label), entries)
}

/// A number in a range: each it may be, with the one it stands at.
fn number_entry(name: &str, values: &Values, field: &Field, min: u32, max: u32) -> Entry {
    let stands_at = values
        .get(&field.key)
        .map_or(field.default.as_str(), String::as_str);
    let numbers: Vec<String> = (min..=max).map(|number| number.to_string()).collect();
    sub(
        icon::NONE,
        format!("{}: {stands_at}", field.label),
        vec![Entry::Radio {
            selected: numbers.iter().position(|number| number == stands_at),
            choices: numbers
                .iter()
                .map(|number| Choice {
                    label: number.clone(),
                    enabled: true,
                    command: Command::set(name, &field.key, number),
                })
                .collect(),
        }],
    )
}

/// A switch: on or off, with which it is. A submenu like the other
/// fields, where a check mark of its own sat in another column than
/// their icons, and its row says which it is so that it needn't be
/// opened to tell.
fn switch_entry(name: &str, values: &Values, field: &Field) -> Entry {
    let on = values.get(&field.key).unwrap_or(&field.default) == "true";
    let choice = |label: &str, value: &str| Choice {
        label: label.to_string(),
        enabled: true,
        command: Command::set(name, &field.key, value),
    };
    let icon = match field.key.as_str() {
        "managed" => icon::MANAGED,
        _ => icon::NONE,
    };
    sub(
        icon,
        format!("{}: {}", field.label, if on { "On" } else { "Off" }),
        vec![Entry::Radio {
            choices: vec![choice("On", "true"), choice("Off", "false")],
            selected: Some(usize::from(!on)),
        }],
    )
}

/// The fields of a profile in the preset form, as the window's detail
/// has them: each a submenu by its kind.
fn field_entries(view: &View, name: &str, values: &Values) -> Vec<Entry> {
    let preset = values.get(PRESET).map(String::as_str).unwrap_or_default();
    // In the order of the window's cards: the pads, the virtual pads,
    // how they are merged, and the rest.
    let card = |field: &Field| match field.group.as_str() {
        form::INPUTS => 0,
        form::OUTPUTS => 1,
        form::ARRANGEMENT => 2,
        _ => 3,
    };
    let mut fields: Vec<&Field> = view
        .fields
        .iter()
        .filter(|field| field.applies_to(preset))
        .collect();
    fields.sort_by_key(|field| card(field));
    fields
        .into_iter()
        .filter_map(|field| match &field.kind {
            Kind::Pad => Some(pad_entry(view, name, values, field)),
            Kind::Choice { choices } => Some(choice_entry(view, name, values, field, choices)),
            Kind::Number { min, max } => Some(number_entry(name, values, field, *min, *max)),
            Kind::Switch => Some(switch_entry(name, values, field)),
            // A kind of field from a later release: left to the window.
            _ => None,
        })
        .collect()
}

/// The profile `name` as its row says it: the default profile carries
/// a check mark, as in the window.
#[must_use]
pub fn shown_name(name: &str) -> String {
    if name == DEFAULT {
        format!("{name} ✓")
    } else {
        name.to_string()
    }
}

/// A profile's menu: what to do with it, who has control while it runs,
/// and its fields, or for one no form can show, what it is made of.
fn profile_entry(view: &View, name: &str) -> Entry {
    let profile: Option<&Profile> = view.profiles.iter().find(|profile| profile.name == name);
    let setup = view.setup(name);
    // The fields are the daemon's to say, and only a preset has them.
    let values = profile.and_then(|profile| form::from_toml(&view.fields, &profile.toml));
    let shown = shown_name(name);
    let mut entries = Vec::new();
    let (label, icon) = match (setup, profile) {
        (Some(setup), _) => {
            let (word, icon) = phase_label(setup.phase);
            entries.push(action(icon::STOP, "Stop", Command::Stop(name.to_string())));
            if let Some(reason) = &setup.reason {
                entries.push(Entry::Separator);
                entries.push(said(icon::NONE, reason.clone()));
            } else {
                control_entries(setup, &mut entries);
            }
            (format!("{shown} ({word})"), icon)
        }
        (
            None,
            Some(Profile {
                problem: Some(problem),
                ..
            }),
        ) => {
            entries.push(said(icon::NONE, format!("Can't run: {problem}")));
            (format!("{shown} (Wrong)"), icon::WRONG)
        }
        (None, Some(profile)) if !profile.unset.is_empty() => {
            entries.push(said(icon::START, "Choose its pads to start"));
            (format!("{shown} (Not set up)"), icon::STOP)
        }
        (None, _) => {
            entries.push(action(
                icon::START,
                "Start",
                Command::Start(name.to_string()),
            ));
            (shown, icon::STOP)
        }
    };
    match (&values, setup) {
        (Some(values), _) => {
            entries.push(Entry::Separator);
            entries.extend(field_entries(view, name, values));
        }
        (None, Some(setup)) if setup.reason.is_none() => {
            entries.push(Entry::Separator);
            part_entries(view, setup, &mut entries);
        }
        _ => {}
    }
    sub(icon, label, entries)
}

/// Every kept profile and any setup whose profile is gone, in the order
/// they are listed: the default profile, then the rest by name.
#[must_use]
pub fn listed(view: &View) -> Vec<&str> {
    let mut names: Vec<&str> = view
        .profiles
        .iter()
        .map(|profile| profile.name.as_str())
        .chain(view.setups.iter().map(|setup| setup.name.as_str()))
        .collect();
    names.sort_unstable_by_key(|name| (*name != DEFAULT, *name));
    names.dedup();
    names
}

fn pad_entries(view: &View) -> Vec<Entry> {
    if view.pads.is_empty() {
        return vec![said(icon::NONE, "No pads are known")];
    }
    let mut pads: Vec<Entry> = view
        .pads
        .iter()
        .map(|pad| {
            let state = match (&pad.setup, pad.connected) {
                (Some(setup), _) => format!(" (in {setup})"),
                (None, false) => " (remembered)".to_string(),
                (None, true) => String::new(),
            };
            let mut entries = vec![said(icon::NONE, pad.id.clone())];
            if pad.connected {
                entries.push(if pad.rumble == Some(false) {
                    said(
                        icon::NONE,
                        if pad.is_steam_controller() {
                            "Buzzed only while a profile uses it"
                        } else {
                            "No rumble to buzz with"
                        },
                    )
                } else {
                    action(
                        icon::RUMBLE,
                        "Identify (buzz)",
                        Command::Identify(pad.id.clone()),
                    )
                });
            } else if pad.setup.is_none() {
                entries.push(action(
                    icon::FORGET,
                    "Forget",
                    Command::Forget(pad.id.clone()),
                ));
            }
            sub(icon::PAD, format!("{}{state}", pad.name), entries)
        })
        .collect();
    // The virtual pads that exist follow the real ones, as in the
    // window: each is buzzed as a game would rumble it.
    let made: Vec<Entry> = view
        .setups
        .iter()
        .flat_map(|setup| setup.sinks.iter().map(move |sink| (setup, sink)))
        .filter_map(|(setup, sink)| {
            let node = sink.node.as_deref()?;
            Some(sub(
                icon::PAD,
                format!("{} (in {})", virtual_name(view, &sink.identity), setup.name),
                vec![
                    said(icon::NONE, node),
                    action(
                        icon::RUMBLE,
                        "Identify (buzz)",
                        Command::Identify(rpc::virtual_pad(&setup.name, &sink.name)),
                    ),
                ],
            ))
        })
        .collect();
    if !made.is_empty() {
        pads.push(Entry::Separator);
        pads.extend(made);
    }
    pads
}
/// The whole menu for what the daemon publishes now.
#[must_use]
pub fn menu(view: &View) -> Vec<Entry> {
    let mut entries = Vec::new();
    if !view.alive {
        entries.push(said(icon::FAILED, "The CtrlAssist daemon isn't running"));
    } else if let Some(theirs) = &view.mismatch {
        entries.push(said(icon::WRONG, mismatch_text(theirs)));
    } else if !view.hydrated {
        entries.push(said(icon::STARTING, "Reading from the daemon…"));
    } else {
        for name in listed(view) {
            entries.push(profile_entry(view, name));
        }
        if !entries.is_empty() {
            entries.push(Entry::Separator);
        }
        let connected = view.pads.iter().filter(|pad| pad.connected).count();
        entries.push(sub(
            icon::PAD,
            format!("Pads ({connected}/{} connected)", view.pads.len()),
            pad_entries(view),
        ));
        if let Some(daemon) = &view.daemon {
            entries.push(Entry::Separator);
            entries.push(said(
                icon::STATUS,
                format!(
                    "Hiding: {} · Real-time: {}",
                    hiding_text(daemon.hiding),
                    if daemon.realtime { "yes" } else { "no" }
                ),
            ));
        }
    }
    entries.push(Entry::Separator);
    // There whatever the daemon is doing: the window says more about a
    // daemon that isn't running than the menu can.
    entries.push(action(icon::SETTINGS, "Settings", Command::Settings));
    entries.push(action(icon::QUIT, "Quit tray", Command::Quit));
    entries
}

/// How the menu says the daemon hides a setup's pads: by the mechanism's
/// name, or, while the daemon has just started and hasn't heard from the
/// hiding service yet, that it is looking for it.
#[must_use]
pub fn hiding_text(hiding: Hiding) -> &'static str {
    match hiding {
        Hiding::Service | Hiding::Grab => hiding.as_str(),
        Hiding::Looking => "looking for the service…",
        // A mechanism from a later release, by its name. The protocol
        // marks the enum non-exhaustive, so this arm can't be left out.
        _ => hiding.as_str(),
    }
}

/// What the menu and the tooltip say of a daemon on release `theirs`,
/// which this tray can't talk to.
#[must_use]
pub fn mismatch_text(theirs: &str) -> String {
    format!(
        "Version mismatch: daemon {theirs}, tray {}. Restart the daemon or the tray so they match.",
        ctrlassist_protocol::VERSION
    )
}

/// The menu as an indented outline, one entry a line: `(o)` and `( )`
/// for marks, a trailing `>` on a submenu, `-` before a row that can't be
/// activated. What the tests read.
#[must_use]
pub fn outline(entries: &[Entry]) -> String {
    fn walk(entries: &[Entry], depth: usize, out: &mut String) {
        let pad = "  ".repeat(depth);
        for entry in entries {
            match entry {
                Entry::Separator => out.push_str(&format!("{pad}---\n")),
                Entry::Item { label, command, .. } => {
                    let dead = if command.is_none() { "- " } else { "" };
                    out.push_str(&format!("{pad}{dead}{label}\n"));
                }

                Entry::Radio { choices, selected } => {
                    for (index, choice) in choices.iter().enumerate() {
                        let mark = if *selected == Some(index) { "o" } else { " " };
                        let dead = if choice.enabled { "" } else { "- " };
                        out.push_str(&format!("{pad}({mark}) {dead}{}\n", choice.label));
                    }
                }
                Entry::Sub { label, entries, .. } => {
                    out.push_str(&format!("{pad}{label} >\n"));
                    walk(entries, depth + 1, out);
                }
            }
        }
    }
    let mut out = String::new();
    walk(entries, 0, &mut out);
    out
}

#[cfg(test)]
mod tests {

    use super::*;
    use crate::state::tests::{profile, view};

    /// The cases the tray's and the window's picks both meet; read by
    /// the window's tests too.
    const SWAP_CASES: &str = include_str!("swap_cases.txt");

    /// Each case of `swap_cases.txt`, run through the menu's pick.
    #[test]
    fn a_pick_moves_the_other_parts_as_the_shared_cases_say() {
        let mut ran = 0;
        for case in SWAP_CASES.split("\ncase: ").skip(1) {
            let mut lines = case.lines();
            let title = lines.next().unwrap_or_default();
            let mut given = BTreeMap::new();
            for line in lines {
                if let Some((key, rest)) = line.split_once(": ") {
                    given.insert(key, rest);
                }
            }
            let mut view = view(None, Hiding::Service);
            view.pads = given["pads"]
                .split(' ')
                .map(|pad| {
                    let (id, rest) = pad.split_once('=').unwrap();
                    let (rest, setup) = rest
                        .split_once('@')
                        .map_or((rest, None), |(rest, setup)| (rest, Some(setup)));
                    let connected = !rest.ends_with('~');
                    let (vendor, product) = rest.trim_end_matches('~').split_once(':').unwrap();
                    Pad {
                        vendor: u16::from_str_radix(vendor, 16).unwrap(),
                        product: u16::from_str_radix(product, 16).unwrap(),
                        ..crate::state::tests::pad(id, id, connected, setup)
                    }
                })
                .collect();
            let fields = |line: &str| -> Vec<(String, String)> {
                line.split(' ')
                    .filter(|pair| !pair.is_empty())
                    .map(|pair| {
                        let (key, value) = pair.split_once('=').unwrap();
                        (key.to_string(), value.to_string())
                    })
                    .collect()
            };
            let mut toml = "mode = \"toggle\"\n".to_string();
            for (key, value) in fields(given["before"]) {
                toml.push_str(&format!("{key} = \"{value}\"\n"));
            }
            view.profiles = vec![Profile {
                toml: toml.clone(),
                ..profile("couch", None)
            }];
            view.setups = given
                .get("running")
                .map(|running| {
                    let mut setup = crate::state::tests::couch(Phase::Running);
                    setup.sources = fields(running)
                        .into_iter()
                        .map(|(name, pad)| ctrlassist::SetupSource {
                            name,
                            pad: Some(pad),
                            present: true,
                        })
                        .collect();
                    setup
                })
                .into_iter()
                .collect();
            let mut values = form::from_toml(&view.fields, &toml).unwrap();
            let (key, value) = fields(given["pick"]).remove(0);
            pick(&view, "couch", &mut values, &key, &value);
            for (key, value) in fields(given["after"]) {
                let held = values.get(&key).map_or("-", String::as_str);
                assert_eq!(held, value, "{title}: {key}");
            }
            ran += 1;
        }
        assert_eq!(ran, 12, "every case was read");
    }

    /// The fixture's daemon with the default profile as a daemon makes
    /// it: its pads still to choose.
    fn with_default(phase: Option<Phase>) -> View {
        let mut view = view(phase, Hiding::Service);
        view.profiles.push(Profile {
            toml: ctrlassist_protocol::profile::DEFAULT_TOML.to_string(),
            unset: vec!["primary".to_string(), "assist".to_string()],
            ..profile("default", None)
        });
        view
    }

    /// One profile's menu, from its row to the next row at the top.
    fn of<'a>(menu: &'a str, row: &str) -> &'a str {
        let from = menu
            .find(row)
            .unwrap_or_else(|| unreachable!("no {row:?} in\n{menu}"));
        let rest = &menu[from..];
        let end = rest[row.len()..]
            .find("\n---\n")
            .into_iter()
            .chain(
                rest.lines()
                    .skip(1)
                    .find(|line| !line.starts_with(' '))
                    .and_then(|next| rest.find(&format!("\n{next}\n"))),
            )
            .min()
            .map_or(rest.len(), |end| end + 1);
        &rest[..end]
    }

    /// The default profile is first, with its mark, and before its pads
    /// are chosen its menu is where they are chosen: the fields in the
    /// order of the window's cards.
    #[test]
    fn the_default_profile_is_first_and_its_menu_has_its_fields() {
        let menu = outline(&menu(&with_default(Some(Phase::Held))));
        let expected = "\
default ✓ (Not set up) >
  - Choose its pads to start
  ---
  Primary: not chosen >
    ( ) - DualSense (in couch)
    ( ) 8BitDo Pro 2
    ( ) - Xbox pad (disconnected)
  Assist: not chosen >
    ( ) - DualSense (in couch)
    ( ) 8BitDo Pro 2
    ( ) - Xbox pad (disconnected)
  Identity: Generic >
    (o) Generic
    ( ) Xbox
    ( ) DualSense
    ( ) Steam Controller
  Mode: Priority >
    (o) Priority
    ( ) Average
    ( ) Toggle
    ( ) Unicast
    ( ) Multicast
  Rumble: Both pads >
    (o) Both pads
    ( ) Primary only
    ( ) Assist only
    ( ) Neither
  Gyro: Primary only >
    (o) Primary only
    - Assist holds
    ( ) South
    ( ) East
    ( ) North
    ( ) West
    ( ) Left bumper
    ( ) Right bumper
    ( ) Select
    ( ) Start
    ( ) Mode
    ( ) Left thumb
    ( ) Right thumb
    ( ) Dpad up
    ( ) Dpad down
    ( ) Dpad left
    ( ) Dpad right
    ( ) Touchpad
    ( ) Paddle 1
    ( ) Paddle 2
    ( ) Paddle 3
    ( ) Paddle 4
    ( ) Left stick touch
    ( ) Right stick touch
    ( ) Left grip touch
    ( ) Right grip touch
  Visibility: Hidden >
    (o) Hidden
    ( ) Grabbed
    ( ) Visible
  Managed: Off >
    ( ) On
    (o) Off
";
        assert_eq!(of(&menu, "default ✓"), expected, "\n{menu}");
        assert!(menu.starts_with("default ✓ (Not set up) >\n"), "{menu}");
        assert!(!menu.contains("Quick start"), "{menu}");
    }

    /// A field's line says the whole choice; in its menu, choices that
    /// share a caption sit under it, each by what is left to say.
    #[test]
    fn choices_that_share_a_caption_are_listed_under_it() {
        let mut view = view(None, Hiding::Service);
        view.profiles.push(Profile {
            toml: "mode = \"priority\"\nassist-gyro = \"right-bumper\"\n".to_string(),
            unset: vec!["primary".to_string(), "assist".to_string()],
            ..profile("default", None)
        });
        let menu = outline(&menu(&view));
        assert!(
            menu.contains(
                "  Gyro: Assist · Right bumper >\n    ( ) Primary only\n    - Assist holds\n    ( ) South\n"
            ),
            "{menu}"
        );
        assert!(
            menu.contains("    ( ) Left bumper\n    (o) Right bumper\n"),
            "{menu}"
        );
    }

    /// A profile that runs has Stop, who has control, and its fields; a
    /// pad that left its setup is said so, and can't be handed to.
    #[test]
    fn a_running_profile_s_menu_has_stop_control_and_its_fields() {
        let menu = outline(&menu(&with_default(Some(Phase::Held))));
        let couch = of(&menu, "couch (Held) >");
        assert!(
            couch.starts_with(
                "couch (Held) >\n  Stop\n  ---\n  Has control >\n    (o) primary\n    ( ) - assist\n  ---\n  Primary: DualSense >\n    (o) DualSense\n    ( ) 8BitDo Pro 2\n    ( ) - Xbox pad (disconnected)\n  Assist: Xbox pad (disconnected) >\n    ( ) DualSense (swap with Primary)\n    ( ) 8BitDo Pro 2\n    (o) - Xbox pad (disconnected)\n"
            ),
            "{couch}"
        );
        assert!(couch.contains("  Mode: Toggle >\n"), "{couch}");
        // One of another preset has that preset's fields.
        let desk = of(&menu, "desk >");
        assert!(
            desk.starts_with(
                "desk >\n  Start\n  ---\n  Pad: 8BitDo Pro 2 >\n    ( ) - DualSense (in couch)\n    (o) 8BitDo Pro 2\n"
            ),
            "{desk}"
        );
        assert!(
            desk.contains("  Outputs: 2 >\n    ( ) 1\n    (o) 2\n"),
            "{desk}"
        );
        assert!(!desk.contains("Rumble"), "{desk}");
        // Then the pads, with the virtual ones after, and the status.
        assert!(
            menu.contains(
                "---\nPads (2/3 connected) >\n  DualSense (in couch) >\n    - address:dualsense\n    Identify (buzz)\n"
            ),
            "{menu}"
        );
        assert!(
            menu.contains(
                "  ---\n  Generic (in couch) >\n    - /dev/input/event31\n    Identify (buzz)\n"
            ),
            "{menu}"
        );
        assert!(
            menu.ends_with("---\n- Hiding: service · Real-time: yes\n---\nSettings\nQuit tray\n"),
            "{menu}"
        );
    }

    /// A row's icon is a name in the desktop's theme, as plain data: a
    /// profile by how it is, a pad, an action by what it does. A virtual
    /// pad is buzzed by its own name, and a pick names its profile.
    #[test]
    fn rows_carry_their_icons_and_their_commands() {
        let entries = menu(&with_default(Some(Phase::Held)));
        let top: Vec<(&str, Icon)> = entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Sub { icon, label, .. } | Entry::Item { icon, label, .. } => {
                    Some((label.as_str(), *icon))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            top,
            [
                ("default ✓ (Not set up)", icon::STOP),
                ("couch (Held)", icon::HELD),
                ("desk", icon::STOP),
                ("Pads (2/3 connected)", icon::PAD),
                ("Hiding: service · Real-time: yes", icon::STATUS),
                ("Settings", icon::SETTINGS),
                ("Quit tray", icon::QUIT),
            ]
        );
        let subs = |entries: &'_ [Entry], label: &str| -> Vec<Entry> {
            entries
                .iter()
                .find_map(|entry| match entry {
                    Entry::Sub {
                        label: found,
                        entries,
                        ..
                    } if found.starts_with(label) => Some(entries.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| unreachable!("no {label}"))
        };
        let made = subs(&subs(&entries, "Pads"), "Generic");
        assert_eq!(
            made[1],
            Entry::Item {
                icon: icon::RUMBLE,
                label: "Identify (buzz)".to_string(),
                command: Some(Command::Identify("virtual:couch/virtual".to_string())),
            }
        );
        let Some(Entry::Radio { choices, .. }) = subs(&subs(&entries, "default"), "Primary")
            .into_iter()
            .next()
        else {
            unreachable!("a pad field is a radio group");
        };
        assert_eq!(
            choices[1].command,
            Command::set("default", "primary", "address:8bitdo")
        );
    }

    /// A pad's long name is cut short on its profile's menu, and in
    /// full where pads are picked from and under Pads.
    #[test]
    fn a_long_pad_name_is_cut_on_a_profile_s_menu_and_whole_where_pads_are_listed() {
        const LONG: &str = "Sony Interactive Entertainment DualSense Wireless Controller";
        let mut view = with_default(Some(Phase::Running));
        view.pads[0].name = LONG.to_string();
        let menu = outline(&menu(&view));
        assert!(
            menu.contains(&format!(
                "  Primary: Sony Interactive Entertainm… >\n    (o) {LONG}\n"
            )),
            "{menu}"
        );
        assert!(
            menu.contains(&format!("\n  {LONG} (in couch) >\n")),
            "{menu}"
        );
        // So for a profile no form can show, which says its parts.
        view.profiles[0].toml = "[source.a]\npad = \"model:0001:0001\"\n".to_string();
        let menu = outline(&super::menu(&view));
        assert!(
            menu.contains("  - Primary: Sony Interactive Entertainm…\n"),
            "{menu}"
        );
    }

    #[test]
    fn a_setup_that_failed_and_a_profile_that_is_wrong_say_why() {
        let mut failed = view(Some(Phase::Failed), Hiding::Service);
        failed
            .profiles
            .push(profile("broken", Some("no such preset")));
        let menu = outline(&menu(&failed));
        assert!(
            menu.contains("couch (Failed) >\n  Stop\n  ---\n  - a virtual pad refused a write\n"),
            "{menu}"
        );
        assert!(
            menu.contains("broken (Wrong) >\n  - Can't run: mode: no such preset\n"),
            "{menu}"
        );
    }

    /// A profile written by hand has no fields to pick: while it runs
    /// its menu says what it is made of. So does a setup whose profile
    /// is gone.
    #[test]
    fn a_profile_no_form_can_show_says_what_it_is_made_of() {
        let mut odd = view(Some(Phase::Running), Hiding::Service);
        odd.profiles.clear();
        odd.setups[0].nodes[0].name = "share".to_string();
        odd.setups[0].sources[0].pad = None;
        odd.setups[0].sinks[0].node = None;
        let menu = outline(&menu(&odd));
        assert!(menu.starts_with("couch (Running) >\n"), "{menu}");
        // Its one node has control, whatever it is called, as the window
        // says.
        assert!(menu.contains("  Has control >\n"), "{menu}");
        assert!(
            menu.contains("- Assist: waiting for a pad that fits"),
            "{menu}"
        );
        assert!(menu.contains("- Primary: DualSense\n"), "{menu}");
        assert!(menu.contains("- Virtual: Generic (not made)"), "{menu}");
        assert!(!menu.contains("Mode:"), "{menu}");
        // Of several, each goes by its name.
        let mut second = odd.setups[0].nodes[0].clone();
        second.name = "route".to_string();
        odd.setups[0].nodes.push(second);
        let menu = outline(&super::menu(&odd));
        assert!(menu.contains("  share >\n"), "{menu}");
        assert!(menu.contains("  route >\n"), "{menu}");
        assert!(!menu.contains("Has control"), "{menu}");
        // Not running, it has Start and nothing else.
        let mut kept = view(None, Hiding::Service);
        kept.profiles[1].toml = "[source.a]\npad = \"model:0001:0001\"\n".to_string();
        let menu = outline(&super::menu(&kept));
        assert!(menu.contains("desk >\n  Start\n---\n"), "{menu}");
    }

    /// Before the daemon has said what a preset's fields are, a
    /// profile's menu has what to do with it and no more.
    #[test]
    fn a_profile_s_fields_wait_for_the_daemon_to_say_what_they_are() {
        let mut unsaid = with_default(None);
        unsaid.fields.clear();
        let menu = outline(&menu(&unsaid));
        assert!(
            menu.starts_with("default ✓ (Not set up) >\n  - Choose its pads to start\ncouch >\n  Start\ndesk >\n  Start\n---\nPads"),
            "{menu}"
        );
    }

    /// A pad that can't be buzzed says why in place of Identify: one
    /// with no rumble, and a Steam Controller whose kernel driver has
    /// none, which can be buzzed once a profile uses it.
    #[test]
    fn a_pad_that_can_t_be_buzzed_says_why() {
        let mut view = view(None, Hiding::Service);
        let menu = outline(&menu(&view));
        assert!(
            menu.contains("  8BitDo Pro 2 >\n    - address:8bitdo\n    - No rumble to buzz with\n"),
            "{menu}"
        );
        view.pads[1].name = "Steam Controller".to_string();
        (view.pads[1].vendor, view.pads[1].product) = (0x28de, 0x1302);
        let menu = outline(&super::menu(&view));
        assert!(
            menu.contains(
                "  Steam Controller >\n    - address:8bitdo\n    - Buzzed only while a profile uses it\n"
            ),
            "{menu}"
        );
    }

    #[test]
    fn a_pad_remembered_but_away_can_be_forgotten_and_none_known_says_so() {
        let mut gone = view(None, Hiding::Grab);
        gone.pads[2].connected = false;
        let menu = outline(&menu(&gone));
        assert!(
            menu.contains("  Xbox pad (remembered) >\n    - serial:xbox\n    Forget\n"),
            "{menu}"
        );
        let empty = View {
            pads: Vec::new(),
            ..gone
        };
        let menu = outline(&super::menu(&empty));
        assert!(menu.contains("- No pads are known"), "{menu}");
        // A pad a profile names that isn't known goes by what it wrote.
        assert!(menu.contains("Primary: address:dualsense >"), "{menu}");
    }

    /// The status line says how pads are hidden, and while the daemon
    /// has just started and hasn't heard from the hiding service yet,
    /// that it is looking for it.
    #[test]
    fn the_status_line_says_how_pads_are_hidden_or_that_the_daemon_is_looking() {
        let line = |hiding| {
            let menu = outline(&menu(&view(None, hiding)));
            menu.lines()
                .find(|line| line.contains("Hiding:"))
                .map(str::to_string)
                .unwrap_or_else(|| unreachable!("no hiding line in\n{menu}"))
        };
        assert_eq!(line(Hiding::Service), "- Hiding: service · Real-time: yes");
        assert_eq!(line(Hiding::Grab), "- Hiding: grab · Real-time: yes");
        assert_eq!(
            line(Hiding::Looking),
            "- Hiding: looking for the service… · Real-time: yes"
        );
    }

    #[test]
    fn without_the_daemon_there_is_only_what_is_wrong_and_the_way_out() {
        let mut down = view(None, Hiding::Service);
        down.alive = false;
        assert_eq!(
            outline(&menu(&down)),
            "- The CtrlAssist daemon isn't running\n---\nSettings\nQuit tray\n"
        );
        let mut arriving = view(None, Hiding::Service);
        arriving.hydrated = false;
        assert!(outline(&menu(&arriving)).starts_with("- Reading from the daemon…\n"));
        let mut other = view(None, Hiding::Service);
        other.mismatch = Some("9.0.0".to_string());
        let menu = outline(&menu(&other));
        assert!(
            menu.starts_with("- Version mismatch: daemon 9.0.0, tray "),
            "{menu}"
        );
    }

    /// A part named by model shows the pad it has while its setup runs,
    /// and otherwise the pad a start would take: connected and free, not
    /// merely the first of the model the daemon knows.
    #[test]
    fn a_part_named_by_model_shows_the_pad_it_has_or_would_have() {
        // Every pad of the fixture is of one model.
        let mut away = view(None, Hiding::Service);
        away.pads[0].connected = false;
        away.profiles[1].toml = "mode = \"unicast\"\npad = \"model:054c:0ce6\"\n".to_string();
        let menu = outline(&menu(&away));
        assert!(
            of(&menu, "desk >").contains(
                "  Pad: 8BitDo Pro 2 >\n    ( ) - DualSense (disconnected)\n    (o) 8BitDo Pro 2\n"
            ),
            "{menu}"
        );
        // Running, each part shows the pad the setup took for it.
        let mut running = view(Some(Phase::Running), Hiding::Service);
        running.profiles[0].toml =
            "mode = \"toggle\"\nprimary = \"model:054c:0ce6\"\nassist = \"model:054c:0ce6\"\n"
                .to_string();
        let menu = outline(&super::menu(&running));
        let couch = of(&menu, "couch (Running) >");
        assert!(couch.contains("  Primary: DualSense >\n"), "{couch}");
        assert!(couch.contains("  Assist: Xbox pad >\n"), "{couch}");
        // Stopped, two parts of one model are two pads of it.
        let mut stopped = view(None, Hiding::Service);
        stopped.profiles[0]
            .toml
            .clone_from(&running.profiles[0].toml);
        let menu = outline(&super::menu(&stopped));
        let couch = of(&menu, "couch >");
        assert!(couch.contains("  Primary: 8BitDo Pro 2 >\n"), "{couch}");
        assert!(couch.contains("  Assist: DualSense >\n"), "{couch}");
    }

    #[test]
    fn an_identity_the_daemon_cannot_make_cannot_be_picked() {
        let mut view = view(None, Hiding::Service);
        view.identities = vec![ctrlassist::Identity {
            id: "xbox".to_string(),
            name: "Xbox".to_string(),
            vendor: 0x045e,
            product: 0x0b12,
            available: false,
        }];
        let menu = outline(&menu(&view));
        assert!(menu.contains("    (o) Generic\n    ( ) - Xbox\n"), "{menu}");
    }
}
