//! The tray's menu as plain data, made from a view and the quick-start
//! choices. Nothing here knows the desktop: the tray turns these entries
//! into its toolkit's items, and tests read them as an outline.

use ctrlassist::form::{Field, Kind, PRESET};
use ctrlassist::{Phase, Setup};

use crate::quick::{self, Quick};
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
    /// Change one of the quick-start choices. Nothing is asked of the
    /// daemon until [`Command::QuickStart`].
    Quick(Edit),
    /// Write the quick-start choices as the profile `quick` and run it.
    QuickStart,
    /// Open the settings window, or raise it if it is open.
    Settings,
    /// End the tray.
    Quit,
}

/// One change to the quick-start choices: a field of the preset form,
/// and what it is set to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    /// The field's key.
    pub key: String,
    /// Its new value, as a profile writes it.
    pub value: String,
}

impl Edit {
    /// Sets the field `key` to `value`.
    #[must_use]
    pub fn set(key: &str, value: &str) -> Self {
        Self {
            key: key.to_string(),
            value: value.to_string(),
        }
    }
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
        /// What it says.
        label: String,
        /// What activating it asks for.
        command: Option<Command>,
    },
    /// A row that is on or off.
    Check {
        /// What it says.
        label: String,
        /// Whether it is on.
        checked: bool,
        /// What activating it asks for.
        command: Command,
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
        /// What it says.
        label: String,
        /// Its entries.
        entries: Vec<Self>,
    },
}

fn said(label: impl Into<String>) -> Entry {
    Entry::Item {
        label: label.into(),
        command: None,
    }
}

fn action(label: impl Into<String>, command: Command) -> Entry {
    Entry::Item {
        label: label.into(),
        command: Some(command),
    }
}

fn sub(label: impl Into<String>, entries: Vec<Entry>) -> Entry {
    Entry::Sub {
        label: label.into(),
        entries,
    }
}

/// A running setup's entries: what to do with it, then what it is made of.
fn setup_entries(view: &View, setup: &Setup) -> Vec<Entry> {
    let mut entries = vec![action("Stop", Command::Stop(setup.name.clone()))];
    if let Some(reason) = &setup.reason {
        entries.push(Entry::Separator);
        entries.push(said(reason.clone()));
        return entries;
    }
    for node in &setup.nodes {
        // A preset's one node needs no name; any other is called by its.
        let label = if node.name == "mux" {
            "Has control".to_string()
        } else {
            format!("{} selects", node.name)
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
            label,
            vec![Entry::Radio {
                choices,
                selected: Some(node.selected),
            }],
        ));
    }
    entries.push(Entry::Separator);
    for source in &setup.sources {
        let pad = match (&source.pad, source.present) {
            (Some(pad), true) => view.pad_name(pad).to_string(),
            (Some(pad), false) => format!("{} (missing)", view.pad_name(pad)),
            (None, _) => "waiting for a pad that fits".to_string(),
        };
        entries.push(said(format!("{}: {pad}", source.name)));
    }
    for sink in &setup.sinks {
        let node = sink
            .node
            .as_deref()
            .and_then(|node| node.rsplit('/').next())
            .unwrap_or("not made");
        entries.push(said(format!("{}: {} at {node}", sink.name, sink.identity)));
    }
    entries
}

fn phase_label(phase: Phase) -> &'static str {
    phase.as_str()
}

/// Whether a pad can be picked in quick start: connected, not picked
/// for another part, and in no setup but quick start's own.
fn pickable(view: &View, id: &str, taken: &[&str]) -> (bool, String) {
    let Some(pad) = view.pad(id) else {
        return (false, id.to_string());
    };
    let name = pad.name.clone();
    if !pad.connected {
        return (false, format!("{name} (not connected)"));
    }
    match pad.setup.as_deref() {
        Some(setup) if setup != quick::NAME => (false, format!("{name} (in {setup})")),
        _ => (!taken.contains(&id), name),
    }
}

/// A pad field: the known pads to pick from, with the one picked.
fn pad_entry(view: &View, quick: &Quick, field: &Field) -> Entry {
    let picked = quick.values.get(&field.key).map(String::as_str);
    // A pad picked for another part can't be picked for this one.
    let taken: Vec<&str> = quick
        .pads(&view.fields)
        .into_iter()
        .filter(|(key, _)| *key != field.key)
        .map(|(_, pad)| pad)
        .collect();
    let choices: Vec<Choice> = view
        .pads
        .iter()
        .map(|pad| {
            let (enabled, label) = pickable(view, &pad.id, &taken);
            Choice {
                label,
                enabled,
                command: Command::Quick(Edit::set(&field.key, &pad.id)),
            }
        })
        .collect();
    let selected = picked.and_then(|picked| view.pads.iter().position(|pad| pad.id == picked));
    sub(
        format!(
            "{}: {}",
            field.label,
            picked.map_or("not chosen", |pad| view.pad_name(pad))
        ),
        vec![Entry::Radio { choices, selected }],
    )
}

