//! The Pads pane: its rows from the daemon's pads and setups, and the
//! pane in the window, by pointer and by keys.

use std::cell::RefCell;
use std::rc::Rc;

use ctrlassist::{Pad, Phase, Profile, Setup, SetupSource};
use ctrlassist_gui::pads::{rows, show};
use ctrlassist_gui::{AppWindow, Nav, Pads, keys, nav};
use slint::platform::Key;
use slint::{ComponentHandle, Model as _};

use crate::support::{find_by_accessible_id, press, settle};

fn pad(id: &str, name: &str, connected: bool, setup: Option<&str>) -> Pad {
    Pad {
        id: id.to_string(),
        name: name.to_string(),
        vendor: 0x054c,
        product: 0x0ce6,
        connected,
        hidden: setup.is_some(),
        setup: setup.map(str::to_string),
        rumble: Some(true),
        bus: connected.then_some(3),
    }
}

fn quick() -> Setup {
    Setup {
        name: "quick".to_string(),
        start: 1,
        phase: Phase::Running,
        reason: None,
        sources: vec![
            SetupSource {
                name: "primary".to_string(),
                pad: Some("address:0a:1b:2c:3d:4e:14".to_string()),
                present: true,
            },
            SetupSource {
                name: "assist".to_string(),
                pad: None,
                present: false,
            },
        ],
        sinks: Vec::new(),
        nodes: Vec::new(),
        stale: false,
    }
}

/// The profile `quick` runs, and another that names its primary.
fn profiles() -> Vec<Profile> {
    let kept = |name: &str, toml: &str| Profile {
        name: name.to_string(),
        toml: toml.to_string(),
        unset: Vec::new(),
        problem: None,
    };
    vec![
        kept(
            "desk",
            "mode = \"unicast\"\npad = \"address:0a:1b:2c:3d:4e:14\"\n",
        ),
        kept(
            "quick",
            "mode = \"priority\"\nprimary = \"address:0a:1b:2c:3d:4e:14\"\nassist = \"address:1a:2b:3c:4d:5e:da\"\n",
        ),
        // One the daemon finds wrong names no pad that can be told.
        kept("broken", "mode = \"priority\"\n"),
    ]
}

fn some_pads() -> Vec<Pad> {
    vec![
        pad("serial:045e:02dd:7EED821FFB17", "Xbox pad", false, None),
        pad(
            "address:1a:2b:3c:4d:5e:da",
            "Wireless Controller",
            true,
            None,
        ),
        pad(
            "address:0a:1b:2c:3d:4e:14",
            "DualSense",
            true,
            Some("quick"),
        ),
        pad("model:1209:0001", "Anonymous pad", true, None),
    ]
}

