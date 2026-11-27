//! The Profiles pane: its rows from kept profiles and served fields,
//! what the window holds until a change makes a profile, and the pane
//! in the window.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use ctrlassist::form::{self, Values};
use ctrlassist::{Pad, Phase, Problem, Profile, Setup, SetupNode, SetupSink, SetupSource};
use ctrlassist_gui::profiles::{self, Known};
use ctrlassist_gui::{
    AppWindow, Pane, ProfileFieldRow, ProfileRow, Profiles, Setups, keys, nav, setups,
};
use slint::{ComponentHandle, Model as _, SharedString};

use crate::support::{activate_shown, find_by_accessible_id, settle};

const COUCH: &str = "mode = \"priority\"\nprimary = \"0a:1b:2c:3d:4e:14\"\nassist = \"model:054c:09cc\"\nidentity = \"xbox\"\nhide = \"grab\"\n";
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
        unset: Vec::new(),
        problem: None,
    }
}

fn known() -> Known {
    Known {
        profiles: vec![
            profile(
                "quick",
                "mode = \"toggle\"\nprimary = \"address:0a:1b:2c:3d:4e:14\"\nassist = \"address:1a:2b:3c:4d:5e:da\"\n",
            ),
            profile("couch", COUCH),
            profile("attic", FULL),
            Profile {
                unset: Vec::new(),
                problem: Some(Problem {
                    at: "mode".into(),
                    message: "no such preset".into(),
                }),
                ..profile("broken", "mode = \"sideways\"\n")
            },
            // As a daemon makes it: its pads still to choose.
            Profile {
                unset: vec!["primary".to_string(), "assist".to_string()],
                ..profile("default", ctrlassist_protocol::profile::DEFAULT_TOML)
            },
        ],
        setups: vec![Setup {
            name: "quick".to_string(),
            start: 1,
            phase: Phase::Running,
            reason: None,
            sources: vec![
                SetupSource {
                    name: "assist".to_string(),
                    pad: Some("address:1a:2b:3c:4d:5e:da".to_string()),
                    present: true,
                },
                SetupSource {
                    name: "primary".to_string(),
                    pad: Some("address:0a:1b:2c:3d:4e:14".to_string()),
                    present: true,
                },
            ],
            sinks: vec![SetupSink {
                name: "virtual".to_string(),
                identity: "generic".to_string(),
                node: Some("/dev/input/event31".to_string()),
            }],
            nodes: vec![SetupNode {
                name: "mux".to_string(),
                choices: vec!["primary".to_string(), "assist".to_string()],
                selected: 0,
            }],
            // Its profile has changed since it started.
            stale: true,
        }],
        pads: vec![
            pad("serial:045e:02dd:7EED821FFB17", "Xbox pad", false),
            pad("address:1a:2b:3c:4d:5e:da", "Wireless Controller", true),
            pad("address:0a:1b:2c:3d:4e:14", "DualSense", true),
        ],
        fields: form::fields(),
    }
}

fn row<'a>(rows: &'a [ProfileRow], name: &str) -> &'a ProfileRow {
    rows.iter()
        .find(|row| row.name == name)
        .expect("the profile has a row")
}

/// Every field of a row's form, in whichever card it is shown.
fn fields(row: &ProfileRow) -> Vec<ProfileFieldRow> {
    let mut all: Vec<ProfileFieldRow> = row.input_fields.iter().collect();
    if !row.identity_field.key.is_empty() {
        all.push(row.identity_field.clone());
    }
    all.extend(row.output_fields.iter());
    all.extend(row.preset_fields.iter());
    all
}

/// The keys of a row's fields, sorted.
fn keys(row: &ProfileRow) -> Vec<String> {
    let mut keys: Vec<String> = fields(row)
        .iter()
        .map(|field| field.key.to_string())
        .collect();
    keys.sort();
    keys
}

fn field(row: &ProfileRow, key: &str) -> ProfileFieldRow {
    fields(row)
        .into_iter()
        .find(|field| field.key == key)
        .expect("the form has the field")
}

