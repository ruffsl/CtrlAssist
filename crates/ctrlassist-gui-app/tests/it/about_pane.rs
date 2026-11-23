//! The About pane: its rows from what the window shows, and the pane
//! itself in the window.

use ctrlassist::Hiding;
use ctrlassist_gui::about::{Parts, refresh, rows};
use ctrlassist_gui::state::Snapshot;
use ctrlassist_gui::{AboutComponentRow, AppWindow, Pane, ServiceHealth};
use ctrlassist_protocol::VERSION;

use crate::support::find_by_accessible_id;

fn row<'a>(rows: &'a [AboutComponentRow], name: &str) -> &'a AboutComponentRow {
    rows.iter()
        .find(|row| row.name == name)
        .expect("the part has a row")
}

#[test]
fn with_nothing_running_every_part_but_the_window_is_absent() {
    let rows = rows(&Parts::default());
    let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
    assert_eq!(names, ["daemon", "hider", "tray", "gui", "cli"]);
    let daemon = row(&rows, "daemon");
    assert_eq!(daemon.health, ServiceHealth::Offline);
    assert_eq!((daemon.chip.as_str(), daemon.online), ("offline", false));
    assert_eq!(daemon.bus_address, "(ctrlassist/alive/daemon)");
    assert_eq!(daemon.config_path, "");
    // Only the daemon can say how the hiding service is.
    assert_eq!(row(&rows, "hider").chip, "unknown");
    assert_eq!(row(&rows, "tray").chip, "unknown");
    let window = row(&rows, "gui");
    assert_eq!(window.health, ServiceHealth::Ok);
    assert_eq!(window.chip.as_str(), format!("v{VERSION}"));
    assert!(
        window.config_path.ends_with("/gui.toml"),
        "{}",
        window.config_path
    );
    let cli = row(&rows, "cli");
    assert_eq!(
        (cli.chip.as_str(), cli.health),
        ("not found", ServiceHealth::Offline)
    );
}

#[test]
fn a_running_daemon_says_its_release_its_files_and_how_pads_are_hidden() {
    let parts = Parts {
        daemon: ServiceHealth::Ok,
        daemon_version: VERSION.to_string(),
        daemon_config: "/home/someone/.config/ctrlassist".to_string(),
        daemon_realtime: true,
        hider: ServiceHealth::Ok,
        hider_version: VERSION.to_string(),
        tray: ServiceHealth::Ok,
        tray_version: VERSION.to_string(),
        cli_version: VERSION.to_string(),
    };
    let all = rows(&parts);
    let daemon = row(&all, "daemon");
    assert_eq!(daemon.chip.as_str(), format!("v{VERSION}"));
    assert_eq!(daemon.tip, "Same release as this window");
    assert_eq!(daemon.config_path, "/home/someone/.config/ctrlassist");
    assert!(!daemon.description.contains("real-time"));
    assert_eq!(row(&all, "hider").chip.as_str(), format!("v{VERSION}"));
    assert_eq!(row(&all, "hider").friendly_name, "Hiding");
    assert_eq!(row(&all, "gui").friendly_name, "GUI");
    // A hiding service from before it could be asked its release.
    let old = rows(&Parts {
        hider_version: String::new(),
        ..parts.clone()
    });
    assert_eq!(row(&old, "hider").chip, "—");
    assert!(row(&old, "hider").tip.contains("too old to say"));
    assert_eq!(row(&all, "tray").chip.as_str(), format!("v{VERSION}"));
    assert_eq!(row(&all, "cli").health, ServiceHealth::Ok);

    // Its release not read yet: there, with nothing against it.
    let unread = rows(&Parts {
        daemon_version: String::new(),
        ..parts.clone()
    });
    assert_eq!(row(&unread, "daemon").chip, "—");
    assert!(row(&unread, "daemon").online);

    // The daemon just started and still looking for the hiding service:
    // said so plainly, in the neutral health, and not as absent.
    let looking = rows(&Parts {
        hider: ServiceHealth::Looking,
        hider_version: String::new(),
        ..parts.clone()
    });
    let hider = row(&looking, "hider");
    assert_eq!(hider.health, ServiceHealth::Looking);
    assert_eq!(hider.chip, "looking…");
    assert!(hider.tip.contains("still looking for the hiding service"));
    assert!(!hider.tip.contains("only grabbed"));

    // Pads only grabbed, no real-time scheduling, another install's
    // command on PATH, and a daemon this window can't talk to.
    let worse = rows(&Parts {
        daemon: ServiceHealth::Incompatible,
        daemon_version: "99.0.0".to_string(),
        daemon_realtime: false,
        hider: ServiceHealth::Offline,
        tray: ServiceHealth::Offline,
        tray_version: String::new(),
        cli_version: "99.0.0".to_string(),
        ..parts
    });
    assert!(row(&worse, "daemon").tip.contains("can't talk"));
    assert!(row(&worse, "daemon").description.contains("real-time"));
    assert_eq!(row(&worse, "hider").chip, "offline");
    assert_eq!(row(&worse, "tray").chip, "offline");
    assert!(row(&worse, "hider").tip.contains("only grabbed"));
    let cli = row(&worse, "cli");
    assert_eq!(cli.health, ServiceHealth::Incompatible);
    assert!(cli.tip.starts_with("From a different install: 99.0.0"));
}

