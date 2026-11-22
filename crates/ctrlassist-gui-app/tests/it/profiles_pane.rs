//! The Profiles pane: its rows from kept profiles and served fields,
//! what the window holds until a change makes a profile, and the pane
//! in the window.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use ctrlassist::form::{self, Values};
use ctrlassist::{Pad, Phase, Problem, Profile, Setup};
use ctrlassist_gui::profiles::{self, Known};
use ctrlassist_gui::{AppWindow, Pane, ProfileFieldRow, ProfileRow, Profiles, keys, nav};
use slint::{ComponentHandle, Model as _, SharedString};

use crate::support::{activate_shown, find_by_accessible_id, settle};

const COUCH: &str = "preset = \"priority\"\nprimary = \"0c:27:56:4f:bd:14\"\nassist = \"model:054c:09cc\"\nidentity = \"xbox\"\n";
const FULL: &str = include_str!("../../../ctrlassist-protocol/tests/it/fixtures/full.toml");

fn pad(id: &str, name: &str, connected: bool) -> Pad {
    Pad {
        id: id.to_string(),
        name: name.to_string(),
        vendor: 0x054c,
        product: 0x0ce6,
        connected,
        hidden: false,
        setup: None,
        rumble: Some(true),
        bus: connected.then_some(3),
    }
}

fn profile(name: &str, toml: &str) -> Profile {
    Profile {
        name: name.to_string(),
        toml: toml.to_string(),
        problem: None,
    }
}

fn known() -> Known {
    Known {
        profiles: vec![
            profile(
                "quick",
                "preset = \"toggle\"\nprimary = \"address:0c:27:56:4f:bd:14\"\nassist = \"address:1c:96:5a:a1:35:da\"\n",
            ),
            profile("couch", COUCH),
            profile("attic", FULL),
            Profile {
                problem: Some(Problem {
                    at: "preset".into(),
                    message: "this preset needs `primary` and `assist`".into(),
                }),
                ..profile("broken", "preset = \"priority\"\n")
            },
        ],
        setups: vec![Setup {
            name: "quick".to_string(),
            phase: Phase::Running,
            reason: None,
            sources: Vec::new(),
            sinks: Vec::new(),
            nodes: Vec::new(),
            stale: false,
        }],
        pads: vec![
            pad("serial:045e:02dd:7EED821FFB17", "Xbox pad", false),
            pad("address:1c:96:5a:a1:35:da", "Wireless Controller", true),
            pad("address:0c:27:56:4f:bd:14", "DualSense", true),
        ],
        fields: form::fields(),
    }
}

fn row<'a>(rows: &'a [ProfileRow], name: &str) -> &'a ProfileRow {
    rows.iter()
        .find(|row| row.name == name)
        .expect("the profile has a row")
}

fn field(row: &ProfileRow, key: &str) -> ProfileFieldRow {
    row.fields
        .iter()
        .find(|field| field.key == key)
        .expect("the form has the field")
}

#[test]
fn rows_show_a_preset_as_a_form_and_anything_else_as_text() {
    let rows = profiles::rows(&known(), &BTreeMap::new());
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["attic", "broken", "couch", "quick"]);

    let couch = row(&rows, "couch");
    assert_eq!(couch.kind, "preset");
    assert_eq!(couch.summary, "priority · xbox");
    assert!(couch.kept && !couch.unfinished && !couch.running && !couch.is_quick);
    let keys: Vec<String> = couch
        .fields
        .iter()
        .map(|field| field.key.to_string())
        .collect();
    assert_eq!(
        keys,
        [
            "preset",
            "primary",
            "assist",
            "identity",
            "rumble",
            "auto-start"
        ],
        "a preset of two pads has no pad count"
    );
    let preset = field(couch, "preset");
    assert_eq!(
        (preset.kind.as_str(), preset.value_label.as_str()),
        ("choice", "Priority")
    );
    assert!(preset.description.starts_with("Two pads as one."));
    assert_eq!(preset.options.row_count(), 5);
    // A pad named by its bare address is the pad of that address; the
    // dropdown lists connected pads first, absent ones marked.
    let primary = field(couch, "primary");
    assert_eq!(primary.value_label, "DualSense · bd:14");
    assert!(!primary.read_only);
    let options: Vec<String> = primary
        .options
        .iter()
        .map(|option| option.to_string())
        .collect();
    assert_eq!(
        options,
        [
            "DualSense · bd:14",
            "Wireless Controller · 35:da",
            "Xbox pad · FFB17 (not connected)"
        ]
    );
    // Any pad of a model is shown, not picked.
    let assist = field(couch, "assist");
    assert!(assist.read_only);
    assert_eq!(assist.value, "model:054c:09cc");
    // What the profile leaves out stands at its default.
    assert_eq!(field(couch, "rumble").value_label, "Both pads");
    assert!(!field(couch, "auto-start").checked);

    let quick = row(&rows, "quick");
    assert!(quick.is_quick && quick.running);
    assert_eq!(
        (quick.phase_label.as_str(), quick.tone.as_str()),
        ("Running", "success")
    );

    let attic = row(&rows, "attic");
    assert_eq!(
        (attic.kind.as_str(), attic.summary.as_str()),
        ("full", "full form")
    );
    assert_eq!(attic.fields.row_count(), 0);
    assert_eq!(attic.toml, FULL);

    let broken = row(&rows, "broken");
    assert_eq!(
        broken.problem,
        "preset: this preset needs `primary` and `assist`"
    );
}

