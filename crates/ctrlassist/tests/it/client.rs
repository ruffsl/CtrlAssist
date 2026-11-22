//! The SDK against a live daemon, and against a stand-in where the test
//! needs a daemon that misbehaves: availability, the release check,
//! hydration, the mirror and the watcher's events.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ctrlassist::{DaemonClient, Error, StateEvent, StateWatcher, StateWatcherConfig};
use ctrlassist_daemon::{Daemon, Paths};
use ctrlassist_device::InputThread;
use ctrlassist_protocol::{VERSION, keys};
use serde_json::json;
use tempfile::TempDir;

use crate::fake::{FakeDaemon, answer_info};

/// How long a case waits for the client to converge.
const CONVERGE: Duration = Duration::from_secs(20);

/// A socket path short enough for the kernel's limit.
pub(crate) fn socket() -> (TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("ca")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = dir.path().join("zenoh.sock");
    (dir, socket)
}

/// Poll `check` until it holds, failing with `what` at the deadline.
pub(crate) async fn eventually<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + CONVERGE;
    while !check().await {
        assert!(tokio::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(crate) async fn next_matching(
    watcher: &mut StateWatcher,
    want: impl Fn(&StateEvent) -> bool,
) -> StateEvent {
    tokio::time::timeout(CONVERGE, async {
        loop {
            let event = watcher.next().await.expect("watcher open");
            if want(&event) {
                return event;
            }
        }
    })
    .await
    .expect("the event never arrived")
}

pub(crate) async fn start_daemon(socket: &std::path::Path) -> Daemon {
    start_daemon_taking(socket, true).await
}

/// A daemon that takes the tests' fake pads for pads, or one that
/// doesn't, as a daemon in use doesn't.
pub(crate) async fn start_daemon_taking(socket: &std::path::Path, test_pads: bool) -> Daemon {
    let (input, reports) = InputThread::spawn().unwrap();
    let paths = Paths {
        socket: socket.to_path_buf(),
        state: socket.with_file_name("state"),
        config: socket.with_file_name("config"),
        hider: socket.with_file_name("hider.sock"),
        test_pads,
        // Steam's, if a test puts one there; never the user's own.
        steam: vec![socket.with_file_name("config.vdf")],
    };
    Daemon::start(&paths, input, reports).await.unwrap()
}

/// No start-order assumptions: a client opened before the daemon's
/// socket even exists connects once the daemon appears, hydrates, and
/// reads the daemon's state; the state goes when the daemon does, and the
/// next lifetime is mirrored afresh.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_follows_the_daemon_through_its_lifetimes() {
    let (_dir, socket) = socket();
    let client = DaemonClient::connect(&socket).await.unwrap();
    let mut watcher = StateWatcher::new(&client, StateWatcherConfig::default());
    assert!(!client.is_available(), "nothing is listening yet");
    assert!(!client.is_hydrated());
    assert!(client.service_info().is_none());
    assert_eq!(client.daemon_state().unwrap(), None);
    assert!(matches!(client.version().await, Err(Error::Unavailable)));

    for lifetime in 1..=2 {
        let daemon = start_daemon(&socket).await;
        next_matching(&mut watcher, |e| *e == StateEvent::DaemonAppeared).await;
        assert!(client.is_available(), "lifetime {lifetime}");
        next_matching(&mut watcher, |e| *e == StateEvent::Hydrated).await;
        assert!(client.is_hydrated(), "lifetime {lifetime}");
        // Hydrated means the listed state is in: no wait before the read.
        let state = client.daemon_state().unwrap().expect("hydrated");
        assert_eq!(state.version, VERSION);
        assert_eq!(state.realtime, daemon.is_realtime());
        assert_eq!(client.service_info().unwrap().version, VERSION);
        assert_eq!(client.version().await.unwrap(), VERSION);
        client.check_release().await.unwrap();

        daemon.stop().await;
        next_matching(&mut watcher, |e| *e == StateEvent::DaemonStateChanged).await;
        next_matching(&mut watcher, |e| *e == StateEvent::DaemonDisappeared).await;
        assert!(!client.is_available(), "lifetime {lifetime}");
        assert!(!client.is_hydrated(), "lifetime {lifetime}");
        assert_eq!(client.daemon_state().unwrap(), None);
        assert!(
            client.service_info().is_none(),
            "the release clears before the disappearance is announced"
        );
    }
}

/// A watcher built once the daemon is up and checked still hears of
/// both, and a change the daemon publishes reaches the mirror.
#[tokio::test(flavor = "multi_thread")]
async fn a_watcher_built_late_hears_what_already_happened() {
    let (_dir, socket) = socket();
    let mut daemon = FakeDaemon::start(&socket, VERSION).await;
    daemon
        .put(
            &keys::daemon(),
            &json!({"version": VERSION, "realtime": false, "hiding": "grab"}),
        )
        .await;
    let client = DaemonClient::connect(&socket).await.unwrap();
    eventually("the client never hydrated", async || client.is_hydrated()).await;
    assert!(!client.daemon_state().unwrap().unwrap().realtime);

    let mut watcher = StateWatcher::new(&client, StateWatcherConfig::default());
    next_matching(&mut watcher, |e| *e == StateEvent::DaemonAppeared).await;
    next_matching(&mut watcher, |e| *e == StateEvent::ReleaseChecked).await;

    daemon
        .put(
            &keys::daemon(),
            &json!({"version": VERSION, "realtime": true, "hiding": "grab"}),
        )
        .await;
    next_matching(&mut watcher, |e| *e == StateEvent::DaemonStateChanged).await;
    assert!(client.daemon_state().unwrap().unwrap().realtime);

    daemon.delete(&keys::daemon()).await;
    next_matching(&mut watcher, |e| *e == StateEvent::DaemonStateChanged).await;
    assert_eq!(client.daemon_state().unwrap(), None);
    daemon.stop().await;
}

/// Dropping every copy of a client releases its session: its link, and
/// the tasks that follow the daemon, don't outlive it.
#[tokio::test(flavor = "multi_thread")]
async fn a_dropped_client_closes_its_link() {
    let (_dir, socket) = socket();
    let daemon = FakeDaemon::start(&socket, VERSION).await;
    let client = DaemonClient::connect(&socket).await.unwrap();
    let copy = client.clone();
    eventually("the client never linked", async || {
        client.is_hydrated() && daemon.peers().await != 0
    })
    .await;

    drop(client);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(daemon.peers().await, 1, "a remaining copy keeps the link");
    drop(copy);
    eventually("the dropped client's link stayed up", async || {
        daemon.peers().await == 0
    })
    .await;
}

/// A daemon on another release line is refused, gated reads included,
/// until it restarts on a matching release.
#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_on_another_release_is_refused_until_it_restarts() {
    let (_dir, socket) = socket();
    let daemon = FakeDaemon::start(&socket, "99.0.0").await;
    let client = DaemonClient::connect(&socket).await.unwrap();
    let mut watcher = StateWatcher::new(&client, StateWatcherConfig::default());

    // The check runs as soon as the daemon appears, so the watcher hears
    // of its release, and of the mismatch, without a call.
    next_matching(&mut watcher, |e| *e == StateEvent::ReleaseChecked).await;
    let event = next_matching(&mut watcher, |e| {
        matches!(e, StateEvent::VersionMismatch { .. })
    })
    .await;
    assert_eq!(
        event,
        StateEvent::VersionMismatch {
            ours: VERSION.to_string(),
            theirs: "99.0.0".to_string(),
        }
    );
    assert!(matches!(
        client.check_release().await,
        Err(Error::VersionMismatch { .. })
    ));
    assert!(matches!(
        client.daemon_state(),
        Err(Error::VersionMismatch { .. })
    ));
    assert_eq!(client.version().await.unwrap(), "99.0.0", "ungated");
    assert_eq!(client.service_info().unwrap().version, "99.0.0", "ungated");
    // A mismatched daemon lists nothing this client can read, so there is
    // nothing to wait for.
    eventually(
        "a mismatched daemon never counted as hydrated",
        async || client.is_hydrated(),
    )
    .await;

    // The daemon restarts on this release: its next lifetime is checked
    // afresh, and calls go through.
    daemon.stop().await;
    next_matching(&mut watcher, |e| *e == StateEvent::DaemonDisappeared).await;
    assert!(client.service_info().is_none());
    let _restarted = FakeDaemon::start(&socket, VERSION).await;
    next_matching(&mut watcher, |e| *e == StateEvent::DaemonAppeared).await;
    next_matching(&mut watcher, |e| *e == StateEvent::ReleaseChecked).await;
    assert_eq!(client.service_info().unwrap().version, VERSION);
    client.check_release().await.unwrap();
    assert!(client.daemon_state().is_ok());
}

