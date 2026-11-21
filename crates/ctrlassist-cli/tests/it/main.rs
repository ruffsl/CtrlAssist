//! The `ctrlassist` binary against a daemon on a temporary socket.

#![expect(clippy::unwrap_used, reason = "tests fail loudly")]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use ctrlassist_daemon::{Daemon, Paths};
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

async fn start(socket: &Path) -> Daemon {
    let (input, reports) = InputThread::spawn().unwrap();
    let paths = Paths {
        socket: socket.to_path_buf(),
        state: socket.with_file_name("state"),
        config: socket.with_file_name("config"),
    };
    Daemon::start(&paths, input, reports).await.unwrap()
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
    let daemon = start(&socket).await;

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
        .args(["service", "daemon", "--state-dir"])
        .arg(socket.with_file_name("state"))
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
    for verb in [
        "status",
        "pads",
        "identities",
        "identify",
        "forget",
        "profile",
        "start",
        "stop",
        "setups",
        "service",
        "--socket",
    ] {
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

#[tokio::test(flavor = "multi_thread")]
async fn pads_lists_a_connected_pad_and_forget_drops_it_once_it_has_left() {
    const TEST: &str = "pads_lists_a_connected_pad_and_forget_drops_it_once_it_has_left";
    if !ctrlassist_test::can_make_pads(TEST) {
        return;
    }
    let (_dir, socket) = socket();
    let fake = ctrlassist_test::FakePad::new("CLI \u{1b}[31mPad", 0x1209, 0x7b01).unwrap();
    let daemon = start(&socket).await;
    let id = "model:1209:7b01:CLI \u{1b}[31mPad";

    let json = ctrlassist(&socket, &["pads", "--json"]);
    assert!(json.status.success(), "{}", text(&json.stderr));
    let pads: Vec<serde_json::Value> = serde_json::from_slice(&json.stdout).unwrap();
    let ours = pads.iter().find(|pad| pad["id"] == id).unwrap();
    assert_eq!(ours["connected"], true);
    assert_eq!(ours["rumble"], false);
    assert_eq!(ours["vendor"], 0x1209);

    // As a table, a pad's own text can't drive the terminal.
    let plain = ctrlassist(&socket, &["pads"]);
    assert!(plain.status.success(), "{}", text(&plain.stderr));
    let said = text(&plain.stdout);
    let mut lines = said.lines();
    let heading = lines.next().unwrap();
    assert!(
        heading.starts_with("ID") && heading.ends_with("RUMBLE"),
        "{said}"
    );
    let row = lines.find(|line| line.contains("1209:7b01")).unwrap();
    assert!(row.starts_with("model:1209:7b01:CLI ?[31mPad  "), "{row}");
    assert!(row.ends_with("yes        no"), "{row}");
    assert!(!said.contains('\u{1b}'), "{said:?}");

    // What can't be done says why, as an error.
    let connected = ctrlassist(&socket, &["forget", "model:1209:7b01"]);
    assert_eq!(connected.status.code(), Some(1));
    assert!(text(&connected.stderr).contains("pad-connected"));
    let silent = ctrlassist(&socket, &["identify", "model:1209:7b01"]);
    assert_eq!(silent.status.code(), Some(1));
    assert!(text(&silent.stderr).contains("pad-without-rumble"));
    let unknown = ctrlassist(&socket, &["identify", "no-such-pad", "--millis", "50"]);
    assert!(text(&unknown.stderr).contains("no known pad is called no-such-pad"));

    drop(fake);
    let mut forgotten = ctrlassist(&socket, &["forget", "model:1209:7b01"]);
    for _ in 0..200 {
        if forgotten.status.success() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
        forgotten = ctrlassist(&socket, &["forget", "model:1209:7b01"]);
    }
    assert!(forgotten.status.success(), "{}", text(&forgotten.stderr));
    let json = ctrlassist(&socket, &["pads", "--json"]);
    let pads: Vec<serde_json::Value> = serde_json::from_slice(&json.stdout).unwrap();
    assert!(pads.iter().all(|pad| pad["id"] != id));
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn identities_lists_what_a_virtual_pad_can_be() {
    let (_dir, socket) = socket();
    let daemon = start(&socket).await;
    let plain = ctrlassist(&socket, &["identities"]);
    assert!(plain.status.success(), "{}", text(&plain.stderr));
    let said = text(&plain.stdout);
    let lines: Vec<&str> = said.lines().collect();
    assert_eq!(lines.len(), 3, "{said}");
    assert!(lines[0].starts_with("ID       NAME"), "{said}");
    assert!(
        lines[1].starts_with("generic  CtrlAssist Virtual Pad"),
        "{said}"
    );
    assert!(lines[2].contains("045e:0b12"), "{said}");

    let json = ctrlassist(&socket, &["identities", "--json"]);
    let identities: Vec<serde_json::Value> = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(identities[0]["id"], "generic");
    assert_eq!(identities[1]["id"], "xbox");
    daemon.stop().await;
}

#[test]
fn a_command_that_needs_the_daemon_says_when_there_is_none() {
    let (_dir, socket) = socket();
    for arguments in [
        &["pads"][..],
        &["identities"],
        &["identify", "x"],
        &["forget", "x"],
        &["profile", "list"],
        &["profile", "delete", "x"],
        &["start", "x"],
        &["stop", "x"],
        &["setups"],
    ] {
        let output = ctrlassist(&socket, arguments);
        assert_eq!(output.status.code(), Some(1), "{arguments:?}");
        assert!(
            text(&output.stderr).contains(&format!("no daemon is running on {}", socket.display())),
            "{arguments:?}: {}",
            text(&output.stderr)
        );
        assert!(output.stdout.is_empty());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_file_is_checked_put_shown_listed_and_deleted() {
    let (dir, socket) = socket();
    let daemon = start(&socket).await;
    let right =
        "preset = \"priority\"\nprimary = \"0c:27:56:4f:bd:14\"\nassist = \"model:054c:09cc\"\n";
    let file = dir.path().join("couch.toml");
    std::fs::write(&file, right).unwrap();
    let wrong = dir.path().join("wrong.toml");
    std::fs::write(&wrong, "preset = \"priority\"\n").unwrap();
    let path = |file: &Path| file.to_str().unwrap().to_string();

    let empty = ctrlassist(&socket, &["profile", "list"]);
    assert_eq!(text(&empty.stdout), "no profiles are kept\n");

    // Checking says where a profile is wrong, as a failure, and keeps
    // nothing either way.
    let checked = ctrlassist(&socket, &["profile", "validate", &path(&file)]);
    assert!(checked.status.success(), "{}", text(&checked.stderr));
    assert!(checked.stdout.is_empty());
    let refused = ctrlassist(&socket, &["profile", "validate", &path(&wrong)]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(
        text(&refused.stderr).contains(&format!("{}: preset: ", wrong.display())),
        "{}",
        text(&refused.stderr)
    );
    let refused = ctrlassist(&socket, &["profile", "put", &path(&wrong)]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(text(&refused.stderr).contains("invalid-profile"));
    let missing = ctrlassist(&socket, &["profile", "put", "/nowhere/x.toml"]);
    assert!(text(&missing.stderr).contains("reading /nowhere/x.toml"));
    let empty = ctrlassist(&socket, &["profile", "list", "--json"]);
    assert_eq!(text(&empty.stdout), "[]\n");

    // Put under the file's name, and under one given.
    let put = ctrlassist(&socket, &["profile", "put", &path(&file)]);
    assert!(put.status.success(), "{}", text(&put.stderr));
    let put = ctrlassist(&socket, &["profile", "put", &path(&file), "--name", "desk"]);
    assert!(put.status.success(), "{}", text(&put.stderr));
    let shown = ctrlassist(&socket, &["profile", "show", "couch"]);
    assert_eq!(text(&shown.stdout), right);
    let listed = ctrlassist(&socket, &["profile", "list"]);
    assert_eq!(text(&listed.stdout), "NAME   PROBLEM\ncouch\ndesk\n");
    let json = ctrlassist(&socket, &["profile", "list", "--json"]);
    let profiles: Vec<serde_json::Value> = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(profiles[1]["name"], "desk");
    assert_eq!(profiles[1]["toml"], right);

    let deleted = ctrlassist(&socket, &["profile", "delete", "desk"]);
    assert!(deleted.status.success(), "{}", text(&deleted.stderr));
    let again = ctrlassist(&socket, &["profile", "delete", "desk"]);
    assert!(text(&again.stderr).contains("unknown-profile"));
    let gone = ctrlassist(&socket, &["profile", "show", "desk"]);
    assert_eq!(gone.status.code(), Some(1));
    assert!(text(&gone.stderr).contains("no profile is called desk"));
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_is_started_listed_and_stopped() {
    const TEST: &str = "a_profile_is_started_listed_and_stopped";
    if !ctrlassist_test::can_make_pads(TEST) {
        return;
    }
    let (dir, socket) = socket();
    let _primary = ctrlassist_test::FakePad::new("CLI Primary", 0x1209, 0x7e01).unwrap();
    let daemon = start(&socket).await;
    let file = dir.path().join("couch.toml");
    std::fs::write(
        &file,
        "preset = \"priority\"\nprimary = \"model:1209:7e01\"\nassist = \"model:1209:7e02\"\n",
    )
    .unwrap();
    let put = ctrlassist(&socket, &["profile", "put", file.to_str().unwrap()]);
    assert!(put.status.success(), "{}", text(&put.stderr));
    let none = ctrlassist(&socket, &["setups"]);
    assert_eq!(text(&none.stdout), "no setups exist\n");

    let started = ctrlassist(&socket, &["start", "couch"]);
    assert!(started.status.success(), "{}", text(&started.stderr));
    let twice = ctrlassist(&socket, &["start", "couch"]);
    assert_eq!(twice.status.code(), Some(1));
    assert!(text(&twice.stderr).contains("setup-exists"));

    // Its assist isn't connected, so it is held, and says what it waits
    // for.
    let listed = ctrlassist(&socket, &["setups"]);
    let said = text(&listed.stdout);
    let lines: Vec<&str> = said.lines().collect();
    assert_eq!(lines.len(), 5, "{said}");
    assert!(lines[0].starts_with("SETUP  PHASE  PART"), "{said}");
    assert!(lines[1].starts_with("couch  held"), "{said}");
    assert!(
        lines[2].ends_with("pad assist           (waiting for a pad that fits)"),
        "{said}"
    );
    assert!(
        lines[3].ends_with("pad primary          model:1209:7e01:CLI Primary"),
        "{said}"
    );
    assert!(
        lines[4].contains("virtual pad virtual")
            && lines[4].contains("generic at /dev/input/event"),
        "{said}"
    );
    let json = ctrlassist(&socket, &["setups", "--json"]);
    let setups: Vec<serde_json::Value> = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(setups[0]["name"], "couch");
    assert_eq!(setups[0]["phase"], "held");

    let stopped = ctrlassist(&socket, &["stop", "couch"]);
    assert!(stopped.status.success(), "{}", text(&stopped.stderr));
    let again = ctrlassist(&socket, &["stop", "couch"]);
    assert!(text(&again.stderr).contains("unknown-setup"));
    let none = ctrlassist(&socket, &["setups", "--json"]);
    assert_eq!(text(&none.stdout), "[]\n");
    daemon.stop().await;
}