#[test]
fn rows_show_a_preset_as_a_form_and_anything_else_as_text() {
    let rows = profiles::rows(&known(), &BTreeMap::new());
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    // The one that runs first; then the default profile, and the rest
    // by name.
    assert_eq!(names, ["quick", "default", "attic", "broken", "couch"]);
    // The default profile as a daemon makes it: a preset with its pads
    // still to choose, which isn't wrong and can't start.
    let default = row(&rows, "default");
    assert!(default.is_default && default.kept && !default.unfinished);
    assert_eq!(
        (default.word.as_str(), default.word_tone.as_str()),
        ("Not set up", "neutral")
    );
    assert_eq!(default.note, "Choose its pads to start it.");
    assert!(!default.can_start && default.problem.is_empty());
    assert_eq!(default.kind, "preset");

    let couch = row(&rows, "couch");
    assert_eq!(couch.kind, "preset");
    assert!(couch.kept && !couch.unfinished && couch.setup_index < 0 && !couch.is_default);
    assert_eq!(
        keys(couch),
        [
            "assist",
            "assist-gyro",
            "hide",
            "identity",
            "mode",
            "primary",
            "rumble"
        ],
        "a preset of two pads has no pad count"
    );
    let preset = field(couch, "mode");
    assert_eq!(
        (preset.kind.as_str(), preset.value_label.as_str()),
        ("choice", "Priority")
    );
    assert_eq!(preset.options.row_count(), 5);
    // A pad named by its bare address is the pad of that address; the
    // dropdown lists connected pads first, absent ones marked.
    let primary = field(couch, "primary");
    assert_eq!(primary.value_label, "DualSense · 4e:14");
    assert!(!primary.read_only);
    let options: Vec<String> = primary
        .options
        .iter()
        .map(|option| option.to_string())
        .collect();
    assert_eq!(
        options,
        [
            "DualSense · 4e:14",
            "Wireless Controller · 5e:da",
            "Xbox pad · FFB17 (not connected)"
        ]
    );
    // Any pad of a model is said as that, and another can be picked.
    let assist = field(couch, "assist");
    assert!(!assist.read_only);
    assert_eq!(assist.value, "model:054c:09cc");
    assert_eq!(assist.value_label, "Any pad of model 054c:09cc");
    assert_eq!(assist.options.row_count(), 3);
    // So can one in place of a pad the daemon doesn't know, as one
    // that was forgotten.
    let mut gone = known();
    gone.pads
        .retain(|pad| pad.id != "address:0a:1b:2c:3d:4e:14");
    let rows_gone = profiles::rows(&gone, &BTreeMap::new());
    let primary = field(row(&rows_gone, "couch"), "primary");
    assert!(!primary.read_only);
    assert_eq!(primary.value_label, "4e:14 (not known)");
    assert_eq!(primary.status_word, "Not known");
    // What the profile leaves out stands at its default.
    assert_eq!(field(couch, "rumble").value_label, "Both pads");
    // Whether it is managed is the row's own, not a field of the form.
    assert!(!couch.managed);

    // Each field says which card it is shown in, and a pad's field how
    // the pad chosen is: its connection, and its id for the pencil.
    let card = |key: &str| field(couch, key).group.to_string();
    assert_eq!(
        [
            card("mode"),
            card("primary"),
            card("identity"),
            card("hide")
        ],
        ["preset", "inputs", "outputs", "preset"]
    );
    let primary = field(couch, "primary");
    assert_eq!(primary.pad, "address:0a:1b:2c:3d:4e:14");
    assert_eq!(
        (primary.status_word.as_str(), primary.status_tone.as_str()),
        ("Connected", "success")
    );
    // A pad the profile that runs has is that profile's, to any other.
    let mut taken = known();
    taken.pads[2].setup = Some("quick".to_string());
    let taken = profiles::rows(&taken, &BTreeMap::new());
    let elsewhere = field(row(&taken, "couch"), "primary");
    assert_eq!(
        (
            elsewhere.status_word.as_str(),
            elsewhere.status_tone.as_str()
        ),
        ("In quick", "warn")
    );
    assert_eq!(
        field(row(&taken, "quick"), "primary").status_word,
        "Connected"
    );
    // The assist of the profile that runs is connected too.
    assert_eq!(
        field(row(&rows, "quick"), "assist").status_word,
        "Connected"
    );
    // Any pad of a model the daemon knows none of.
    let assist = field(couch, "assist");
    assert_eq!(
        (assist.pad.as_str(), assist.status_word.as_str()),
        ("", "Not known")
    );

    // Its virtual pad, and where it is while it runs.
    let outputs: Vec<String> = row(&rows, "quick")
        .outputs
        .iter()
        .map(|output| format!("{} {} {}", output.name, output.identity, output.node))
        .collect();
    assert_eq!(outputs, ["Virtual Generic event31"]);
    // It exists while its profile runs, and its row opens it in Pads;
    // the first virtual pad's row carries the identity.
    let made = row(&rows, "quick").outputs.row_data(0).unwrap();
    assert_eq!(
        (made.word.as_str(), made.pad.as_str()),
        ("Connected", "virtual:quick/virtual")
    );
    let idle = couch.outputs.row_data(0).unwrap();
    assert_eq!(
        (idle.word.as_str(), idle.pad.as_str()),
        ("Disconnected", "")
    );
    assert_eq!(couch.identity_field.key, "identity");
    assert_eq!(couch.output_fields.row_count(), 0);

    // How each is, in a word, for the list and the chip.
    let said = |name: &str| {
        let row = row(&rows, name);
        format!("{} {}", row.word, row.word_tone)
    };
    assert_eq!(said("quick"), "Running success");
    assert_eq!(said("broken"), "Wrong danger");
    assert_eq!(said("couch"), "Waiting neutral");
    assert!(row(&rows, "couch").can_start && !row(&rows, "broken").can_start);
    assert_eq!(row(&rows, "couch").shape, "merge");
    // A field that has a default says whether it stands at it.
    assert!(field(couch, "rumble").at_default);
    let identity = field(couch, "identity");
    assert!(!identity.at_default);
    assert_eq!(identity.default_value, "generic");
    assert_eq!(field(couch, "primary").default_value, "");

    let quick = row(&rows, "quick");
    assert_eq!(quick.setup_index, 0);
    assert_eq!(row(&rows, "couch").setup_index, -1);
    assert_eq!(quick.note, "Games see the generic pad at event31.");
    // The pads it names, as its setup has them, for the header's links.
    let uses: Vec<String> = quick
        .uses
        .iter()
        .map(|used| format!("{} {} {}", used.part, used.name, used.pad))
        .collect();
    assert_eq!(
        uses,
        [
            "Assist Wireless Controller address:1a:2b:3c:4d:5e:da",
            "Primary DualSense address:0a:1b:2c:3d:4e:14"
        ]
    );
    // Any pad of a model the daemon knows none of goes by what the
    // profile calls it, and links nowhere.
    let couch_uses: Vec<String> = row(&rows, "couch")
        .uses
        .iter()
        .map(|used| format!("{} {} [{}]", used.part, used.name, used.pad))
        .collect();
    assert_eq!(
        couch_uses,
        [
            "Assist model:054c:09cc []",
            "Primary DualSense [address:0a:1b:2c:3d:4e:14]"
        ]
    );
    let attic = row(&rows, "attic");
    assert_eq!(attic.kind, "full");
    assert!(fields(attic).is_empty());
    // Written by hand: a shape for its tile and how it is, and nothing
    // of its text.
    assert_eq!(attic.shape, "merge", "it names more than one pad");
    assert_eq!(attic.word, "Waiting");
    assert_eq!(attic.mode_label, "Custom");
    // What it names is listed, each pad with its connection.
    assert_eq!(attic.uses.row_count(), 3);
    assert!(attic.uses.iter().all(|used| !used.word.is_empty()));
    assert_eq!(attic.outputs.row_count(), 2);

    let broken = row(&rows, "broken");
    assert_eq!(broken.problem, "mode: no such preset");
}

