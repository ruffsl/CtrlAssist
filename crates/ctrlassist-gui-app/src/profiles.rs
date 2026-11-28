//! The Profiles pane: its rows from the daemon's mirror and the fields
//! the daemon serves for a preset, the form's edits, and what the pane
//! asks of the daemon.
//!
//! Each change applies at once: a field set is a profile put, and a
//! setup running that profile is updated. A change that doesn't make a
//! profile the daemon would keep (a new profile with no pads picked
//! yet, a preset swapped for one of other pads) can't be put, so the
//! window holds it, shows the form as changed, and says what is
//! missing; the next change that completes it is put. What the window
//! holds is dropped once the kept profile says the same, or on Discard.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use ctrlassist::form::{self, Field, Kind, Values};
use ctrlassist::{DaemonClient, Pad, Profile, Setup, SetupSource};
use slint::{ComponentHandle, Model as _, ModelRc, SharedString, VecModel};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinSet;
use tracing::warn;

use crate::lanes::Lanes;
use crate::nav::NavHandles;
use crate::status::Status;
use crate::{
    AppWindow, ErrorState, Errors, ProfileFieldRow, ProfileOutputRow, ProfileRow, ProfileUseRow,
    Profiles, on_ui,
};

use ctrlassist_protocol::profile::{DEFAULT, PadRef};

/// The key of the field that says what the virtual pads present
/// themselves as.
const IDENTITY: &str = "identity";

/// The key of the field that says the daemon starts the profile itself.
const MANAGED: &str = "managed";

/// What the daemon last said, for making the rows again when only what
/// the window holds has changed.
#[derive(Clone, Debug, Default)]
pub struct Known {
    /// The kept profiles.
    pub profiles: Vec<Profile>,
    /// The setups that exist.
    pub setups: Vec<Setup>,
    /// The known pads.
    pub pads: Vec<Pad>,
    /// A preset's fields, as served.
    pub fields: Vec<Field>,
}

thread_local! {
    /// The changes the window holds that aren't kept, by profile name.
    static HELD: RefCell<BTreeMap<String, Values>> = const { RefCell::new(BTreeMap::new()) };
    /// What the daemon last said.
    static KNOWN: RefCell<Known> = RefCell::new(Known::default());
}

/// What is wrong with a form's values as a profile, if anything: the
/// daemon's own check, run here.
fn unfinished_why(fields: &[Field], values: &Values) -> Option<String> {
    ctrlassist_protocol::profile::Profile::check(&form::to_toml(fields, values))
        .err()
        .map(|problem| problem.to_string())
}

/// A pad as a role's dropdown lists it.
fn pad_label(pad: &Pad) -> String {
    let tag = crate::pads::tag_of(&pad.id);
    let mut label = pad.name.clone();
    if !tag.is_empty() {
        label.push_str(&format!(" · {tag}"));
    }
    if !pad.connected {
        label.push_str(" (not connected)");
    }
    label
}

/// Whether a profile's name for a pad means this pad: its id, or its
/// bare address.
fn names_pad(value: &str, pad: &Pad) -> bool {
    value == pad.id || pad.id.strip_prefix("address:") == Some(value)
}

fn strings(items: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    ))
}

/// One field of the form, holding `value`.
fn field_row(
    field: &Field,
    value: &str,
    pads: &[Pad],
    profile: &str,
    meant: Option<&Pad>,
) -> ProfileFieldRow {
    let mut row = ProfileFieldRow {
        group: SharedString::from(field.group.as_str()),
        key: SharedString::from(field.key.as_str()),
        label: SharedString::from(field.label.as_str()),
        value: SharedString::from(value),
        default_value: SharedString::from(field.default.as_str()),
        at_default: value == field.default,
        ..ProfileFieldRow::default()
    };
    match &field.kind {
        Kind::Choice { choices } => {
            row.kind = SharedString::from("choice");
            row.options = strings(choices.iter().map(|choice| choice.label.clone()));
            row.option_values = strings(choices.iter().map(|choice| choice.value.clone()));
            if let Some(chosen) = choices.iter().find(|choice| choice.value == value) {
                row.value_label = SharedString::from(chosen.label.as_str());
            }
        }
        Kind::Pad => {
            row.kind = SharedString::from("pad");
            // Connected pads first, as the Pads pane lists them.
            let mut sorted: Vec<&Pad> = pads.iter().collect();
            sorted.sort_by(|a, b| {
                b.connected
                    .cmp(&a.connected)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                    .then_with(|| a.id.cmp(&b.id))
            });
            row.options = strings(sorted.iter().map(|pad| pad_label(pad)));
            row.option_values = strings(sorted.iter().map(|pad| pad.id.clone()));
            let chosen = sorted.iter().find(|pad| names_pad(value, pad)).copied();
            match chosen {
                Some(pad) => {
                    row.value_label = SharedString::from(pad_label(pad));
                    row.pad = SharedString::from(pad.id.as_str());
                }
                // Any pad of a model, or one the daemon doesn't know,
                // as one forgotten: said as what it is, and still a
                // dropdown, so that another can be picked in its place.
                None if !value.is_empty() => {
                    let label = match PadRef::parse(value) {
                        Some(PadRef::Model(vendor, product)) => {
                            format!("Any pad of model {vendor:04x}:{product:04x}")
                        }
                        _ => format!("{} (not known)", crate::pads::tag_of(value)),
                    };
                    row.value_label = SharedString::from(label);
                }
                None => {}
            }
            // Nothing picked has nothing to say of. Any pad of a model
            // is spoken for by the pad of that model its setup has or a
            // start would take, `meant`, as the profile's word judges it.
            if !value.is_empty() {
                let (word, tone) = connection(chosen.or(meant), profile);
                row.status_word = SharedString::from(word);
                row.status_tone = SharedString::from(tone);
            }
        }
        Kind::Number { min, max } => {
            row.kind = SharedString::from("number");
            row.min = i32::try_from(*min).unwrap_or(i32::MAX);
            row.max = i32::try_from(*max).unwrap_or(i32::MAX);
            row.number = value.parse().unwrap_or(row.min);
        }
        Kind::Switch => {
            row.kind = SharedString::from("switch");
            row.checked = value == "true";
        }
        // A kind a later daemon serves: shown as it is written.
        _ => row.read_only = true,
    }
    row
}

/// A pad's connection as a profile's row says it: connected,
/// disconnected, or in another profile that is running, which is why a
/// start would be refused. `pad` is none for one the daemon doesn't
/// know.
fn connection(pad: Option<&Pad>, profile: &str) -> (String, &'static str) {
    match pad {
        None => ("Not known".to_string(), "neutral"),
        Some(pad) if !pad.connected => ("Disconnected".to_string(), "neutral"),
        Some(pad) => match pad.setup.as_deref() {
            Some(other) if other != profile => (format!("In {other}"), "warn"),
            _ => ("Connected".to_string(), "success"),
        },
    }
}

/// The fields of one card, in the order they are served.
fn in_card(fields: &[ProfileFieldRow], belongs: impl Fn(&str) -> bool) -> ModelRc<ProfileFieldRow> {
    ModelRc::new(VecModel::from(
        fields
            .iter()
            .filter(|field| belongs(field.group.as_str()))
            .cloned()
            .collect::<Vec<_>>(),
    ))
}

/// A line of how a running profile is, beside its Stop.
fn setup_note(setup: &Setup) -> String {
    match setup.phase {
        // A pad refused as it came back says why, which is what to do
        // about it; otherwise one is only missing.
        ctrlassist::Phase::Held => match &setup.reason {
            Some(reason) => {
                format!("Held: {reason}. Its virtual pad stays, and the rest plays on.")
            }
            None => {
                "Held: a pad it needs is missing. Its virtual pad stays, and the rest plays on."
                    .to_string()
            }
        },
        ctrlassist::Phase::Failed => format!(
            "Failed: {}. Stop it to clear it.",
            setup.reason.as_deref().unwrap_or("it stopped working")
        ),
        ctrlassist::Phase::Running => {
            let sinks: Vec<String> = setup
                .sinks
                .iter()
                .map(|sink| match &sink.node {
                    Some(node) => {
                        let node = node.rsplit('/').next().unwrap_or(node);
                        format!("{} pad at {node}", crate::status::identity(&sink.identity))
                    }
                    None => format!("{} pad", crate::status::identity(&sink.identity)),
                })
                .collect();
            // "the", which suits every identity's name: "a Xbox pad" doesn't.
            format!("Games see the {}.", sinks.join(" and the "))
        }
        _ => "Starting.".to_string(),
    }
}

