//! The daemon on a temporary socket, seen as a client sees it.

#![expect(clippy::unwrap_used, reason = "tests fail loudly")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use ctrlassist_daemon::{Daemon, Paths};
use ctrlassist_device::InputThread;
use ctrlassist_protocol::caller::{Caller, RpcError};
use ctrlassist_protocol::endpoint::daemon;
use ctrlassist_protocol::{VERSION, bootstrap, keys, payload};
use tempfile::TempDir;
use zenoh::Session;
use zenoh_ext::{AdvancedSubscriberBuilderExt, HistoryConfig};

/// A socket path short enough for the kernel's limit, in a directory the
/// daemon has to make.
fn socket() -> (TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("ca")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = dir.path().join("runtime").join("zenoh.sock");
    (dir, socket)
}

/// The daemon's paths around `socket`: its state goes beside it.
fn paths(socket: &Path) -> Paths {
    Paths {
        socket: socket.to_path_buf(),
        state: socket.with_file_name("state"),
    }
}

async fn start(socket: &Path) -> Daemon {
    let (input, _reports) = InputThread::spawn().unwrap();
    Daemon::start(&paths(socket), input).await.unwrap()
}

async fn is_alive(client: &Session) -> bool {
    let replies = client
        .liveliness()
        .get(keys::daemon_alive())
        .timeout(Duration::from_secs(2))
        .await
        .unwrap();
    matches!(replies.recv_async().await, Ok(reply) if reply.result().is_ok())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_finds_the_daemon_alive_asks_its_release_and_reads_its_state() {
    let (_dir, socket) = socket();
    let daemon = start(&socket).await;
    let client = zenoh::open(bootstrap::client_config(&socket).unwrap())
        .await
        .unwrap();

    assert!(is_alive(&client).await);
    let info = Caller::new(client.clone())
        .call::<daemon::Info>(&(), &())
        .await
        .unwrap();
    assert_eq!(info.version, VERSION);
    // What it publishes from a cache is listed, so a client knows when it
    // has all of it.
    for key in [
        keys::daemon(),
        keys::identity("generic"),
        keys::identity("xbox"),
    ] {
        assert!(info.keys.contains(&key), "{key} isn't in {:?}", info.keys);
    }

    // State was published before this client connected, and still
    // arrives: it is read from the daemon's cache.
    let state = client
        .declare_subscriber(keys::daemon())
        .history(HistoryConfig::default().detect_late_publishers())
        .await
        .unwrap();
    let sample = tokio::time::timeout(Duration::from_secs(10), state.recv_async())
        .await
        .unwrap()
        .unwrap();
    let state: payload::Daemon = serde_json::from_slice(&sample.payload().to_bytes()).unwrap();
    assert_eq!(state.version, VERSION);
    assert_eq!(state.realtime, daemon.is_realtime());
    assert!(format!("{daemon:?}").contains("realtime"));

    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stopped_daemon_is_gone_to_a_client_that_was_connected() {
    let (_dir, socket) = socket();
    let daemon = start(&socket).await;
    // A resilient client outlives the daemon's side of the link.
    let client = zenoh::open(bootstrap::client_config_resilient(&socket).unwrap())
        .await
        .unwrap();
    let caller = Caller::new(client.clone());
    let mut outcome = caller.call::<daemon::Info>(&(), &()).await;
    for _ in 0..10 {
        if outcome.is_ok() {
            break;
        }
        outcome = caller.call::<daemon::Info>(&(), &()).await;
    }
    outcome.unwrap();

    daemon.stop().await;
    assert!(!is_alive(&client).await);
    let error = caller.call::<daemon::Info>(&(), &()).await.unwrap_err();
    assert!(matches!(error, RpcError::Unavailable { .. }), "{error:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_socket_s_directory_is_made_for_the_user_alone() {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, socket) = socket();
    let daemon = start(&socket).await;
    let mode = std::fs::metadata(socket.parent().unwrap())
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o700);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_that_cannot_make_its_socket_s_directory_says_so() {
    let (input, _reports) = InputThread::spawn().unwrap();
    let error = Daemon::start(&paths(Path::new("/proc/nowhere/zenoh.sock")), input)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("creating /proc/nowhere"),
        "{error}"
    );
}

#[test]
fn a_daemon_given_no_paths_uses_the_user_s_runtime_and_state_directories() {
    let paths = Paths::default_for_user();
    assert!(paths.socket.ends_with("ctrlassist/zenoh.sock"));
    assert!(paths.state.ends_with("ctrlassist"));
    assert_ne!(paths.socket.parent(), Some(paths.state.as_path()));
}