/// A field of choices: each by its label, with the one it stands at.
fn choice_entry(
    view: &View,
    quick: &Quick,
    field: &Field,
    choices: &[ctrlassist::form::Choice],
) -> Entry {
    // Of the presets, the ones quick start offers.
    let offered: Vec<&ctrlassist::form::Choice> = if field.key == PRESET {
        quick::presets(&view.fields)
    } else {
        choices.iter().collect()
    };
    let stands_at = if field.key == PRESET {
        quick.preset(&view.fields)
    } else {
        quick.value(field)
    };
    let selected = offered.iter().position(|choice| choice.value == stands_at);
    let label = selected.map_or(stands_at, |at| offered[at].label.as_str());
    let radio = Entry::Radio {
        choices: offered
            .iter()
            .map(|choice| Choice {
                label: choice.label.clone(),
                // An identity the daemon says it can't make now can't
                // be picked.
                enabled: field.key != "identity"
                    || view
                        .identities
                        .iter()
                        .find(|known| known.id == choice.value)
                        .is_none_or(|known| known.available),
                command: Command::Quick(Edit::set(&field.key, &choice.value)),
            })
            .collect(),
        selected,
    };
    sub(format!("{}: {label}", field.label), vec![radio])
}

fn quick_entries(view: &View, quick: &Quick) -> Vec<Entry> {
    // What there is to choose is the daemon's to say.
    if view.fields.is_empty() {
        return vec![said("Waiting for the daemon to say what can be chosen")];
    }
    let preset = quick.preset(&view.fields);
    let mut entries: Vec<Entry> = view
        .fields
        .iter()
        .filter(|field| field.applies_to(preset))
        .filter_map(|field| match &field.kind {
            Kind::Pad => Some(pad_entry(view, quick, field)),
            Kind::Choice { choices } => Some(choice_entry(view, quick, field, choices)),
            Kind::Switch => {
                let checked = quick.value(field) == "true";
                Some(Entry::Check {
                    label: field.label.clone(),
                    checked,
                    command: Command::Quick(Edit::set(&field.key, &(!checked).to_string())),
                })
            }
            // No preset quick start offers has another kind of field.
            _ => None,
        })
        .collect();
    let running = view.setup(quick::NAME).is_some();
    let ready = quick.profile(&view.fields).is_some()
        && quick
            .pads(&view.fields)
            .into_iter()
            .all(|(_, pad)| pickable(view, pad, &[]).0);
    // Every pick is applied to a running `quick` as it is made, so
    // there is something to restart only if that didn't happen: the
    // picks then differ from the profile as kept.
    let waiting = view
        .kept_quick()
        .is_none_or(|kept| kept.text(&view.fields) != quick.text(&view.fields));
    let (verb, ready) = match (running, waiting) {
        (false, _) => ("Start", ready),
        (true, true) => ("Restart", ready),
        (true, false) => ("Running", false),
    };
    entries.push(Entry::Separator);
    entries.push(Entry::Item {
        label: format!("{verb} as `{}`", quick::NAME),
        command: ready.then_some(Command::QuickStart),
    });
    entries
}

fn pad_entries(view: &View) -> Vec<Entry> {
    if view.pads.is_empty() {
        return vec![said("No pads are known")];
    }
    view.pads
        .iter()
        .map(|pad| {
            let state = match (&pad.setup, pad.connected) {
                (Some(setup), _) => format!(" (in {setup})"),
                (None, false) => " (not connected)".to_string(),
                (None, true) => String::new(),
            };
            let mut entries = vec![said(pad.id.clone())];
            if pad.connected {
                entries.push(if pad.rumble == Some(false) {
                    said("No rumble to buzz with")
                } else {
                    action("Identify (buzz)", Command::Identify(pad.id.clone()))
                });
            } else if pad.setup.is_none() {
                entries.push(action("Forget", Command::Forget(pad.id.clone())));
            }
            sub(format!("{}{state}", pad.name), entries)
        })
        .collect()
}

