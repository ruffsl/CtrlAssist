//! Setups through the SDK, against a live daemon and fake pads: a
//! profile run end to end, from a press on a pad to the same press on
//! the virtual pad a game would read.

use std::fs::File;
use std::os::fd::OwnedFd;
use std::path::Path;
use std::time::Duration;

use ctrlassist::{
    DaemonClient, ErrorCode, Phase, Setup, StateEvent, StateWatcher, StateWatcherConfig,
};
use ctrlassist_core::gamepad;
use ctrlassist_device::Pad;
use ctrlassist_test::{FakePad, can_make_pads, phys_connected, phys_of};

use crate::client::{eventually, next_matching, socket, start_daemon};

const VENDOR: u16 = 0x1209;

/// A priority profile over two models of fake pad.
fn priority(primary: u16, assist: u16) -> String {
    format!(
        "preset = \"priority\"\nprimary = \"model:{VENDOR:04x}:{primary:04x}\"\nassist = \"model:{VENDOR:04x}:{assist:04x}\"\n"
    )
}

/// A client whose mirror holds all the daemon has published.
async fn hydrated(socket: &Path) -> DaemonClient {
    let client = DaemonClient::connect(socket).await.unwrap();
    eventually("the client never hydrated", async || client.is_hydrated()).await;
    client
}

/// The setup, once its state is `phase`.
async fn in_phase(client: &DaemonClient, name: &str, phase: Phase) -> Setup {
    eventually(
        &format!("{name} never was {}", phase.as_str()),
        async || {
            client
                .setup(name)
                .unwrap()
                .is_some_and(|setup| setup.phase == phase)
        },
    )
    .await;
    client.setup(name).unwrap().unwrap()
}