/// A pad a profile names, for the header's link: the pad its setup has
/// for that part while it runs, or else the pad the name means, `meant`.
fn use_row(
    source: &ctrlassist_protocol::profile::PlannedSource,
    setup: Option<&Setup>,
    pads: &[Pad],
    profile: &str,
    meant: Option<&Pad>,
) -> ProfileUseRow {
    let taken = setup
        .and_then(|setup| setup.sources.iter().find(|taken| taken.name == source.name))
        .and_then(|taken| taken.pad.as_deref())
        .and_then(|id| pads.iter().find(|pad| pad.id == id));
    let pad = taken.or(meant);
    let (word, tone) = connection(pad, profile);
    ProfileUseRow {
        word: SharedString::from(word),
        tone: SharedString::from(tone),
        part: SharedString::from(crate::status::capitalized(&source.name)),
        pad: SharedString::from(pad.map_or("", |pad| pad.id.as_str())),
        name: SharedString::from(match (pad, &source.pad) {
            (Some(pad), _) => pad.name.clone(),
            (None, PadRef::Exact(id)) => id.clone(),
            (None, PadRef::Model(vendor, product)) => {
                format!("model:{vendor:04x}:{product:04x}")
            }
        }),
    }
}

/// A pad a setup has taken, for the header's link, when there is no
/// profile left to say which pad it names.
fn taken_row(source: &SetupSource, pads: &[Pad], profile: &str) -> ProfileUseRow {
    let pad = source
        .pad
        .as_deref()
        .and_then(|id| pads.iter().find(|pad| pad.id == id));
    let (word, tone) = connection(pad, profile);
    ProfileUseRow {
        word: SharedString::from(word),
        tone: SharedString::from(tone),
        part: SharedString::from(crate::status::capitalized(&source.name)),
        pad: SharedString::from(pad.map_or("", |pad| pad.id.as_str())),
        name: SharedString::from(match (pad, source.pad.as_deref()) {
            (Some(pad), _) => pad.name.as_str(),
            (None, Some(id)) => id,
            (None, None) => "—",
        }),
    }
}

/// A virtual pad of the profile `profile`, by its name there and what
/// it presents itself as, and where it is while `setup` runs.
fn output_row(
    profile: &str,
    sink: &str,
    identity: &str,
    setup: Option<&Setup>,
) -> ProfileOutputRow {
    // It exists once its setup has made it.
    let node = setup
        .and_then(|setup| setup.sinks.iter().find(|made| made.name == sink))
        .and_then(|made| made.node.as_deref());
    let made = node.is_some();
    ProfileOutputRow {
        pad: SharedString::from(if made {
            crate::pads::virtual_id(profile, sink)
        } else {
            String::new()
        }),
        word: SharedString::from(if made { "Connected" } else { "Disconnected" }),
        tone: SharedString::from(if made { "success" } else { "neutral" }),
        name: SharedString::from(crate::status::capitalized(sink)),
        // A row's own word, so with a capital.
        identity: SharedString::from(crate::status::capitalized(crate::status::identity(
            identity,
        ))),
        node: SharedString::from(
            node.map(|node| node.rsplit('/').next().unwrap_or(node))
                .unwrap_or_default(),
        ),
    }
}