#[test]
fn what_the_window_holds_shows_as_unfinished_until_it_is_a_profile() {
    let fields = form::fields();
    // A new profile: a preset chosen, no pads.
    let mut held = BTreeMap::new();
    let mut new = Values::new();
    new.insert("mode".into(), "unicast".into());
    new.insert("outputs".into(), "3".into());
    held.insert("den".to_string(), new.clone());
    let rows = profiles::rows(&known(), &held);
    let den = row(&rows, "den");
    assert!(den.unfinished && !den.kept);
    assert_eq!(den.unfinished_why, "mode: this preset needs `pad`");
    assert_eq!(keys(den), ["hide", "identity", "mode", "outputs", "pad"]);
    let outputs = field(den, "outputs");
    assert_eq!((outputs.number, outputs.min, outputs.max), (3, 1, 8));
    assert_eq!(field(den, "pad").value_label, "");

    // With its pad picked it is a profile, and no longer unfinished.
    new.insert("pad".into(), "address:0a:1b:2c:3d:4e:14".into());
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
    let known = known();
    let kept: Vec<String> = known
        .profiles
        .iter()
        .map(|profile| profile.name.clone())
        .collect();
    setups::show(&app, setups::rows(&known.setups, &kept));
    profiles::show(&app, known);
    app
}

