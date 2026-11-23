//! The Settings pane: the tray's notification switches as the window
//! shows them, and what each asks of the tray.

use std::cell::RefCell;
use std::rc::Rc;

use ctrlassist::{NotificationSettings, NotificationSettingsPatch};
use ctrlassist_gui::settings::{patch, shown};
use ctrlassist_gui::state::Snapshot;
use ctrlassist_gui::{AppWindow, Couch, Pane, Tray};
use slint::ComponentHandle;

use crate::support::{find_by_accessible_id, settle};

/// The window on the Settings pane, tall enough that nothing of the
/// pane is scrolled out of the element tree.
fn app() -> AppWindow {
    i_slint_backend_testing::init_no_event_loop();
    let app = AppWindow::new().expect("the window builds on the testing backend");
    app.set_active_pane(Pane::Settings);
    app.window()
        .set_size(slint::LogicalSize::new(1000.0, 1600.0));
    settle();
    app
}

fn switch_is(app: &AppWindow, id: &str) -> (bool, bool) {
    let switch = find_by_accessible_id(app, id).unwrap_or_else(|| unreachable!("{id} is there"));
    (
        switch.accessible_checked().unwrap_or(false),
        switch.accessible_enabled().unwrap_or(false),
    )
}

const SWITCHES: [&str; 4] = [
    "settings-notify-setup-started",
    "settings-notify-setup-trouble",
    "settings-notify-pad-connection",
    "settings-notify-refused",
];

#[test]
fn each_switch_asks_for_its_own_change_and_nothing_else() {
    for (key, changed) in [
        (
            "setup_started",
            NotificationSettingsPatch {
                setup_started: Some(false),
                ..NotificationSettingsPatch::default()
            },
        ),
        (
            "setup_trouble",
            NotificationSettingsPatch {
                setup_trouble: Some(false),
                ..NotificationSettingsPatch::default()
            },
        ),
        (
            "pad_connection",
            NotificationSettingsPatch {
                pad_connection: Some(false),
                ..NotificationSettingsPatch::default()
            },
        ),
        (
            "refused",
            NotificationSettingsPatch {
                refused: Some(false),
                ..NotificationSettingsPatch::default()
            },
        ),
    ] {
        assert_eq!(patch(key, false), Some(changed), "{key}");
    }
    assert_eq!(patch("something-else", true), None);
    let all_but_one = NotificationSettings {
        pad_connection: false,
        ..NotificationSettings::default()
    };
    let shown = shown(all_but_one);
    assert!(shown.setup_started && shown.setup_trouble && shown.refused);
    assert!(!shown.pad_connection);
}

/// With no tray the switches are there, as last known and disabled,
/// under a line that says why; with one they are the tray's own and can
/// be set.
#[test]
fn the_switches_follow_the_tray_and_wait_for_one() {
    let app = app();
    assert!(find_by_accessible_id(&app, "settings-tray-offline").is_some());
    for id in SWITCHES {
        assert_eq!(switch_is(&app, id), (true, false), "{id}");
    }
    let test = find_by_accessible_id(&app, "settings-notify-test").unwrap();
    assert_eq!(test.accessible_enabled(), Some(false));

    Snapshot {
        available: true,
        tray_available: true,
        tray_config: "/somewhere/tray.toml".to_string(),
        notifications: Some(NotificationSettings {
            pad_connection: false,
            ..NotificationSettings::default()
        }),
        ..Snapshot::default()
    }
    .show(&app);
    settle();
    assert!(find_by_accessible_id(&app, "settings-tray-offline").is_none());
    assert_eq!(
        switch_is(&app, "settings-notify-pad-connection"),
        (false, true)
    );
    assert_eq!(switch_is(&app, "settings-notify-refused"), (true, true));
    assert_eq!(
        app.global::<Tray>().get_config_path(),
        "/somewhere/tray.toml"
    );

    // The tray gone again, they stay as it left them.
    Snapshot::default().show(&app);
    settle();
    assert!(find_by_accessible_id(&app, "settings-tray-offline").is_some());
    assert_eq!(
        switch_is(&app, "settings-notify-pad-connection"),
        (false, false)
    );
    assert_eq!(
        app.global::<Tray>().get_config_path(),
        "/somewhere/tray.toml"
    );
}

/// A switch and the test button reach the tray by the pane's callbacks,
/// each switch under the tray's own key for it.
#[test]
fn a_switch_and_the_test_button_ask_the_tray() {
    let app = app();
    Snapshot {
        available: true,
        tray_available: true,
        notifications: Some(NotificationSettings::default()),
        ..Snapshot::default()
    }
    .show(&app);
    settle();
    let asked = Rc::new(RefCell::new(Vec::new()));
    let tray = app.global::<Tray>();
    tray.on_set_notification({
        let asked = Rc::clone(&asked);
        move |key, on| asked.borrow_mut().push(format!("{key}={on}"))
    });
    tray.on_send_test_notification({
        let asked = Rc::clone(&asked);
        move || asked.borrow_mut().push("test".to_string())
    });
    for id in SWITCHES {
        find_by_accessible_id(&app, id)
            .unwrap()
            .invoke_accessible_default_action();
        settle();
    }
    find_by_accessible_id(&app, "settings-notify-test")
        .unwrap()
        .invoke_accessible_default_action();
    settle();
    assert_eq!(
        *asked.borrow(),
        [
            "setup_started=false",
            "setup_trouble=false",
            "pad_connection=false",
            "refused=false",
            "test"
        ]
    );
}

/// A switch asks and shows what the tray then publishes, after a toggle
/// as before it: a refused request leaves it showing the tray's
/// setting, and a change made elsewhere reaches it.
#[test]
fn a_switch_shows_the_tray_s_setting_and_not_what_was_asked() {
    let app = app();
    let tray_says = |setup_started: bool| {
        Snapshot {
            available: true,
            tray_available: true,
            notifications: Some(NotificationSettings {
                setup_started,
                ..NotificationSettings::default()
            }),
            ..Snapshot::default()
        }
        .show(&app);
        settle();
    };
    tray_says(true);
    let asked = Rc::new(RefCell::new(Vec::new()));
    app.global::<Tray>().on_set_notification({
        let asked = Rc::clone(&asked);
        move |key, on| asked.borrow_mut().push(format!("{key}={on}"))
    });
    let id = "settings-notify-setup-started";
    find_by_accessible_id(&app, id)
        .unwrap()
        .invoke_accessible_default_action();
    settle();
    assert_eq!(*asked.borrow(), ["setup_started=false"]);
    // Nothing published yet, or the tray refused: as it was.
    assert_eq!(switch_is(&app, id), (true, true));
    tray_says(false);
    assert_eq!(switch_is(&app, id), (false, true));
    // Turned on again from the tray's file or another window.
    tray_says(true);
    assert_eq!(switch_is(&app, id), (true, true));
}

/// Couch mode's two controls are in the pane, with where the
/// preference is kept.
#[test]
fn couch_mode_is_set_from_the_pane() {
    let app = app();
    app.global::<Couch>()
        .set_config_path("/somewhere/gui.toml".into());
    settle();
    for id in [
        "settings-couch-mode",
        "settings-couch-scale",
        "settings-help-couch",
        "settings-help-notify",
    ] {
        assert!(find_by_accessible_id(&app, id).is_some(), "{id}");
    }
    // Where the file is kept is About's to say, not this pane's.
    assert!(find_by_accessible_id(&app, "settings-couch-path").is_none());
}
