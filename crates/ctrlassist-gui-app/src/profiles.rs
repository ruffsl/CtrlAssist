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
use std::collections::BTreeMap;

use ctrlassist::form::{self, Field, Kind, Values};
use ctrlassist::{DaemonClient, Pad, Profile, Setup, SetupSource};
use slint::{ComponentHandle, Model as _, ModelRc, SharedString, VecModel};
use tracing::warn;

use crate::nav::NavHandles;
use crate::status::Status;
use crate::{
    AppWindow, ErrorState, Errors, ProfileFieldRow, ProfileOutputRow, ProfileRow, ProfileUseRow,
    Profiles, on_ui,
};

use ctrlassist_protocol::profile::DEFAULT;

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
fn field_row(field: &Field, value: &str, pads: &[Pad], profile: &str) -> ProfileFieldRow {
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
                    let label = match ctrlassist_protocol::profile::PadRef::parse(value) {
                        Some(ctrlassist_protocol::profile::PadRef::Model(vendor, product)) => {
                            format!("Any pad of model {vendor:04x}:{product:04x}")
                        }
                        _ => format!("{} (not known)", crate::pads::tag_of(value)),
                    };
                    row.value_label = SharedString::from(label);
                }
                None => {}
            }
            // Nothing picked has nothing to say of. Any pad of a model
            // is spoken for by the pad of that model a start would take,
            // as the profile's word judges it.
            if !value.is_empty() {
                let meant = chosen.or_else(|| {
                    let wanted = ctrlassist_protocol::profile::PadRef::parse(value)?;
                    crate::status::meant(&wanted, pads, profile)
                });
                let (word, tone) = connection(meant, profile);
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
        ctrlassist::Phase::Held => {
            "Held: a pad it needs is missing. Its virtual pad stays, and the rest plays on."
                .to_string()
        }
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
            format!("Games see a {}.", sinks.join(" and a "))
        }
        _ => "Starting.".to_string(),
    }
}