/// The pane's rows, those with a setup first and each group in name
/// order: every kept profile, every one the window holds that isn't
/// kept yet, and every setup whose profile is gone, so that it can
/// still be stopped.
#[must_use]
pub fn rows(known: &Known, held: &BTreeMap<String, Values>) -> Vec<ProfileRow> {
    let mut names: Vec<&str> = known
        .profiles
        .iter()
        .map(|profile| profile.name.as_str())
        .chain(held.keys().map(String::as_str))
        .chain(known.setups.iter().map(|setup| setup.name.as_str()))
        .collect();
    let runs = |name: &str| known.setups.iter().any(|setup| setup.name == name);
    // Those with a setup first, as the list shows them; among each, the
    // default profile and then the rest by name.
    names.sort_unstable_by(|a, b| {
        runs(b)
            .cmp(&runs(a))
            .then_with(|| (*a != DEFAULT).cmp(&(*b != DEFAULT)))
            .then_with(|| a.cmp(b))
    });
    names.dedup();
    // A setup's place among the Setups rows, which go by name.
    let mut running: Vec<&str> = known
        .setups
        .iter()
        .map(|setup| setup.name.as_str())
        .collect();
    running.sort_unstable();
    names
        .into_iter()
        .map(|name| {
            let kept = known.profiles.iter().find(|profile| profile.name == name);
            let held = held.get(name);
            let values = held
                .cloned()
                .or_else(|| kept.and_then(|kept| form::from_toml(&known.fields, &kept.toml)));
            let why = held.and_then(|held| unfinished_why(&known.fields, held));
            let setup = known.setups.iter().find(|setup| setup.name == name);
            // A setup whose profile was deleted, or whose file a reload
            // found gone: it runs as its profile was when it started.
            let gone = kept.is_none() && held.is_none() && setup.is_some();
            let preset = values
                .as_ref()
                .and_then(|values| values.get(form::PRESET))
                .cloned()
                .unwrap_or_default();
            // The pad each pad field means, the parts bound together as a
            // start would bind them.
            let meant_fields = values
                .as_ref()
                .map(|values| pads_meant(known, name, values))
                .unwrap_or_default();
            let fields: Vec<ProfileFieldRow> = values
                .as_ref()
                .map(|values| {
                    known
                        .fields
                        .iter()
                        // Whether it is managed is the STATUS group's
                        // to show, not a row of the arrangement.
                        .filter(|field| field.applies_to(&preset) && field.key != MANAGED)
                        .map(|field| {
                            let value = values.get(&field.key).map_or("", String::as_str);
                            let meant = meant_fields.get(&field.key).copied();
                            field_row(field, value, &known.pads, name, meant)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let plan =
                kept.and_then(|kept| ctrlassist_protocol::profile::Profile::check(&kept.toml).ok());
            // Its pads still to choose: as picked here, or as kept.
            let unset = match &values {
                Some(values) => !ctrlassist_protocol::profile::Profile::unchosen(&form::to_toml(
                    &known.fields,
                    values,
                ))
                .is_empty(),
                None => kept.is_some_and(|kept| !kept.unset.is_empty()),
            };
            let status = if why.is_some() || (kept.is_none() && !gone) {
                Status::Unfinished
            } else {
                crate::status::of(name, plan.as_ref(), unset, &known.setups, &known.pads)
            };
            // How many pads it takes and makes: as the profile says, or
            // as its setup has them when the profile is gone.
            let counts = plan
                .as_ref()
                .map(|plan| (plan.sources.len(), plan.sinks.len()))
                .or_else(|| {
                    setup
                        .filter(|_| gone)
                        .map(|setup| (setup.sources.len(), setup.sinks.len()))
                });
            let shape = match counts {
                Some((sources, _)) if sources > 1 => "merge",
                Some((_, sinks)) if sinks > 1 => "split",
                _ => match preset.as_str() {
                    "priority" | "average" | "toggle" => "merge",
                    "unicast" | "multicast" => "split",
                    _ => "",
                },
            };
            let note = match (setup, status) {
                (Some(setup), _) => setup_note(setup),
                (None, Status::Ready) => "Its pads are connected and free.".to_string(),
                (None, Status::Unfinished) => "It isn't a profile yet.".to_string(),
                (None, Status::NotSetUp) => "Choose its pads to start it.".to_string(),
                (None, Status::Wrong) => "The daemon can't run it as it is written.".to_string(),
                (None, _) => {
                    "A pad it names is missing, or in a profile that is running.".to_string()
                }
            };
            // The pads it names and its virtual pads, each with where
            // it is while it runs; with no profile left, its setup's.
            let (uses, outputs): (Vec<ProfileUseRow>, Vec<ProfileOutputRow>) =
                match (plan.as_ref(), setup) {
                    (Some(plan), _) => {
                        let parts: Vec<(&str, &PadRef)> = plan
                            .sources
                            .iter()
                            .map(|source| (source.name.as_str(), &source.pad))
                            .collect();
                        let meant = crate::status::meant(&known.pads, &parts, setup);
                        (
                            plan.sources
                                .iter()
                                .map(|source| {
                                    let pad = meant.get(&source.name).copied();
                                    use_row(source, setup, &known.pads, name, pad)
                                })
                                .collect(),
                            plan.sinks
                                .iter()
                                .map(|sink| {
                                    output_row(name, &sink.name, sink.identity.as_str(), setup)
                                })
                                .collect(),
                        )
                    }
                    (None, Some(running)) if gone => (
                        running
                            .sources
                            .iter()
                            .map(|source| taken_row(source, &known.pads, name))
                            .collect(),
                        running
                            .sinks
                            .iter()
                            .map(|sink| output_row(name, &sink.name, &sink.identity, setup))
                            .collect(),
                    ),
                    _ => (Vec::new(), Vec::new()),
                };
            let no_outputs = outputs.is_empty();
            // The mode as the served field labels it.
            let mode_label = match &values {
                None if gone => String::new(),
                None => "Custom".to_string(),
                Some(_) => known
                    .fields
                    .iter()
                    .find(|field| field.key == form::PRESET)
                    .and_then(|field| match &field.kind {
                        Kind::Choice { choices } => {
                            choices.iter().find(|choice| choice.value == preset)
                        }
                        _ => None,
                    })
                    .map(|choice| choice.label.clone())
                    .unwrap_or_default(),
            };
            ProfileRow {
                shape: SharedString::from(shape),
                mode_label: SharedString::from(mode_label),
                word: SharedString::from(status.word()),
                word_tone: SharedString::from(status.tone()),
                note: SharedString::from(note),
                // A change the window holds says so before it is kept.
                managed: values
                    .as_ref()
                    .and_then(|values| values.get(MANAGED))
                    .map_or_else(
                        || plan.as_ref().is_some_and(|plan| plan.managed),
                        |value| value == "true",
                    ),
                can_start: setup.is_none() && matches!(status, Status::Ready | Status::Waiting),
                setup_index: running
                    .iter()
                    .position(|running| *running == name)
                    .and_then(|at| i32::try_from(at).ok())
                    .unwrap_or(-1),
                uses: ModelRc::new(VecModel::from(uses)),
                outputs: ModelRc::new(VecModel::from(outputs)),
                name: SharedString::from(name),
                kind: SharedString::from(match (&values, gone) {
                    (Some(_), _) => "preset",
                    (None, true) => "gone",
                    (None, false) => "full",
                }),
                unfinished: why.is_some(),
                unfinished_why: SharedString::from(why.unwrap_or_default()),
                kept: kept.is_some(),
                problem: SharedString::from(
                    kept.and_then(|kept| kept.problem.as_ref())
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                ),
                is_default: name == DEFAULT,
                input_fields: in_card(&fields, |group| group == form::INPUTS),
                // The first virtual pad's row carries the identity,
                // once the profile's virtual pads can be told.
                output_fields: ModelRc::new(VecModel::from(
                    fields
                        .iter()
                        .filter(|field| {
                            field.group == form::OUTPUTS && (no_outputs || field.key != IDENTITY)
                        })
                        .cloned()
                        .collect::<Vec<_>>(),
                )),
                identity_field: fields
                    .iter()
                    .find(|field| field.key == IDENTITY && !no_outputs)
                    .cloned()
                    .unwrap_or_default(),
                preset_fields: in_card(&fields, |group| {
                    group != form::INPUTS && group != form::OUTPUTS
                }),
            }
        })
        .collect()
}

/// Makes the rows from what is known and what is held, and puts them
/// on the window, the selection staying on its profile.
fn render(app: &AppWindow) {
    let mut rows = KNOWN.with_borrow(|known| HELD.with_borrow(|held| rows(known, held)));
    // A profile with something on its way says so, in place of how it
    // is, until that has landed.
    PENDING.with_borrow(|pending| {
        for row in &mut rows {
            if let Some(progress) = pending.get(row.name.as_str()) {
                let status = if progress.applying {
                    Status::Applying
                } else {
                    Status::Saving
                };
                row.word = SharedString::from(status.word());
                row.word_tone = SharedString::from(status.tone());
            }
        }
    });
    let profiles = app.global::<Profiles>();
    let was = selected(app);
    // In place, down to each form's fields: see `models`.
    let current = profiles.get_rows();
    for row in &mut rows {
        if let Some(old) = current.iter().find(|old| old.name == row.name) {
            let same = |a: &ProfileFieldRow, b: &ProfileFieldRow| {
                a.key == b.key && a.kind == b.kind && a.read_only == b.read_only
            };
            row.input_fields =
                crate::models::sync(&old.input_fields, row.input_fields.iter().collect(), same);
            row.output_fields =
                crate::models::sync(&old.output_fields, row.output_fields.iter().collect(), same);
            row.preset_fields =
                crate::models::sync(&old.preset_fields, row.preset_fields.iter().collect(), same);
            // The pads it names and its virtual pads: the first virtual
            // pad's row holds the Identity dropdown.
            row.uses = crate::models::sync(&old.uses, row.uses.iter().collect(), |a, b| {
                a.part == b.part
            });
            row.outputs =
                crate::models::sync(&old.outputs, row.outputs.iter().collect(), |a, b| {
                    a.name == b.name
                });
        }
    }
    let index = rows
        .iter()
        .position(|row| row.name == was)
        .filter(|_| !was.is_empty())
        .and_then(|at| i32::try_from(at).ok())
        .unwrap_or(-1);
    let running = rows.iter().filter(|row| row.setup_index >= 0).count();
    profiles.set_running_count(i32::try_from(running).unwrap_or(i32::MAX));
    profiles.set_rows(crate::models::sync(&current, rows, |a, b| a.name == b.name));
    profiles.set_selected_index(index);
}

/// Takes in what the daemon says now. A change the window held that the
/// kept profile now says too is held no longer.
pub fn show(app: &AppWindow, known: Known) {
    HELD.with_borrow_mut(|held| {
        held.retain(|name, values| {
            let kept = known
                .profiles
                .iter()
                .find(|profile| profile.name == *name)
                .and_then(|kept| form::from_toml(&known.fields, &kept.toml));
            kept.as_ref() != Some(values)
        });
    });
    KNOWN.set(known);
    render(app);
}

/// The name of the profile selected, empty for none.
pub(crate) fn selected(app: &AppWindow) -> SharedString {
    let profiles = app.global::<Profiles>();
    profiles
        .get_rows()
        .row_data(usize::try_from(profiles.get_selected_index()).unwrap_or(usize::MAX))
        .map(|row| row.name)
        .unwrap_or_default()
}

/// Selects the profile of that name, or nothing when there is none.
pub(crate) fn select(app: &AppWindow, name: &str) {
    let profiles = app.global::<Profiles>();
    let index = profiles
        .get_rows()
        .iter()
        .position(|row| row.name == name)
        .and_then(|at| i32::try_from(at).ok())
        .unwrap_or(-1);
    profiles.set_selected_index(index);
}

/// What is wrong with `name` for a new profile; empty for a good one.
#[must_use]
pub fn name_problem(name: &str) -> String {
    if name.is_empty() {
        return String::new();
    }
    if !ctrlassist_protocol::profile::is_name(name) {
        return "Up to 32 lowercase letters, digits and hyphens".to_string();
    }
    let taken = KNOWN.with_borrow(|known| known.profiles.iter().any(|kept| kept.name == name))
        || HELD.with_borrow(|held| held.contains_key(name));
    if taken {
        return "A profile has that name already".to_string();
    }
    String::new()
}

/// The values the form shows for a profile now: what is held, else what
/// is kept.
fn values_of(name: &str) -> Option<Values> {
    HELD.with_borrow(|held| held.get(name).cloned())
        .or_else(|| {
            KNOWN.with_borrow(|known| {
                let kept = known.profiles.iter().find(|kept| kept.name == name)?;
                form::from_toml(&known.fields, &kept.toml)
            })
        })
}

/// The pane's banner for something the daemon refused, of the `kind`
/// that raised it: a later outcome of that kind clears it, and no other.
fn refused(kind: &str, what: &str, error: &dyn std::fmt::Display) -> ErrorState {
    warn!(%error, "{what}");
    ErrorState {
        kind: SharedString::from(kind),
        tone: SharedString::from("error"),
        title: SharedString::from(what),
        message: SharedString::from(error.to_string()),
        action_label: SharedString::default(),
    }
}

/// A change to put: a profile's whole text as the form makes it, and
/// whether a setup running it takes it up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// The profile.
    pub name: String,
    /// The number the window gave it, counting up; the latest asked of
    /// a profile is the one whose outcome is shown.
    pub number: u64,
    /// The profile's whole text.
    pub toml: String,
    /// A profile still short of pads is kept as not set up, and a setup
    /// running it can't take that up, so it isn't asked to.
    pub update: bool,
}

/// What the window asks of the daemon about a profile. Each is taken up
/// on the lane of the profile it is asked of, after what was asked of
/// that profile before, and beside what is asked of the others: a change
/// queued can't land after the profile's delete and bring it back, nor
/// after a rename and leave two.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// Keep a change, and have a setup running the profile take it up.
    Put(Change),
    /// Start the profile.
    Start(String),
    /// Keep `toml`, the profile `from` as the window shows it, as the
    /// new profile `to`; with `rename`, delete `from` once that is kept,
    /// on `from`'s own lane after what waits there.
    Copy {
        /// The profile copied.
        from: String,
        /// The new profile.
        to: String,
        /// Its text.
        toml: String,
        /// Whether `from` goes once `to` is kept.
        rename: bool,
    },
    /// Delete the profile.
    Delete(String),
}

impl Op {
    /// The profile whose lane it waits on.
    #[must_use]
    pub fn lane(&self) -> &str {
        match self {
            Self::Put(change) => &change.name,
            Self::Copy { to, .. } => to,
            Self::Start(name) | Self::Delete(name) => name,
        }
    }

    /// Whether the change `self` is carried whole by `next`, asked of the
    /// same profile after it: both are changes, each the profile's whole
    /// text, so only the later need be put.
    fn carried_by(&self, next: &Self) -> bool {
        matches!((self, next), (Self::Put(_), Self::Put(_)))
    }
}

/// Why a request didn't come to all it asked.
#[derive(Debug)]
enum Failed {
    /// The daemon didn't do it: a change, a copy, a start or a delete.
    Put(ctrlassist::Error),
    /// A change was kept, and the setup running it didn't take it up.
    Update(ctrlassist::Error),
}

/// What is on its way for one profile.
#[derive(Clone, Copy, Debug, Default)]
struct Progress {
    /// Requests asked of it and not settled.
    ops: usize,
    /// Whether the one under way is a change kept, that the setup
    /// running it is taking up.
    applying: bool,
}

thread_local! {
    /// The number of the last change asked of each profile whose
    /// outcome is still to come.
    static ASKED: RefCell<BTreeMap<String, u64>> = const { RefCell::new(BTreeMap::new()) };
    /// How many changes have been asked.
    static COUNTED: Cell<u64> = const { Cell::new(0) };
    /// What is on its way for each profile, by name.
    static PENDING: RefCell<BTreeMap<String, Progress>> = const { RefCell::new(BTreeMap::new()) };
    /// The profiles asked to go, by a delete or a rename, that haven't
    /// gone yet: nothing more is asked of them.
    static GOING: RefCell<BTreeSet<String>> = const { RefCell::new(BTreeSet::new()) };
}

/// Where the window's requests about profiles go: one lane for each
/// profile, so that what is asked of one is done in the order asked, and
/// what is asked of another doesn't wait for it.
#[derive(Clone, Debug)]
pub struct Writer {
    ops: mpsc::UnboundedSender<Op>,
    /// How many requests were sent, those a request sent after it
    /// among them.
    sent: Arc<AtomicU64>,
    /// How many have been settled: done, or passed over for a later one
    /// that carries them.
    settled: watch::Receiver<u64>,
}

impl Writer {
    /// A writer whose requests are only handed to the receiver: for
    /// tests, which play the daemon's part.
    #[must_use]
    pub fn detached() -> (Self, mpsc::UnboundedReceiver<Op>) {
        let (ops, asked) = mpsc::unbounded_channel();
        let (_settle, settled) = watch::channel(0);
        let writer = Self {
            ops,
            sent: Arc::default(),
            settled,
        };
        (writer, asked)
    }

    /// A writer that does each request by `perform`, on the current
    /// runtime, and has the window take what came of it. `perform`
    /// answers what came of it, and a request it sends after it.
    fn spawn<P, F>(app: slint::Weak<AppWindow>, perform: P) -> Self
    where
        P: Fn(Op) -> F + Send + Sync + 'static,
        F: Future<Output = (Result<(), Failed>, Option<Op>)> + Send + 'static,
    {
        let (ops, asked) = mpsc::unbounded_channel();
        let (settle, settled) = watch::channel(0);
        let sent: Arc<AtomicU64> = Arc::default();
        tokio::spawn(write(asked, app, perform, Arc::clone(&sent), settle));
        Self { ops, sent, settled }
    }

    /// Sends a request; false when nothing writes them any more.
    fn send(&self, op: Op) -> bool {
        self.sent.fetch_add(1, Ordering::SeqCst);
        if self.ops.send(op).is_ok() {
            return true;
        }
        self.sent.fetch_sub(1, Ordering::SeqCst);
        false
    }

    /// Waits until every request sent so far has been settled, at most
    /// `timeout`; `false` when they hadn't been by then. The couch
    /// preferences' writer is flushed the same way.
    pub async fn flush(&self, timeout: Duration) -> bool {
        let sent = Arc::clone(&self.sent);
        let mut settled = self.settled.clone();
        let drained = async move {
            while *settled.borrow_and_update() < sent.load(Ordering::SeqCst) {
                if settled.changed().await.is_err() {
                    break;
                }
            }
        };
        tokio::time::timeout(timeout, drained).await.is_ok()
    }
}

/// Does the requests as they come, each on its profile's lane: in turn
/// there, and beside the others. Ends once nothing sends requests any
/// more and every one sent is done.
async fn write<P, F>(
    mut asked: mpsc::UnboundedReceiver<Op>,
    app: slint::Weak<AppWindow>,
    perform: P,
    sent: Arc<AtomicU64>,
    settle: watch::Sender<u64>,
) where
    P: Fn(Op) -> F + Sync,
    F: Future<Output = (Result<(), Failed>, Option<Op>)> + Send + 'static,
{
    let mut lanes: Lanes<Op> = Lanes::default();
    let mut running: JoinSet<Option<Op>> = JoinSet::new();
    let mut on: BTreeMap<tokio::task::Id, String> = BTreeMap::new();
    let mut settled = 0;
    let mut open = true;
    let start = |op: Op, running: &mut JoinSet<Option<Op>>, on: &mut BTreeMap<_, _>| {
        let lane = op.lane().to_string();
        let doing = perform(op.clone());
        let app = app.clone();
        let id = running
            .spawn(async move {
                let (outcome, then) = doing.await;
                on_ui(&app, move |app| finished(&app, &op, outcome));
                then
            })
            .id();
        on.insert(id, lane);
    };
    loop {
        tokio::select! {
            op = asked.recv(), if open => match op {
                Some(op) => {
                    let lane = op.lane().to_string();
                    if let Some(op) = lanes.take(&lane, op) {
                        start(op, &mut running, &mut on);
                    }
                }
                None => open = false,
            },
            Some(ended) = running.join_next_with_id() => {
                let (id, then) = match ended {
                    Ok(ended) => ended,
                    Err(error) => {
                        warn!(%error, "a request about a profile ended early");
                        (error.id(), None)
                    }
                };
                // Sent before this one is settled, so that a flush
                // never sees all settled with it still to come.
                if let Some(then) = then {
                    sent.fetch_add(1, Ordering::SeqCst);
                    let lane = then.lane().to_string();
                    if let Some(then) = lanes.take(&lane, then) {
                        start(then, &mut running, &mut on);
                    }
                }
                let mut settled_now = 1;
                if let Some(lane) = on.remove(&id) {
                    let (passed, next) = lanes.done(&lane, Op::carried_by);
                    for op in passed {
                        on_ui(&app, move |app| finished(&app, &op, Ok(())));
                        settled_now += 1;
                    }
                    if let Some(next) = next {
                        start(next, &mut running, &mut on);
                    }
                }
                settled += settled_now;
                let _ = settle.send(settled);
            }
            else => return,
        }
    }
}

/// Does one request of the daemon: what came of it, and a request to
/// send after it.
async fn perform(
    daemon: DaemonClient,
    app: slint::Weak<AppWindow>,
    op: Op,
) -> (Result<(), Failed>, Option<Op>) {
    match op {
        Op::Put(change) => (apply(&daemon, &app, &change).await, None),
        Op::Start(name) => (daemon.start_setup(&name).await.map_err(Failed::Put), None),
        Op::Delete(name) => (
            daemon.delete_profile(&name).await.map_err(Failed::Put),
            None,
        ),
        // The new one first, so a failure leaves the old.
        Op::Copy {
            from,
            to,
            toml,
            rename,
        } => match daemon.put_profile(&to, &toml).await {
            Ok(()) => (Ok(()), rename.then_some(Op::Delete(from))),
            Err(error) => (Err(Failed::Put(error)), None),
        },
    }
}

/// Puts a change, and has a setup running it take it up.
async fn apply(
    daemon: &DaemonClient,
    app: &slint::Weak<AppWindow>,
    change: &Change,
) -> Result<(), Failed> {
    daemon
        .put_profile(&change.name, &change.toml)
        .await
        .map_err(Failed::Put)?;
    // A setup running it takes it at once, as the tray's does.
    if change.update
        && daemon
            .setup(&change.name)
            .map_err(Failed::Update)?
            .is_some()
    {
        let name = change.name.clone();
        on_ui(app, move |app| applying(&app, &name));
        daemon
            .update_setup(&change.name, true)
            .await
            .map_err(Failed::Update)?;
    }
    Ok(())
}

/// The change to `name` under way was kept, and the setup running it is
/// taking it up.
fn applying(app: &AppWindow, name: &str) {
    PENDING.with_borrow_mut(|pending| {
        if let Some(progress) = pending.get_mut(name) {
            progress.applying = true;
        }
    });
    render(app);
}

/// One more request is on its way for `name`, or one fewer.
fn pending(name: &str, more: bool) {
    PENDING.with_borrow_mut(|pending| {
        if more {
            pending.entry(name.to_string()).or_default().ops += 1;
            return;
        }
        if let Some(progress) = pending.get_mut(name) {
            progress.ops = progress.ops.saturating_sub(1);
            progress.applying = false;
            if progress.ops == 0 {
                pending.remove(name);
            }
        }
    });
}

/// Sends `op`, with the profiles it is about shown as having it on its
/// way. A rename's delete, sent after it, is counted now.
fn request(app: &AppWindow, writer: &Writer, op: Op) {
    let mut about = vec![op.lane().to_string()];
    if let Op::Copy {
        from, rename: true, ..
    } = &op
    {
        about.push(from.clone());
    }
    for name in &about {
        pending(name, true);
    }
    if !writer.send(op) {
        warn!("not asked: nothing writes the window's requests any more");
        for name in &about {
            pending(name, false);
        }
    }
    render(app);
}

/// What came of a request, on the window's thread: it is no longer on
/// its way, and what it came to is said. A banner says a failure, and an
/// outcome of the same kind clears it, and no other kind: a start that
/// succeeds leaves another profile's warning standing.
fn finished(app: &AppWindow, op: &Op, outcome: Result<(), Failed>) {
    pending(op.lane(), false);
    let outcome = match op {
        Op::Put(change) => {
            done(app, &change.name, change.number, outcome);
            render(app);
            return;
        }
        Op::Start(name) => (
            format!("start:{name}"),
            "The profile couldn't be started".to_string(),
            outcome,
        ),
        Op::Delete(name) => {
            GOING.with_borrow_mut(|going| going.remove(name));
            (
                format!("delete:{name}"),
                format!("'{name}' couldn't be deleted"),
                outcome,
            )
        }
        Op::Copy {
            from, to, rename, ..
        } => {
            if outcome.is_err() {
                // Nothing was made: what the window held for the new one
                // is the old one's again, and the old one stays.
                HELD.with_borrow_mut(|held| {
                    let carried = held.remove(to);
                    if *rename && let Some(values) = carried {
                        held.entry(from.clone()).or_insert(values);
                    }
                });
                if *rename {
                    GOING.with_borrow_mut(|going| going.remove(from));
                    pending(from, false);
                }
            }
            let what = if *rename { "renamed" } else { "duplicated" };
            (
                format!("copy:{to}"),
                format!("The profile couldn't be {what}"),
                outcome,
            )
        }
    };
    let (kind, title, outcome) = outcome;
    let errors = app.global::<Errors>();
    match outcome {
        Ok(()) => {
            if errors.get_profiles().kind == kind {
                errors.set_profiles(ErrorState::default());
            }
        }
        Err(Failed::Put(error) | Failed::Update(error)) => {
            errors.set_profiles(refused(&kind, &title, &error));
        }
    }
    render(app);
}

/// What a change came to. Only the latest asked of a profile is said: an
/// earlier one is carried whole by the later, whose outcome is the one
/// that counts. A change the daemon didn't keep is dropped from what the
/// window holds; one that was kept stays held until the daemon says it,
/// as any is.
fn done(app: &AppWindow, name: &str, number: u64, outcome: Result<(), Failed>) {
    let latest = ASKED.with_borrow(|asked| asked.get(name) == Some(&number));
    if !latest {
        return;
    }
    ASKED.with_borrow_mut(|asked| asked.remove(name));
    let errors = app.global::<Errors>();
    // The banner a change to this profile raised, and nothing else.
    let kind = format!("change:{name}");
    let banner = match outcome {
        Ok(()) => {
            if errors.get_profiles().kind == kind {
                errors.set_profiles(ErrorState::default());
            }
            return;
        }
        Err(Failed::Put(error)) => {
            HELD.with_borrow_mut(|held| held.remove(name));
            render(app);
            refused(
                &kind,
                &format!("The change to '{name}' couldn't be kept"),
                &error,
            )
        }
        Err(Failed::Update(error)) => ErrorState {
            tone: SharedString::from("warn"),
            ..refused(
                &kind,
                &format!(
                    "The change to '{name}' was kept, but the running profile couldn't take it up"
                ),
                &error,
            )
        },
    };
    errors.set_profiles(banner);
}

/// A field was set. The window holds the change and shows it; if it
/// makes a profile, the profile is put and a setup running it updated.
/// The held change goes once the kept profile says the same, or is
/// dropped with a banner if the daemon refuses it. Nothing is asked of a
/// profile that is going.
fn set_field(app: &AppWindow, writer: &Writer, name: &str, key: &str, value: &str) {
    if GOING.with_borrow(|going| going.contains(name)) {
        return;
    }
    let Some(mut values) = values_of(name) else {
        return;
    };
    if values.get(key).map(String::as_str) == Some(value) {
        return;
    }
    KNOWN.with_borrow(|known| change_places(known, name, &mut values, key, value));
    values.insert(key.to_string(), value.to_string());
    let change = KNOWN.with_borrow(|known| {
        if key == form::PRESET {
            // Another preset: its own fields at their defaults, and
            // none of the last one's that it doesn't have.
            values.retain(|key, _| {
                known
                    .fields
                    .iter()
                    .any(|field| field.key == *key && field.applies_to(value))
            });
            for field in &known.fields {
                if field.applies_to(value) && !field.default.is_empty() {
                    values
                        .entry(field.key.clone())
                        .or_insert_with(|| field.default.clone());
                }
            }
        }
        change(&known.fields, name, &values)
    });
    HELD.with_borrow_mut(|held| held.insert(name.to_string(), values));
    render(app);
    let Some(mut change) = change else {
        return;
    };
    change.number = COUNTED.get() + 1;
    COUNTED.set(change.number);
    ASKED.with_borrow_mut(|asked| asked.insert(name.to_string(), change.number));
    request(app, writer, Op::Put(change));
}

/// The change to put for `values`, if they are something the daemon
/// keeps: a profile, or a preset that lacks only its pads, which it
/// keeps as not set up and no setup can take up. Its number is still
/// to give.
fn change(fields: &[Field], name: &str, values: &Values) -> Option<Change> {
    let toml = form::to_toml(fields, values);
    let unchosen = ctrlassist_protocol::profile::Profile::unchosen(&toml);
    if unfinished_why(fields, values).is_some() && unchosen.is_empty() {
        return None;
    }
    Some(Change {
        name: name.to_string(),
        number: 0,
        toml,
        update: unchosen.is_empty(),
    })
}

/// The text a copy of `name` is made of: the profile as the window shows
/// it, what it holds and hasn't kept yet among it, where that is
/// something the daemon keeps; the text as kept otherwise.
fn text_of(name: &str) -> Option<String> {
    values_of(name)
        .and_then(|values| KNOWN.with_borrow(|known| change(&known.fields, name, &values)))
        .map(|change| change.toml)
        .or_else(|| {
            KNOWN.with_borrow(|known| {
                known
                    .profiles
                    .iter()
                    .find(|kept| kept.name == name)
                    .map(|kept| kept.toml.clone())
            })
        })
}

/// Copies the profile `from` to the new profile `to`, as the window
/// shows it; with `rename`, `from` goes once the copy is kept. What the
/// window holds of `from` is held of `to`: moved, for a rename.
fn copy(app: &AppWindow, writer: &Writer, from: &str, to: &str, rename: bool) {
    if GOING.with_borrow(|going| going.contains(from)) {
        return;
    }
    let Some(toml) = text_of(from) else {
        return;
    };
    HELD.with_borrow_mut(|held| {
        let carried = if rename {
            held.remove(from)
        } else {
            held.get(from).cloned()
        };
        if let Some(values) = carried {
            held.insert(to.to_string(), values);
        }
    });
    if rename {
        GOING.with_borrow_mut(|going| going.insert(from.to_string()));
    }
    request(
        app,
        writer,
        Op::Copy {
            from: from.to_string(),
            to: to.to_string(),
            toml,
            rename,
        },
    );
}

/// The parts of the profile in `values` that name a pad, each with the
/// pad it names.
fn pad_parts(known: &Known, values: &Values) -> Vec<(String, PadRef)> {
    let preset = values
        .get(form::PRESET)
        .map(String::as_str)
        .unwrap_or_default();
    known
        .fields
        .iter()
        .filter(|field| field.kind == Kind::Pad && field.applies_to(preset))
        .filter_map(|field| {
            let value = values.get(&field.key)?;
            Some((
                field.key.clone(),
                crate::status::wanted(value, &known.pads)?,
            ))
        })
        .collect()
}

/// The pad each pad part of the profile `name` takes, by its key: while
/// its setup runs, the pad that setup has for one named by model; or,
/// with `fresh`, as a start would take them whether or not it runs, its
/// own pads counting as free. See [`crate::status::bind`].
fn pads_taken<'a>(
    known: &'a Known,
    name: &str,
    values: &Values,
    fresh: bool,
) -> BTreeMap<String, &'a Pad> {
    let parts = pad_parts(known, values);
    let parts: Vec<(&str, &PadRef)> = parts.iter().map(|(key, pad)| (key.as_str(), pad)).collect();
    if fresh {
        return crate::status::bind(&known.pads, &parts, Some(name));
    }
    let setup = known.setups.iter().find(|setup| setup.name == name);
    crate::status::taken(&known.pads, &parts, setup)
}

