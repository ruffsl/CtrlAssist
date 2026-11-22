//! The Pads pane: its rows from the daemon's pads and setups, and the
//! pane in the window, by pointer and by keys.

use std::cell::RefCell;
use std::rc::Rc;

use ctrlassist::{Pad, Phase, Setup, SetupSource};
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
        phase: Phase::Running,
        reason: None,
        sources: vec![
            SetupSource {
                name: "primary".to_string(),
                pad: Some("address:0c:27:56:4f:bd:14".to_string()),
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
    }
}

fn some_pads() -> Vec<Pad> {
    vec![
        pad("serial:045e:02dd:7EED821FFB17", "Xbox pad", false, None),
        pad(
            "address:1c:96:5a:a1:35:da",
            "Wireless Controller",
            true,
            None,
        ),
        pad(
            "address:0c:27:56:4f:bd:14",
            "DualSense",
            true,
            Some("quick"),
        ),
        pad("model:1209:0001", "Anonymous pad", true, None),
    ]
}

#[test]
fn rows_put_connected_pads_first_and_say_what_each_is() {
    let made = rows(&some_pads(), &[quick()]);
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
    assert_eq!(dualsense.address, "0c:27:56:4f:bd:14");
    assert_eq!(dualsense.tag, "bd:14");
    assert_eq!(dualsense.key, "address-0c-27-56-4f-bd-14");
    // Known by its model alone: nothing tells it from another, and it
    // has no address.
    assert_eq!((made[0].tag.as_str(), made[0].address.as_str()), ("", ""));
    // Remembered: no connection to name.
    let xbox = &made[3];
    assert!(!xbox.connected);
    assert_eq!(xbox.connection, "");
    assert_eq!(xbox.tag, "FFB17");

    // A bus that is neither, and rumble the daemon couldn't tell.
    let odd = Pad {
        bus: Some(6),
        rumble: None,
        ..pad("port:usb-1/input0", "Odd", true, None)
    };
    let row = &rows(&[odd], &[])[0];
    assert_eq!((row.connection.as_str(), row.rumble.as_str()), ("", ""));
}

fn app() -> AppWindow {
    i_slint_backend_testing::init_no_event_loop();
    let app = AppWindow::new().expect("the window builds on the testing backend");
    let _ = nav::wire(&app);
    keys::bind_key_router(&app);
    app.set_daemon_available(true);
    show(&app, rows(&some_pads(), &[quick()]));
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
    show(&app, rows(&fewer, &[]));
    assert_eq!(pads.get_selected_index(), 1);
    assert_eq!(
        pads.get_rows().row_data(1).unwrap().name,
        "Wireless Controller"
    );
    // It is forgotten: nothing is selected.
    fewer.remove(1);
    show(&app, rows(&fewer, &[]));
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
    assert_eq!(*asked.borrow(), ["identify address:0c:27:56:4f:bd:14"]);

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