/// A profile's name too long for the header at the window's least width
/// is one line cut short, as a pad's title is, and leaves the header's
/// buttons their room: it slides to show its end under the pointer.
#[test]
fn a_long_name_is_one_line_cut_short_in_the_header_at_the_least_width() {
    const LONG: &str = "abcdefghijklmnopqrstuvwxyz-01234";
    let app = app();
    let mut known_now = known();
    known_now.profiles.push(profile(LONG, COUCH));
    profiles::show(&app, known_now);
    app.window()
        .set_size(slint::LogicalSize::new(720.0, 1600.0));
    let shown = |app: &AppWindow, name: &str| {
        app.global::<Profiles>().invoke_select(name.into());
        settle();
        let shown = find_by_accessible_id(app, "profiles-name").expect("the header's name");
        assert_eq!(shown.accessible_label().unwrap_or_default(), name);
        (shown.size(), shown.absolute_position().x)
    };
    let (short, _) = shown(&app, "couch");
    let (long, left) = shown(&app, LONG);
    assert!(
        (long.height - short.height).abs() < 0.5,
        "one line: {} against {}",
        long.height,
        short.height
    );
    assert!(
        long.width >= short.width,
        "it has no less room than a short one"
    );
    // Short of the window's edge by the chips beside it.
    assert!(left + long.width < 720.0 - 60.0, "{left} + {}", long.width);

    // The header keeps to the window at its least width: its chips end
    // inside it, the buttons beside the mode end short of the chips, and
    // the mode has room left.
    let mode_width = |app: &AppWindow| {
        find_by_accessible_id(app, "profiles-mode")
            .expect("the header's mode")
            .size()
            .width
    };
    // Where the buttons start, for each profile shown.
    let mut starts = Vec::new();
    app.window().set_size(slint::LogicalSize::new(720.0, 480.0));
    for name in ["couch", LONG] {
        app.global::<Profiles>().invoke_select(name.into());
        settle();
        let edges = |id: &str| {
            let found = find_by_accessible_id(&app, id).unwrap_or_else(|| panic!("{id}"));
            let left = found.absolute_position().x;
            (left, left + found.size().width)
        };
        let (chip_left, chip_right) = edges("profiles-managed-chip");
        let (_, word_right) = edges("profiles-word");
        let (_, delete_right) = edges("profiles-delete");
        assert!(
            chip_right <= 720.0 && word_right <= 720.0,
            "{name}: {chip_right}, {word_right}"
        );
        assert!(
            delete_right <= chip_left,
            "{name}: {delete_right} against {chip_left}"
        );
        let mode = find_by_accessible_id(&app, "profiles-mode").unwrap();
        assert_eq!(
            mode.accessible_label().unwrap_or_default(),
            "Mode: Priority"
        );
        assert!(mode_width(&app) > 0.0, "{name}: the mode has no room");
        starts.push(edges("profiles-rename").0);
    }
    // One written by hand says Custom, a word of another length, and
    // the buttons are where they were: they keep to the row's far end.
    app.global::<Profiles>().invoke_select("attic".into());
    settle();
    let custom = find_by_accessible_id(&app, "profiles-mode").unwrap();
    assert_eq!(
        custom.accessible_label().unwrap_or_default(),
        "Mode: Custom"
    );
    let rename = find_by_accessible_id(&app, "profiles-rename").unwrap();
    starts.push(rename.absolute_position().x);
    assert!(
        starts.iter().all(|start| (start - starts[0]).abs() < 0.5),
        "{starts:?}"
    );
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
    assert_eq!(field(den, "mode").value, "priority");
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

    // Running: its one button is Stop, which asks first; it can't be
    // renamed from under its setup; and what its setup has is here: an
    // update to take, its pads, who has control.
    let running = app.global::<Setups>();
    running.on_stop(log("stop"));
    running.on_ask_update(log("ask-update"));
    let handed = asked.clone();
    running.on_select(move |name, node, choice| {
        handed
            .borrow_mut()
            .push(format!("select {name} {node} {choice}"));
    });
    let opened = asked.clone();
    pane.on_open_pad(move |id| opened.borrow_mut().push(format!("pad {id}")));
    select("quick");
    assert!(find_by_accessible_id(&app, "profiles-start").is_none());
    assert!(enabled("profiles-stop"));
    assert!(!enabled("profiles-rename"));
    // Stop is done as pressed, with no dialog: Start undoes it.
    activate_shown(&app, "profiles-stop");
    assert!(find_by_accessible_id(&app, "dialog-confirm").is_none());
    activate_shown(&app, "profiles-update");
    activate_shown(&app, "profiles-pad-assist");
    activate_shown(&app, "profiles-pad-primary");
    assert!(find_by_accessible_id(&app, "profiles-select-mux").is_some());
    assert_eq!(
        *asked.borrow(),
        [
            "stop quick",
            "ask-update quick",
            "pad address:1a:2b:3c:4d:5e:da",
            "pad address:0a:1b:2c:3d:4e:14"
        ]
    );
    asked.borrow_mut().clear();
    // What the update costs is said before it is done.
    running.on_update(log("update"));
    running.set_pending_update_cost("restart".into());
    running.set_pending_update("quick".into());
    settle();
    activate_shown(&app, "dialog-confirm");
    assert_eq!(*asked.borrow(), ["update quick"]);
    assert_eq!(running.get_pending_update(), "");
    asked.borrow_mut().clear();

    // Something wrong with it: not startable, and it says what.
    select("broken");
    assert!(!enabled("profiles-start"));
    assert!(find_by_accessible_id(&app, "profiles-problem").is_some());

    // Written by hand: no form, and none of its text.
    select("attic");
    assert!(find_by_accessible_id(&app, "profiles-by-hand").is_some());
    assert!(find_by_accessible_id(&app, "profiles-field-mode").is_none());
    assert!(find_by_accessible_id(&app, "profiles-update").is_none());

    // The default profile keeps its name and is always kept; its row
    // carries its mark.
    select("default");
    assert!(!enabled("profiles-rename") && !enabled("profiles-delete"));
    assert!(!enabled("profiles-start"));
    assert!(enabled("profiles-duplicate"));
    let marked = find_by_accessible_id(&app, "profiles-row-default")
        .and_then(|row| row.accessible_label())
        .unwrap_or_default();
    assert_eq!(marked, "default, default profile");
    assert!(find_by_accessible_id(&app, "profiles-field-primary").is_some());

    select("couch");
    assert!(enabled("profiles-rename") && enabled("profiles-delete"));
    assert!(enabled("profiles-start"));
    activate_shown(&app, "profiles-start");
    assert_eq!(*asked.borrow(), ["start couch"]);
    assert!(find_by_accessible_id(&app, "profiles-field-mode").is_some());
    for id in [
        "profiles-help-status",
        "profiles-help-inputs",
        "profiles-help-outputs",
    ] {
        assert!(find_by_accessible_id(&app, id).is_some(), "{id}");
    }

    // Delete asks first.
    activate_shown(&app, "profiles-delete");
    assert_eq!(asked.borrow().len(), 1);
    activate_shown(&app, "dialog-confirm");
    assert_eq!(asked.borrow().last().unwrap(), "delete couch");

    // Duplicate offers a name that is free, and makes the copy.
    activate_shown(&app, "profiles-duplicate");
    activate_shown(&app, "dialog-confirm");
    assert_eq!(asked.borrow().last().unwrap(), "duplicate couch couch-copy");

    // The Managed switch, in the STATUS group, sets its field at once.
    activate_shown(&app, "profiles-managed");
    assert_eq!(asked.borrow().last().unwrap(), "set couch managed true");

    // A field that isn't at its default can be put back; one that is
    // has nothing to reset.
    let reset = asked.clone();
    pane.on_reset_field(move |name, key| reset.borrow_mut().push(format!("reset {name} {key}")));
    assert!(enabled("profiles-reset-hide"));
    assert!(!enabled("profiles-reset-rumble"));
    assert!(find_by_accessible_id(&app, "profiles-reset-primary").is_none());
    activate_shown(&app, "profiles-reset-hide");
    assert_eq!(asked.borrow().last().unwrap(), "reset couch hide");
    // The identity is the virtual pad's row's, which ends in the one
    // slot an input's row has: it has no reset.
    assert!(find_by_accessible_id(&app, "profiles-field-identity").is_some());
    assert!(find_by_accessible_id(&app, "profiles-reset-identity").is_none());
    // The header says the mode, not the pads.
    let mode = find_by_accessible_id(&app, "profiles-mode")
        .and_then(|text| text.accessible_label())
        .unwrap_or_default();
    assert_eq!(mode, "Mode: Priority");
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
        "mode = \"priority\"\nprimary = \"address:0a:1b:2c:3d:4e:14\"\nassist = \"address:1a:2b:3c:4d:5e:da\"\nrumble = \"assist\"\n",
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

/// A profile's pad and a pad's profile are a click apart, and back
/// returns to each in turn: what the Setups pane's links did, between
/// the two panes that are left.
#[test]
fn a_profile_s_pad_and_a_pad_s_profile_are_a_click_apart_and_back_returns() {
    use ctrlassist_gui::{Pads, pads};
    use slint::platform::Key;

    use crate::support::press_with_alt;

    i_slint_backend_testing::init_no_event_loop();
    profiles::forget_held();
    let app = AppWindow::new().expect("the window builds on the testing backend");
    let handles = nav::wire(&app);
    keys::bind_key_router(&app);
    profiles::bind_local(&app, &handles);
    pads::bind_links(&app, &handles);
    app.set_daemon_available(true);
    app.window()
        .set_size(slint::LogicalSize::new(1000.0, 1600.0));
    let known = known();
    let kept: Vec<String> = known
        .profiles
        .iter()
        .map(|profile| profile.name.clone())
        .collect();
    setups::show(&app, setups::rows(&known.setups, &kept));
    pads::show(
        &app,
        pads::rows(&known.pads, &known.setups, &known.profiles, &[]),
    );
    profiles::show(&app, known);
    let pads_pane = app.global::<Pads>();
    let profiles_pane = app.global::<Profiles>();
    let pad_shown = || {
        pads_pane
            .get_rows()
            .row_data(usize::try_from(pads_pane.get_selected_index()).unwrap())
            .unwrap()
            .name
    };
    let profile_shown = || {
        profiles_pane
            .get_rows()
            .row_data(usize::try_from(profiles_pane.get_selected_index()).unwrap())
            .unwrap()
            .name
    };

    // From the DualSense's detail to the profile running with it.
    pads_pane.set_selected_index(0);
    settle();
    assert_eq!(pad_shown(), "DualSense");
    activate_shown(&app, "pads-profile-quick");
    assert_eq!(app.get_active_pane(), Pane::Profiles);
    assert_eq!(profile_shown(), "quick");

    // From that profile's header to its assist.
    activate_shown(&app, "profiles-pad-assist");
    assert_eq!(app.get_active_pane(), Pane::Pads);
    assert_eq!(pad_shown(), "Wireless Controller");

    // Back, to the profile; back again, to the pad first looked at.
    press_with_alt(&app, Key::LeftArrow);
    assert_eq!(app.get_active_pane(), Pane::Profiles);
    assert_eq!(profile_shown(), "quick");
    // Focus is put back where it was on that page a tick later; a key
    // before then would find nothing focused.
    for _ in 0..4 {
        settle();
    }
    press_with_alt(&app, Key::LeftArrow);
    assert_eq!(app.get_active_pane(), Pane::Pads);
    assert_eq!(pad_shown(), "DualSense");
}

/// The cases the tray's picks and the window's both meet, kept with the
/// tray's.
const SWAP_CASES: &str = include_str!("../../../ctrlassist-tray/src/swap_cases.txt");

/// Each case of the tray's `swap_cases.txt`, run through the window's
/// own rule for what a pick moves.
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
        let pads: Vec<Pad> = given["pads"]
            .split(' ')
            .map(|entry| {
                let (id, rest) = entry.split_once('=').unwrap();
                let (rest, setup) = rest
                    .split_once('@')
                    .map_or((rest, None), |(rest, setup)| (rest, Some(setup)));
                let connected = !rest.ends_with('~');
                let (vendor, product) = rest.trim_end_matches('~').split_once(':').unwrap();
                Pad {
                    vendor: u16::from_str_radix(vendor, 16).unwrap(),
                    product: u16::from_str_radix(product, 16).unwrap(),
                    setup: setup.map(str::to_string),
                    ..pad(id, id, connected)
                }
            })
            .collect();
        let pairs = |line: &str| -> Vec<(String, String)> {
            line.split(' ')
                .filter(|pair| !pair.is_empty())
                .map(|pair| {
                    let (key, value) = pair.split_once('=').unwrap();
                    (key.to_string(), value.to_string())
                })
                .collect()
        };
        let mut toml = "mode = \"toggle\"\n".to_string();
        for (key, value) in pairs(given["before"]) {
            toml.push_str(&format!("{key} = \"{value}\"\n"));
        }
        let setups = given
            .get("running")
            .map(|running| Setup {
                name: "couch".to_string(),
                start: 1,
                phase: Phase::Running,
                reason: None,
                sources: pairs(running)
                    .into_iter()
                    .map(|(name, pad)| SetupSource {
                        name,
                        pad: Some(pad),
                        present: true,
                    })
                    .collect(),
                sinks: Vec::new(),
                nodes: Vec::new(),
                stale: false,
            })
            .into_iter()
            .collect();
        let known = Known {
            profiles: vec![profile("couch", &toml)],
            setups,
            pads,
            fields: form::fields(),
        };
        let mut values = form::from_toml(&known.fields, &toml).unwrap();
        let (key, value) = pairs(given["pick"]).remove(0);
        profiles::change_places(&known, "couch", &mut values, &key, &value);
        values.insert(key, value);
        for (key, value) in pairs(given["after"]) {
            let held = values.get(&key).map_or("-", String::as_str);
            assert_eq!(held, value, "{title}: {key}");
        }
        ran += 1;
    }
    assert_eq!(ran, 12, "every case was read");
}