/// The whole menu for what the daemon publishes now and the quick-start
/// choices.
#[must_use]
pub fn menu(view: &View, quick: &Quick) -> Vec<Entry> {
    let mut entries = Vec::new();
    if !view.alive {
        entries.push(said("The CtrlAssist daemon isn't running"));
    } else if let Some(theirs) = &view.mismatch {
        entries.push(said(mismatch_text(theirs)));
    } else if !view.hydrated {
        entries.push(said("Reading from the daemon…"));
    } else {
        // Every kept profile, and any setup whose profile is gone.
        let mut names: Vec<&str> = view
            .profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect();
        names.extend(view.setups.iter().map(|setup| setup.name.as_str()));
        names.sort_unstable();
        names.dedup();
        for name in names {
            let profile = view.profiles.iter().find(|profile| profile.name == name);
            entries.push(match view.setup(name) {
                Some(setup) => sub(
                    format!("{name} ({})", phase_label(setup.phase)),
                    setup_entries(view, setup),
                ),
                None => {
                    let problem = profile.and_then(|profile| profile.problem.as_ref());
                    sub(
                        name,
                        match problem {
                            Some(problem) => vec![said(format!("Can't run: {problem}"))],
                            None => vec![action("Start", Command::Start(name.to_string()))],
                        },
                    )
                }
            });
        }
        if !entries.is_empty() {
            entries.push(Entry::Separator);
        }
        entries.push(sub("Quick start", quick_entries(view, quick)));
        let connected = view.pads.iter().filter(|pad| pad.connected).count();
        entries.push(sub(
            format!("Pads ({connected} connected)"),
            pad_entries(view),
        ));
        if let Some(daemon) = &view.daemon {
            entries.push(Entry::Separator);
            entries.push(said(format!(
                "Hiding: {} · Real-time: {}",
                daemon.hiding.as_str(),
                if daemon.realtime { "yes" } else { "no" }
            )));
        }
    }
    entries.push(Entry::Separator);
    // There whatever the daemon is doing: the window says more about a
    // daemon that isn't running than the menu can.
    entries.push(action("Settings", Command::Settings));
    entries.push(action("Quit tray", Command::Quit));
    entries
}

/// What the menu and tooltip say of a daemon on a release this tray
/// can't talk to.
#[must_use]
pub fn mismatch_text(theirs: &str) -> String {
    format!(
        "Version mismatch: daemon {theirs}, tray {}. Restart the daemon or the tray so they match.",
        ctrlassist_protocol::VERSION
    )
}

