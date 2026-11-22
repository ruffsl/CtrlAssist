//! The Setups pane: its rows from the daemon's setups, the pane in the
//! window, and the click-throughs between it and the Pads pane.

use std::cell::RefCell;
use std::rc::Rc;

use ctrlassist::{Pad, Phase, Setup, SetupNode, SetupSink, SetupSource};
use ctrlassist_gui::state::Snapshot;
use ctrlassist_gui::{AppWindow, Pane, Setups, keys, nav, pads, setups};
use slint::platform::Key;
use slint::{ComponentHandle, Model as _, SharedString};

use crate::support::{activate_shown, find_all_by_accessible_id, press, settle};

fn pad(id: &str, name: &str, setup: Option<&str>) -> Pad {
    Pad {
        id: id.to_string(),
        name: name.to_string(),
        vendor: 0x054c,
        product: 0x0ce6,
        connected: true,
        hidden: setup.is_some(),
        setup: setup.map(str::to_string),
        rumble: Some(true),
        bus: Some(3),
    }
}

fn setup(name: &str, phase: Phase) -> Setup {
    Setup {
        name: name.to_string(),
        phase,
        reason: (phase == Phase::Failed).then(|| "a virtual pad refused a write".to_string()),
        sources: vec![
            SetupSource {
                name: "assist".to_string(),
                pad: Some("address:1c:96:5a:a1:35:da".to_string()),
                present: phase != Phase::Held,
            },
            SetupSource {
                name: "primary".to_string(),
                pad: Some("address:0c:27:56:4f:bd:14".to_string()),
                present: true,
            },
        ],
        sinks: vec![SetupSink {
            name: "virtual".to_string(),
            identity: "dualsense".to_string(),
            node: Some("/dev/input/event261".to_string()),
        }],
        nodes: vec![SetupNode {
            name: "mux".to_string(),
            choices: vec!["primary".to_string(), "assist".to_string()],
            selected: 0,
        }],
        stale: false,
    }
}

fn some_pads() -> Vec<Pad> {
    vec![
        pad("address:0c:27:56:4f:bd:14", "DualSense", Some("couch")),
        pad(
            "address:1c:96:5a:a1:35:da",
            "Wireless Controller",
            Some("couch"),
        ),
    ]
}

#[test]
fn rows_say_each_setup_s_phase_pads_virtual_pads_and_selections() {
    let stale = Setup {
        stale: true,
        ..setup("couch", Phase::Running)
    };
    let orphan = Setup {
        stale: true,
        // A pad the daemon no longer knows, and one never found.
        sources: vec![
            SetupSource {
                name: "primary".to_string(),
                pad: Some("serial:gone".to_string()),
                present: false,
            },
            SetupSource {
                name: "assist".to_string(),
                pad: None,
                present: false,
            },
        ],
        ..setup("attic", Phase::Held)
    };
    let all = [stale, orphan, setup("den", Phase::Failed)];
    let rows = setups::rows(
        &all,
        &some_pads(),
        &["couch".to_string(), "den".to_string()],
    );
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["attic", "couch", "den"]);

    let couch = &rows[1];
    assert_eq!(
        (couch.phase.as_str(), couch.tone.as_str()),
        ("running", "success")
    );
    assert!(couch.stale && couch.profile_kept);
    let pads: Vec<_> = couch.pads.iter().collect();
    assert_eq!(pads[0].role, "assist");
    assert_eq!(pads[0].name, "Wireless Controller");
    assert!(pads[0].present);
    let sink = couch.sinks.row_data(0).unwrap();
    assert_eq!(
        (sink.identity.as_str(), sink.node.as_str()),
        ("dualsense", "/dev/input/event261")
    );
    let node = couch.nodes.row_data(0).unwrap();
    assert_eq!(node.selected, 0);
    assert_eq!(node.choices.row_count(), 2);

    let attic = &rows[0];
    assert_eq!(attic.tone, "warn");
    assert!(attic.stale && !attic.profile_kept, "its profile is gone");
    let pads: Vec<_> = attic.pads.iter().collect();
    assert_eq!(
        (pads[0].pad.as_str(), pads[0].name.as_str()),
        ("serial:gone", "serial:gone")
    );
    assert_eq!(pads[1].pad, "");

    let den = &rows[2];
    assert_eq!(den.tone, "danger");
    assert_eq!(den.reason, "a virtual pad refused a write");

    // The badge is the worst of them.
    assert_eq!(setups::trouble(&all), "danger");
    assert_eq!(setups::trouble(&all[..2]), "warn");
    assert_eq!(setups::trouble(&[setup("couch", Phase::Running)]), "");
    assert_eq!(setups::trouble(&[]), "");
}

