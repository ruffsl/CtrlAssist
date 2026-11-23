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

/// The fake pads' vendor. Each test makes pads of products of its own:
/// a test's daemon sees every pad on the machine, those of the tests
/// running beside it included, and a profile that names a model takes
/// whichever pad of it it finds.
const VENDOR: u16 = 0x1209;

/// A priority profile over two models of fake pad.
fn priority(primary: u16, assist: u16) -> String {
    format!(
        "mode = \"priority\"\nprimary = \"model:{VENDOR:04x}:{primary:04x}\"\nassist = \"model:{VENDOR:04x}:{assist:04x}\"\n"
    )
}

/// What the stand-in for the hiding service was asked about setups'
/// pads. A DualSense connected to the machine is copied by any daemon
/// with a hiding service, which is asked for its raw node; that is no
/// part of what these tests are about.
fn asked(hider: &ctrlassist_test::FakeHider) -> Vec<ctrlassist_test::Asked> {
    let mut asked = hider.asked();
    asked.retain(|asked| !matches!(asked, ctrlassist_test::Asked::Raw(_)));
    asked
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
            "mode = \"unicast\"\npad = \"model:1209:7c01:Setup Primary\"\n",
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
    let mut watcher = StateWatcher::new(&client, None, StateWatcherConfig::default());
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

    // The same pad named twice, two ways: wrong as a profile already.
    let refused = client
        .put_profile(
            "twice",
            "mode = \"priority\"\nprimary = \"0c:27:56:4f:bd:14\"\nassist = \"address:0c:27:56:4f:bd:14\"\n",
        )
        .await
        .unwrap_err();
    assert_eq!(refused.rpc_code(), Some(ErrorCode::InvalidProfile));
    assert!(
        refused.to_string().contains("each needs its own"),
        "{refused}"
    );

    // A pad still to choose: right as a profile, wrong to run.
    client
        .put_profile(
            "half",
            "mode = \"priority\"\nprimary = \"model:1209:8e01\"\n",
        )
        .await
        .unwrap();
    let refused = client.start_setup("half").await.unwrap_err();
    assert_eq!(refused.rpc_code(), Some(ErrorCode::InvalidProfile));
    assert!(refused.to_string().starts_with("mode: "), "{refused}");
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
    // A Steam of this test's own, which already leaves one model alone
    // by the user's doing, and has one left by a daemon that died.
    let steam = socket.with_file_name("config.vdf");
    let listing = |list: &str| {
        format!("\"InstallConfigStore\"\n{{\n\t\"controller_blacklist\"\t\t\"{list}\"\n}}\n")
    };
    std::fs::write(&steam, listing("28de/1142,1209/dead")).unwrap();
    let state = socket.with_file_name("state");
    std::fs::create_dir_all(&state).unwrap();
    let left = format!(
        "schema = 1\n[added]\n\"{}\" = [\"1209/dead\"]\n",
        steam.display()
    );
    std::fs::write(state.join("steam-blacklist.toml"), left).unwrap();
    let daemon = start_daemon(&socket).await;
    assert_eq!(
        std::fs::read_to_string(&steam).unwrap(),
        listing("28de/1142")
    );
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
    // Steam reads some pads another way than a grab silences, so it is
    // told to leave the setup's models alone, beside what it was told.
    assert_eq!(
        std::fs::read_to_string(&steam).unwrap(),
        listing("28de/1142,1209/7f01,1209/7f02")
    );
    let mut game = game(&setup).await;
    primary.south(true).unwrap();
    south_is(&mut game, true).await;
    // The press reached the game through the virtual pad, and nobody
    // reading the pad itself heard it.
    other.read(|_| {}).unwrap();
    assert!(other.state().get(gamepad::SOUTH) < 0.5);
    primary.south(false).unwrap();
    south_is(&mut game, false).await;

    // Stopped, the pad is everyone's again, Steam's too.
    client.stop_setup("couch").await.unwrap();
    primary.south(true).unwrap();
    south_is(&mut other, true).await;
    assert_eq!(
        std::fs::read_to_string(&steam).unwrap(),
        listing("28de/1142")
    );
    // A daemon that stops with a setup running leaves Steam as it was.
    client.start_setup("couch").await.unwrap();
    in_phase(&client, "couch", Phase::Running).await;
    assert_ne!(
        std::fs::read_to_string(&steam).unwrap(),
        listing("28de/1142")
    );
    daemon.stop().await;
    assert_eq!(
        std::fs::read_to_string(&steam).unwrap(),
        listing("28de/1142")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_hiding_service_that_starts_after_the_daemon_is_found_when_it_is_needed() {
    const TEST: &str = "a_hiding_service_that_starts_after_the_daemon_is_found_when_it_is_needed";
    if !can_make_pads(TEST) {
        return;
    }
    let _primary = FakePad::new("Late Primary", VENDOR, 0x8b01).unwrap();
    let _assist = FakePad::new("Late Assist", VENDOR, 0x8b02).unwrap();
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    let hiding = |client: &DaemonClient| client.daemon_state().unwrap().unwrap().hiding;
    assert_eq!(hiding(&client), ctrlassist::Hiding::Grab);

    // The service comes up only now. The next setup's pads are hidden
    // through it, and the daemon says that is how pads are hidden.
    let hider = ctrlassist_test::FakeHider::listen(&socket.with_file_name("hider.sock")).unwrap();
    client
        .put_profile("couch", &priority(0x8b01, 0x8b02))
        .await
        .unwrap();
    client.start_setup("couch").await.unwrap();
    in_phase(&client, "couch", Phase::Running).await;
    assert_eq!(asked(&hider).len(), 2, "{:?}", hider.asked());
    eventually("the daemon never said it found the service", async || {
        hiding(&client) == ctrlassist::Hiding::Service
    })
    .await;
    // And which release of it, which the service says on being found.
    assert_eq!(
        client.daemon_state().unwrap().unwrap().hider_version,
        Some(env!("CARGO_PKG_VERSION").to_string())
    );
    client.stop_setup("couch").await.unwrap();

    // The service is restarted: the connection the daemon had is gone.
    // The first thing asked of it finds that out, and the daemon goes
    // back to grabbing until the service is found again, which the next
    // setup to start does.
    drop(hider);
    let hider = ctrlassist_test::FakeHider::listen(&socket.with_file_name("hider.sock")).unwrap();
    client.start_setup("couch").await.unwrap();
    in_phase(&client, "couch", Phase::Running).await;
    client.stop_setup("couch").await.unwrap();
    let before = asked(&hider).len();
    client.start_setup("couch").await.unwrap();
    in_phase(&client, "couch", Phase::Running).await;
    let hides = asked(&hider)[before..]
        .iter()
        .filter(|asked| matches!(asked, ctrlassist_test::Asked::Hide(_)))
        .count();
    assert_eq!(hides, 2, "{:?}", hider.asked());
    client.stop_setup("couch").await.unwrap();
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
        asked(&hider),
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
    // Almost always. The daemon can hear from the input thread that the
    // pad went, and find a pad on its node again, before udev has told
    // anyone the pad is back: the service then says it is absent, and
    // the daemon hides it afresh and gives up the old name, which leaves
    // the pad hidden once all the same.
    let all = asked(&hider);
    let reopened = all.last() == Some(&Asked::Reopen(assist_id.clone()));
    let afresh = all.ends_with(&[
        Asked::Reopen(assist_id.clone()),
        Asked::Hide(assist_id.clone()),
        Asked::Restore(assist_id.clone()),
    ]);
    assert!(reopened || afresh, "everything asked: {all:?}");
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
    // The assist twice, if it was hidden afresh above.
    restored.dedup();
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
        asked(&hider).last() == Some(&Asked::Closed)
    })
    .await;
}