/// A pad a profile names, for the header's link: the pad its setup has
/// for that part while it runs, or else the pad the name means.
fn use_row(
    source: &ctrlassist_protocol::profile::PlannedSource,
    setup: Option<&Setup>,
    pads: &[Pad],
    profile: &str,
) -> ProfileUseRow {
    let taken = setup
        .and_then(|setup| setup.sources.iter().find(|taken| taken.name == source.name))
        .and_then(|taken| taken.pad.as_deref())
        .and_then(|id| pads.iter().find(|pad| pad.id == id));
    let pad = taken.or_else(|| crate::status::meant(&source.pad, pads, profile));
    let (word, tone) = connection(pad, profile);
    ProfileUseRow {
        word: SharedString::from(word),
        tone: SharedString::from(tone),
        part: SharedString::from(crate::status::capitalized(&source.name)),
        pad: SharedString::from(pad.map_or("", |pad| pad.id.as_str())),
        name: SharedString::from(match (pad, &source.pad) {
            (Some(pad), _) => pad.name.clone(),
            (None, ctrlassist_protocol::profile::PadRef::Exact(id)) => id.clone(),
            (None, ctrlassist_protocol::profile::PadRef::Model(vendor, product)) => {
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
                            field_row(field, value, &known.pads, name)
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
                    (Some(plan), _) => (
                        plan.sources
                            .iter()
                            .map(|source| use_row(source, setup, &known.pads, name))
                            .collect(),
                        plan.sinks
                            .iter()
                            .map(|sink| output_row(name, &sink.name, sink.identity.as_str(), setup))
                            .collect(),
                    ),
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
                None => "Written by hand".to_string(),
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

/// The pane's banner for something the daemon refused.
fn refused(what: &str, error: &ctrlassist::Error) -> ErrorState {
    warn!(%error, "{what}");
    ErrorState {
        kind: SharedString::from("profiles"),
        tone: SharedString::from("error"),
        title: SharedString::from(what),
        message: SharedString::from(error.to_string()),
        action_label: SharedString::default(),
    }
}

/// A change to put: a profile's whole text as the form makes it, and
/// whether a setup running it takes it up.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Change {
    name: String,
    /// The number the window gave it, counting up; the latest asked of
    /// a profile is the one whose outcome is shown.
    number: u64,
    toml: String,
    /// A profile still short of pads is kept as not set up, and a setup
    /// running it can't take that up, so it isn't asked to.
    update: bool,
}

/// Why a change didn't come to be.
#[derive(Debug)]
enum Failed {
    /// The daemon didn't keep it.
    Put(ctrlassist::Error),
    /// It was kept, and the setup running it didn't take it up.
    Update(ctrlassist::Error),
}

/// Where changes go to be put, one at a time and in the order they were
/// asked: each is its own task otherwise, and a quick run of them (an
/// arrow held on the virtual-pad count) could land out of order.
#[derive(Clone)]
struct Writer(tokio::sync::mpsc::UnboundedSender<Change>);

thread_local! {
    /// The number of the last change asked of each profile whose
    /// outcome is still to come.
    static ASKED: RefCell<BTreeMap<String, u64>> = const { RefCell::new(BTreeMap::new()) };
    /// How many changes have been asked.
    static COUNTED: Cell<u64> = const { Cell::new(0) };
}

/// Of a run of changes, those still to put: each carries its profile's
/// whole text, so one a later change of the same profile follows is
/// left out. The rest keep their order.
fn latest(batch: Vec<Change>) -> Vec<Change> {
    let mut kept: Vec<Change> = Vec::new();
    for change in batch {
        kept.retain(|earlier| earlier.name != change.name);
        kept.push(change);
    }
    kept
}

/// Puts a change, and has a setup running it take it up.
async fn apply(daemon: &DaemonClient, change: &Change) -> Result<(), Failed> {
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
        daemon
            .update_setup(&change.name, true)
            .await
            .map_err(Failed::Update)?;
    }
    Ok(())
}

/// Puts the changes as they come, one after another.
async fn write(
    mut changes: tokio::sync::mpsc::UnboundedReceiver<Change>,
    daemon: DaemonClient,
    app: slint::Weak<AppWindow>,
) {
    while let Some(first) = changes.recv().await {
        let mut batch = vec![first];
        while let Ok(next) = changes.try_recv() {
            batch.push(next);
        }
        for change in latest(batch) {
            let outcome = apply(&daemon, &change).await;
            on_ui(&app, move |app| {
                done(&app, &change.name, change.number, outcome);
            });
        }
    }
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
            refused(&format!("The change to '{name}' couldn't be kept"), &error)
        }
        Err(Failed::Update(error)) => ErrorState {
            tone: SharedString::from("warn"),
            ..refused(
                &format!(
                    "The change to '{name}' was kept, but the running profile couldn't take it up"
                ),
                &error,
            )
        },
    };
    errors.set_profiles(ErrorState {
        kind: SharedString::from(kind),
        ..banner
    });
}

/// A field was set. The window holds the change and shows it; if it
/// makes a profile, the profile is put and a setup running it updated.
/// The held change goes once the kept profile says the same, or is
/// dropped with a banner if the daemon refuses it.
fn set_field(app: &AppWindow, writer: &Writer, name: &str, key: &str, value: &str) {
    let Some(mut values) = values_of(name) else {
        return;
    };
    if values.get(key).map(String::as_str) == Some(value) {
        return;
    }
    KNOWN.with_borrow(|known| change_places(known, &mut values, key, value));
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
    if writer.0.send(change).is_err() {
        warn!(
            profile = name,
            "the change wasn't put: nothing writes changes any more"
        );
    }
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

/// Before `key` takes the pad `value`: if another part of the profile
/// has that pad, it takes the pad `key` had, so the two change places
/// in one pick. Without it two pads could only change parts by way of
/// a profile that names one pad twice, which is none.
pub fn change_places(known: &Known, values: &mut Values, key: &str, value: &str) {
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
    let other = values
        .iter()
        .find(|(other, held)| other.as_str() != key && is_pad(other) && names_pad(held, picked))
        .map(|(other, _)| other.clone());
    if let Some(other) = other {
        match values.get(key).cloned() {
            Some(had) => values.insert(other, had),
            None => values.remove(&other),
        };
    }
}

/// Runs `ask` and shows its refusal, if it is refused.
fn ask<F>(app: &AppWindow, what: &'static str, ask: F)
where
    F: Future<Output = ctrlassist::Result<()>> + Send + 'static,
{
    let weak = app.as_weak();
    tokio::spawn(async move {
        let banner = match ask.await {
            Ok(()) => ErrorState::default(),
            Err(error) => refused(what, &error),
        };
        on_ui(&weak, move |app| {
            app.global::<Errors>().set_profiles(banner);
        });
    });
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

/// Binds the pane's actions.
pub(crate) fn wire(app: &AppWindow, daemon: &DaemonClient, nav: &NavHandles) {
    bind_local(app, nav);
    let profiles = app.global::<Profiles>();
    let (changes, to_write) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(write(to_write, daemon.clone(), app.as_weak()));
    let writer = Writer(changes);

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
    let (weak, client) = (app.as_weak(), writer);
    profiles.on_set_number(move |name, key, number| {
        if let Some(app) = weak.upgrade() {
            set_field(&app, &client, &name, &key, &number.to_string());
        }
    });

    let (weak, client) = (app.as_weak(), daemon.clone());
    profiles.on_start(move |name| {
        let Some(app) = weak.upgrade() else { return };
        let (client, name) = (client.clone(), name.to_string());
        ask(&app, "The profile couldn't be started", async move {
            client.start_setup(&name).await
        });
    });

    let (weak, client) = (app.as_weak(), daemon.clone());
    profiles.on_delete(move |name| {
        let Some(app) = weak.upgrade() else { return };
        HELD.with_borrow_mut(|held| held.remove(name.as_str()));
        let (client, name) = (client.clone(), name.to_string());
        ask(&app, "The profile couldn't be deleted", async move {
            client.delete_profile(&name).await
        });
    });

    // A copy, and a rename, are made of the text as kept.
    let kept_text = |name: &str| {
        KNOWN.with_borrow(|known| {
            known
                .profiles
                .iter()
                .find(|kept| kept.name == name)
                .map(|kept| kept.toml.clone())
        })
    };
    let (weak, client) = (app.as_weak(), daemon.clone());
    profiles.on_duplicate(move |source, name| {
        let (Some(app), Some(toml)) = (weak.upgrade(), kept_text(&source)) else {
            return;
        };
        let (client, name) = (client.clone(), name.to_string());
        ask(&app, "The profile couldn't be duplicated", async move {
            client.put_profile(&name, &toml).await
        });
    });
    let (weak, client) = (app.as_weak(), daemon.clone());
    profiles.on_rename(move |source, name| {
        let (Some(app), Some(toml)) = (weak.upgrade(), kept_text(&source)) else {
            return;
        };
        let (client, source, name) = (client.clone(), source.to_string(), name.to_string());
        ask(&app, "The profile couldn't be renamed", async move {
            // The new one first, so a failure leaves the old.
            client.put_profile(&name, &toml).await?;
            client.delete_profile(&source).await
        });
    });

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
}

/// Forgets what the window holds; for tests, which share a thread's
/// state between windows.
pub fn forget_held() {
    HELD.with_borrow_mut(BTreeMap::clear);
    ASKED.with_borrow_mut(BTreeMap::clear);
    KNOWN.set(Known::default());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered(name: &str, number: u64) -> Change {
        Change {
            name: name.to_string(),
            number,
            toml: format!("# {number}\n"),
            update: true,
        }
    }

    #[test]
    fn of_a_run_of_changes_the_last_of_each_profile_is_put_in_order() {
        let put = latest(vec![
            numbered("a", 1),
            numbered("b", 2),
            numbered("a", 3),
            numbered("c", 4),
            numbered("b", 5),
        ]);
        let numbers: Vec<u64> = put.iter().map(|change| change.number).collect();
        assert_eq!(numbers, [3, 4, 5]);
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