/// The menu as an indented outline, one entry a line: `[x]` and `( )`
/// for marks, a trailing `>` on a submenu, `-` before a row that can't be
/// activated. What the tests read.
#[must_use]
pub fn outline(entries: &[Entry]) -> String {
    fn walk(entries: &[Entry], depth: usize, out: &mut String) {
        let pad = "  ".repeat(depth);
        for entry in entries {
            match entry {
                Entry::Separator => out.push_str(&format!("{pad}---\n")),
                Entry::Item { label, command } => {
                    let dead = if command.is_none() { "- " } else { "" };
                    out.push_str(&format!("{pad}{dead}{label}\n"));
                }
                Entry::Check { label, checked, .. } => {
                    out.push_str(&format!(
                        "{pad}[{}] {label}\n",
                        if *checked { "x" } else { " " }
                    ));
                }
                Entry::Radio { choices, selected } => {
                    for (index, choice) in choices.iter().enumerate() {
                        let mark = if *selected == Some(index) { "o" } else { " " };
                        let dead = if choice.enabled { "" } else { "- " };
                        out.push_str(&format!("{pad}({mark}) {dead}{}\n", choice.label));
                    }
                }
                Entry::Sub { label, entries } => {
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
    use ctrlassist::Hiding;

    use super::*;
    use crate::state::tests::{profile, view};

    fn quick() -> Quick {
        let mut quick = Quick::default();
        quick.set("primary", "address:dualsense");
        quick.set("assist", "address:8bitdo");
        quick
    }

    /// What quick start offers is the daemon's to say: until it has,
    /// there is nothing to choose and nothing to start.
    #[test]
    fn quick_start_waits_for_the_daemon_to_say_what_can_be_chosen() {
        let mut unsaid = view(None, Hiding::Service);
        unsaid.fields.clear();
        let menu = outline(&menu(&unsaid, &quick()));
        assert!(
            menu.contains(
                "Quick start >\n  - Waiting for the daemon to say what can be chosen\nPads"
            ),
            "{menu}"
        );
    }

    /// The whole menu, as the maintainer's mockup has it: a toggle held
    /// because its assist left, a profile not running, quick start, the
    /// pads, and the daemon's status.
    #[test]
    fn the_menu_shows_profiles_then_quick_start_then_pads_then_status() {
        let menu = outline(&menu(&view(Some(Phase::Held), Hiding::Service), &quick()));
        let expected = "\
couch (held) >
  Stop
  ---
  Has control >
    (o) primary
    ( ) - assist
  ---
  - assist: Xbox pad (missing)
  - primary: DualSense
  - virtual: generic at event31
desk >
  Start
---
Quick start >
  Preset: Priority >
    (o) Priority
    ( ) Average
    ( ) Toggle
  Primary: DualSense >
    (o) - DualSense (in couch)
    ( ) - 8BitDo Pro 2
    ( ) - Xbox pad (not connected)
  Assist: 8BitDo Pro 2 >
    ( ) - DualSense (in couch)
    (o) 8BitDo Pro 2
    ( ) - Xbox pad (not connected)
  Identity: Generic >
    (o) Generic
    ( ) Xbox
    ( ) DualSense
  Rumble goes to: Both pads >
    (o) Both pads
    ( ) Primary only
    ( ) Assist only
  Real pads: Hidden >
    (o) Hidden
    ( ) Grabbed
    ( ) Visible
  [ ] Managed
  ---
  - Start as `quick`
Pads (2 connected) >
  DualSense (in couch) >
    - address:dualsense
    Identify (buzz)
  8BitDo Pro 2 >
    - address:8bitdo
    - No rumble to buzz with
  Xbox pad (in couch) >
    - serial:xbox
---
- Hiding: service · Real-time: yes
---
Settings
Quit tray
";
        assert_eq!(menu, expected, "\n{menu}");
    }

    #[test]
    fn quick_start_can_start_once_two_free_pads_are_chosen() {
        let free = view(None, Hiding::Grab);
        let menu = outline(&menu(&free, &quick()));
        assert!(menu.contains("\n  Start as `quick`\n"), "{menu}");
        assert!(menu.contains("    ( ) Xbox pad\n"), "{menu}");
        // The pad chosen as the other can't be chosen again.
        assert!(
            menu.contains("Primary: DualSense >\n    (o) DualSense\n    ( ) - 8BitDo Pro 2\n"),
            "{menu}"
        );
        // A pad remembered but not connected can be forgotten.
        let mut gone = free.clone();
        gone.pads[2].connected = false;
        assert!(outline(&super::menu(&gone, &quick())).contains("    Forget\n"));
        // Nothing chosen, nothing to start; nothing known, nothing to list.
        let empty = View {
            pads: Vec::new(),
            ..free
        };
        let menu = outline(&super::menu(&empty, &Quick::default()));
        assert!(menu.contains("Primary: not chosen >"), "{menu}");
        assert!(menu.contains("- Start as `quick`"), "{menu}");
        assert!(menu.contains("- No pads are known"), "{menu}");
    }

    #[test]
    fn a_setup_that_failed_and_a_profile_that_is_wrong_say_why() {
        let mut failed = view(Some(Phase::Failed), Hiding::Service);
        failed
            .profiles
            .push(profile("broken", Some("no such preset")));
        let menu = outline(&menu(&failed, &quick()));
        assert!(
            menu.contains("couch (failed) >\n  Stop\n  ---\n  - a virtual pad refused a write\n"),
            "{menu}"
        );
        assert!(
            menu.contains("broken >\n  - Can't run: preset: no such preset\n"),
            "{menu}"
        );
    }

    #[test]
    fn a_setup_whose_profile_is_gone_and_a_node_with_its_own_name_are_still_shown() {
        let mut odd = view(Some(Phase::Running), Hiding::Service);
        odd.profiles.clear();
        odd.setups[0].nodes[0].name = "share".to_string();
        odd.setups[0].sources[0].pad = None;
        odd.setups[0].sinks[0].node = None;
        let menu = outline(&menu(&odd, &quick()));
        assert!(menu.starts_with("couch (running) >\n"), "{menu}");
        assert!(menu.contains("share selects >"), "{menu}");
        assert!(
            menu.contains("- assist: waiting for a pad that fits"),
            "{menu}"
        );
        assert!(menu.contains("- virtual: generic at not made"), "{menu}");
    }

    #[test]
    fn without_the_daemon_there_is_only_what_is_wrong_and_the_way_out() {
        let mut down = view(None, Hiding::Service);
        down.alive = false;
        assert_eq!(
            outline(&menu(&down, &quick())),
            "- The CtrlAssist daemon isn't running\n---\nSettings\nQuit tray\n"
        );
        let mut arriving = view(None, Hiding::Service);
        arriving.hydrated = false;
        assert!(outline(&menu(&arriving, &quick())).starts_with("- Reading from the daemon…\n"));
        let mut other = view(None, Hiding::Service);
        other.mismatch = Some("9.0.0".to_string());
        let menu = outline(&menu(&other, &quick()));
        assert!(
            menu.starts_with("- Version mismatch: daemon 9.0.0, tray "),
            "{menu}"
        );
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
        let menu = outline(&menu(&view, &quick()));
        assert!(menu.contains("    (o) Generic\n    ( ) - Xbox\n"), "{menu}");
    }
}