#[test]
fn rows_put_connected_pads_first_and_say_what_each_is() {
    let made = rows(&some_pads(), &[quick()], &profiles(), &[]);
    let names: Vec<&str> = made.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Anonymous pad",
            "DualSense",
            "Wireless Controller",
            "Xbox pad"
        ]
    );
    let dualsense = &made[1];
    assert_eq!(dualsense.model, "054c:0ce6");
    assert_eq!(dualsense.connection, "USB");
    assert_eq!(dualsense.rumble, "Yes");
    assert_eq!(
        (dualsense.setup.as_str(), dualsense.role.as_str()),
        ("quick", "primary")
    );
    assert!(dualsense.hidden);
    assert_eq!(dualsense.address, "0a:1b:2c:3d:4e:14");
    assert_eq!(dualsense.tag, "4e:14");
    assert_eq!(dualsense.key, "address-0a-1b-2c-3d-4e-14");
    // What it is doing, its maker's tile, and whether games see it.
    assert_eq!(
        (dualsense.word.as_str(), dualsense.tone.as_str()),
        ("In quick", "success")
    );
    assert_eq!(dualsense.maker, "sony");
    assert_eq!(dualsense.visibility, "Hidden");
    // Every profile that names it, with its part and how the profile is.
    let named: Vec<String> = dualsense
        .profiles
        .iter()
        .map(|row| format!("{} {} {} {}", row.name, row.part, row.word, row.tone))
        .collect();
    assert_eq!(
        named,
        ["desk Pad Waiting neutral", "quick Primary Running success"]
    );
    let free = &made[2];
    assert_eq!((free.word.as_str(), free.tone.as_str()), ("Free", "info"));
    assert_eq!(free.visibility, "Visible");
    assert_eq!(free.profiles.row_count(), 1);
    assert_eq!(made[0].profiles.row_count(), 0);
    assert_eq!(made[0].maker, "sony");
    // Known by its model alone: nothing tells it from another, and it
    // has no address.
    assert_eq!((made[0].tag.as_str(), made[0].address.as_str()), ("", ""));
    // Remembered: no connection to name.
    let xbox = &made[3];
    assert!(!xbox.connected);
    assert_eq!(xbox.connection, "");
    assert_eq!(xbox.tag, "FFB17");
    assert_eq!(
        (xbox.word.as_str(), xbox.tone.as_str()),
        ("Remembered", "neutral")
    );

    // A bus that is neither, and rumble the daemon couldn't tell.
    let odd = Pad {
        bus: Some(6),
        rumble: None,
        ..pad("port:usb-1/input0", "Odd", true, None)
    };
    let row = &rows(&[odd], &[], &[], &[])[0];
    assert_eq!((row.connection.as_str(), row.rumble.as_str()), ("", ""));
    // What the pane goes by is apart from the word it shows.
    assert!(!row.without_rumble && !dualsense.without_rumble);
    let still = Pad {
        rumble: Some(false),
        ..pad("port:usb-2/input0", "Still", true, None)
    };
    let row = &rows(&[still], &[], &[], &[])[0];
    assert!(row.without_rumble);
    assert_eq!(row.rumble, "No");
}

fn app() -> AppWindow {
    i_slint_backend_testing::init_no_event_loop();
    let app = AppWindow::new().expect("the window builds on the testing backend");
    let _ = nav::wire(&app);
    keys::bind_key_router(&app);
    app.set_daemon_available(true);
    show(&app, rows(&some_pads(), &[quick()], &profiles(), &[]));
    app
}

#[test]
fn the_selection_stays_on_its_pad_as_rows_move_and_goes_with_it() {
    let app = app();
    let pads = app.global::<Pads>();
    assert_eq!(pads.get_connected_count(), 3);
    assert_eq!(pads.get_selected_index(), -1);
    // The Wireless Controller, third.
    pads.set_selected_index(2);
    // The DualSense goes: the Wireless Controller is second now.
    let mut fewer = some_pads();
    fewer.remove(2);
    show(&app, rows(&fewer, &[], &[], &[]));
    assert_eq!(pads.get_selected_index(), 1);
    assert_eq!(
        pads.get_rows().row_data(1).unwrap().name,
        "Wireless Controller"
    );
    // It is forgotten: nothing is selected.
    fewer.remove(1);
    show(&app, rows(&fewer, &[], &[], &[]));
    assert_eq!(pads.get_selected_index(), -1);
}

#[test]
fn identify_needs_the_pad_there_and_forget_needs_it_gone_and_asks_first() {
    let app = app();
    let pads = app.global::<Pads>();
    let asked: Rc<RefCell<Vec<String>>> = Rc::default();
    let (buzzed, forgotten) = (asked.clone(), asked.clone());
    pads.on_identify(move |id| buzzed.borrow_mut().push(format!("identify {id}")));
    pads.on_forget(move |id| forgotten.borrow_mut().push(format!("forget {id}")));

    // A connected pad: Identify works, Forget doesn't.
    pads.set_selected_index(1);
    settle();
    let enabled = |id: &str| {
        // Each pad has a detail; the one shown is the selected pad's.
        crate::support::find_all_by_accessible_id(&app, id)
            .into_iter()
            .filter(|control| control.computed_opacity() > 0.0)
            .any(|control| control.accessible_enabled().unwrap_or(false))
    };
    assert!(enabled("pads-identify"));
    assert!(!enabled("pads-forget"));
    crate::support::activate_shown(&app, "pads-identify");
    assert_eq!(*asked.borrow(), ["identify address:0a:1b:2c:3d:4e:14"]);

    // A remembered pad: the other way about, and Forget asks first.
    pads.set_selected_index(3);
    settle();
    assert!(!enabled("pads-identify"));
    assert!(enabled("pads-forget"));
    crate::support::activate_shown(&app, "pads-forget");
    assert_eq!(
        asked.borrow().len(),
        1,
        "nothing is forgotten until confirmed"
    );
    // The dialog opens on Cancel, so a stray Return forgets nothing.
    press(&app, Key::Return);
    assert_eq!(asked.borrow().len(), 1);
    crate::support::activate_shown(&app, "pads-forget");
    crate::support::activate_shown(&app, "dialog-confirm");
    assert_eq!(
        asked.borrow().last().unwrap(),
        "forget serial:045e:02dd:7EED821FFB17"
    );
}

