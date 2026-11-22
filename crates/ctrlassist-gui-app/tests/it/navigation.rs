//! The shell by keys alone (docs/gui/remote-navigation.md): history,
//! couch mode's pane cycle and its way out, and focus that was lost.

use std::cell::Cell;
use std::rc::Rc;

use ctrlassist_gui::{AppWindow, Couch, Nav, Pane, keys, nav};
use slint::ComponentHandle;
use slint::platform::Key;

use crate::support::{find_by_accessible_id, press, press_shift_tab, press_with_alt, settle};

fn app() -> AppWindow {
    i_slint_backend_testing::init_no_event_loop();
    let app = AppWindow::new().expect("the window builds on the testing backend");
    let _ = nav::wire(&app);
    keys::bind_key_router(&app);
    app.show().expect("the testing backend shows a window");
    settle();
    app
}

fn activate(app: &AppWindow, id: &str) {
    find_by_accessible_id(app, id)
        .expect("the sidebar item is in the window")
        .invoke_accessible_default_action();
    settle();
}

#[test]
fn alt_arrows_walk_the_history_of_panes() {
    let app = app();
    activate(&app, "sidebar-settings");
    activate(&app, "sidebar-about");
    assert_eq!(app.get_active_pane(), Pane::About);
    press_with_alt(&app, Key::LeftArrow);
    assert_eq!(app.get_active_pane(), Pane::Settings);
    press_with_alt(&app, Key::LeftArrow);
    assert_eq!(app.get_active_pane(), Pane::Pads);
    // Nothing further back.
    press_with_alt(&app, Key::LeftArrow);
    assert_eq!(app.get_active_pane(), Pane::Pads);
    press_with_alt(&app, Key::RightArrow);
    assert_eq!(app.get_active_pane(), Pane::Settings);
}

#[test]
fn tab_cycles_the_panes_in_couch_mode_only() {
    let app = app();
    app.invoke_focus_active_sidebar_item();
    settle();
    // On a desktop Tab walks the stops and no pane changes.
    press(&app, Key::Tab);
    assert_eq!(app.get_active_pane(), Pane::Pads);

    app.global::<Couch>().set_active(true);
    app.invoke_focus_active_sidebar_item();
    settle();
    press(&app, Key::Tab);
    assert_eq!(app.get_active_pane(), Pane::Setups);
    press_shift_tab(&app);
    assert_eq!(app.get_active_pane(), Pane::Pads);
    // Around the end.
    press_shift_tab(&app);
    assert_eq!(app.get_active_pane(), Pane::About);
    press(&app, Key::Tab);
    assert_eq!(app.get_active_pane(), Pane::Pads);
}

#[test]
fn escape_on_the_sidebar_closes_the_window_in_couch_mode_only() {
    let app = app();
    let closed = Rc::new(Cell::new(0));
    let count = closed.clone();
    app.global::<Nav>()
        .on_close(move || count.set(count.get() + 1));
    app.invoke_focus_active_sidebar_item();
    settle();
    press(&app, Key::Escape);
    assert_eq!(
        closed.get(),
        0,
        "a desktop's Escape on the sidebar does nothing"
    );

    app.global::<Couch>().set_active(true);
    press(&app, Key::Escape);
    assert_eq!(closed.get(), 1);
}

#[test]
fn a_key_nothing_saw_puts_focus_back_on_the_sidebar() {
    let app = app();
    // Nothing has focus, so no handler counts a key: the same count
    // before and after is how the winit hook knows.
    let seen = keys::key_seen(&app);
    assert!(keys::recover_if_unhandled(&app, seen));
    settle();
    assert_eq!(app.global::<Nav>().get_focus_id(), "sidebar-pads");
    // A key the shell did see leaves focus alone.
    let seen = keys::key_seen(&app);
    press(&app, Key::DownArrow);
    assert!(!keys::recover_if_unhandled(&app, seen));
}