/// The pad each pad field of the profile `name` means, for showing.
fn pads_meant<'a>(known: &'a Known, name: &str, values: &Values) -> BTreeMap<String, &'a Pad> {
    let parts = pad_parts(known, values);
    let parts: Vec<(&str, &PadRef)> = parts.iter().map(|(key, pad)| (key.as_str(), pad)).collect();
    let setup = known.setups.iter().find(|setup| setup.name == name);
    crate::status::meant(&known.pads, &parts, setup)
}

/// Before `key` of the profile `name` takes the pad `value`: a part that
/// names that pad by its id takes the pad `key` had, so the two change
/// places in one pick, or is left to choose when `key` had none. Without
/// it two pads could only change parts by way of a profile that names
/// one pad twice, which is none. A part named by model whose pad is the
/// one picked keeps its model when a start would give it another pad of
/// the model, and otherwise takes the pad `key` had; with none, it keeps
/// its model, never being left to choose by a pick. The tray's menu does
/// the same, and the tray's `swap_cases.txt` holds the cases both meet.
pub fn change_places(known: &Known, name: &str, values: &mut Values, key: &str, value: &str) {
    let is_pad = |key: &str| {
        known
            .fields
            .iter()
            .any(|field| field.key == key && field.kind == Kind::Pad)
    };
    if !is_pad(key) {
        return;
    }
    let Some(picked) = known.pads.iter().find(|pad| names_pad(value, pad)) else {
        return;
    };
    let had = values.get(key).cloned();
    let others: Vec<String> = pads_taken(known, name, values, false)
        .into_iter()
        .filter(|(other, pad)| other != key && pad.id == picked.id)
        .map(|(other, _)| other)
        .collect();
    let mut then = values.clone();
    then.insert(key.to_string(), value.to_string());
    let taken_then = pads_taken(known, name, &then, true);
    for other in others {
        let by_model = values
            .get(&other)
            .and_then(|value| crate::status::wanted(value, &known.pads))
            .is_some_and(|wanted| matches!(wanted, PadRef::Model(..)));
        if by_model {
            let elsewhere = taken_then
                .get(&other)
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

/// Binds what needs no daemon: making a new profile's draft, dropping
/// what is held, naming, selecting, and the link to a setup.
pub fn bind_local(app: &AppWindow, nav: &NavHandles) {
    let profiles = app.global::<Profiles>();
    profiles.on_name_problem(|name| SharedString::from(name_problem(&name)));
    profiles.on_unfinished(|name| {
        HELD.with_borrow(|held| {
            held.get(name.as_str()).is_some_and(|values| {
                KNOWN.with_borrow(|known| unfinished_why(&known.fields, values).is_some())
            })
        })
    });

    let weak = app.as_weak();
    profiles.on_create(move |name| {
        let Some(app) = weak.upgrade() else { return };
        if !name_problem(&name).is_empty() || name.is_empty() {
            return;
        }
        // The first preset, its fields at their defaults, no pads yet.
        let values = KNOWN.with_borrow(|known| {
            let preset = known.fields.iter().find(|field| field.key == form::PRESET);
            let first = match preset.map(|field| &field.kind) {
                Some(Kind::Choice { choices }) => {
                    choices.first().map(|choice| choice.value.clone())
                }
                _ => None,
            }
            .unwrap_or_default();
            let mut values = Values::new();
            values.insert(form::PRESET.to_string(), first.clone());
            for field in &known.fields {
                if field.applies_to(&first) && !field.default.is_empty() {
                    values.insert(field.key.clone(), field.default.clone());
                }
            }
            values
        });
        HELD.with_borrow_mut(|held| held.insert(name.to_string(), values));
        render(&app);
        select(&app, &name);
    });

    let weak = app.as_weak();
    profiles.on_discard(move |name| {
        if let Some(app) = weak.upgrade() {
            HELD.with_borrow_mut(|held| held.remove(name.as_str()));
            render(&app);
        }
    });

    let weak = app.as_weak();
    profiles.on_select(move |name| {
        if let Some(app) = weak.upgrade() {
            select(&app, &name);
        }
    });

    let (weak, go_pad) = (app.as_weak(), nav.go_pad.clone());
    profiles.on_open_pad(move |id| {
        if let Some(app) = weak.upgrade() {
            go_pad(&app, id);
        }
    });
}

/// Binds what the pane asks of the daemon to `writer`: a field set, a
/// start, a delete, a copy and a rename, each on its profile's lane.
pub fn bind_edits(app: &AppWindow, writer: &Writer) {
    let profiles = app.global::<Profiles>();
    let (weak, client) = (app.as_weak(), writer.clone());
    profiles.on_set_field(move |name, key, value| {
        if let Some(app) = weak.upgrade() {
            set_field(&app, &client, &name, &key, &value);
        }
    });
    // Back to its default: what leaving the field out means.
    let (weak, client) = (app.as_weak(), writer.clone());
    profiles.on_reset_field(move |name, key| {
        let default = KNOWN.with_borrow(|known| {
            known
                .fields
                .iter()
                .find(|field| field.key == key.as_str())
                .map(|field| field.default.clone())
        });
        if let (Some(app), Some(default)) = (weak.upgrade(), default.filter(|d| !d.is_empty())) {
            set_field(&app, &client, &name, &key, &default);
        }
    });
    let (weak, client) = (app.as_weak(), writer.clone());
    profiles.on_set_number(move |name, key, number| {
        if let Some(app) = weak.upgrade() {
            set_field(&app, &client, &name, &key, &number.to_string());
        }
    });

    let (weak, client) = (app.as_weak(), writer.clone());
    profiles.on_start(move |name| {
        if let Some(app) = weak.upgrade()
            && !GOING.with_borrow(|going| going.contains(name.as_str()))
        {
            request(&app, &client, Op::Start(name.to_string()));
        }
    });

    let (weak, client) = (app.as_weak(), writer.clone());
    profiles.on_delete(move |name| {
        let Some(app) = weak.upgrade() else { return };
        HELD.with_borrow_mut(|held| held.remove(name.as_str()));
        // Nothing more is asked of it: a change queued before lands
        // first, on its lane, and none can come after.
        if !GOING.with_borrow_mut(|going| going.insert(name.to_string())) {
            return;
        }
        request(&app, &client, Op::Delete(name.to_string()));
    });

    let (weak, client) = (app.as_weak(), writer.clone());
    profiles.on_duplicate(move |source, name| {
        if let Some(app) = weak.upgrade() {
            copy(&app, &client, &source, &name, false);
        }
    });
    let (weak, client) = (app.as_weak(), writer.clone());
    profiles.on_rename(move |source, name| {
        if let Some(app) = weak.upgrade() {
            copy(&app, &client, &source, &name, true);
        }
    });
}

/// Binds the pane's actions. What it asks of the daemon goes through the
/// writer handed back, which the window flushes as it closes.
pub(crate) fn wire(app: &AppWindow, daemon: &DaemonClient, nav: &NavHandles) -> Writer {
    bind_local(app, nav);
    let (client, weak) = (daemon.clone(), app.as_weak());
    let writer = Writer::spawn(app.as_weak(), move |op| {
        perform(client.clone(), weak.clone(), op)
    });
    bind_edits(app, &writer);

    let profiles = app.global::<Profiles>();
    let weak = app.as_weak();
    profiles.on_reveal(move |name| {
        let Some(app) = weak.upgrade() else { return };
        let directory = app.get_daemon_config().to_string();
        if directory.is_empty() {
            return;
        }
        let path = std::path::Path::new(&directory)
            .join("profiles")
            .join(format!("{name}.toml"));
        tokio::spawn(async move {
            let revealed = match zbus::Connection::session().await {
                Ok(bus) => crate::file_manager::reveal(&bus, &path).await,
                Err(error) => Err(error.into()),
            };
            if let Err(error) = revealed {
                warn!(%error, path = %path.display(), "couldn't show the profile's file");
            }
        });
    });
    writer
}

/// Forgets what the window holds; for tests, which share a thread's
/// state between windows.
pub fn forget_held() {
    HELD.with_borrow_mut(BTreeMap::clear);
    ASKED.with_borrow_mut(BTreeMap::clear);
    PENDING.with_borrow_mut(BTreeMap::clear);
    GOING.with_borrow_mut(BTreeSet::clear);
    KNOWN.set(Known::default());
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEN: &str = "mode = \"toggle\"\nprimary = \"address:0a:1b:2c:3d:4e:14\"\nassist = \"address:1a:2b:3c:4d:5e:da\"\n";

    /// A held setup's line says why a pad was refused where the daemon
    /// said, and that a pad is missing where it didn't.
    #[test]
    fn a_held_setup_s_line_says_why_where_the_daemon_said() {
        let held = Setup {
            name: "den".to_string(),
            start: 1,
            phase: ctrlassist::Phase::Held,
            reason: None,
            sources: Vec::new(),
            sinks: Vec::new(),
            nodes: Vec::new(),
            stale: false,
        };
        assert!(setup_note(&held).contains("a pad it needs is missing"));
        let refused = Setup {
            reason: Some("its raw interface can't be opened".to_string()),
            ..held
        };
        assert_eq!(
            setup_note(&refused),
            "Held: its raw interface can't be opened. Its virtual pad stays, and the rest plays on."
        );
    }

    fn pad(id: &str, name: &str) -> Pad {
        Pad {
            id: id.to_string(),
            name: name.to_string(),
            vendor: 0x054c,
            product: 0x0ce6,
            connected: true,
            hidden: false,
            setup: None,
            rumble: Some(true),
            bus: None,
        }
    }

    /// A window that knows two profiles alike, `den` and `loft`, whose
    /// requests go to the receiver handed back.
    fn window() -> (AppWindow, mpsc::UnboundedReceiver<Op>) {
        i_slint_backend_testing::init_no_event_loop();
        forget_held();
        let app = AppWindow::new().expect("the window builds on the testing backend");
        let (writer, asked) = Writer::detached();
        bind_edits(&app, &writer);
        let kept = |name: &str| Profile {
            name: name.to_string(),
            toml: DEN.to_string(),
            unset: Vec::new(),
            problem: None,
        };
        show(
            &app,
            Known {
                profiles: vec![kept("den"), kept("loft")],
                setups: Vec::new(),
                pads: vec![
                    pad("address:0a:1b:2c:3d:4e:14", "DualSense"),
                    pad("address:1a:2b:3c:4d:5e:da", "Other DualSense"),
                ],
                fields: form::fields(),
            },
        );
        (app, asked)
    }

    fn sent(asked: &mut mpsc::UnboundedReceiver<Op>) -> Vec<Op> {
        let mut ops = Vec::new();
        while let Ok(op) = asked.try_recv() {
            ops.push(op);
        }
        ops
    }

    /// The word a profile's row shows.
    fn word(app: &AppWindow, name: &str) -> String {
        app.global::<Profiles>()
            .get_rows()
            .iter()
            .find(|row| row.name == name)
            .map(|row| row.word.to_string())
            .unwrap_or_default()
    }

    /// A field set, then a delete: both on the profile's lane, the
    /// change first, and nothing asked of the profile after; a queued
    /// change can't bring it back.
    #[test]
    fn a_delete_waits_for_the_changes_before_it_and_nothing_comes_after() {
        let (app, mut asked) = window();
        let profiles = app.global::<Profiles>();
        profiles.invoke_set_field("den".into(), "rumble".into(), "assist".into());
        assert_eq!(word(&app, "den"), "Saving…");
        profiles.invoke_delete("den".into());
        profiles.invoke_set_field("den".into(), "rumble".into(), "primary".into());
        profiles.invoke_start("den".into());
        let ops = sent(&mut asked);
        assert!(
            matches!(&ops[..], [Op::Put(change), Op::Delete(name)]
                if change.name == "den" && change.toml.contains("rumble = \"assist\"") && name == "den"),
            "{ops:?}"
        );
        // On the lane, the delete waits for the change, and another
        // profile's start for neither.
        let mut lanes = Lanes::default();
        let mut started = Vec::new();
        for op in ops.iter().cloned().chain([Op::Start("loft".into())]) {
            let lane = op.lane().to_string();
            started.extend(lanes.take(&lane, op));
        }
        assert_eq!(started.len(), 2, "{started:?}");
        assert!(matches!(&started[1], Op::Start(name) if name == "loft"));
        assert_eq!(
            lanes.done("den", Op::carried_by),
            (Vec::new(), Some(ops[1].clone()))
        );
        // Settled, the profile has nothing on its way and can be asked
        // again once the daemon says it is there.
        finished(&app, &ops[0], Ok(()));
        assert_eq!(word(&app, "den"), "Saving…", "the delete is on its way");
        finished(&app, &ops[1], Ok(()));
        assert_ne!(word(&app, "den"), "Saving…");
        profiles.invoke_set_field("den".into(), "rumble".into(), "primary".into());
        assert_eq!(sent(&mut asked).len(), 1);
        forget_held();
    }

    /// A rename and a duplicate carry what the window holds and hasn't
    /// kept yet; the rename's old profile takes nothing more, and goes
    /// only once the new one is kept, after what waits on its lane.
    #[test]
    fn a_rename_and_a_duplicate_carry_what_is_held() {
        let (app, mut asked) = window();
        let profiles = app.global::<Profiles>();
        profiles.invoke_set_field("loft".into(), "rumble".into(), "assist".into());
        profiles.invoke_rename("loft".into(), "attic".into());
        profiles.invoke_set_field("loft".into(), "rumble".into(), "primary".into());
        profiles.invoke_duplicate("den".into(), "den-two".into());
        let ops = sent(&mut asked);
        assert_eq!(ops.len(), 3, "{ops:?}");
        assert!(
            matches!(&ops[1], Op::Copy { from, to, toml, rename: true }
                if from == "loft" && to == "attic" && toml.contains("rumble = \"assist\"")),
            "{ops:?}"
        );
        assert!(
            matches!(&ops[2], Op::Copy { from, to, rename: false, .. } if from == "den" && to == "den-two"),
            "{ops:?}"
        );
        // What was held of the old one is held of the new.
        HELD.with_borrow(|held| {
            assert!(!held.contains_key("loft"));
            assert_eq!(
                held.get("attic").and_then(|values| values.get("rumble")),
                Some(&"assist".to_string())
            );
        });
        assert_eq!(word(&app, "attic"), "Saving…");
        // Kept: the old one has its delete still to come.
        finished(&app, &ops[1], Ok(()));
        assert_eq!(word(&app, "loft"), "Saving…");
        assert_ne!(word(&app, "attic"), "Saving…");
        finished(&app, &ops[0], Ok(()));
        finished(&app, &Op::Delete("loft".into()), Ok(()));
        assert_ne!(word(&app, "loft"), "Saving…");
        assert!(GOING.with_borrow(BTreeSet::is_empty));

        // A rename the daemon refuses leaves the old one, holding what
        // it held.
        profiles.invoke_set_field("den".into(), "rumble".into(), "assist".into());
        profiles.invoke_rename("den".into(), "study".into());
        let ops = sent(&mut asked);
        finished(&app, &ops[1], Err(Failed::Put(refused_error())));
        HELD.with_borrow(|held| {
            assert!(held.contains_key("den"));
            assert!(!held.contains_key("study"));
        });
        assert!(GOING.with_borrow(BTreeSet::is_empty));
        finished(&app, &ops[0], Ok(()));
        assert_ne!(word(&app, "den"), "Saving…");
        forget_held();
    }

    fn refused_error() -> ctrlassist::Error {
        ctrlassist::Error::Rpc {
            code: ctrlassist::ErrorCode::BadRequest,
            message: "no".to_string(),
        }
    }

    /// A change on its way says so: as saving, then as applying while
    /// the setup running the profile takes it up, until it lands.
    #[test]
    fn a_change_on_its_way_says_so_until_it_lands() {
        let (app, mut asked) = window();
        let before = word(&app, "den");
        app.global::<Profiles>()
            .invoke_set_field("den".into(), "rumble".into(), "assist".into());
        assert_eq!(word(&app, "den"), "Saving…");
        assert_eq!(
            word(&app, "loft"),
            before,
            "another profile has nothing on its way"
        );
        applying(&app, "den");
        assert_eq!(word(&app, "den"), "Applying…");
        let ops = sent(&mut asked);
        finished(&app, &ops[0], Ok(()));
        assert_eq!(word(&app, "den"), before);
        forget_held();
    }

    /// A start, a delete or a copy that lands clears only the banner of
    /// its own kind: not a warning another profile's change raised.
    #[test]
    fn an_outcome_clears_only_the_banner_of_its_own_kind() {
        let (app, _asked) = window();
        let errors = app.global::<Errors>();
        ASKED.with_borrow_mut(|asked| asked.insert("den".to_string(), 1));
        let change = Change {
            name: "den".to_string(),
            number: 1,
            toml: DEN.to_string(),
            update: true,
        };
        finished(&app, &Op::Put(change), Err(Failed::Update(refused_error())));
        let warning = errors.get_profiles();
        assert_eq!(warning.kind, "change:den");
        for op in [
            Op::Start("loft".into()),
            Op::Delete("loft".into()),
            Op::Copy {
                from: "loft".into(),
                to: "attic".into(),
                toml: DEN.to_string(),
                rename: false,
            },
        ] {
            finished(&app, &op, Ok(()));
            assert_eq!(errors.get_profiles(), warning, "{op:?}");
        }
        // Its own kind it clears.
        finished(
            &app,
            &Op::Start("loft".into()),
            Err(Failed::Put(refused_error())),
        );
        assert_eq!(errors.get_profiles().kind, "start:loft");
        finished(&app, &Op::Start("loft".into()), Ok(()));
        assert_eq!(errors.get_profiles(), ErrorState::default());
        forget_held();
    }

    /// The writer's flush waits for every request sent, a rename's
    /// delete sent after it among them, and says when they aren't all
    /// done.
    #[tokio::test]
    async fn the_flush_waits_for_every_request_and_what_one_sends_after_it() {
        let gate = Arc::new(tokio::sync::Notify::new());
        let done: Arc<std::sync::Mutex<Vec<String>>> = Arc::default();
        let writer = Writer::spawn(slint::Weak::default(), {
            let (gate, done) = (Arc::clone(&gate), Arc::clone(&done));
            move |op: Op| {
                let (gate, done) = (Arc::clone(&gate), Arc::clone(&done));
                async move {
                    if matches!(op, Op::Copy { .. }) {
                        gate.notified().await;
                    }
                    done.lock().unwrap().push(format!("{op:?}"));
                    let then = match op {
                        Op::Copy {
                            from, rename: true, ..
                        } => Some(Op::Delete(from)),
                        _ => None,
                    };
                    (Ok(()), then)
                }
            }
        });
        assert!(writer.flush(Duration::from_secs(5)).await, "nothing sent");
        assert!(writer.send(Op::Copy {
            from: "loft".into(),
            to: "attic".into(),
            toml: DEN.to_string(),
            rename: true,
        }));
        assert!(writer.send(Op::Start("den".into())));
        assert!(!writer.flush(Duration::from_millis(50)).await);
        gate.notify_one();
        assert!(writer.flush(Duration::from_secs(5)).await);
        let done = done.lock().unwrap();
        assert_eq!(done.len(), 3, "{done:?}");
        assert_eq!(done[2], "Delete(\"loft\")");
    }

    #[test]
    fn a_profile_still_short_of_pads_is_put_and_no_setup_asked_to_take_it_up() {
        let fields = form::fields();
        let mut values = Values::new();
        values.insert(form::PRESET.to_string(), "unicast".to_string());
        let short = change(&fields, "den", &values).expect("kept as not set up");
        assert!(!short.update);
        values.insert("pad".to_string(), "address:0a:1b:2c:3d:4e:14".to_string());
        assert!(change(&fields, "den", &values).expect("a profile").update);
        // Not a profile, whatever its pads.
        values.insert(form::PRESET.to_string(), "sideways".to_string());
        assert_eq!(change(&fields, "den", &values), None);
    }

    #[test]
    fn only_the_last_change_s_outcome_is_said_and_one_kept_stays_held() {
        i_slint_backend_testing::init_no_event_loop();
        forget_held();
        let app = AppWindow::new().expect("the window builds on the testing backend");
        let errors = app.global::<Errors>();
        let refused = || ctrlassist::Error::Rpc {
            code: ctrlassist::ErrorCode::BadRequest,
            message: "no".to_string(),
        };
        let held = || HELD.with_borrow(|held| held.contains_key("den"));
        HELD.with_borrow_mut(|held| held.insert("den".to_string(), Values::new()));
        let ask = |number: u64| {
            ASKED.with_borrow_mut(|asked| asked.insert("den".to_string(), number));
        };

        // The first of two fails: the second carries it whole and is
        // still to come, so nothing is said or dropped yet.
        ask(2);
        done(&app, "den", 1, Err(Failed::Put(refused())));
        assert!(held());
        assert_eq!(errors.get_profiles(), ErrorState::default());

        // Kept, and not taken up by the setup running it: said as
        // that, and held until the daemon says it.
        done(&app, "den", 2, Err(Failed::Update(refused())));
        assert!(held());
        let banner = errors.get_profiles();
        assert_eq!(banner.tone, "warn");
        assert!(banner.title.contains("was kept"), "{}", banner.title);

        // The next that lands clears what the last raised.
        ask(3);
        done(&app, "den", 3, Ok(()));
        assert_eq!(errors.get_profiles(), ErrorState::default());

        // One the daemon didn't keep is dropped from what is held.
        ask(4);
        done(&app, "den", 4, Err(Failed::Put(refused())));
        assert!(!held());
        assert!(errors.get_profiles().title.contains("couldn't be kept"));
        forget_held();
    }
}