/// Picking the pad another part has makes the two change places, so two
/// pads can change parts without the profile ever naming one twice.
#[test]
fn picking_the_other_part_s_pad_changes_the_two_places() {
    let known = known();
    let two = "mode = \"toggle\"\nprimary = \"address:0a:1b:2c:3d:4e:14\"\nassist = \"address:1a:2b:3c:4d:5e:da\"\n";
    let mut values = form::from_toml(&known.fields, two).unwrap();
    let (primary, assist) = (values["primary"].clone(), values["assist"].clone());
    profiles::change_places(&known, "den", &mut values, "primary", &assist);
    assert_eq!(values["assist"], primary);
    // Nothing changes places for a pad no other part has, a pad that
    // isn't known, or a field that isn't a pad.
    let before = values.clone();
    profiles::change_places(
        &known,
        "den",
        &mut values,
        "primary",
        "serial:045e:02dd:7EED821FFB17",
    );
    profiles::change_places(
        &known,
        "den",
        &mut values,
        "primary",
        "address:00:00:00:00:00:00",
    );
    profiles::change_places(&known, "den", &mut values, "identity", "xbox");
    assert_eq!(values, before);
    // A part with no pad yet takes the other's, which is left to choose.
    let mut one = form::from_toml(
        &known.fields,
        "mode = \"priority\"\nassist = \"address:0a:1b:2c:3d:4e:14\"\n",
    )
    .unwrap();
    profiles::change_places(
        &known,
        "den",
        &mut one,
        "primary",
        "address:0a:1b:2c:3d:4e:14",
    );
    assert!(!one.contains_key("assist"));
}

/// By keys from the sidebar to the control of that id in the form of
/// the profile selected; the window shown, as keys need.
fn focus_by_keys(app: &AppWindow, id: &str) {
    use slint::platform::Key;
    app.show().expect("the testing backend shows a window");
    app.invoke_focus_active_sidebar_item();
    settle();
    let nav = app.global::<ctrlassist_gui::Nav>();
    crate::support::press(app, Key::RightArrow);
    crate::support::press(app, Key::RightArrow);
    for _ in 0..16 {
        if nav.get_focus_id() == id {
            break;
        }
        crate::support::press(app, Key::DownArrow);
    }
    assert_eq!(nav.get_focus_id(), id);
}

/// Whatever the window is asked to set, as `name key value`.
fn record_set_field(app: &AppWindow) -> Rc<RefCell<Vec<String>>> {
    let asked: Rc<RefCell<Vec<String>>> = Rc::default();
    let set = asked.clone();
    app.global::<Profiles>()
        .on_set_field(move |name, key, value| {
            set.borrow_mut().push(format!("{name} {key} {value}"));
        });
    asked
}