/// A pad's detail lists the profiles that name it, and a row opens its
/// profile; the pads that are only remembered sit under their own
/// caption.
#[test]
fn a_pad_s_profiles_open_from_its_detail() {
    let app = app();
    let pads = app.global::<Pads>();
    let opened: Rc<RefCell<Vec<String>>> = Rc::default();
    let seen = opened.clone();
    pads.on_open_profile(move |name| seen.borrow_mut().push(name.to_string()));
    pads.set_selected_index(1);
    settle();
    assert!(find_by_accessible_id(&app, "pads-no-profiles").is_none());
    crate::support::activate_shown(&app, "pads-profile-desk");
    crate::support::activate_shown(&app, "pads-profile-quick");
    assert_eq!(*opened.borrow(), ["desk", "quick"]);
    // A pad no profile names says so.
    pads.set_selected_index(0);
    settle();
    assert!(find_by_accessible_id(&app, "pads-no-profiles").is_some());
}

/// The virtual pads the running profiles make follow the real pads,
/// under a caption of their own: each is its profile's, shows what it
/// presents itself as, and opens that profile; it can't be buzzed or
/// forgotten.
#[test]
fn virtual_pads_follow_the_real_ones() {
    use ctrlassist::{Identity, SetupSink};

    let mut running = quick();
    running.sinks = vec![SetupSink {
        name: "virtual".to_string(),
        identity: "xbox".to_string(),
        node: Some("/dev/input/event31".to_string()),
    }];
    let identities = [Identity {
        id: "xbox".to_string(),
        name: "Xbox".to_string(),
        vendor: 0x045e,
        product: 0x0b12,
        available: true,
    }];
    let made = rows(&some_pads(), &[running], &profiles(), &identities);
    assert_eq!(made.len(), 5);
    let pad = &made[4];
    assert!(pad.is_virtual && pad.connected);
    assert_eq!(pad.id, "virtual:quick/virtual");
    // It goes by the name a game sees, with whose it is under that and
    // its node to tell it from another.
    assert_eq!(pad.name, "Xbox");
    assert_eq!(pad.maker, "microsoft");
    assert_eq!(
        (pad.word.as_str(), pad.tone.as_str(), pad.tag.as_str()),
        ("In quick", "success", "event31")
    );
    assert_eq!(
        (pad.identity.as_str(), pad.model.as_str(), pad.node.as_str()),
        ("Xbox", "045e:0b12", "/dev/input/event31")
    );
    let its: Vec<String> = pad
        .profiles
        .iter()
        .map(|row| format!("{} {} {}", row.name, row.part, row.word))
        .collect();
    assert_eq!(its, ["quick Virtual Running"]);

    let app = app();
    show(&app, made);
    let pads = app.global::<Pads>();
    // The header counts the real pads, and the caption starts the rest.
    assert_eq!((pads.get_connected_count(), pads.get_real_count()), (3, 4));
    let caption = |title: &'static str| {
        i_slint_backend_testing::ElementQuery::from_root(&app)
            .match_descendants()
            .match_predicate(move |element| element.accessible_label().as_deref() == Some(title))
            .find_first()
            .is_some()
    };
    assert!(caption("VIRTUAL"));
    assert!(!caption("REMEMBERED"), "remembered pads are among the real");

    let opened: Rc<RefCell<Vec<String>>> = Rc::default();
    let seen = opened.clone();
    pads.on_open_profile(move |name| seen.borrow_mut().push(name.to_string()));
    pads.set_selected_index(4);
    settle();
    crate::support::activate_shown(&app, "pads-profile-quick");
    assert_eq!(*opened.borrow(), ["quick"]);
    // It is buzzed by its own name, as a game would rumble it; it can't
    // be forgotten.
    let buzzed: Rc<RefCell<Vec<String>>> = Rc::default();
    let felt = buzzed.clone();
    pads.on_identify(move |id| felt.borrow_mut().push(id.to_string()));
    crate::support::activate_shown(&app, "pads-identify");
    assert_eq!(*buzzed.borrow(), ["virtual:quick/virtual"]);
    assert!(find_by_accessible_id(&app, "pads-forget").is_none());
    // Each group says what it is about, behind its "i".
    for id in ["pads-help-status", "pads-help-profiles", "pads-help-info"] {
        let help = find_by_accessible_id(&app, id).unwrap_or_else(|| unreachable!("no {id}"));
        assert!(
            !help.accessible_description().unwrap_or_default().is_empty(),
            "{id}"
        );
    }

    // One that presents itself as nobody else's is CtrlAssist's own.
    let mut generic = quick();
    generic.sinks = vec![SetupSink {
        name: "virtual".to_string(),
        identity: "generic".to_string(),
        node: None,
    }];
    let own = &rows(&[], &[generic], &[], &[])[0];
    assert_eq!(
        (own.maker.as_str(), own.name.as_str()),
        ("ctrlassist", "Generic")
    );
    assert!(!own.connected, "not made yet");
}