#[test]
fn the_pane_shows_the_rows_the_window_holds() {
    i_slint_backend_testing::init_no_event_loop();
    let app = AppWindow::new().expect("the window builds on the testing backend");
    refresh(&app);
    // Not shown: a shown window's tree leaves out what its scroll view
    // clips, and the notices link is at the pane's foot.
    app.set_active_pane(Pane::About);
    for id in [
        "about-brand-link",
        "about-version-chip",
        "about-issue-button",
        "about-diagnostics-button",
        "about-notices-button",
        "about-help-components",
    ] {
        assert!(find_by_accessible_id(&app, id).is_some(), "{id}");
    }
    use slint::Model as _;
    assert_eq!(app.get_about_components().row_count(), 5);

    // A snapshot of the daemon reaches the rows.
    Snapshot {
        available: true,
        hydrated: true,
        version: Some(VERSION.to_string()),
        hiding: Some(Hiding::Service),
        hider: Some(VERSION.to_string()),
        config: "/somewhere".to_string(),
        realtime: true,
        tray_available: true,
        tray: Some(VERSION.to_string()),
        ..Snapshot::default()
    }
    .show(&app);
    let rows: Vec<AboutComponentRow> = app.get_about_components().iter().collect();
    assert_eq!(row(&rows, "daemon").config_path, "/somewhere");
    assert_eq!(row(&rows, "hider").chip.as_str(), format!("v{VERSION}"));
    assert_eq!(row(&rows, "tray").health, ServiceHealth::Ok);
    assert_eq!(app.get_tray_health(), ServiceHealth::Ok);
}

/// A Copy the clipboard refuses says so, naming what wasn't copied. The
/// clipboard itself isn't driven here: it is the desktop's own.
#[test]
fn a_failed_copy_says_what_wasn_t_copied_and_why() {
    use ctrlassist_gui::about::copy_failed;
    assert_eq!(
        copy_failed("Pad ID", "no data-control protocol"),
        "Couldn't copy Pad ID: no data-control protocol"
    );
    assert_eq!(copy_failed("", "no seat"), "Couldn't copy: no seat");
}

/// Each change the window shows makes About's rows again in the model
/// Slint already has, so the Config buttons in them stay.
#[test]
fn the_rows_are_made_again_in_place() {
    use slint::Model as _;
    i_slint_backend_testing::init_no_event_loop();
    let app = AppWindow::new().expect("the window builds on the testing backend");
    refresh(&app);
    let before = app.get_about_components();
    Snapshot {
        available: true,
        config: "/somewhere".to_string(),
        ..Snapshot::default()
    }
    .show(&app);
    assert!(app.get_about_components() == before);
    assert_eq!(
        before.row_data(0).map(|row| row.config_path),
        Some("/somewhere".into())
    );
}
