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

use crate::fake::{FakeDaemon, answer_info, answer_info_from};

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
    let mut watcher = StateWatcher::new(&client, None, StateWatcherConfig::default());
    assert!(!client.is_available(), "nothing is listening yet");
    assert!(!client.is_hydrated());
    assert!(client.service_info().is_none());
    assert_eq!(client.daemon_state().unwrap(), None);
    assert!(matches!(
        client.version().await,
        Err(Error::Unavailable { .. })
    ));

    for lifetime in 1..=2 {
        let daemon = start_daemon(&socket).await;
        next_matching(&mut watcher, |e| *e == StateEvent::DaemonAppeared).await;
        assert!(client.is_available(), "lifetime {lifetime}");
        next_matching(&mut watcher, |e| {
            *e == StateEvent::Hydrated(ctrlassist::Service::Daemon)
        })
        .await;
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

    // Handed over at once, in order, before anything forwarded.
    let mut watcher = StateWatcher::new(&client, None, StateWatcherConfig::default());
    assert_eq!(watcher.try_next(), Some(StateEvent::DaemonAppeared));
    assert_eq!(
        watcher.try_next(),
        Some(StateEvent::ReleaseChecked(ctrlassist::Service::Daemon))
    );

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
    let mut watcher = StateWatcher::new(&client, None, StateWatcherConfig::default());

    // The check runs as soon as the daemon appears, so the watcher hears
    // of its release, and of the mismatch, without a call.
    next_matching(&mut watcher, |e| {
        *e == StateEvent::ReleaseChecked(ctrlassist::Service::Daemon)
    })
    .await;
    let event = next_matching(&mut watcher, |e| {
        matches!(e, StateEvent::VersionMismatch { .. })
    })
    .await;
    assert_eq!(
        event,
        StateEvent::VersionMismatch {
            service: ctrlassist::Service::Daemon,
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
    next_matching(&mut watcher, |e| {
        *e == StateEvent::ReleaseChecked(ctrlassist::Service::Daemon)
    })
    .await;
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
    // The verdict settles before any watcher exists.
    eventually("the release check never settled", async || {
        client.service_info().is_some()
    })
    .await;
    assert!(matches!(
        client.check_release().await,
        Err(Error::VersionMismatch { .. })
    ));
    let mut watcher = StateWatcher::new(&client, None, StateWatcherConfig::default());
    next_matching(&mut watcher, |e| {
        *e == StateEvent::ReleaseChecked(ctrlassist::Service::Daemon)
    })
    .await;
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
    let session = zenoh::open(ctrlassist_protocol::bootstrap::router_config(&socket).unwrap())
        .await
        .unwrap();
    let _token = session
        .liveliness()
        .declare_token(keys::daemon_alive())
        .await
        .unwrap();
    let client = DaemonClient::connect(&socket).await.unwrap();
    let mut watcher = StateWatcher::new(&client, None, StateWatcherConfig::default());
    next_matching(&mut watcher, |e| *e == StateEvent::DaemonAppeared).await;
    // Outlast the first check, which waits out the matching deadline.
    tokio::time::sleep(ctrlassist_protocol::caller::MATCHING_DEADLINE + Duration::from_millis(500))
        .await;
    assert!(client.service_info().is_none());
    let _serve = answer_info(&session, VERSION, &Arc::new(Mutex::new(BTreeSet::new()))).await;
    next_matching(&mut watcher, |e| {
        *e == StateEvent::ReleaseChecked(ctrlassist::Service::Daemon)
    })
    .await;
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

/// A daemon that has withdrawn its token, and still answers while it
/// drains, can't settle the release check for the daemon that comes
/// next: that one is checked afresh once it appears, and its check is
/// announced after its appearance.
#[tokio::test(flavor = "multi_thread")]
async fn a_draining_daemon_s_release_is_not_kept_for_the_next() {
    let (_dir, socket) = socket();
    let session = zenoh::open(ctrlassist_protocol::bootstrap::router_config(&socket).unwrap())
        .await
        .unwrap();
    let release = Arc::new(Mutex::new("99.0.0".to_string()));
    let _serve = answer_info_from(&session, &release, &Arc::default()).await;
    let token = session
        .liveliness()
        .declare_token(keys::daemon_alive())
        .await
        .unwrap();
    let client = DaemonClient::connect(&socket).await.unwrap();
    let mut watcher = StateWatcher::new(&client, None, StateWatcherConfig::default());
    next_matching(&mut watcher, |e| {
        matches!(e, StateEvent::VersionMismatch { .. })
    })
    .await;

    // The token goes, and the daemon answers on as it drains.
    token.undeclare().await.unwrap();
    next_matching(&mut watcher, |e| *e == StateEvent::DaemonDisappeared).await;
    assert!(matches!(
        client.check_release().await,
        Err(Error::VersionMismatch { .. })
    ));
    assert!(
        client.service_info().is_none(),
        "nothing kept with no token"
    );

    // The next daemon, on this release.
    *release.lock().unwrap() = VERSION.to_string();
    let _token = session
        .liveliness()
        .declare_token(keys::daemon_alive())
        .await
        .unwrap();
    let lifecycle = |e: &StateEvent| {
        matches!(
            e,
            StateEvent::DaemonAppeared
                | StateEvent::DaemonDisappeared
                | StateEvent::ReleaseChecked(_)
                | StateEvent::VersionMismatch { .. }
        )
    };
    assert_eq!(
        next_matching(&mut watcher, lifecycle).await,
        StateEvent::DaemonAppeared
    );
    assert_eq!(
        next_matching(&mut watcher, lifecycle).await,
        StateEvent::ReleaseChecked(ctrlassist::Service::Daemon)
    );
    assert_eq!(client.service_info().unwrap().version, VERSION);
    client.check_release().await.unwrap();
}

/// A client dropped while its release check still retries lets go of
/// its link: the retrying ends with the client.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_dropped_before_its_release_check_settles_closes_its_link() {
    let (_dir, socket) = socket();
    let session = zenoh::open(ctrlassist_protocol::bootstrap::router_config(&socket).unwrap())
        .await
        .unwrap();
    // Present, but never answering `info`.
    let _token = session
        .liveliness()
        .declare_token(keys::daemon_alive())
        .await
        .unwrap();
    let client = DaemonClient::connect(&socket).await.unwrap();
    let mut watcher = StateWatcher::new(&client, None, StateWatcherConfig::default());
    next_matching(&mut watcher, |e| *e == StateEvent::DaemonAppeared).await;
    assert!(client.service_info().is_none());
    drop(client);
    eventually("the dropped client's link stayed up", async || {
        session.info().peers_zid().await.count() == 0
    })
    .await;
    let ended = tokio::time::timeout(CONVERGE, watcher.next()).await;
    assert_eq!(ended.ok(), Some(None), "the watcher ends with the client");
}

/// A client dropped at any point while it takes up a daemon, its
/// subscriptions half declared included, lets go of its link: no task
/// that reads them outlives it.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_dropped_as_it_subscribes_closes_its_link() {
    let (_dir, socket) = socket();
    let mut daemon = FakeDaemon::start(&socket, VERSION).await;
    daemon
        .put(
            &keys::daemon(),
            &json!({"version": VERSION, "realtime": false, "hiding": "grab"}),
        )
        .await;
    for wait in 0..20 {
        let client = DaemonClient::connect(&socket).await.unwrap();
        tokio::time::sleep(Duration::from_millis(wait)).await;
        drop(client);
        eventually("a dropped client's link stayed up", async || {
            daemon.peers().await == 0
        })
        .await;
    }
    daemon.stop().await;
}

/// `try_next` hands over a queued event without waiting, and nothing when
/// the queue is empty.
#[tokio::test(flavor = "multi_thread")]
async fn try_next_drains_without_waiting() {
    let (_dir, socket) = socket();
    let client = DaemonClient::connect(&socket).await.unwrap();
    let mut idle = StateWatcher::new(&client, None, StateWatcherConfig::default());
    assert!(idle.try_next().is_none(), "no daemon, no events");
    let mut config = StateWatcherConfig::default();
    config.fallback_poll_interval = Duration::ZERO;
    let mut silent = StateWatcher::new(&client, None, config.clone());
    config.fallback_poll_interval = Duration::from_millis(20);
    let mut polling = StateWatcher::new(&client, None, config);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(polling.try_next(), Some(StateEvent::FallbackPoll));
    assert!(silent.try_next().is_none(), "zero turns the poll off");
}