/// A watcher built after the mismatch is already known still hears of it:
/// the announcement went out before it subscribed, so it replays it.
#[tokio::test(flavor = "multi_thread")]
async fn a_watcher_built_late_still_hears_of_a_known_mismatch() {
    let (_dir, socket) = socket();
    let _daemon = FakeDaemon::start(&socket, "99.0.0").await;
    let client = DaemonClient::connect(&socket).await.unwrap();
    // A call settles the verdict before any watcher exists.
    assert!(matches!(
        client.check_release().await,
        Err(Error::VersionMismatch { .. })
    ));
    let mut watcher = StateWatcher::new(&client, StateWatcherConfig::default());
    next_matching(&mut watcher, |e| *e == StateEvent::ReleaseChecked).await;
    let event = next_matching(&mut watcher, |e| {
        matches!(e, StateEvent::VersionMismatch { .. })
    })
    .await;
    assert!(matches!(
        event,
        StateEvent::VersionMismatch { theirs, .. } if theirs == "99.0.0"
    ));
}

/// A release check that fails is retried while the daemon stays up, so
/// its release is known without any call: here the daemon's liveliness
/// token comes up before it serves its `info` rpc.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_release_check_is_retried() {
    let (_dir, socket) = socket();
    let session = zenoh::open(ctrlassist_protocol::bootstrap::peer_config(&socket).unwrap())
        .await
        .unwrap();
    let _token = session
        .liveliness()
        .declare_token(keys::daemon_alive())
        .await
        .unwrap();
    let client = DaemonClient::connect(&socket).await.unwrap();
    let mut watcher = StateWatcher::new(&client, StateWatcherConfig::default());
    next_matching(&mut watcher, |e| *e == StateEvent::DaemonAppeared).await;
    // Outlast the first check, which waits out the matching deadline.
    tokio::time::sleep(ctrlassist_protocol::caller::MATCHING_DEADLINE + Duration::from_millis(500))
        .await;
    assert!(client.service_info().is_none());
    let _serve = answer_info(&session, VERSION, &Arc::new(Mutex::new(BTreeSet::new()))).await;
    next_matching(&mut watcher, |e| *e == StateEvent::ReleaseChecked).await;
    assert_eq!(client.service_info().unwrap().version, VERSION);
}