#[test]
fn keys_go_from_the_sidebar_into_the_list_and_on_into_the_detail() {
    let app = app();
    app.show().expect("the testing backend shows a window");
    app.invoke_focus_active_sidebar_item();
    settle();
    let nav = app.global::<Nav>();
    let pads = app.global::<Pads>();
    // Right from the sidebar lands on the first row and selects it.
    press(&app, Key::RightArrow);
    assert_eq!(nav.get_column(), 1);
    assert_eq!(pads.get_selected_index(), 0);
    // Down steps the rows, and stops at the last.
    press(&app, Key::DownArrow);
    assert_eq!(pads.get_selected_index(), 1);
    press(&app, Key::DownArrow);
    press(&app, Key::DownArrow);
    press(&app, Key::DownArrow);
    assert_eq!(pads.get_selected_index(), 3);
    press(&app, Key::UpArrow);
    press(&app, Key::UpArrow);
    assert_eq!(pads.get_selected_index(), 1);
    // Right enters the detail; Escape comes back to the row, and again
    // to the sidebar.
    press(&app, Key::RightArrow);
    assert_eq!(nav.get_column(), 2);
    assert_eq!(nav.get_focus_id(), "pads-identify");
    press(&app, Key::Escape);
    assert_eq!(nav.get_column(), 1);
    assert_eq!(pads.get_selected_index(), 1);
    press(&app, Key::Escape);
    assert_eq!(nav.get_column(), 0);
}

#[test]
fn an_empty_list_says_why() {
    let app = app();
    show(&app, Vec::new());
    let said = |app: &AppWindow| {
        find_by_accessible_id(app, "pads-empty")
            .and_then(|text| text.accessible_label())
            .map(|label| label.to_string())
            .unwrap_or_default()
    };
    assert_eq!(said(&app), "No pads known. Plug one in.");
    app.set_daemon_loading(true);
    assert_eq!(said(&app), "Loading");
    app.set_daemon_available(false);
    assert_eq!(said(&app), "Daemon offline");
}

