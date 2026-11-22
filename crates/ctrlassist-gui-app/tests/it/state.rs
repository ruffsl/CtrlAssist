//! The daemon's state in the window: what a snapshot of the mirror
//! says, and what the window shows of it.

use ctrlassist::{DaemonClient, Problem, Profile, Reloaded};
use ctrlassist_daemon::{Daemon, Paths};
use ctrlassist_device::InputThread;
use ctrlassist_gui::state::{Snapshot, health, reload_banner, reload_summary};
use ctrlassist_gui::{AppWindow, Errors, ServiceHealth};
use ctrlassist_protocol::VERSION;
use slint::ComponentHandle;

use crate::support::find_by_accessible_id;

fn app() -> AppWindow {
    i_slint_backend_testing::init_no_event_loop();
    AppWindow::new().expect("the window builds on the testing backend")
}

#[test]
fn health_goes_by_whether_a_service_runs_and_the_release_it_reports() {
    assert_eq!(health(false, None), ServiceHealth::Offline);
    assert_eq!(health(false, Some(VERSION)), ServiceHealth::Offline);
    // Running, and nothing against it yet.
    assert_eq!(health(true, None), ServiceHealth::Ok);
    assert_eq!(health(true, Some(VERSION)), ServiceHealth::Ok);
    assert_eq!(health(true, Some("99.0.0")), ServiceHealth::Incompatible);
    assert_eq!(
        health(true, Some("not a release")),
        ServiceHealth::Incompatible
    );
}

#[test]
fn the_window_shows_a_snapshot_and_the_banner_for_another_release() {
    let app = app();
    // As it opens: nothing there.
    assert!(!app.get_daemon_available());
    assert_eq!(app.get_daemon_health(), ServiceHealth::Offline);

    let loading = Snapshot {
        available: true,
        hydrated: false,
        version: None,
        hidden: None,
        ..Snapshot::default()
    };
    loading.show(&app);
    assert!(app.get_daemon_available());
    assert!(app.get_daemon_loading());
    assert_eq!(app.get_daemon_health(), ServiceHealth::Ok);
    assert_eq!(app.get_hider_health(), ServiceHealth::Offline);
    assert!(
        find_by_accessible_id(&app, "sidebar-reload").is_some(),
        "Reload is there to press"
    );

    let running = Snapshot {
        available: true,
        hydrated: true,
        version: Some(VERSION.to_string()),
        hidden: Some(true),
        ..Snapshot::default()
    };
    running.show(&app);
    assert!(!app.get_daemon_loading());
    assert_eq!(app.get_hider_health(), ServiceHealth::Ok);
    assert_eq!(app.global::<Errors>().get_version().title, "");

    // Pads only grabbed: the hiding service isn't there.
    Snapshot {
        hidden: Some(false),
        ..running.clone()
    }
    .show(&app);
    assert_eq!(app.get_hider_health(), ServiceHealth::Offline);

    Snapshot {
        version: Some("99.0.0".to_string()),
        ..running
    }
    .show(&app);
    assert_eq!(app.get_daemon_health(), ServiceHealth::Incompatible);
    let banner = app.global::<Errors>().get_version();
    assert!(banner.message.contains("99.0.0"), "{}", banner.message);
    // Gone again, and the banner with it.
    Snapshot::default().show(&app);
    assert_eq!(app.get_daemon_health(), ServiceHealth::Offline);
    assert_eq!(app.global::<Errors>().get_version().title, "");
}

#[test]
fn a_reload_is_summed_up_and_its_wrong_profiles_listed() {
    assert_eq!(
        reload_summary(&Reloaded::default()),
        "Nothing changed on disk"
    );
    let reloaded = Reloaded {
        changed: vec!["couch".into(), "den".into()],
        removed: vec!["desk".into()],
        wrong: vec![Profile {
            name: "den".into(),
            toml: String::new(),
            problem: Some(Problem {
                at: "preset".into(),
                message: "needs a primary".into(),
            }),
        }],
        updating: vec!["couch".into()],
    };
    assert_eq!(
        reload_summary(&reloaded),
        "Profiles: 2 read again, 1 gone, 1 running updated"
    );
    assert_eq!(reload_banner(&Reloaded::default()).title, "");
    let banner = reload_banner(&reloaded);
    assert_eq!(banner.message, "den.toml: preset: needs a primary");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_snapshot_reads_a_live_daemon_and_its_going() {
    let dir = tempfile::Builder::new()
        .prefix("ca")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = dir.path().join("zenoh.sock");
    let client = DaemonClient::connect(&socket).await.unwrap();
    assert_eq!(Snapshot::read(&client, None), Snapshot::default());

    let (input, reports) = InputThread::spawn().unwrap();
    let paths = Paths {
        socket: socket.clone(),
        state: socket.with_file_name("state"),
        config: socket.with_file_name("config"),
        hider: socket.with_file_name("hider.sock"),
        test_pads: false,
        steam: vec![socket.with_file_name("config.vdf")],
    };
    let daemon = Daemon::start(&paths, input, reports).await.unwrap();
    let mut snapshot = Snapshot::default();
    for _ in 0..500 {
        snapshot = Snapshot::read(&client, None);
        if snapshot.hydrated && snapshot.version.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        snapshot,
        Snapshot {
            available: true,
            hydrated: true,
            version: Some(VERSION.to_string()),
            // No hiding service at that socket: pads would be grabbed.
            hidden: Some(false),
            config: socket.with_file_name("config").display().to_string(),
            // Granted or not is the machine's to say.
            realtime: snapshot.realtime,
            tray_available: false,
            tray: None,
            tray_config: String::new(),
            notifications: None,
            // Whatever pads this machine has are its own.
            pads: snapshot.pads.clone(),
            setups: Vec::new(),
            profiles: Vec::new(),
            // Asked for by the loop that follows the daemon, not read.
            fields: Vec::new(),
        }
    );
    assert_eq!(snapshot.daemon_health(), ServiceHealth::Ok);

    daemon.stop().await;
    for _ in 0..500 {
        snapshot = Snapshot::read(&client, None);
        if !snapshot.available {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(snapshot.daemon_health(), ServiceHealth::Offline);
    assert_eq!(snapshot.hidden, None);
}