/// The Identity dropdown sits in the first virtual pad's row. A change
/// the daemon reports while it is open, or after a pick, keeps that row
/// and so the dropdown: the list stays its own, a pick from it lands,
/// and the keys go on from it.
#[test]
fn a_dropdown_in_a_virtual_pad_s_row_lives_through_a_daemon_change() {
    use ctrlassist_gui::Select;
    use slint::platform::Key;
    let app = app();
    let asked = record_set_field(&app);
    app.global::<Profiles>().invoke_select("couch".into());
    settle();
    let outputs = |app: &AppWindow| {
        let rows: Vec<ProfileRow> = app.global::<Profiles>().get_rows().iter().collect();
        row(&rows, "couch").outputs.clone()
    };
    let before = outputs(&app);
    focus_by_keys(&app, "profiles-field-identity");
    crate::support::press(&app, Key::Return);
    let select = app.global::<Select>();
    assert!(select.get_open());
    // A pad connects, or anything else the daemon says.
    profiles::show(&app, known());
    settle();
    assert!(before == outputs(&app), "the rows are the same ones");
    crate::support::press(&app, Key::DownArrow);
    crate::support::press(&app, Key::Return);
    assert!(!select.get_open());
    assert_eq!(*asked.borrow(), ["couch identity dualsense"]);
    // Again after the pick: focus is still on the dropdown.
    profiles::show(&app, known());
    settle();
    crate::support::press(&app, Key::Return);
    assert!(select.get_open(), "the dropdown kept focus");
    crate::support::press(&app, Key::Escape);
}

/// A dropdown made anew while its list is open, its profile's detail
/// rebuilt, still takes the pick: it is routed by the control's key, the
/// same for the one made in its place, not by the button that opened it.
#[test]
fn a_pick_lands_on_a_dropdown_made_anew_while_its_list_was_open() {
    use ctrlassist_gui::Select;
    use slint::platform::Key;
    let app = app();
    let asked = record_set_field(&app);
    app.global::<Profiles>().invoke_select("couch".into());
    settle();
    focus_by_keys(&app, "profiles-field-identity");
    crate::support::press(&app, Key::Return);
    let select = app.global::<Select>();
    assert!(select.get_open());
    // The profile goes and comes back: its row and its detail, and the
    // dropdown in it, are made anew.
    let mut without = known();
    without.profiles.retain(|profile| profile.name != "couch");
    profiles::show(&app, without);
    settle();
    profiles::show(&app, known());
    app.global::<Profiles>().invoke_select("couch".into());
    settle();
    assert!(select.get_open());
    let values: Vec<String> = select
        .get_values()
        .iter()
        .map(|value| value.to_string())
        .collect();
    let at = values
        .iter()
        .position(|value| value == "dualsense")
        .expect("an identity to pick");
    select.invoke_pick(i32::try_from(at).unwrap());
    settle();
    assert_eq!(*asked.borrow(), ["couch identity dualsense"]);
}

/// A pad list puts connected pads first, so it may change order while
/// it is open; the pick is the pad that was picked, not the one now in
/// its place.
#[test]
fn a_pick_is_the_pad_shown_when_the_list_opened() {
    use ctrlassist_gui::Select;
    use slint::platform::Key;
    let app = app();
    let asked = record_set_field(&app);
    app.global::<Profiles>().invoke_select("couch".into());
    settle();
    focus_by_keys(&app, "profiles-field-primary");
    crate::support::press(&app, Key::Return);
    let select = app.global::<Select>();
    assert!(select.get_open());
    // The cursor to the Xbox pad, last as it isn't connected.
    crate::support::press(&app, Key::DownArrow);
    crate::support::press(&app, Key::DownArrow);
    assert_eq!(
        select.get_options().row_data(2).unwrap(),
        "Xbox pad · FFB17 (not connected)"
    );
    // It connects and the DualSense leaves: the two change places.
    let mut moved = known();
    moved.pads[0].connected = true;
    moved.pads[2].connected = false;
    profiles::show(&app, moved);
    settle();
    crate::support::press(&app, Key::Return);
    assert_eq!(
        *asked.borrow(),
        ["couch primary serial:045e:02dd:7EED821FFB17"]
    );
}

/// A pad named by its model is said by the pad of that model a start
/// would take: a free one before one another profile has.
#[test]
fn any_pad_of_a_model_is_said_by_the_one_a_start_would_take() {
    let mut known_now = known();
    let of_model = |id: &str, setup: Option<&str>| Pad {
        product: 0x09cc,
        setup: setup.map(str::to_string),
        ..pad(id, "Wireless Controller", true)
    };
    known_now
        .pads
        .push(of_model("address:aa:aa:aa:aa:aa:01", Some("quick")));
    known_now
        .pads
        .push(of_model("address:aa:aa:aa:aa:aa:02", None));
    let rows = profiles::rows(&known_now, &BTreeMap::new());
    let couch = row(&rows, "couch");
    let assist = field(couch, "assist");
    assert_eq!(
        (assist.status_word.as_str(), assist.status_tone.as_str()),
        ("Connected", "success")
    );
    let used = couch
        .uses
        .iter()
        .find(|used| used.part == "Assist")
        .unwrap();
    assert_eq!(used.pad, "address:aa:aa:aa:aa:aa:02");
}

/// Leaving a draft that isn't a profile yet asks first; discarding it
/// asks nothing, though the selection moves off it too.
#[test]
fn leaving_an_unfinished_draft_asks_and_discarding_it_does_not() {
    let app = app();
    let pane = app.global::<Profiles>();
    // The pane is made as the tree is first walked, as a window shown
    // makes it at once; what it follows starts from then.
    assert!(find_by_accessible_id(&app, "profiles-new").is_some());
    pane.invoke_create("den".into());
    settle();
    assert!(find_by_accessible_id(&app, "profiles-discard").is_some());
    pane.invoke_select("couch".into());
    settle();
    assert!(find_by_accessible_id(&app, "dialog-confirm").is_some());
    // Back to it, to finish it.
    activate_shown(&app, "dialog-cancel");
    let rows: Vec<ProfileRow> = pane.get_rows().iter().collect();
    assert_eq!(
        rows[usize::try_from(pane.get_selected_index()).unwrap()].name,
        "den"
    );
    pane.invoke_discard("den".into());
    settle();
    assert!(find_by_accessible_id(&app, "dialog-confirm").is_none());
    assert_eq!(pane.get_selected_index(), -1);
}

