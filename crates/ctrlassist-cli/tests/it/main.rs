//! The `ctrlassist` binary against a daemon on a temporary socket.

#![expect(clippy::unwrap_used, reason = "tests fail loudly")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use ctrlassist_daemon::Daemon;
use ctrlassist_device::InputThread;
use ctrlassist_protocol::VERSION;
use tempfile::TempDir;

fn socket() -> (TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("ca")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = dir.path().join("zenoh.sock");
    (dir, socket)
}

/// Runs the built `ctrlassist` binary.
fn ctrlassist(socket: &Path, arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ctrlassist"))
        .arg("--socket")
        .arg(socket)
        .args(arguments)
        .output()
        .unwrap()
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn status_reports_a_running_daemon_and_its_release() {
    let (_dir, socket) = socket();
    let (input, _reports) = InputThread::spawn().unwrap();
    let daemon = Daemon::start(&socket, input).await.unwrap();

    let plain = ctrlassist(&socket, &["status"]);
    assert!(plain.status.success(), "{}", text(&plain.stderr));
    let said = text(&plain.stdout);
    assert!(
        said.contains(&format!("daemon alive at {}", socket.display())),
        "{said}"
    );
    assert!(
        said.contains(&format!(
            "daemon release: {VERSION} (this command: {VERSION})"
        )),
        "{said}"
    );

    let json = ctrlassist(&socket, &["status", "--json"]);
    assert!(json.status.success(), "{}", text(&json.stderr));
    let found: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(found["alive"], true);
    assert_eq!(found["release"], VERSION);
    assert_eq!(found["compatible"], true);
    assert_eq!(found["socket"], socket.to_str().unwrap());

    daemon.stop().await;
}

#[test]
fn status_reports_no_daemon_as_a_failure_that_says_so() {
    let (_dir, socket) = socket();
    let plain = ctrlassist(&socket, &["status"]);
    assert_eq!(plain.status.code(), Some(1));
    assert!(text(&plain.stderr).contains(&format!("no daemon is running on {}", socket.display())));
    assert!(plain.stdout.is_empty());

    // As JSON: the object, exit status 1, and nothing on stderr.
    let json = ctrlassist(&socket, &["status", "--json"]);
    assert_eq!(json.status.code(), Some(1));
    let found: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(found["alive"], false);
    assert!(found["release"].is_null() && found["compatible"].is_null());
    assert!(json.stderr.is_empty(), "{}", text(&json.stderr));
}

#[test]
fn the_daemon_as_a_service_verb_runs_until_told_to_stop_and_stops_cleanly() {
    let (_dir, socket) = socket();
    let daemon = Command::new(env!("CARGO_BIN_EXE_ctrlassist"))
        .arg("--socket")
        .arg(&socket)
        .args(["service", "daemon"])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // It is up once `status` finds it; each try is a whole connection.
    let mut found = ctrlassist(&socket, &["status", "--json"]);
    for _ in 0..200 {
        if found.status.success() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
        found = ctrlassist(&socket, &["status", "--json"]);
    }
    assert!(found.status.success(), "the daemon never came up");

    // Asked to stop as systemd asks, it exits with status 0.
    let pid = daemon.id().to_string();
    assert!(
        Command::new("kill")
            .args(["-TERM", &pid])
            .status()
            .unwrap()
            .success()
    );
    let output = daemon.wait_with_output().unwrap();
    assert!(output.status.success(), "{}", text(&output.stderr));
    assert!(!ctrlassist(&socket, &["status", "--json"]).status.success());
}

#[test]
fn the_command_explains_itself() {
    let help = Command::new(env!("CARGO_BIN_EXE_ctrlassist"))
        .arg("--help")
        .output()
        .unwrap();
    let said = text(&help.stdout);
    assert!(help.status.success());
    for verb in ["status", "service", "--socket"] {
        assert!(said.contains(verb), "{said}");
    }
    let version = Command::new(env!("CARGO_BIN_EXE_ctrlassist"))
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        text(&version.stdout).trim(),
        format!("ctrlassist {VERSION}")
    );
}
