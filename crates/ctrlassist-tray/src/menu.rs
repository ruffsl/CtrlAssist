//! The tray's menu as plain data, made from a view and the quick-start
//! choices. Nothing here knows the desktop: the tray turns these entries
//! into its toolkit's items, and tests read them as an outline.

use ctrlassist::form::{Field, Kind, PRESET};
use ctrlassist::{Phase, Setup};
use ctrlassist_protocol::rpc;

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
    /// Quick start.
    pub const QUICK: Icon = &["system-run"];
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

/// The name a game sees a virtual pad of `identity` under.
pub(crate) fn virtual_name(view: &View, identity: &str) -> String {
    view.identities
        .iter()
        .find(|known| known.id == identity)
        .map_or_else(|| capitalized(identity), |known| known.name.clone())
}

/// A running setup's entries: what to do with it, then what it is made of.
fn setup_entries(view: &View, setup: &Setup) -> Vec<Entry> {
    let mut entries = vec![action(
        icon::STOP,
        "Stop",
        Command::Stop(setup.name.clone()),
    )];
    if let Some(reason) = &setup.reason {
        entries.push(Entry::Separator);
        entries.push(said(icon::NONE, reason.clone()));
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
            icon::CONTROL,
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
            (Some(pad), false) => format!("{} (disconnected)", view.pad_name(pad)),
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
                virtual_name(view, &sink.identity)
            ),
        ));
    }
    entries
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

/// Whether a pad can be picked in quick start: connected, not picked
/// for another part, and in no setup but quick start's own.
fn pickable(view: &View, id: &str, taken: &[&str]) -> (bool, String) {
    let Some(pad) = view.pad(id) else {
        return (false, id.to_string());
    };
    let name = pad.name.clone();
    if !pad.connected {
        return (false, format!("{name} (disconnected)"));
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
        icon::PAD,
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
    let icon = match field.key.as_str() {
        PRESET => icon::MODE,
        "rumble" => icon::RUMBLE,
        "hide" => icon::HIDE,
        "identity" => icon::IDENTITY,
        _ => icon::NONE,
    };
    sub(icon, format!("{}: {label}", field.label), vec![radio])
}

fn quick_entries(view: &View, quick: &Quick) -> Vec<Entry> {
    // What there is to choose is the daemon's to say.
    if view.fields.is_empty() {
        return vec![said(
            icon::NONE,
            "Waiting for the daemon to say what can be chosen",
        )];
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
        icon: icon::START,
        label: format!("{verb} as `{}`", quick::NAME),
        command: ready.then_some(Command::QuickStart),
    });
    entries
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
                    said(icon::NONE, "No rumble to buzz with")
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

/// The whole menu for what the daemon publishes now and the quick-start
/// choices.
#[must_use]
pub fn menu(view: &View, quick: &Quick) -> Vec<Entry> {
    let mut entries = Vec::new();
    if !view.alive {
        entries.push(said(icon::FAILED, "The CtrlAssist daemon isn't running"));
    } else if let Some(theirs) = &view.mismatch {
        entries.push(said(icon::WRONG, mismatch_text(theirs)));
    } else if !view.hydrated {
        entries.push(said(icon::STARTING, "Reading from the daemon…"));
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
                Some(setup) => {
                    let (word, icon) = phase_label(setup.phase);
                    sub(icon, format!("{name} ({word})"), setup_entries(view, setup))
                }
                None => {
                    let problem = profile.and_then(|profile| profile.problem.as_ref());
                    match problem {
                        Some(problem) => sub(
                            icon::WRONG,
                            format!("{name} (Wrong)"),
                            vec![said(icon::NONE, format!("Can't run: {problem}"))],
                        ),
                        None => sub(
                            icon::STOP,
                            name,
                            vec![action(
                                icon::START,
                                "Start",
                                Command::Start(name.to_string()),
                            )],
                        ),
                    }
                }
            });
        }
        if !entries.is_empty() {
            entries.push(Entry::Separator);
        }
        entries.push(sub(icon::QUICK, "Quick start", quick_entries(view, quick)));
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
                    daemon.hiding.as_str(),
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
                Entry::Item { label, command, .. } => {
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
couch (Held) >
  Stop
  ---
  Has control >
    (o) primary
    ( ) - assist
  ---
  - Assist: Xbox pad (disconnected)
  - Primary: DualSense
  - Virtual: Generic
desk >
  Start
---
Quick start >
  Mode: Priority >
    (o) Priority
    ( ) Average
    ( ) Toggle
  Primary: DualSense >
    (o) - DualSense (in couch)
    ( ) - 8BitDo Pro 2
    ( ) - Xbox pad (disconnected)
  Assist: 8BitDo Pro 2 >
    ( ) - DualSense (in couch)
    (o) 8BitDo Pro 2
    ( ) - Xbox pad (disconnected)
  Identity: Generic >
    (o) Generic
    ( ) Xbox
    ( ) DualSense
  Rumble goes to: Both pads >
    (o) Both pads
    ( ) Primary only
    ( ) Assist only
    ( ) Neither
  Real pads: Hidden >
    (o) Hidden
    ( ) Grabbed
    ( ) Visible
  [ ] Managed
  ---
  - Start as `quick`
Pads (2/3 connected) >
  DualSense (in couch) >
    - address:dualsense
    Identify (buzz)
  8BitDo Pro 2 >
    - address:8bitdo
    - No rumble to buzz with
  Xbox pad (in couch) >
    - serial:xbox
  ---
  Generic (in couch) >
    - /dev/input/event31
    Identify (buzz)
---
- Hiding: service · Real-time: yes
---
Settings
Quit tray
";
        assert_eq!(menu, expected, "\n{menu}");
    }

    /// A row's icon is a name in the desktop's theme, as plain data: a
    /// profile by how it is, a pad, an action by what it does. A virtual
    /// pad is buzzed by its own name.
    #[test]
    fn rows_carry_their_icons_and_a_virtual_pad_is_buzzed_by_its_name() {
        let entries = menu(&view(Some(Phase::Held), Hiding::Service), &quick());
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
                ("couch (Held)", icon::HELD),
                ("desk", icon::STOP),
                ("Quick start", icon::QUICK),
                ("Pads (2/3 connected)", icon::PAD),
                ("Hiding: service · Real-time: yes", icon::STATUS),
                ("Settings", icon::SETTINGS),
                ("Quit tray", icon::QUIT),
            ]
        );
        let Some(Entry::Sub { entries: pads, .. }) = entries.get(4) else {
            unreachable!("the pads follow quick start");
        };
        let Some(Entry::Sub { entries: made, .. }) = pads.last() else {
            unreachable!("the virtual pad is last");
        };
        assert_eq!(
            made[1],
            Entry::Item {
                icon: icon::RUMBLE,
                label: "Identify (buzz)".to_string(),
                command: Some(Command::Identify("virtual:couch/virtual".to_string())),
            }
        );
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
            menu.contains("couch (Failed) >\n  Stop\n  ---\n  - a virtual pad refused a write\n"),
            "{menu}"
        );
        assert!(
            menu.contains("broken (Wrong) >\n  - Can't run: mode: no such preset\n"),
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
        assert!(menu.starts_with("couch (Running) >\n"), "{menu}");
        assert!(menu.contains("share selects >"), "{menu}");
        assert!(
            menu.contains("- Assist: waiting for a pad that fits"),
            "{menu}"
        );
        assert!(menu.contains("- Virtual: Generic (not made)"), "{menu}");
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