#[test]
fn what_the_window_holds_shows_as_unfinished_until_it_is_a_profile() {
    let fields = form::fields();
    // A new profile: a preset chosen, no pads.
    let mut held = BTreeMap::new();
    let mut new = Values::new();
    new.insert("preset".into(), "unicast".into());
    new.insert("outputs".into(), "3".into());
    held.insert("den".to_string(), new.clone());
    let rows = profiles::rows(&known(), &held);
    let den = row(&rows, "den");
    assert!(den.unfinished && !den.kept);
    assert_eq!(den.unfinished_why, "preset: this preset needs `pad`");
    let keys: Vec<String> = den
        .fields
        .iter()
        .map(|field| field.key.to_string())
        .collect();
    assert_eq!(keys, ["preset", "pad", "outputs", "identity", "auto-start"]);
    let outputs = field(den, "outputs");
    assert_eq!((outputs.number, outputs.min, outputs.max), (3, 1, 8));
    assert_eq!(field(den, "pad").value_label, "");

    // With its pad picked it is a profile, and no longer unfinished.
    new.insert("pad".into(), "address:0c:27:56:4f:bd:14".into());
    held.insert("den".to_string(), new.clone());
    let rows = profiles::rows(&known(), &held);
    assert!(!row(&rows, "den").unfinished);
    assert!(ctrlassist_protocol::profile::Profile::check(&form::to_toml(&fields, &new)).is_ok());

    // A kept profile with a held change shows the change.
    let mut changed = form::from_toml(&fields, COUCH).unwrap();
    changed.insert("identity".into(), "dualsense".into());
    held.insert("couch".to_string(), changed);
    let rows = profiles::rows(&known(), &held);
    assert_eq!(
        field(row(&rows, "couch"), "identity").value_label,
        "DualSense"
    );
}

fn app() -> AppWindow {
    i_slint_backend_testing::init_no_event_loop();
    profiles::forget_held();
    let app = AppWindow::new().expect("the window builds on the testing backend");
    let handles = nav::wire(&app);
    keys::bind_key_router(&app);
    profiles::bind_local(&app, &handles);
    app.set_daemon_available(true);
    app.set_active_pane(Pane::Profiles);
    // Tall enough for the whole form: what a scroll view clips isn't in
    // the tree the tests search.
    app.window()
        .set_size(slint::LogicalSize::new(1000.0, 1600.0));
    profiles::show(&app, known());
    app
}

#[test]
fn a_new_profile_is_a_draft_that_is_named_checked_and_can_be_dropped() {
    let app = app();
    let pane = app.global::<Profiles>();
    assert_eq!(
        pane.invoke_name_problem("couch".into()),
        "A profile has that name already"
    );
    assert_eq!(
        pane.invoke_name_problem("Not A Name".into()),
        "Up to 32 lowercase letters, digits and hyphens"
    );
    assert_eq!(pane.invoke_name_problem("den".into()), "");

    pane.invoke_create("den".into());
    let rows: Vec<ProfileRow> = pane.get_rows().iter().collect();
    let den = row(&rows, "den");
    assert!(den.unfinished && !den.kept);
    assert_eq!(field(den, "preset").value, "priority");
    // It is selected, to be filled in; and its name is now taken.
    assert_eq!(
        pane.get_rows()
            .row_data(usize::try_from(pane.get_selected_index()).unwrap())
            .unwrap()
            .name,
        "den"
    );
    assert_eq!(
        pane.invoke_name_problem("den".into()),
        "A profile has that name already"
    );

    // What the daemon says next leaves it held: it isn't kept yet.
    profiles::show(&app, known());
    assert!(pane.get_rows().iter().any(|row| row.name == "den"));

    pane.invoke_discard("den".into());
    assert!(!pane.get_rows().iter().any(|row| row.name == "den"));
    assert_eq!(pane.get_selected_index(), -1);
}