/// A listed key that never arrives (deleted after the `info` reply) holds
/// hydration only until the deadline.
#[tokio::test(flavor = "multi_thread")]
async fn hydration_ends_at_the_deadline_without_a_listed_key() {
    let (_dir, socket) = socket();
    let daemon = FakeDaemon::start(&socket, VERSION).await;
    daemon.list_unpublished(&keys::daemon());
    let client = DaemonClient::connect(&socket).await.unwrap();
    eventually("the daemon never appeared", async || {
        client.service_info().is_some()
    })
    .await;
    assert!(!client.is_hydrated(), "a listed key is missing");
    eventually("hydration never ended", async || client.is_hydrated()).await;
    assert_eq!(client.daemon_state().unwrap(), None);
}

/// `try_next` hands over a queued event without waiting, and nothing when
/// the queue is empty.
#[tokio::test(flavor = "multi_thread")]
async fn try_next_drains_without_waiting() {
    let (_dir, socket) = socket();
    let client = DaemonClient::connect(&socket).await.unwrap();
    let mut idle = StateWatcher::new(&client, StateWatcherConfig::default());
    assert!(idle.try_next().is_none(), "no daemon, no events");
    let mut config = StateWatcherConfig::default();
    config.fallback_poll_interval = Duration::ZERO;
    let mut silent = StateWatcher::new(&client, config.clone());
    config.fallback_poll_interval = Duration::from_millis(20);
    let mut polling = StateWatcher::new(&client, config);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(polling.try_next(), Some(StateEvent::FallbackPoll));
    assert!(silent.try_next().is_none(), "zero turns the poll off");
}

/// A tray's announcement reaches other clients through the daemon's own
/// state, since one client of the daemon's socket can't see another:
/// there while the tray is, gone when it goes, and there again in a
/// daemon that starts later.
#[tokio::test(flavor = "multi_thread")]
async fn a_tray_s_presence_is_the_daemon_s_to_report() {
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let window = DaemonClient::connect(&socket).await.unwrap();
    let tray_of =
        |client: &DaemonClient| client.daemon_state().unwrap().and_then(|state| state.tray);
    eventually("the daemon's state never arrived", async || {
        window.daemon_state().unwrap().is_some()
    })
    .await;
    assert_eq!(tray_of(&window), None);

    let tray = DaemonClient::connect(&socket).await.unwrap();
    let presence = tray.announce_tray().await.unwrap();
    eventually("the tray was never reported", async || {
        tray_of(&window).as_deref() == Some(VERSION)
    })
    .await;

    // A daemon that starts later hears of the tray that was already there.
    daemon.stop().await;
    eventually("the daemon never went", async || !window.is_available()).await;
    let daemon = start_daemon(&socket).await;
    eventually("the tray wasn't reported by the next daemon", async || {
        tray_of(&window).as_deref() == Some(VERSION)
    })
    .await;

    drop(presence);
    eventually("the tray was still reported after it went", async || {
        window.is_available() && tray_of(&window).is_none()
    })
    .await;
    daemon.stop().await;
}
