use std::path::{Path, PathBuf};
use std::time::Duration;

use ctrlassist::{
    DaemonClient, ErrorCode, NotificationSettings, NotificationSettingsPatch, TrayClient,
};
use ctrlassist_daemon::{Daemon, Paths};
use ctrlassist_protocol::VERSION;
use ctrlassist_tray::notify::{Category, Notifier, serve_while_daemon};
use ctrlassist_tray::state::Toast;
use tokio::sync::mpsc;

fn paths(dir: &Path) -> Paths {
    Paths {
        socket: dir.join("zenoh.sock"),
        state: dir.join("state"),
        config: dir.join("config"),
        hider: dir.join("hider.sock"),
        test_pads: true,
        steam: Vec::new(),
    }
}

async fn start_daemon(paths: &Paths) -> Daemon {
    let (input, reports) = Daemon::input_thread().unwrap();
    Daemon::start(paths, input, reports).await.unwrap()
}

async fn eventually(what: &str, mut holds: impl AsyncFnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !holds().await {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn toast(category: Category, summary: &str) -> Toast {
    Toast {
        category,
        summary: summary.to_string(),
        body: String::new(),
        icon: "input-gaming",
    }
}

/// A tray with no desktop: its settings service, and where what it
/// would show goes instead.
struct Tray {
    notifier: Notifier,
    shown: mpsc::UnboundedReceiver<Toast>,
    config: PathBuf,
}

async fn start_tray(paths: &Paths) -> Tray {
    let config = paths.config.join("tray.toml");
    let (notifier, shown) = Notifier::capturing(NotificationSettings::default());
    let daemon = DaemonClient::connect(&paths.socket).await.unwrap();
    tokio::spawn(serve_while_daemon(
        daemon,
        paths.socket.clone(),
        config.clone(),
        notifier.clone(),
    ));
    Tray {
        notifier,
        shown,
        config,
    }
}

/// A window, as far as the tray goes: the two clients on one session.
async fn window(paths: &Paths) -> (DaemonClient, TrayClient) {
    let daemon = DaemonClient::connect(&paths.socket).await.unwrap();
    let tray = TrayClient::beside(&daemon).await.unwrap();
    (daemon, tray)
}

/// What the tray serves reaches another client of the daemon: that it
/// is there, its release and file, and its switches; a switch set from
/// there is kept, published and obeyed.
#[tokio::test(flavor = "multi_thread")]
async fn the_tray_s_switches_are_read_and_set_through_the_daemon() {
    let dir = tempfile::Builder::new()
        .prefix("ca")
        .tempdir_in("/tmp")
        .unwrap();
    let paths = paths(dir.path());
    let daemon = start_daemon(&paths).await;
    let (_window, client) = window(&paths).await;
    assert!(!client.is_available(), "no tray yet");
    assert_eq!(client.notification_settings(), None);

    let mut tray = start_tray(&paths).await;
    eventually("the tray's switches never arrived", async || {
        client.notification_settings().is_some()
    })
    .await;
    assert!(client.is_available());
    assert_eq!(
        client.notification_settings(),
        Some(NotificationSettings::default())
    );
    eventually("the tray never said its release", async || {
        client.service_info().is_some()
    })
    .await;
    let info = client.service_info().unwrap();
    assert_eq!(info.version, VERSION);
    assert_eq!(info.config, tray.config.display().to_string());

    // One switch off: published, kept, and obeyed.
    client
        .update_notification_settings(NotificationSettingsPatch {
            pad_connection: Some(false),
            ..NotificationSettingsPatch::default()
        })
        .await
        .unwrap();
    let wanted = NotificationSettings {
        pad_connection: false,
        ..NotificationSettings::default()
    };
    eventually("the switch never changed in the mirror", async || {
        client.notification_settings() == Some(wanted)
    })
    .await;
    assert_eq!(client.notification_settings_status().await.unwrap(), wanted);
    let kept = std::fs::read_to_string(&tray.config).unwrap();
    assert!(kept.contains("pad_connection = false"), "{kept}");
    assert!(kept.contains("refused = true"), "{kept}");
    tray.notifier
        .notify(toast(Category::PadConnection, "a pad connected"));
    tray.notifier.notify(toast(Category::Refused, "refused"));
    assert_eq!(tray.shown.recv().await.unwrap().summary, "refused");

    // The test button is shown whatever the switches say.
    client
        .update_notification_settings(NotificationSettingsPatch {
            refused: Some(false),
            ..NotificationSettingsPatch::default()
        })
        .await
        .unwrap();
    client.send_test_notification().await.unwrap();
    assert_eq!(
        tray.shown.recv().await.unwrap().summary,
        "Test notification"
    );

    // A hand edit is taken up when asked, and one that isn't settings
    // is refused and changes nothing.
    std::fs::write(&tray.config, "schema = 1\nsetup_started = false\n").unwrap();
    assert!(client.reload_config().await.unwrap().changed);
    let edited = NotificationSettings {
        setup_started: false,
        ..NotificationSettings::default()
    };
    eventually("the hand edit never reached the mirror", async || {
        client.notification_settings() == Some(edited)
    })
    .await;
    assert!(!client.reload_config().await.unwrap().changed);
    std::fs::write(&tray.config, "setup_started = \"no\"\n").unwrap();
    let refused = client.reload_config().await.unwrap_err();
    assert_eq!(refused.rpc_code(), Some(ErrorCode::SettingsUnreadable));
    assert_eq!(client.notification_settings(), Some(edited));
    daemon.stop().await;
}

/// The tray serves through the daemon, so it starts serving when a
/// daemon comes, is gone with it, and is back with the next, with what
/// it was told kept.
#[tokio::test(flavor = "multi_thread")]
async fn the_tray_is_served_through_each_daemon_that_comes() {
    let dir = tempfile::Builder::new()
        .prefix("ca")
        .tempdir_in("/tmp")
        .unwrap();
    let paths = paths(dir.path());
    // The tray and the window first, with nobody listening.
    let _tray = start_tray(&paths).await;
    let (window, client) = window(&paths).await;
    assert!(!client.is_available());

    let daemon = start_daemon(&paths).await;
    eventually("the tray wasn't served through the daemon", async || {
        client.notification_settings().is_some()
    })
    .await;
    client
        .update_notification_settings(NotificationSettingsPatch {
            setup_trouble: Some(false),
            ..NotificationSettingsPatch::default()
        })
        .await
        .unwrap();

    daemon.stop().await;
    eventually("the tray was still seen with no daemon", async || {
        !window.is_available() && !client.is_available()
    })
    .await;
    assert_eq!(client.notification_settings(), None);

    let daemon = start_daemon(&paths).await;
    eventually(
        "the tray wasn't served through the next daemon",
        async || client.notification_settings().is_some(),
    )
    .await;
    assert!(!client.notification_settings().unwrap().setup_trouble);
    daemon.stop().await;
}