#[test]
fn a_held_change_goes_once_the_kept_profile_says_the_same() {
    let app = app();
    let pane = app.global::<Profiles>();
    pane.invoke_create("den".into());
    assert!(
        pane.get_rows()
            .iter()
            .any(|row| row.name == "den" && row.unfinished)
    );
    // The daemon now keeps `den` as the window held it but for its
    // pads, which is another text: still held.
    let mut known_now = known();
    known_now.profiles.push(profile("den", COUCH));
    profiles::show(&app, known_now.clone());
    let rows: Vec<ProfileRow> = pane.get_rows().iter().collect();
    assert!(row(&rows, "den").unfinished && row(&rows, "den").kept);
    // Dropped, the form shows what is kept.
    pane.invoke_discard("den".into());
    let rows: Vec<ProfileRow> = pane.get_rows().iter().collect();
    assert!(!row(&rows, "den").unfinished);
    assert_eq!(field(row(&rows, "den"), "identity").value, "xbox");
}

#[test]
fn the_pane_s_buttons_go_by_what_a_profile_is_and_the_dialogs_name_and_confirm() {
    let app = app();
    let pane = app.global::<Profiles>();
    let asked: Rc<RefCell<Vec<String>>> = Rc::default();
    let log = |what: &'static str| {
        let asked = asked.clone();
        move |name: SharedString| asked.borrow_mut().push(format!("{what} {name}"))
    };
    pane.on_start(log("start"));
    pane.on_delete(log("delete"));
    let copied = asked.clone();
    pane.on_duplicate(move |from, to| copied.borrow_mut().push(format!("duplicate {from} {to}")));
    let set = asked.clone();
    pane.on_set_field(move |name, key, value| {
        set.borrow_mut().push(format!("set {name} {key} {value}"));
    });
    let enabled = |id: &str| {
        find_by_accessible_id(&app, id)
            .and_then(|control| control.accessible_enabled())
            .unwrap_or(false)
    };
    let select = |name: &str| {
        pane.invoke_select(name.into());
        settle();
    };

    // Running: can't be started again, nor renamed from under its setup.
    select("quick");
    assert!(!enabled("profiles-start"));
    assert!(!enabled("profiles-rename"));
    assert!(find_by_accessible_id(&app, "profiles-setup-chip").is_some());

    // Something wrong with it: not startable.
    select("broken");
    assert!(!enabled("profiles-start"));
    assert!(find_by_accessible_id(&app, "profiles-problem").is_some());

    // The full form shows its text and no form.
    select("attic");
    assert!(find_by_accessible_id(&app, "profiles-text").is_some());
    assert!(find_by_accessible_id(&app, "profiles-field-preset").is_none());

    select("couch");
    assert!(enabled("profiles-start"));
    activate_shown(&app, "profiles-start");
    assert_eq!(*asked.borrow(), ["start couch"]);
    assert!(find_by_accessible_id(&app, "profiles-field-preset").is_some());

    // Delete asks first.
    activate_shown(&app, "profiles-delete");
    assert_eq!(asked.borrow().len(), 1);
    activate_shown(&app, "dialog-confirm");
    assert_eq!(asked.borrow().last().unwrap(), "delete couch");

    // Duplicate offers a name that is free, and makes the copy.
    activate_shown(&app, "profiles-duplicate");
    activate_shown(&app, "dialog-confirm");
    assert_eq!(asked.borrow().last().unwrap(), "duplicate couch couch-copy");

    // A switch in the form sets its field at once.
    activate_shown(&app, "profiles-field-auto-start");
    assert_eq!(asked.borrow().last().unwrap(), "set couch auto-start true");
}

/// A dropdown opens with its cursor on the current choice, the second
/// time as the first.
#[test]
fn a_dropdown_opens_on_its_current_choice_every_time() {
    use ctrlassist_gui::Select;
    use slint::platform::Key;
    let app = app();
    let mut known_now = known();
    known_now.profiles.push(profile(
        "den",
        "preset = \"priority\"\nprimary = \"address:0c:27:56:4f:bd:14\"\nassist = \"address:1c:96:5a:a1:35:da\"\nrumble = \"assist\"\n",
    ));
    profiles::show(&app, known_now);
    app.global::<Profiles>().invoke_select("den".into());
    settle();
    // By keys to the dropdown: the sidebar, the list, the detail, then
    // down the form.
    app.show().expect("the testing backend shows a window");
    app.invoke_focus_active_sidebar_item();
    settle();
    let nav = app.global::<ctrlassist_gui::Nav>();
    crate::support::press(&app, Key::RightArrow);
    crate::support::press(&app, Key::RightArrow);
    for _ in 0..12 {
        if nav.get_focus_id() == "profiles-field-rumble" {
            break;
        }
        crate::support::press(&app, Key::DownArrow);
    }
    assert_eq!(nav.get_focus_id(), "profiles-field-rumble");
    let select = app.global::<Select>();
    for time in ["first", "second"] {
        crate::support::press(&app, Key::Return);
        assert!(select.get_open(), "the {time} time");
        assert_eq!(select.get_highlight(), 2, "the {time} time");
        crate::support::press(&app, Key::Escape);
        assert!(!select.get_open());
    }
}