/// Managed follows the profile as the daemon keeps it, after a toggle
/// as before. Through the window's own path: a toggle is held and shown
/// at once, says it is on its way and asks for the change; what the
/// daemon then keeps is what it shows, and a change from elsewhere
/// reaches it.
#[test]
fn the_managed_switch_follows_the_profile_after_a_toggle() {
    let app = app();
    let (writer, mut asked) = profiles::Writer::detached();
    profiles::bind_edits(&app, &writer);
    app.global::<Profiles>().invoke_select("couch".into());
    settle();
    let checked = |app: &AppWindow| {
        find_by_accessible_id(app, "profiles-managed")
            .and_then(|switch| switch.accessible_checked())
            .unwrap_or_default()
    };
    let chip = |app: &AppWindow| {
        find_by_accessible_id(app, "profiles-managed-chip")
            .and_then(|chip| chip.accessible_label())
            .unwrap_or_default()
            .to_string()
    };
    let kept = |app: &AppWindow, managed: bool| {
        let mut known_now = known();
        let couch = known_now
            .profiles
            .iter_mut()
            .find(|profile| profile.name == "couch")
            .unwrap();
        couch.toml = format!("{COUCH}managed = {managed}\n");
        profiles::show(app, known_now);
        settle();
    };
    assert!(!checked(&app));
    let word = |app: &AppWindow| {
        let rows: Vec<ProfileRow> = app.global::<Profiles>().get_rows().iter().collect();
        row(&rows, "couch").word.to_string()
    };
    activate_shown(&app, "profiles-managed");
    let put = match asked.try_recv() {
        Ok(profiles::Op::Put(change)) => change,
        other => unreachable!("a toggle asks for the change, not {other:?}"),
    };
    assert_eq!(put.name, "couch");
    assert!(put.toml.contains("managed = true"), "{}", put.toml);
    // Held and shown at once, and said to be on its way.
    assert!(checked(&app));
    assert_eq!(word(&app), "Saving…");
    kept(&app, true);
    assert!(checked(&app));
    assert_eq!(chip(&app), "Managed");
    // Turned off from the tray, then on again from a file.
    kept(&app, false);
    assert!(!checked(&app));
    assert_eq!(chip(&app), "Unmanaged");
    kept(&app, true);
    assert!(checked(&app));
}

/// A switch the daemon serves, other than Managed, gets a control in
/// its card, as any field does.
#[test]
fn a_served_switch_has_a_control_of_its_own() {
    use ctrlassist::form::{Field, Kind};
    let app = app();
    let asked = record_set_field(&app);
    let mut known_now = known();
    known_now.fields.push(Field {
        key: "lights".to_string(),
        label: "Lights".to_string(),
        help: String::new(),
        kind: Kind::Switch,
        presets: Vec::new(),
        default: "false".to_string(),
        group: form::ARRANGEMENT.to_string(),
    });
    profiles::show(&app, known_now);
    app.global::<Profiles>().invoke_select("couch".into());
    settle();
    let switch = find_by_accessible_id(&app, "profiles-field-lights").expect("a control");
    assert_eq!(switch.accessible_checked(), Some(false));
    activate_shown(&app, "profiles-field-lights");
    assert_eq!(*asked.borrow(), ["couch lights true"]);
}

/// A profile deleted while it runs leaves its setup running: the setup
/// keeps a row, with its pads and virtual pads as it has them, and its
/// Stop.
#[test]
fn a_running_profile_that_is_deleted_keeps_a_row_to_stop_it() {
    let mut known_now = known();
    known_now.profiles.retain(|profile| profile.name != "quick");
    let rows = profiles::rows(&known_now, &BTreeMap::new());
    let gone = row(&rows, "quick");
    assert_eq!(gone.kind, "gone");
    assert!(!gone.kept && !gone.unfinished && !gone.can_start);
    assert_eq!(
        (gone.word.as_str(), gone.word_tone.as_str()),
        ("Running", "success")
    );
    assert_eq!(gone.setup_index, 0);
    assert_eq!(gone.shape, "merge");
    let uses: Vec<String> = gone
        .uses
        .iter()
        .map(|used| format!("{} {} {}", used.part, used.name, used.word))
        .collect();
    assert_eq!(
        uses,
        [
            "Assist Wireless Controller Connected",
            "Primary DualSense Connected"
        ]
    );
    let made = gone.outputs.row_data(0).unwrap();
    assert_eq!(
        (made.name.as_str(), made.pad.as_str()),
        ("Virtual", "virtual:quick/virtual")
    );

    let app = app();
    let stopped: Rc<RefCell<Vec<String>>> = Rc::default();
    let log = stopped.clone();
    app.global::<Setups>()
        .on_stop(move |name| log.borrow_mut().push(name.to_string()));
    let kept: Vec<String> = known_now
        .profiles
        .iter()
        .map(|profile| profile.name.clone())
        .collect();
    setups::show(&app, setups::rows(&known_now.setups, &kept));
    profiles::show(&app, known_now);
    app.global::<Profiles>().invoke_select("quick".into());
    settle();
    for id in ["profiles-gone", "profiles-stale"] {
        assert!(find_by_accessible_id(&app, id).is_some(), "{id}");
    }
    // Nothing to update to, delete, or say how it starts.
    for id in [
        "profiles-update",
        "profiles-delete",
        "profiles-managed",
        "profiles-managed-chip",
    ] {
        assert!(find_by_accessible_id(&app, id).is_none(), "{id}");
    }
    activate_shown(&app, "profiles-stop");
    assert_eq!(*stopped.borrow(), ["quick"]);
}

