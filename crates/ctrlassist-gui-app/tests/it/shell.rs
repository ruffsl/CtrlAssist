//! The shell: the sidebar's items and the pane each shows.

use ctrlassist_gui::{AppWindow, Pane, keys, nav};
use slint::ComponentHandle;
use slint::platform::Key;

use crate::support::{find_by_accessible_id, press, settle};

fn app() -> AppWindow {
    i_slint_backend_testing::init_no_event_loop();
    let app = AppWindow::new().expect("the window builds on the testing backend");
    let _ = nav::wire(&app);
    keys::bind_key_router(&app);
    app.show().expect("the testing backend shows a window");
    settle();
    app
}

#[test]
fn the_sidebar_lists_every_pane_and_reload() {
    let app = app();
    for id in ["pads", "setups", "profiles", "settings", "about", "reload"] {
        assert!(
            find_by_accessible_id(&app, &format!("sidebar-{id}")).is_some(),
            "sidebar-{id}"
        );
    }
    assert_eq!(app.get_active_pane(), Pane::Pads);
}

#[test]
fn activating_an_item_shows_its_pane() {
    let app = app();
    let item = find_by_accessible_id(&app, "sidebar-settings").unwrap();
    item.invoke_accessible_default_action();
    settle();
    assert_eq!(app.get_active_pane(), Pane::Settings);
}

#[test]
fn the_pane_follows_focus_down_the_rail() {
    let app = app();
    app.invoke_focus_active_sidebar_item();
    settle();
    press(&app, Key::DownArrow);
    assert_eq!(app.get_active_pane(), Pane::Setups);
    press(&app, Key::DownArrow);
    assert_eq!(app.get_active_pane(), Pane::Profiles);
    press(&app, Key::UpArrow);
    assert_eq!(app.get_active_pane(), Pane::Setups);
}
