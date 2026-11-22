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

use std::cell::RefCell;
use std::collections::BTreeMap;

use ctrlassist::form::{self, Field, Kind, Values};
use ctrlassist::{DaemonClient, Pad, Profile, Setup};
use slint::{ComponentHandle, Model as _, ModelRc, SharedString, VecModel};
use tracing::warn;

use crate::nav::NavHandles;
use crate::{AppWindow, ErrorState, Errors, ProfileFieldRow, ProfileRow, Profiles, on_ui};

/// The profile the tray's quick menu writes.
const QUICK: &str = "quick";

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
fn field_row(field: &Field, value: &str, pads: &[Pad]) -> ProfileFieldRow {
    let mut row = ProfileFieldRow {
        key: SharedString::from(field.key.as_str()),
        label: SharedString::from(field.label.as_str()),
        help: SharedString::from(field.help.as_str()),
        value: SharedString::from(value),
        ..ProfileFieldRow::default()
    };
    match &field.kind {
        Kind::Choice { choices } => {
            row.kind = SharedString::from("choice");
            row.options = strings(choices.iter().map(|choice| choice.label.clone()));
            row.option_values = strings(choices.iter().map(|choice| choice.value.clone()));
            if let Some(chosen) = choices.iter().find(|choice| choice.value == value) {
                row.value_label = SharedString::from(chosen.label.as_str());
                row.description = SharedString::from(chosen.description.as_str());
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
            match sorted.iter().find(|pad| names_pad(value, pad)) {
                Some(pad) => row.value_label = SharedString::from(pad_label(pad)),
                // Any pad of a model, or one the daemon has never seen.
                None => row.read_only = !value.is_empty(),
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

/// The pane's rows, in name order: every kept profile, and every one
/// the window holds that isn't kept yet.
#[must_use]
pub fn rows(known: &Known, held: &BTreeMap<String, Values>) -> Vec<ProfileRow> {
    let mut names: Vec<&str> = known
        .profiles
        .iter()
        .map(|profile| profile.name.as_str())
        .chain(held.keys().map(String::as_str))
        .collect();
    names.sort_unstable();
    names.dedup();
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
            let (phase_label, tone) =
                setup.map_or(("", ""), |setup| crate::setups::phase_text(setup.phase));
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
                        .filter(|field| field.applies_to(&preset))
                        .map(|field| {
                            let value = values.get(&field.key).map_or("", String::as_str);
                            field_row(field, value, &known.pads)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let summary = match &values {
                Some(values) => match values.get("identity") {
                    Some(identity) => format!("{preset} · {identity}"),
                    None => preset,
                },
                None => "full form".to_string(),
            };
            ProfileRow {
                name: SharedString::from(name),
                kind: SharedString::from(if values.is_some() { "preset" } else { "full" }),
                unfinished: why.is_some(),
                unfinished_why: SharedString::from(why.unwrap_or_default()),
                kept: kept.is_some(),
                problem: SharedString::from(
                    kept.and_then(|kept| kept.problem.as_ref())
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                ),
                summary: SharedString::from(summary),
                running: setup.is_some(),
                phase_label: SharedString::from(phase_label),
                tone: SharedString::from(tone),
                is_quick: name == QUICK,
                fields: ModelRc::new(VecModel::from(fields)),
                toml: SharedString::from(kept.map_or("", |kept| kept.toml.as_str())),
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
            row.fields = crate::models::sync(&old.fields, row.fields.iter().collect(), |a, b| {
                a.key == b.key && a.kind == b.kind && a.read_only == b.read_only
            });
        }
    }
    let index = rows
        .iter()
        .position(|row| row.name == was)
        .filter(|_| !was.is_empty())
        .and_then(|at| i32::try_from(at).ok())
        .unwrap_or(-1);
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

/// A field was set. The window holds the change and shows it; if it
/// makes a profile, the profile is put and a setup running it updated.
/// The held change goes once the kept profile says the same, or is
/// dropped with a banner if the daemon refuses it.
fn set_field(app: &AppWindow, daemon: &DaemonClient, name: &str, key: &str, value: &str) {
    let Some(mut values) = values_of(name) else {
        return;
    };
    if values.get(key).map(String::as_str) == Some(value) {
        return;
    }
    values.insert(key.to_string(), value.to_string());
    let (toml, complete) = KNOWN.with_borrow(|known| {
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
        (
            form::to_toml(&known.fields, &values),
            unfinished_why(&known.fields, &values).is_none(),
        )
    });
    HELD.with_borrow_mut(|held| held.insert(name.to_string(), values));
    render(app);
    if !complete {
        return;
    }
    let (weak, daemon, name) = (app.as_weak(), daemon.clone(), name.to_string());
    tokio::spawn(async move {
        let outcome = async {
            daemon.put_profile(&name, &toml).await?;
            // A setup running it takes it at once, as the tray's does.
            if daemon.setup(&name)?.is_some() {
                daemon.update_setup(&name, true).await?;
            }
            Ok::<(), ctrlassist::Error>(())
        }
        .await;
        on_ui(&weak, move |app| {
            let banner = match outcome {
                Ok(()) => ErrorState::default(),
                Err(error) => {
                    // Back to what is kept.
                    HELD.with_borrow_mut(|held| held.remove(&name));
                    render(&app);
                    refused("The change couldn't be applied", &error)
                }
            };
            app.global::<Errors>().set_profiles(banner);
        });
    });
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

    let (weak, go_setup) = (app.as_weak(), nav.go_setup.clone());
    profiles.on_open_setup(move |name| {
        if let Some(app) = weak.upgrade() {
            go_setup(&app, name);
        }
    });
}

/// Binds the pane's actions.
pub(crate) fn wire(app: &AppWindow, daemon: &DaemonClient, nav: &NavHandles) {
    bind_local(app, nav);
    let profiles = app.global::<Profiles>();

    let (weak, client) = (app.as_weak(), daemon.clone());
    profiles.on_set_field(move |name, key, value| {
        if let Some(app) = weak.upgrade() {
            set_field(&app, &client, &name, &key, &value);
        }
    });
    let (weak, client) = (app.as_weak(), daemon.clone());
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
    KNOWN.set(Known::default());
}