fn app(setups_now: Vec<Setup>) -> AppWindow {
    i_slint_backend_testing::init_no_event_loop();
    let app = AppWindow::new().expect("the window builds on the testing backend");
    let handles = nav::wire(&app);
    keys::bind_key_router(&app);
    pads::bind_links(&app, &handles);
    setups::bind_links(&app, &handles);
    Snapshot {
        available: true,
        hydrated: true,
        pads: some_pads(),
        profiles: setups_now
            .iter()
            .map(|setup| ctrlassist::Profile {
                name: setup.name.clone(),
                toml: String::new(),
                problem: None,
            })
            .collect(),
        setups: setups_now,
        ..Snapshot::default()
    }
    .show(&app);
    app
}

fn enabled(app: &AppWindow, id: &str) -> bool {
    find_all_by_accessible_id(app, id)
        .into_iter()
        .filter(|control| control.computed_opacity() > 0.0)
        .any(|control| control.accessible_enabled().unwrap_or(false))
}

#[test]
fn a_setup_is_stopped_updated_and_steered_from_its_detail() {
    let app = app(vec![
        Setup {
            stale: true,
            ..setup("couch", Phase::Running)
        },
        setup("den", Phase::Running),
    ]);
    app.set_active_pane(Pane::Setups);
    let pane = app.global::<Setups>();
    let asked: Rc<RefCell<Vec<String>>> = Rc::default();
    let log = |what: &'static str| {
        let asked = asked.clone();
        move |name: SharedString| asked.borrow_mut().push(format!("{what} {name}"))
    };
    pane.on_stop(log("stop"));
    pane.on_ask_update(log("ask-update"));
    pane.on_update(log("update"));
    let selected = asked.clone();
    pane.on_select(move |name, node, choice| {
        selected
            .borrow_mut()
            .push(format!("select {name} {node} {choice}"));
    });

    // The one whose profile is as it runs has nothing to update.
    pane.set_selected_index(1);
    settle();
    assert!(!enabled(&app, "setups-update"));
    pane.set_selected_index(0);
    settle();
    assert!(enabled(&app, "setups-update"));

    // Update asks the cost first; the dialog is Rust's to open, and
    // confirming it updates.
    activate_shown(&app, "setups-update");
    assert_eq!(*asked.borrow(), ["ask-update couch"]);
    pane.set_pending_update_cost("restart".into());
    pane.set_pending_update("couch".into());
    settle();
    activate_shown(&app, "dialog-confirm");
    assert_eq!(asked.borrow().last().unwrap(), "update couch");
    assert_eq!(pane.get_pending_update(), "");

    // Stop asks first, and a stray Return on the dialog stops nothing.
    activate_shown(&app, "setups-stop");
    press(&app, Key::Return);
    assert_eq!(asked.borrow().len(), 2);
    activate_shown(&app, "setups-stop");
    activate_shown(&app, "dialog-confirm");
    assert_eq!(asked.borrow().last().unwrap(), "stop couch");

    // Control is handed over by the choice's button.
    activate_shown(&app, "setups-node-mux-assist");
    assert_eq!(asked.borrow().last().unwrap(), "select couch mux assist");
}