/// The one node of `setup` that selects: what it selects between, and
/// which is selected.
/// A profile may ask for less than hiding, with the hiding service
/// there to be asked: only a grab, or nothing, so that a game reads the
/// pad beside the virtual one. Neither has the service or Steam told
/// anything, and going from one to the other is another start.
#[tokio::test(flavor = "multi_thread")]
async fn a_profile_that_asks_for_less_than_hiding_is_left_with_what_it_asks() {
    use ctrlassist_test::Asked;
    const TEST: &str = "a_profile_that_asks_for_less_than_hiding_is_left_with_what_it_asks";
    if !can_make_pads(TEST) {
        return;
    }
    let mut primary = FakePad::new("Less Primary", VENDOR, 0x7f11).unwrap();
    let _assist = FakePad::new("Less Assist", VENDOR, 0x7f12).unwrap();
    let (_dir, socket) = socket();
    let hider = ctrlassist_test::FakeHider::listen(&socket.with_file_name("hider.sock")).unwrap();
    let steam = socket.with_file_name("config.vdf");
    let untouched = "\"InstallConfigStore\"\n{\n\t\"controller_blacklist\"\t\t\"28de/1142\"\n}\n";
    std::fs::write(&steam, untouched).unwrap();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    let hid = |hider: &ctrlassist_test::FakeHider| {
        asked(hider)
            .iter()
            .any(|asked| matches!(asked, Asked::Hide(_)))
    };

    // Grabbed only: nobody else hears the pad, and the service that
    // could have hidden it isn't asked to.
    let grabbed = format!("{}hide = \"grab\"\n", priority(0x7f11, 0x7f12));
    client.put_profile("couch", &grabbed).await.unwrap();
    let mut other = bystander(&primary);
    client.start_setup("couch").await.unwrap();
    let setup = in_phase(&client, "couch", Phase::Running).await;
    assert!(!hid(&hider), "{:?}", asked(&hider));
    assert!(client.list_pads().unwrap().iter().all(|pad| !pad.hidden));
    let mut seen = game(&setup).await;
    primary.south(true).unwrap();
    south_is(&mut seen, true).await;
    other.read(|_| {}).unwrap();
    assert!(other.state().get(gamepad::SOUTH) < 0.5);
    primary.south(false).unwrap();
    south_is(&mut seen, false).await;

    // Not at all: taken up by a start afresh, and then a press is heard
    // both through the virtual pad and on the pad itself.
    let visible = format!("{}hide = \"none\"\n", priority(0x7f11, 0x7f12));
    client.put_profile("couch", &visible).await.unwrap();
    assert!(
        client
            .update_setup("couch", false)
            .await
            .unwrap()
            .disruptive
    );
    assert!(client.update_setup("couch", true).await.unwrap().disruptive);
    let setup = in_phase(&client, "couch", Phase::Running).await;
    let mut seen = game(&setup).await;
    primary.south(true).unwrap();
    south_is(&mut seen, true).await;
    south_is(&mut other, true).await;
    assert!(!hid(&hider), "{:?}", asked(&hider));

    // Steam was told nothing either time: that is for a pad that was to
    // be hidden and couldn't be.
    assert_eq!(std::fs::read_to_string(&steam).unwrap(), untouched);
    client.stop_setup("couch").await.unwrap();
    daemon.stop().await;
}