/// Reads a virtual pad as a game would: once udev has announced it, by
/// when its node can be opened.
async fn game(setup: &Setup) -> Pad {
    let node = setup.sinks[0].node.clone().unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        match File::open(&node) {
            Ok(file) => return Pad::from_fd(OwnedFd::from(file)).unwrap(),
            Err(error) => assert!(
                tokio::time::Instant::now() < deadline,
                "{node} never opened: {error}"
            ),
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Waits until the virtual pad a game reads holds `south` as wanted.
async fn south_is(game: &mut Pad, wanted: bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        game.read(|_| {}).unwrap();
        if (game.state().get(gamepad::SOUTH) > 0.5) == wanted {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the virtual pad never held south as {wanted}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_runs_from_a_press_on_a_pad_to_the_virtual_pad() {
    const TEST: &str = "a_profile_runs_from_a_press_on_a_pad_to_the_virtual_pad";
    if !can_make_pads(TEST) {
        return;
    }
    let mut primary = FakePad::new("Setup Primary", VENDOR, 0x7c01).unwrap();
    let mut assist = FakePad::new("Setup Assist", VENDOR, 0x7c02).unwrap();
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    client
        .put_profile("couch", &priority(0x7c01, 0x7c02))
        .await
        .unwrap();

    client.start_setup("couch").await.unwrap();
    // The reply means it runs: its state says so, with what it is made of.
    let setup = in_phase(&client, "couch", Phase::Running).await;
    assert_eq!(setup.reason, None);
    let sources: Vec<(&str, Option<&str>, bool)> = setup
        .sources
        .iter()
        .map(|source| (source.name.as_str(), source.pad.as_deref(), source.present))
        .collect();
    assert_eq!(
        sources,
        [
            ("assist", Some("model:1209:7c02:Setup Assist"), true),
            ("primary", Some("model:1209:7c01:Setup Primary"), true),
        ]
    );
    assert_eq!(setup.sinks.len(), 1);
    assert_eq!(setup.sinks[0].identity, "generic");
    // Each pad says whose it is.
    let pads = client.list_pads().unwrap();
    let ours: Vec<_> = pads.iter().filter(|pad| pad.vendor == VENDOR).collect();
    assert!(
        ours.iter()
            .filter(|pad| pad.product & 0xff00 == 0x7c00)
            .all(|pad| pad.setup.as_deref() == Some("couch"))
    );

    // Either pad's press is the virtual pad's.
    let mut game = game(&setup).await;
    primary.south(true).unwrap();
    south_is(&mut game, true).await;
    primary.south(false).unwrap();
    south_is(&mut game, false).await;
    assist.south(true).unwrap();
    south_is(&mut game, true).await;
    assist.south(false).unwrap();
    south_is(&mut game, false).await;

    // It can't be started twice, nor its pads taken by another.
    let twice = client.start_setup("couch").await.unwrap_err();
    assert_eq!(twice.rpc_code(), Some(ErrorCode::SetupExists));
    client
        .put_profile(
            "other",
            "preset = \"unicast\"\npad = \"model:1209:7c01:Setup Primary\"\n",
        )
        .await
        .unwrap();
    let taken = client.start_setup("other").await.unwrap_err();
    assert_eq!(taken.rpc_code(), Some(ErrorCode::PadInUse));
    assert_eq!(client.setup("other").unwrap(), None);

    // Stopped, its virtual pad is gone and its pads are free.
    let phys = phys_of(Path::new(setup.sinks[0].node.as_deref().unwrap())).unwrap();
    client.stop_setup("couch").await.unwrap();
    eventually("the setup stayed listed", async || {
        client.setup("couch").unwrap().is_none()
    })
    .await;
    eventually("the virtual pad stayed", async || !phys_connected(&phys)).await;
    eventually("the pads stayed taken", async || {
        client
            .list_pads()
            .unwrap()
            .iter()
            .all(|pad| pad.setup.is_none())
    })
    .await;
    let unknown = client.stop_setup("couch").await.unwrap_err();
    assert_eq!(unknown.rpc_code(), Some(ErrorCode::UnknownSetup));
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_setup_is_held_while_a_pad_is_missing_and_takes_it_back() {
    const TEST: &str = "a_setup_is_held_while_a_pad_is_missing_and_takes_it_back";
    if !can_make_pads(TEST) {
        return;
    }
    let mut primary = FakePad::new("Held Primary", VENDOR, 0x7d01).unwrap();
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    let mut watcher = StateWatcher::new(&client, StateWatcherConfig::default());
    client
        .put_profile("couch", &priority(0x7d01, 0x7d02))
        .await
        .unwrap();

    // Started without its assist, it is held, waiting for a pad that
    // fits, and the primary plays alone.
    client.start_setup("couch").await.unwrap();
    let setup = in_phase(&client, "couch", Phase::Held).await;
    assert_eq!(setup.sources[0].pad, None);
    assert!(!setup.sources[0].present && setup.sources[1].present);
    let mut game = game(&setup).await;
    primary.south(true).unwrap();
    south_is(&mut game, true).await;
    primary.south(false).unwrap();
    south_is(&mut game, false).await;

    // The assist connects: the setup takes it and runs.
    let mut assist = FakePad::new("Held Assist", VENDOR, 0x7d02).unwrap();
    let setup = in_phase(&client, "couch", Phase::Running).await;
    assert_eq!(
        setup.sources[0].pad.as_deref(),
        Some("model:1209:7d02:Held Assist")
    );
    assist.south(true).unwrap();
    south_is(&mut game, true).await;

    // It leaves holding a button: held again, and the button is let go.
    drop(assist);
    let setup = in_phase(&client, "couch", Phase::Held).await;
    assert!(!setup.sources[0].present);
    assert_eq!(
        setup.sources[0].pad.as_deref(),
        Some("model:1209:7d02:Held Assist"),
        "it waits for that pad now"
    );
    south_is(&mut game, false).await;

    // It returns: the same virtual pad carries on.
    let mut assist = FakePad::new("Held Assist", VENDOR, 0x7d02).unwrap();
    let setup = in_phase(&client, "couch", Phase::Running).await;
    assert!(setup.sources.iter().all(|source| source.present));
    assist.south(true).unwrap();
    south_is(&mut game, true).await;

    // Every change along the way was announced.
    let changed = StateEvent::SetupChanged {
        setup: "couch".to_string(),
    };
    next_matching(&mut watcher, |e| *e == changed).await;

    // A daemon that stops takes its setups and their virtual pads along.
    let phys = phys_of(Path::new(setup.sinks[0].node.as_deref().unwrap())).unwrap();
    daemon.stop().await;
    eventually("the virtual pad outlived the daemon", async || {
        !phys_connected(&phys)
    })
    .await;
    eventually("the setup outlived the daemon", async || {
        client.list_setups().unwrap().is_empty()
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_setup_that_cannot_start_says_why_and_leaves_nothing() {
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    let unknown = client.start_setup("nothing").await.unwrap_err();
    assert_eq!(unknown.rpc_code(), Some(ErrorCode::UnknownProfile));

    // The same pad named twice: right as a profile, wrong to run.
    client
        .put_profile(
            "twice",
            "preset = \"priority\"\nprimary = \"0c:27:56:4f:bd:14\"\nassist = \"address:0c:27:56:4f:bd:14\"\n",
        )
        .await
        .unwrap();
    let refused = client.start_setup("twice").await.unwrap_err();
    assert_eq!(refused.rpc_code(), Some(ErrorCode::InvalidProfile));
    assert!(
        refused.to_string().contains("another source's pad too"),
        "{refused}"
    );
    assert!(client.list_setups().unwrap().is_empty());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(client.list_setups().unwrap().is_empty());
    daemon.stop().await;
}

/// Opens a pad's node as some other program would, beside the daemon.
fn bystander(pad: &FakePad) -> Pad {
    let file: OwnedFd = File::open(pad.event_node()).unwrap().into();
    Pad::from_fd(file).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn without_the_hiding_service_a_setup_s_pads_are_grabbed() {
    const TEST: &str = "without_the_hiding_service_a_setup_s_pads_are_grabbed";
    if !can_make_pads(TEST) {
        return;
    }
    let mut primary = FakePad::new("Grab Primary", VENDOR, 0x7f01).unwrap();
    let _assist = FakePad::new("Grab Assist", VENDOR, 0x7f02).unwrap();
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    assert_eq!(
        client.daemon_state().unwrap().unwrap().hiding,
        ctrlassist::Hiding::Grab
    );
    client
        .put_profile("couch", &priority(0x7f01, 0x7f02))
        .await
        .unwrap();
    let mut other = bystander(&primary);

    client.start_setup("couch").await.unwrap();
    let setup = in_phase(&client, "couch", Phase::Running).await;
    // Grabbed isn't hidden: the pads' state doesn't claim it.
    assert!(client.list_pads().unwrap().iter().all(|pad| !pad.hidden));
    let mut game = game(&setup).await;
    primary.south(true).unwrap();
    south_is(&mut game, true).await;
    // The press reached the game through the virtual pad, and nobody
    // reading the pad itself heard it.
    other.read(|_| {}).unwrap();
    assert!(other.state().get(gamepad::SOUTH) < 0.5);
    primary.south(false).unwrap();
    south_is(&mut game, false).await;

    // Stopped, the pad is everyone's again.
    client.stop_setup("couch").await.unwrap();
    primary.south(true).unwrap();
    south_is(&mut other, true).await;
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn with_the_hiding_service_a_setup_s_pads_are_hidden_reopened_and_restored() {
    use ctrlassist_test::Asked;
    const TEST: &str = "with_the_hiding_service_a_setup_s_pads_are_hidden_reopened_and_restored";
    if !can_make_pads(TEST) {
        return;
    }
    let mut primary = FakePad::new("Hide Primary", VENDOR, 0x8001).unwrap();
    let assist = FakePad::new("Hide Assist", VENDOR, 0x8002).unwrap();
    let (_dir, socket) = socket();
    let hider = ctrlassist_test::FakeHider::listen(&socket.with_file_name("hider.sock")).unwrap();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    assert_eq!(
        client.daemon_state().unwrap().unwrap().hiding,
        ctrlassist::Hiding::Service
    );
    client
        .put_profile("couch", &priority(0x8001, 0x8002))
        .await
        .unwrap();
    let primary_id = "model:1209:8001:Hide Primary".to_string();
    let assist_id = "model:1209:8002:Hide Assist".to_string();

    // Started: each pad was handed to the service, in source order, and
    // says it is hidden.
    client.start_setup("couch").await.unwrap();
    let setup = in_phase(&client, "couch", Phase::Running).await;
    assert_eq!(
        hider.asked(),
        [
            Asked::Hide(assist_id.clone()),
            Asked::Hide(primary_id.clone())
        ]
    );
    eventually("the pads never said they were hidden", async || {
        client
            .list_pads()
            .unwrap()
            .iter()
            .filter(|pad| pad.setup.as_deref() == Some("couch"))
            .all(|pad| pad.hidden)
    })
    .await;
    // It reads the pads through what the service handed back.
    let mut game = game(&setup).await;
    primary.south(true).unwrap();
    south_is(&mut game, true).await;
    primary.south(false).unwrap();
    south_is(&mut game, false).await;

    // A hidden pad that leaves and returns is opened through the service
    // again, not hidden afresh.
    drop(assist);
    in_phase(&client, "couch", Phase::Held).await;
    let mut assist = FakePad::new("Hide Assist", VENDOR, 0x8002).unwrap();
    in_phase(&client, "couch", Phase::Running).await;
    assert_eq!(
        hider.asked().last(),
        Some(&Asked::Reopen(assist_id.clone()))
    );
    assist.south(true).unwrap();
    south_is(&mut game, true).await;

    // Stopped: each pad is restored, and no longer says it is hidden.
    client.stop_setup("couch").await.unwrap();
    // The reply means they are: the stop is answered once it is done.
    let mut restored: Vec<Asked> = hider
        .asked()
        .into_iter()
        .filter(|asked| matches!(asked, Asked::Restore(_)))
        .collect();
    restored.sort_by_key(|asked| format!("{asked:?}"));
    assert_eq!(
        restored,
        [Asked::Restore(primary_id), Asked::Restore(assist_id)]
    );
    eventually("the pads still said they were hidden", async || {
        client.list_pads().unwrap().iter().all(|pad| !pad.hidden)
    })
    .await;

    // A daemon that stops, however it stops, closes its connection, which
    // is what restores anything it still had hidden.
    client.start_setup("couch").await.unwrap();
    in_phase(&client, "couch", Phase::Running).await;
    daemon.stop().await;
    eventually("the connection never closed", async || {
        hider.asked().last() == Some(&Asked::Closed)
    })
    .await;
}