/// By keys from the sidebar into the detail of the pad at `at`, then
/// down to the control of that id.
fn focus_by_keys(app: &AppWindow, at: i32, id: &str) {
    app.show().expect("the testing backend shows a window");
    app.invoke_focus_active_sidebar_item();
    settle();
    let nav = app.global::<Nav>();
    press(app, Key::RightArrow);
    for _ in 0..at {
        press(app, Key::DownArrow);
    }
    assert_eq!(app.global::<Pads>().get_selected_index(), at);
    press(app, Key::RightArrow);
    for _ in 0..8 {
        if nav.get_focus_id() == id {
            break;
        }
        press(app, Key::DownArrow);
    }
    assert_eq!(nav.get_focus_id(), id);
}

/// A profile whose pads are still partly to choose names the pads it
/// has: each lists it, as not set up.
#[test]
fn a_profile_not_set_up_is_listed_on_the_pads_it_names() {
    let den = Profile {
        name: "den".to_string(),
        toml: "mode = \"priority\"\nprimary = \"address:0a:1b:2c:3d:4e:14\"\n".to_string(),
        unset: vec!["assist".to_string()],
        problem: None,
    };
    let made = rows(&some_pads(), &[], std::slice::from_ref(&den), &[]);
    let named: Vec<String> = made[1]
        .profiles
        .iter()
        .map(|row| format!("{} {} {} {}", row.name, row.part, row.word, row.tone))
        .collect();
    assert_eq!(named, ["den Primary Not set up neutral"]);
    // The pad still to choose is none of them.
    assert!(
        made.iter()
            .all(|row| row.id == made[1].id || row.profiles.row_count() == 0)
    );

    let app = app();
    show(&app, made);
    app.global::<Pads>().set_selected_index(1);
    settle();
    assert!(find_by_accessible_id(&app, "pads-profile-den").is_some());
    assert!(find_by_accessible_id(&app, "pads-no-profiles").is_none());
}

/// Entering a pad's detail by keys lands on a stop in it, whether or
/// not Identify can buzz the pad.
#[test]
fn keys_into_a_detail_land_somewhere_when_identify_cannot_buzz() {
    use ctrlassist::SetupSink;
    let mut pads = some_pads();
    // The Wireless Controller, connected, with no rumble.
    pads[1].rumble = Some(false);
    let mut running = quick();
    // A virtual pad not made yet.
    running.sinks = vec![SetupSink {
        name: "virtual".to_string(),
        identity: "generic".to_string(),
        node: None,
    }];
    let app = app();
    show(&app, rows(&pads, &[running], &profiles(), &[]));
    app.show().expect("the testing backend shows a window");
    let nav = app.global::<Nav>();
    let list = app.global::<Pads>();
    for (at, name) in [(2, "Wireless Controller"), (4, "Generic")] {
        app.invoke_focus_active_sidebar_item();
        settle();
        press(&app, Key::RightArrow);
        for _ in 0..at {
            press(&app, Key::DownArrow);
        }
        assert_eq!(
            list.get_rows()
                .row_data(usize::try_from(at).unwrap())
                .unwrap()
                .name,
            name
        );
        press(&app, Key::RightArrow);
        assert_eq!(nav.get_column(), 2, "{name}");
        // On the next stop after it: the Games row's "i", or the
        // PROFILES group's.
        let landed = nav.get_focus_id();
        assert!(
            !landed.is_empty() && landed != "pads-identify",
            "{name}: {landed}"
        );
        // And the keys go on from there.
        press(&app, Key::Escape);
        assert_eq!(nav.get_column(), 1, "{name}");
        press(&app, Key::Escape);
    }
}

/// A change the daemon reports keeps the rows of a pad's PROFILES, so
/// the pencil with keyboard focus keeps it.
#[test]
fn a_pad_s_profile_pencil_keeps_focus_through_a_daemon_change() {
    let app = app();
    let pads = app.global::<Pads>();
    let opened: Rc<RefCell<Vec<String>>> = Rc::default();
    let seen = opened.clone();
    pads.on_open_profile(move |name| seen.borrow_mut().push(name.to_string()));
    focus_by_keys(&app, 1, "pads-profile-desk");
    show(&app, rows(&some_pads(), &[quick()], &profiles(), &[]));
    settle();
    press(&app, Key::Return);
    assert_eq!(*opened.borrow(), ["desk"]);
}