/// A running profile has a row in the sidebar, from every pane: what its
/// setup is in, on the rail between the last pane and Reload, and a way
/// to its profile.
#[test]
fn a_running_profile_s_row_in_the_sidebar_says_its_state_and_opens_it() {
    use ctrlassist_gui::Nav;
    use slint::platform::Key;

    use crate::support::{elapse, press};

    /// A name too long for the sidebar to show whole.
    const LONG: &str = "zz-a-profile-name-far-too-long";

    i_slint_backend_testing::init_no_event_loop();
    profiles::forget_held();
    let app = AppWindow::new().expect("the window builds on the testing backend");
    let handles = nav::wire(&app);
    keys::bind_key_router(&app);
    profiles::bind_local(&app, &handles);
    setups::bind_links(&app, &handles);
    app.set_daemon_available(true);
    app.show().expect("the testing backend shows a window");
    settle();

    // Nothing runs: no rows.
    assert!(find_by_accessible_id(&app, "sidebar-status-quick").is_none());

    let mut known = known();
    let held = Setup {
        name: "couch".to_string(),
        phase: Phase::Held,
        ..known.setups[0].clone()
    };
    known.setups.push(held);
    let long = Setup {
        name: LONG.to_string(),
        ..known.setups[0].clone()
    };
    known.setups.push(long);
    let kept: Vec<String> = known
        .profiles
        .iter()
        .map(|profile| profile.name.clone())
        .collect();
    setups::show(&app, setups::rows(&known.setups, &kept));
    profiles::show(&app, known);
    settle();
    let label = |name: &str| {
        find_by_accessible_id(&app, &format!("sidebar-status-{name}"))
            .and_then(|row| row.accessible_label())
            .map(|label| label.to_string())
    };
    assert_eq!(label("quick").as_deref(), Some("quick: running"));
    assert_eq!(
        label("couch").as_deref(),
        Some("couch: held, waiting for a pad")
    );

    // Down the rail from the last pane: each row in name order, then
    // Reload, and back up the same way.
    find_by_accessible_id(&app, "sidebar-about")
        .unwrap()
        .invoke_accessible_default_action();
    settle();
    app.invoke_focus_active_sidebar_item();
    settle();
    let focus = || app.global::<Nav>().get_focus_id().to_string();
    assert_eq!(focus(), "sidebar-about");
    let mut walked = Vec::new();
    for key in [
        Key::DownArrow,
        Key::DownArrow,
        Key::DownArrow,
        Key::DownArrow,
        Key::DownArrow,
        Key::UpArrow,
        Key::UpArrow,
        Key::UpArrow,
        Key::UpArrow,
    ] {
        press(&app, key);
        settle();
        walked.push(focus());
    }
    assert_eq!(
        walked,
        [
            "sidebar-status-couch",
            "sidebar-status-quick",
            &format!("sidebar-status-{LONG}"),
            "sidebar-reload",
            "sidebar-reload",
            &format!("sidebar-status-{LONG}"),
            "sidebar-status-quick",
            "sidebar-status-couch",
            "sidebar-about",
        ]
    );
    // The pane didn't follow focus onto a row.
    assert_eq!(app.get_active_pane(), Pane::About);

    // A name too long to show whole slides to its end while its row has
    // focus, once, and stays there; it is back, cut short, when the row
    // hasn't. One that fits never moves.
    let left_of = |name: &str| {
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(&app, name)
            .filter(|text| text.size().width > 0.0)
            .map(|text| text.absolute_position().x)
            .fold(f32::INFINITY, f32::min)
    };
    let (still, fits) = (left_of(LONG), left_of("couch"));
    for _ in 0..3 {
        press(&app, Key::DownArrow);
        settle();
    }
    assert_eq!(focus(), format!("sidebar-status-{LONG}"));
    elapse(500);
    assert_eq!(left_of(LONG), still, "it waits before it sets off");
    elapse(1000);
    let going = left_of(LONG);
    assert!(going < still, "it slides toward its end");
    elapse(60_000);
    let there = left_of(LONG);
    assert!(there <= going);
    elapse(60_000);
    assert_eq!(left_of(LONG), there, "and stays there");
    for _ in 0..3 {
        press(&app, Key::UpArrow);
        settle();
    }
    assert_eq!(left_of(LONG), still, "it is back at once");
    assert_eq!(left_of("couch"), fits);

    // One that fits stays where it is with focus on its row, however
    // long; and Return on a row opens its profile.
    press(&app, Key::DownArrow);
    settle();
    assert_eq!(focus(), "sidebar-status-couch");
    elapse(60_000);
    assert_eq!(left_of("couch"), fits);
    press(&app, Key::Return);
    settle();
    assert_eq!(app.get_active_pane(), Pane::Profiles);
    let pane = app.global::<Profiles>();
    let shown = pane
        .get_rows()
        .row_data(usize::try_from(pane.get_selected_index()).unwrap())
        .unwrap()
        .name;
    assert_eq!(shown, "couch");

    // Back on the sidebar: Tab stops at one row of them, the one last
    // focused, on its way from the active item to Reload.
    find_by_accessible_id(&app, "sidebar-about")
        .unwrap()
        .invoke_accessible_default_action();
    settle();
    app.invoke_focus_active_sidebar_item();
    settle();
    let mut tabbed = Vec::new();
    for _ in 0..2 {
        press(&app, Key::Tab);
        settle();
        tabbed.push(focus());
    }
    assert_eq!(tabbed, ["sidebar-status-couch", "sidebar-reload"]);

    // The state in words comes to a row with focus, as to one under a
    // pointer, once focus has rested on it.
    press(&app, Key::UpArrow);
    settle();
    assert_eq!(focus(), format!("sidebar-status-{LONG}"));
    press(&app, Key::UpArrow);
    settle();
    assert_eq!(focus(), "sidebar-status-quick");
    let help = app.global::<ctrlassist_gui::Help>();
    assert!(!help.get_shown());
    elapse(800);
    assert!(help.get_shown());
    assert_eq!(help.get_text(), "quick: running");

    // Another profile starts while a row has focus, and the rows are
    // made again: the row of the same profile has focus still.
    let mut more = self::known().setups;
    more.push(Setup {
        name: "attic".to_string(),
        ..more[0].clone()
    });
    setups::show(&app, setups::rows(&more, &kept));
    for _ in 0..4 {
        settle();
    }
    assert_eq!(label("attic").as_deref(), Some("attic: running"));
    assert_eq!(focus(), "sidebar-status-quick");

    // A setup that failed and one still starting say so too, and the
    // rows go as their setups do.
    let state = |name: &str, phase: Phase| Setup {
        name: name.to_string(),
        phase,
        ..self::known().setups[0].clone()
    };
    setups::show(
        &app,
        setups::rows(
            &[state("den", Phase::Failed), state("nook", Phase::Starting)],
            &kept,
        ),
    );
    settle();
    assert_eq!(label("den").as_deref(), Some("den: failed"));
    assert_eq!(label("nook").as_deref(), Some("nook: starting"));
    assert_eq!(label("couch"), None, "its setup ended");
    setups::show(&app, Vec::new());
    settle();
    assert_eq!(label("den"), None);
    assert_eq!(label("nook"), None);
}