fn selecting(setup: &Setup) -> (Vec<&str>, usize) {
    assert_eq!(setup.nodes.len(), 1, "{:?}", setup.nodes);
    let node = &setup.nodes[0];
    (
        node.choices.iter().map(String::as_str).collect(),
        node.selected,
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_toggle_hands_over_when_asked_and_says_who_has_it() {
    const TEST: &str = "a_toggle_hands_over_when_asked_and_says_who_has_it";
    if !can_make_pads(TEST) {
        return;
    }
    let mut primary = FakePad::new("Toggle Primary", VENDOR, 0x8101).unwrap();
    let mut assist = FakePad::new("Toggle Assist", VENDOR, 0x8102).unwrap();
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    let toggle = priority(0x8101, 0x8102).replace("priority", "toggle");
    client.put_profile("turns", &toggle).await.unwrap();
    client.start_setup("turns").await.unwrap();
    let selected = async |wanted: usize| {
        eventually(&format!("choice {wanted} never was selected"), async || {
            client.setup("turns").unwrap().is_some_and(|setup| {
                setup
                    .nodes
                    .first()
                    .is_some_and(|node| node.selected == wanted)
            })
        })
        .await;
    };

    // It starts with the primary, and says what there is to choose from.
    selected(0).await;
    let setup = in_phase(&client, "turns", Phase::Running).await;
    assert_eq!(setup.nodes[0].name, "mux");
    assert_eq!(selecting(&setup), (vec!["primary", "assist"], 0));
    let mut game = game(&setup).await;
    assist.south(true).unwrap();
    primary.left_x(i16::MAX).unwrap();
    let primary_has_it = async |game: &mut Pad, has: bool| {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        loop {
            game.read(|_| {}).unwrap();
            let south = game.state().get(gamepad::SOUTH) > 0.5;
            let stick = game.state().get(gamepad::LEFT_X) > 0.5;
            if (stick, south) == (has, !has) {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the virtual pad never followed the {}",
                if has { "primary" } else { "assist" }
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    primary_has_it(&mut game, true).await;

    // Asked for the assist by name: its held button arrives, and the
    // primary's stick is let go.
    client.select("turns", "mux", Some("assist")).await.unwrap();
    selected(1).await;
    primary_has_it(&mut game, false).await;
    // Asked again, nothing changes; asked for the next, it goes round.
    client.select("turns", "mux", Some("assist")).await.unwrap();
    client.select("turns", "mux", None).await.unwrap();
    selected(0).await;
    primary_has_it(&mut game, true).await;
    // By place, too.
    client.select("turns", "mux", Some("1")).await.unwrap();
    selected(1).await;

    // What can't be selected is refused with what can.
    let refused = client
        .select("turns", "mux", Some("nobody"))
        .await
        .unwrap_err();
    assert_eq!(refused.rpc_code(), Some(ErrorCode::BadRequest));
    assert!(refused.to_string().contains("primary, assist"), "{refused}");
    let refused = client.select("turns", "mux", Some("2")).await.unwrap_err();
    assert_eq!(refused.rpc_code(), Some(ErrorCode::BadRequest));
    let refused = client.select("turns", "other", None).await.unwrap_err();
    assert!(refused.to_string().contains("those are mux"), "{refused}");
    let refused = client.select("nothing", "mux", None).await.unwrap_err();
    assert_eq!(refused.rpc_code(), Some(ErrorCode::UnknownSetup));

    // A setup that blends its pads has nothing to select.
    client.stop_setup("turns").await.unwrap();
    client
        .put_profile("both", &priority(0x8101, 0x8102))
        .await
        .unwrap();
    client.start_setup("both").await.unwrap();
    let setup = in_phase(&client, "both", Phase::Running).await;
    assert!(setup.nodes.is_empty());
    let refused = client.select("both", "mux", None).await.unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("no node of the setup both selects"),
        "{refused}"
    );
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_that_asks_to_starts_by_itself_once_its_pads_are_there() {
    const TEST: &str = "a_profile_that_asks_to_starts_by_itself_once_its_pads_are_there";
    if !can_make_pads(TEST) {
        return;
    }
    let _primary = FakePad::new("Auto Primary", VENDOR, 0x8201).unwrap();
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    let profile = format!("{}managed = true\n", priority(0x8201, 0x8202));
    let absent = async |client: &DaemonClient| {
        // Long enough for a start to have shown, had one been coming.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(client.setup("couch").unwrap(), None);
    };

    // Put with a pad missing, it waits: started by hand it would be held,
    // but by itself it starts only when it can run.
    client.put_profile("couch", &profile).await.unwrap();
    absent(&client).await;
    // A pad it doesn't name changes nothing.
    let _bystander = FakePad::new("Auto Bystander", VENDOR, 0x8203).unwrap();
    absent(&client).await;

    // Its last pad connects: it starts, and runs.
    let assist = FakePad::new("Auto Assist", VENDOR, 0x8202).unwrap();
    in_phase(&client, "couch", Phase::Running).await;

    // Stopped by hand, it stays stopped, whatever else connects.
    client.stop_setup("couch").await.unwrap();
    let _another = FakePad::new("Auto Another", VENDOR, 0x8204).unwrap();
    absent(&client).await;

    // One of its pads plugged in again is the cue to start again.
    drop(assist);
    eventually("the assist never left", async || {
        client
            .list_pads()
            .unwrap()
            .iter()
            .any(|pad| pad.product == 0x8202 && !pad.connected)
    })
    .await;
    let _assist = FakePad::new("Auto Assist", VENDOR, 0x8202).unwrap();
    in_phase(&client, "couch", Phase::Running).await;

    // A daemon that starts with the pads there starts it at once.
    daemon.stop().await;
    eventually("the setup outlived the daemon", async || {
        client.setup("couch").unwrap().is_none()
    })
    .await;
    let daemon = start_daemon(&socket).await;
    in_phase(&client, "couch", Phase::Running).await;

    // Put again while it runs, nothing starts twice; put without the
    // asking, and stopped, it no longer starts by itself.
    client.put_profile("couch", &profile).await.unwrap();
    client.stop_setup("couch").await.unwrap();
    client
        .put_profile("couch", &priority(0x8201, 0x8202))
        .await
        .unwrap();
    absent(&client).await;
    // Put with the asking and its pads there, it starts at the put.
    client.put_profile("couch", &profile).await.unwrap();
    in_phase(&client, "couch", Phase::Running).await;
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_update_is_taken_as_the_setup_runs_or_restarts_it_and_says_which() {
    const TEST: &str = "an_update_is_taken_as_the_setup_runs_or_restarts_it_and_says_which";
    if !can_make_pads(TEST) {
        return;
    }
    let mut primary = FakePad::new("Update Primary", VENDOR, 0x8401).unwrap();
    let _assist = FakePad::new("Update Assist", VENDOR, 0x8402).unwrap();
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    let blended = priority(0x8401, 0x8402);
    client.put_profile("couch", &blended).await.unwrap();
    let unknown = client.update_setup("couch", true).await.unwrap_err();
    assert_eq!(unknown.rpc_code(), Some(ErrorCode::UnknownSetup));
    client.start_setup("couch").await.unwrap();
    let before = in_phase(&client, "couch", Phase::Running).await;
    let mut game_before = game(&before).await;
    let phys = phys_of(Path::new(before.sinks[0].node.as_deref().unwrap())).unwrap();
    assert!(before.nodes.is_empty());
    primary.south(true).unwrap();
    south_is(&mut game_before, true).await;

    assert!(!before.stale);
    // Putting a profile changes nothing that runs.
    let toggle = blended.replace("priority", "toggle");
    client.put_profile("couch", &toggle).await.unwrap();
    // But the setup now says the kept profile is another than it runs.
    eventually("the setup never said its profile had changed", async || {
        client
            .setup("couch")
            .unwrap()
            .is_some_and(|setup| setup.stale)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(client.setup("couch").unwrap().unwrap().nodes.is_empty());
    // Asked what an update would cost, the daemon says, and does nothing:
    // the same pads into the same virtual pad is no restart.
    assert!(
        !client
            .update_setup("couch", false)
            .await
            .unwrap()
            .disruptive
    );
    assert!(client.setup("couch").unwrap().unwrap().nodes.is_empty());

    // Asked to, the setup takes the profile as it runs: the game's pad
    // is the one it had, still holding what the primary holds, and the
    // setup now has a node that selects.
    assert!(!client.update_setup("couch", true).await.unwrap().disruptive);
    eventually(
        "the setup never said what its new node selects",
        async || {
            client
                .setup("couch")
                .unwrap()
                .is_some_and(|setup| setup.nodes.len() == 1)
        },
    )
    .await;
    let taken = client.setup("couch").unwrap().unwrap();
    assert_eq!(taken.sinks[0].node, before.sinks[0].node);
    assert!(!taken.stale, "it runs the kept profile again");
    assert!(phys_connected(&phys));
    south_is(&mut game_before, true).await;
    // The new graph is what plays: handed to the assist, the primary's
    // press is no longer the game's, on the same virtual pad.
    client.select("couch", "mux", Some("assist")).await.unwrap();
    south_is(&mut game_before, false).await;
    // A further update keeps who had control.
    client
        .put_profile("couch", &format!("{toggle}managed = true\n"))
        .await
        .unwrap();
    assert!(!client.update_setup("couch", true).await.unwrap().disruptive);
    eventually("the toggle didn't stay with the assist", async || {
        client
            .setup("couch")
            .unwrap()
            .is_some_and(|setup| setup.nodes.len() == 1 && setup.nodes[0].selected == 1)
    })
    .await;
    south_is(&mut game_before, false).await;

    // Another identity is another virtual pad: that restarts the setup
    // on the profile as kept now. The old virtual pad is gone by the
    // reply, and the new setup selects.
    client
        .put_profile("couch", &format!("{toggle}identity = \"xbox\"\n"))
        .await
        .unwrap();
    assert!(
        client
            .update_setup("couch", false)
            .await
            .unwrap()
            .disruptive
    );
    assert!(phys_connected(&phys));
    assert!(client.update_setup("couch", true).await.unwrap().disruptive);
    assert!(
        !phys_connected(&phys),
        "the old virtual pad outlived the update"
    );
    let after = in_phase(&client, "couch", Phase::Running).await;
    eventually("the new setup never said what it selects", async || {
        client
            .setup("couch")
            .unwrap()
            .is_some_and(|setup| setup.nodes.len() == 1)
    })
    .await;
    assert!(after.sources.iter().all(|source| source.present));
    assert_eq!(after.sinks[0].identity, "xbox");
    // The game's old pad is gone for good.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while game_before.read(|_| {}).is_ok() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the game still read the old virtual pad"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // A profile that is wrong now, or gone, leaves what runs as it was.
    let node = after.sinks[0].node.clone();
    client.delete_profile("couch").await.unwrap();
    let gone = client.update_setup("couch", true).await.unwrap_err();
    assert_eq!(gone.rpc_code(), Some(ErrorCode::UnknownProfile));
    assert!(
        client.setup("couch").unwrap().unwrap().stale,
        "its profile is gone"
    );
    assert_eq!(client.setup("couch").unwrap().unwrap().sinks[0].node, node);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn another_pad_is_taken_as_the_setup_runs_and_the_one_given_up_is_shown_again() {
    use ctrlassist_test::Asked;
    const TEST: &str = "another_pad_is_taken_as_the_setup_runs_and_the_one_given_up_is_shown_again";
    if !can_make_pads(TEST) {
        return;
    }
    let mut primary = FakePad::new("Change Primary", VENDOR, 0x8901).unwrap();
    let mut assist = FakePad::new("Change Assist", VENDOR, 0x8902).unwrap();
    let mut third = FakePad::new("Change Third", VENDOR, 0x8903).unwrap();
    let id = |product: u16, name: &str| format!("model:1209:{product:04x}:{name}");
    let (_dir, socket) = socket();
    let hider = ctrlassist_test::FakeHider::listen(&socket.with_file_name("hider.sock")).unwrap();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    client
        .put_profile("couch", &priority(0x8901, 0x8902))
        .await
        .unwrap();
    client.start_setup("couch").await.unwrap();
    let before = in_phase(&client, "couch", Phase::Running).await;
    let mut game = game(&before).await;
    let user = |pad: &str| {
        let pads = client.list_pads().unwrap();
        let pad = pads.iter().find(|known| known.id == pad).unwrap();
        (pad.setup.clone(), pad.hidden)
    };

    // The third pad as the assist: no restart, on the virtual pad the
    // game has. The third is hidden, the primary is left as it is, and
    // the old assist is shown again only once nothing of ours reads it.
    client
        .put_profile("couch", &priority(0x8901, 0x8903))
        .await
        .unwrap();
    assert!(
        !client
            .update_setup("couch", false)
            .await
            .unwrap()
            .disruptive
    );
    assert!(!client.update_setup("couch", true).await.unwrap().disruptive);
    assert_eq!(
        asked(&hider)[2..],
        [
            Asked::Hide(id(0x8903, "Change Third")),
            Asked::Restore(id(0x8902, "Change Assist"))
        ]
    );
    let after = client.setup("couch").unwrap().unwrap();
    assert_eq!(after.phase, Phase::Running);
    assert_eq!(after.sinks[0].node, before.sinks[0].node);
    let pads: Vec<Option<&str>> = after
        .sources
        .iter()
        .map(|source| source.pad.as_deref())
        .collect();
    let (third_id, primary_id) = (id(0x8903, "Change Third"), id(0x8901, "Change Primary"));
    assert_eq!(pads, [Some(third_id.as_str()), Some(primary_id.as_str())]);
    eventually("the pads never said whose they are", async || {
        user(&third_id) == (Some("couch".to_string()), true)
            && user(&id(0x8902, "Change Assist")) == (None, false)
    })
    .await;

    // The pad given up is no longer the game's; the other two are.
    assist.south(true).unwrap();
    third.south(true).unwrap();
    south_is(&mut game, true).await;
    third.south(false).unwrap();
    south_is(&mut game, false).await;
    primary.south(true).unwrap();
    south_is(&mut game, true).await;
    primary.south(false).unwrap();
    south_is(&mut game, false).await;

    // The two change parts: nothing is opened or shown again for that.
    let before = asked(&hider).len();
    client
        .put_profile("couch", &priority(0x8903, 0x8901))
        .await
        .unwrap();
    assert!(!client.update_setup("couch", true).await.unwrap().disruptive);
    assert_eq!(asked(&hider).len(), before);
    let swapped = client.setup("couch").unwrap().unwrap();
    let primary_now = swapped
        .sources
        .iter()
        .find(|source| source.name == "primary");
    assert_eq!(primary_now.unwrap().pad.as_deref(), Some(third_id.as_str()));
    assert!(swapped.sources.iter().all(|source| source.present));

    // A pad another setup has can't be taken, and the setup stays as it is.
    client
        .put_profile("solo", "mode = \"multicast\"\npad = \"model:1209:8902\"\n")
        .await
        .unwrap();
    client.start_setup("solo").await.unwrap();
    in_phase(&client, "solo", Phase::Running).await;
    let exact = format!(
        "mode = \"priority\"\nprimary = \"{}\"\nassist = \"{third_id}\"\n",
        id(0x8902, "Change Assist")
    );
    client.put_profile("couch", &exact).await.unwrap();
    let in_use = client.update_setup("couch", true).await.unwrap_err();
    assert_eq!(in_use.rpc_code(), Some(ErrorCode::PadInUse));
    assert_eq!(
        client.setup("couch").unwrap().unwrap().sources,
        swapped.sources
    );
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pad_in_a_setup_is_buzzed_through_the_setup() {
    const TEST: &str = "a_pad_in_a_setup_is_buzzed_through_the_setup";
    if !can_make_pads(TEST) {
        return;
    }
    let fake = ctrlassist_test::RumblePad::new("Setup Rumble Pad", VENDOR, 0x8501).unwrap();
    let (_dir, socket) = socket();
    // With the hiding service: the daemon's only way to the pad is the
    // descriptor its setup holds.
    let _hider = ctrlassist_test::FakeHider::listen(&socket.with_file_name("hider.sock")).unwrap();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    client
        .put_profile(
            "solo",
            "mode = \"multicast\"\npad = \"model:1209:8501\"\noutputs = 2\n",
        )
        .await
        .unwrap();
    client.start_setup("solo").await.unwrap();
    let setup = in_phase(&client, "solo", Phase::Running).await;
    assert_eq!(setup.sinks.len(), 2);

    let asked = std::time::Instant::now();
    client
        .identify_pad("model:1209:8501", Duration::from_millis(200))
        .await
        .unwrap();
    eventually("the buzz never ended", async || fake.motors().len() >= 2).await;
    assert_eq!(fake.motors(), [(0xa000, 0xa000), (0, 0)]);
    assert!(asked.elapsed() >= Duration::from_millis(200));

    // A virtual pad of the setup is buzzed as a game would rumble it,
    // and the setup sends that on to the pad behind it.
    let virtual_pad = ctrlassist_protocol::rpc::virtual_pad("solo", &setup.sinks[0].name);
    client
        .identify_pad(&virtual_pad, Duration::from_millis(200))
        .await
        .unwrap();
    eventually("the virtual pad's buzz never reached the pad", async || {
        fake.motors().len() >= 4
    })
    .await;
    let felt = fake.motors();
    assert_ne!(felt[2], (0, 0), "{felt:?}");
    assert_eq!(felt.last(), Some(&(0, 0)), "{felt:?}");
    // One that no setup has is refused.
    let none = client
        .identify_pad("virtual:solo/nothing", Duration::from_millis(200))
        .await
        .unwrap_err();
    assert_eq!(none.rpc_code(), Some(ErrorCode::UnknownPad));
    daemon.stop().await;
}

/// The kernel hands a freed node to the next pad to connect, so the node
/// a pad was last seen on can be another pad's before its leaving has
/// been heard of. A setup takes only the pad it wants.
#[tokio::test(flavor = "multi_thread")]
async fn a_setup_never_takes_another_pad_that_got_its_pad_s_node() {
    const TEST: &str = "a_setup_never_takes_another_pad_that_got_its_pad_s_node";
    if !can_make_pads(TEST) {
        return;
    }
    let _primary = FakePad::new("Node Primary", VENDOR, 0x8601).unwrap();
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    client
        .put_profile("couch", &priority(0x8601, 0x8602))
        .await
        .unwrap();
    client.start_setup("couch").await.unwrap();
    in_phase(&client, "couch", Phase::Held).await;

    // Pads of other models come and go as fast as they can, taking and
    // freeing node numbers, while the assist does the same.
    for round in 0..12 {
        let assist = FakePad::new("Node Assist", VENDOR, 0x8602).unwrap();
        in_phase(&client, "couch", Phase::Running).await;
        drop(assist);
        let _stranger = FakePad::new(&format!("Node Stranger {round}"), VENDOR, 0x8603).unwrap();
        let setup = in_phase(&client, "couch", Phase::Held).await;
        assert_eq!(
            setup.sources[0].pad.as_deref(),
            Some("model:1209:8602:Node Assist")
        );
        // Long enough for a wrong pad to have been taken, had one been.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let setup = client.setup("couch").unwrap().unwrap();
        assert!(!setup.sources[0].present, "round {round}: {setup:?}");
        let stranger = client
            .list_pads()
            .unwrap()
            .into_iter()
            .find(|pad| pad.product == 0x8603 && pad.connected);
        assert!(
            stranger.is_none_or(|pad| pad.setup.is_none()),
            "round {round}: a stranger was taken into the setup"
        );
    }
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connected_dualsense_is_copied_and_virtual_ones_are_made_from_it() {
    use ctrlassist_test::Asked;
    const TEST: &str = "a_connected_dualsense_is_copied_and_virtual_ones_are_made_from_it";
    if !ctrlassist_test::can_make_hid_pads(TEST) {
        return;
    }
    let real = ctrlassist_test::FakeDualSense::new(0x01).unwrap();
    // The stand-in service hands over a raw node only if this user can
    // open it, which a machine's own rules decide.
    let raw = ctrlassist_test::raw_node_of(real.event_node());
    if !raw.is_some_and(|raw| ctrlassist_test::can_open(&raw, true)) {
        use std::io::Write as _;
        let _ = writeln!(
            std::io::stderr(),
            "SKIPPED {TEST}: a DualSense's raw node isn't this user's to open here"
        );
        return;
    }
    let id = "address:02:ca:7e:00:51:01";
    let (_dir, socket) = socket();
    let hider = ctrlassist_test::FakeHider::listen(&socket.with_file_name("hider.sock")).unwrap();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    let offered = |client: &DaemonClient| {
        let identities = client.list_identities().unwrap();
        let dualsense = identities
            .iter()
            .find(|identity| identity.id == "dualsense");
        dualsense.is_some_and(|identity| identity.available)
    };

    // A DualSense is something a virtual pad can be from the start. The
    // pad was connected when the daemon started, so it is copied, and
    // virtual ones are made from its data in place of the shipped.
    assert!(offered(&client));
    let kept = socket
        .with_file_name("state")
        .join("identities/dualsense.toml");
    eventually("the DualSense never was copied", async || kept.exists()).await;
    let copied = std::fs::read_to_string(&kept).unwrap();
    // One pad is copied: this one, or a real one if the machine has one
    // connected. A real one that can't be, as when something has it
    // hidden, is asked about first and then this one is.
    let asked = hider.asked();
    assert!(
        !asked.is_empty() && asked.iter().all(|asked| matches!(asked, Asked::Raw(_))),
        "{asked:?}"
    );

    // One pad as two virtual DualSenses.
    client
        .put_profile(
            "twin",
            "mode = \"multicast\"\npad = \"02:ca:7e:00:51:01\"\nidentity = \"dualsense\"\n",
        )
        .await
        .unwrap();
    client.start_setup("twin").await.unwrap();
    let setup = in_phase(&client, "twin", Phase::Running).await;
    assert_eq!(setup.sinks.len(), 2);
    assert!(setup.sinks.iter().all(|sink| sink.identity == "dualsense"));
    assert_ne!(setup.sinks[0].node, setup.sinks[1].node);
    // Each is a pad of the kernel's driver with an address of its own,
    // and neither is taken for a pad to play with.
    let mut games = Vec::new();
    for sink in &setup.sinks {
        let file = File::open(sink.node.as_deref().unwrap()).unwrap();
        let pad = Pad::from_fd(OwnedFd::from(file)).unwrap();
        let ids = pad.identifiers().clone();
        assert_eq!((ids.vendor, ids.product), (0x054c, 0x0ce6));
        assert!(pad.is_own_output(), "{ids:?}");
        games.push((pad, ids.address));
    }
    assert_ne!(games[0].1, games[1].1);
    let known = client.list_pads().unwrap();
    assert!(known.iter().any(|pad| pad.id == id), "{known:?}");
    assert!(
        known
            .iter()
            .all(|pad| !pad.id.starts_with("address:02:ca:55:")),
        "{known:?}"
    );

    // What the pad does, both games see.
    real.south(true);
    for (game, _) in &mut games {
        south_is(game, true).await;
    }
    real.south(false);
    for (game, _) in &mut games {
        south_is(game, false).await;
    }
    client.stop_setup("twin").await.unwrap();
    daemon.stop().await;
    drop(hider);
    drop(real);

    // What was copied is kept for the next daemon, which has no
    // DualSense to ask the service about.
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    assert!(offered(&client));
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), copied);
    // Without one ever copied, a virtual DualSense is made from the
    // shipped data.
    let _pad = FakePad::new("Shipped Pad", VENDOR, 0x8a01).unwrap();
    let (_other_dir, other) = crate::client::socket();
    let empty = start_daemon(&other).await;
    let fresh = hydrated(&other).await;
    assert!(offered(&fresh));
    fresh
        .put_profile(
            "solo",
            "mode = \"multicast\"\npad = \"model:1209:8a01\"\noutputs = 1\nidentity = \"dualsense\"\n",
        )
        .await
        .unwrap();
    fresh.start_setup("solo").await.unwrap();
    let solo = in_phase(&fresh, "solo", Phase::Running).await;
    let file = File::open(solo.sinks[0].node.as_deref().unwrap()).unwrap();
    let pad = Pad::from_fd(OwnedFd::from(file)).unwrap();
    assert_eq!(pad.identifiers().product, 0x0ce6);
    assert!(!other.with_file_name("state").join("identities").exists());
    fresh.stop_setup("solo").await.unwrap();
    empty.stop().await;
    daemon.stop().await;
}

/// What a game sends a virtual DualSense beyond rumble reaches a real
/// DualSense behind it, with the rumble that pad should be at.
#[tokio::test(flavor = "multi_thread")]
async fn a_game_s_lights_reach_a_dualsense_behind_a_virtual_one() {
    use std::io::Write as _;
    const TEST: &str = "a_game_s_lights_reach_a_dualsense_behind_a_virtual_one";
    if !ctrlassist_test::can_make_hid_pads(TEST) {
        return;
    }
    let real = ctrlassist_test::FakeDualSense::new(0x02).unwrap();
    let raw = ctrlassist_test::raw_node_of(real.event_node());
    if !raw.is_some_and(|raw| ctrlassist_test::can_open(&raw, true)) {
        let _ = writeln!(
            std::io::stderr(),
            "SKIPPED {TEST}: a DualSense's raw node isn't this user's to open here"
        );
        return;
    }
    let (_dir, socket) = socket();
    let hider = ctrlassist_test::FakeHider::listen(&socket.with_file_name("hider.sock")).unwrap();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    client
        .put_profile(
            "solo",
            "mode = \"multicast\"\npad = \"02:ca:7e:00:51:02\"\noutputs = 1\nidentity = \"dualsense\"\n",
        )
        .await
        .unwrap();
    client.start_setup("solo").await.unwrap();
    let setup = in_phase(&client, "solo", Phase::Running).await;
    let id = "address:02:ca:7e:00:51:02".to_string();
    assert!(
        hider.asked().contains(&ctrlassist_test::Asked::RawOf(id)),
        "{:?}",
        hider.asked()
    );

    // A game sets the lightbar and rumbles, through the virtual pad's
    // raw node, as SDL does.
    let node = setup.sinks[0].node.as_deref().unwrap();
    let virtual_raw = ctrlassist_test::raw_node_of(Path::new(node)).unwrap();
    let mut game = std::fs::OpenOptions::new()
        .write(true)
        .open(virtual_raw)
        .unwrap();
    let mut report = [0u8; 48];
    report[0] = 0x02;
    // First a rumble, in the older marking. It reaches the pad marked
    // the way that pad's firmware takes it, which is the newer one.
    (report[1], report[3], report[4]) = (0x03, 0x40, 0x80);
    game.write_all(&report).unwrap();
    let rumbling =
        |sent: &[u8]| sent[39] & 0x04 != 0 && sent[1] & 0x01 == 0 && sent[3..5] == [0x40, 0x80];
    eventually("the real pad never got the rumble", async || {
        real.outputs().last().is_some_and(|sent| rumbling(sent))
    })
    .await;
    // Then the lightbar's color, with the flag that says to take it and
    // the rumble as it was: the pad gets the color, still rumbling.
    report[2] = 0x04;
    (report[45], report[46], report[47]) = (0xff, 0x80, 0x10);
    game.write_all(&report).unwrap();
    eventually("the real pad never got the lightbar", async || {
        real.outputs().last().is_some_and(|sent| {
            sent[2] & 0x04 != 0 && sent[45..48] == [0xff, 0x80, 0x10] && rumbling(sent)
        })
    })
    .await;
    // And the rumble ends when the game's does: a report that marks none.
    report[1] = 0;
    game.write_all(&report).unwrap();
    eventually("the real pad's rumble never ended", async || {
        real.outputs()
            .last()
            .is_some_and(|sent| sent[39] & 0x04 == 0 && sent[1] & 0x01 == 0 && sent[3..5] == [0, 0])
    })
    .await;
    client.stop_setup("solo").await.unwrap();
    daemon.stop().await;
}

/// A reload brings a running setup up to its profile as it is on disk:
/// as it runs where the virtual pads stay, and by a restart where one
/// must be made anew.
#[tokio::test(flavor = "multi_thread")]
async fn a_reload_brings_a_running_setup_up_to_its_profile() {
    const TEST: &str = "a_reload_brings_a_running_setup_up_to_its_profile";
    if !can_make_pads(TEST) {
        return;
    }
    let _primary = FakePad::new("Reload Primary", VENDOR, 0x8d01).unwrap();
    let _assist = FakePad::new("Reload Assist", VENDOR, 0x8d02).unwrap();
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    let blended = priority(0x8d01, 0x8d02);
    client.put_profile("couch", &blended).await.unwrap();
    client.start_setup("couch").await.unwrap();
    let before = in_phase(&client, "couch", Phase::Running).await;
    // Once udev has announced the virtual pad, which is when it can be
    // asked where it is.
    let _game = game(&before).await;
    let phys = phys_of(Path::new(before.sinks[0].node.as_deref().unwrap())).unwrap();
    assert!(before.nodes.is_empty());
    let file = socket.with_file_name("config").join("profiles/couch.toml");

    // The same virtual pad: taken as the setup runs.
    let toggle = blended.replace("priority", "toggle");
    std::fs::write(&file, &toggle).unwrap();
    let reloaded = client.reload().await.unwrap();
    assert_eq!(reloaded.changed, ["couch"]);
    assert_eq!(reloaded.updating, ["couch"]);
    eventually("the setup never took the reloaded profile", async || {
        client
            .setup("couch")
            .unwrap()
            .is_some_and(|setup| setup.nodes.len() == 1)
    })
    .await;
    assert_eq!(
        client.setup("couch").unwrap().unwrap().sinks[0].node,
        before.sinks[0].node
    );
    assert!(phys_connected(&phys));

    // Another identity: the setup is restarted on it.
    std::fs::write(&file, format!("{toggle}identity = \"xbox\"\n")).unwrap();
    assert_eq!(client.reload().await.unwrap().updating, ["couch"]);
    eventually(
        "the setup never restarted on the reloaded profile",
        async || {
            client.setup("couch").unwrap().is_some_and(|setup| {
                setup.phase == Phase::Running
                    && setup
                        .sinks
                        .first()
                        .is_some_and(|sink| sink.identity == "xbox")
            })
        },
    )
    .await;
    assert!(
        !phys_connected(&phys),
        "the old virtual pad outlived the reload"
    );

    // A profile that is wrong now leaves what runs as it was.
    std::fs::write(&file, "mode = \"priority\"\npad = \"model:1209:8d01\"\n").unwrap();
    let reloaded = client.reload().await.unwrap();
    assert_eq!(reloaded.wrong.len(), 1);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let still = client.setup("couch").unwrap().unwrap();
    assert_eq!(still.phase, Phase::Running);
    assert_eq!(still.sinks[0].identity, "xbox");
    daemon.stop().await;
}
