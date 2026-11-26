//! Pads through the SDK, against a live daemon and fake pads: what the
//! daemon knows of them, what it remembers, and the two things to ask of
//! one.
//!
//! The machine's own pads are in every list too, so each case looks only
//! for the pad it made, by a model number no other case uses.

use std::time::{Duration, Instant};

use ctrlassist::{DaemonClient, ErrorCode, Pad, StateEvent, StateWatcher, StateWatcherConfig};
use ctrlassist_test::{FakePad, RumblePad, can_make_pads, uinput_available};

use crate::client::{eventually, next_matching, socket, start_daemon};

const VENDOR: u16 = 0x1209;

/// A client whose mirror holds all the daemon has published.
async fn hydrated(socket: &std::path::Path) -> DaemonClient {
    let client = DaemonClient::connect(socket).await.unwrap();
    eventually("the client never hydrated", async || client.is_hydrated()).await;
    client
}

fn of_model(client: &DaemonClient, product: u16) -> Option<Pad> {
    client
        .list_pads()
        .unwrap()
        .into_iter()
        .find(|pad| (pad.vendor, pad.product) == (VENDOR, product))
}

fn refusal(outcome: ctrlassist::Result<()>) -> ErrorCode {
    outcome
        .expect_err("the daemon refuses this")
        .rpc_code()
        .expect("the failure is the daemon's refusal")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pad_is_listed_while_connected_and_remembered_until_forgotten() {
    const TEST: &str = "a_pad_is_listed_while_connected_and_remembered_until_forgotten";
    if !can_make_pads(TEST) {
        return;
    }
    const PRODUCT: u16 = 0x7a01;
    let model = format!("model:{VENDOR:04x}:{PRODUCT:04x}");
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    assert_eq!(of_model(&client, PRODUCT), None);
    assert_eq!(
        refusal(client.forget_pad(&model).await),
        ErrorCode::UnknownPad
    );

    // It connects: listed, with what the kernel says of it.
    let mut watcher = StateWatcher::new(&client, None, StateWatcherConfig::default());
    let fake = FakePad::new("SDK Listed Pad", VENDOR, PRODUCT).unwrap();
    let id = format!("{model}:SDK Listed Pad");
    let changed = StateEvent::PadChanged { pad: id.clone() };
    next_matching(&mut watcher, |e| *e == changed).await;
    // Whether it can rumble is learned after it is taken in, and follows
    // as a change.
    while of_model(&client, PRODUCT).unwrap().rumble.is_none() {
        next_matching(&mut watcher, |e| *e == changed).await;
    }
    let pad = of_model(&client, PRODUCT).unwrap();
    assert_eq!(pad.id, id);
    assert_eq!(pad.name, "SDK Listed Pad");
    assert!(pad.connected && !pad.hidden && pad.setup.is_none());
    assert_eq!(pad.rumble, Some(false));

    // While connected it can't be forgotten, and it has nothing to buzz
    // with.
    assert_eq!(
        refusal(client.forget_pad(&id).await),
        ErrorCode::PadConnected
    );
    assert_eq!(
        refusal(
            client
                .identify_pad(&model, Duration::from_millis(100))
                .await
        ),
        ErrorCode::PadWithoutRumble
    );

    // It leaves: still known, no longer connected.
    // The change that said whether it can rumble may still be queued.
    drop(fake);
    while of_model(&client, PRODUCT).unwrap().connected {
        next_matching(&mut watcher, |e| *e == changed).await;
    }
    assert_eq!(
        refusal(
            client
                .identify_pad(&model, Duration::from_millis(100))
                .await
        ),
        ErrorCode::PadDisconnected
    );

    // A daemon that starts later still knows it, from its first state on.
    daemon.stop().await;
    let daemon = start_daemon(&socket).await;
    next_matching(&mut watcher, |e| {
        *e == StateEvent::Hydrated(ctrlassist::Service::Daemon)
    })
    .await;
    let remembered = of_model(&client, PRODUCT).unwrap();
    assert_eq!(remembered.id, id);
    assert!(!remembered.connected);
    assert_eq!(remembered.rumble, Some(false));

    // Forgotten, it is gone from the list, and from the next daemon's.
    client.forget_pad(&model).await.unwrap();
    next_matching(&mut watcher, |e| *e == changed).await;
    assert_eq!(of_model(&client, PRODUCT), None);
    daemon.stop().await;
    let daemon = start_daemon(&socket).await;
    next_matching(&mut watcher, |e| {
        *e == StateEvent::Hydrated(ctrlassist::Service::Daemon)
    })
    .await;
    assert_eq!(of_model(&client, PRODUCT), None);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pad_connected_before_the_daemon_is_in_its_first_state() {
    const TEST: &str = "a_pad_connected_before_the_daemon_is_in_its_first_state";
    if !can_make_pads(TEST) {
        return;
    }
    const PRODUCT: u16 = 0x7a02;
    let _fake = FakePad::new("SDK Early Pad", VENDOR, PRODUCT).unwrap();
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    // Hydrated means all of it: no wait before the read.
    let client = hydrated(&socket).await;
    assert!(of_model(&client, PRODUCT).unwrap().connected);
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn identify_buzzes_the_pad_for_as_long_as_asked() {
    const TEST: &str = "identify_buzzes_the_pad_for_as_long_as_asked";
    if !can_make_pads(TEST) {
        return;
    }
    const PRODUCT: u16 = 0x7a03;
    let fake = RumblePad::new("SDK Rumble Pad", VENDOR, PRODUCT).unwrap();
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    let pad = of_model(&client, PRODUCT).unwrap();
    assert_eq!(pad.rumble, Some(true));

    // Named by its key segment, as a client that read the key would.
    let segment = ctrlassist_protocol::keys::pad_id(&pad.id);
    let asked = Instant::now();
    client
        .identify_pad(&segment, Duration::from_millis(300))
        .await
        .unwrap();
    // The reply means the pad took the effect; a thread of the pad's own
    // then sets the motors running.
    eventually("the buzz never started", async || !fake.motors().is_empty()).await;
    assert_eq!(fake.motors()[..1], [(0xa000, 0xa000)]);
    eventually("the buzz never ended", async || fake.motors().len() > 1).await;
    assert_eq!(fake.motors(), [(0xa000, 0xa000), (0, 0)]);
    let buzzed = asked.elapsed();
    assert!(
        buzzed >= Duration::from_millis(300),
        "a 300 ms buzz ended after {buzzed:?}"
    );

    // A daemon that stops takes a buzz under way with it.
    client
        .identify_pad(&pad.id, Duration::from_secs(5))
        .await
        .unwrap();
    daemon.stop().await;
    eventually("the buzz outlived the daemon", async || {
        fake.motors().len() == 4
    })
    .await;
    assert_eq!(fake.motors()[2..], [(0xa000, 0xa000), (0, 0)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_buzz_that_cannot_be_asked_for_is_refused_with_why() {
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    let nobody = "address:00:00:00:00:00:00";
    assert_eq!(
        refusal(
            client
                .identify_pad(nobody, Duration::from_millis(100))
                .await
        ),
        ErrorCode::UnknownPad
    );
    for duration in [
        Duration::ZERO,
        Duration::from_secs(6),
        Duration::from_secs(1 << 40),
    ] {
        let error = client.identify_pad(nobody, duration).await.unwrap_err();
        assert_eq!(
            error.rpc_code(),
            Some(ErrorCode::BadRequest),
            "{duration:?}"
        );
        assert!(error.to_string().contains("from 1 to 5000 ms"), "{error}");
    }
    daemon.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_identities_are_listed_with_whether_they_can_be_made() {
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    let identities = client.list_identities().unwrap();
    let listed: Vec<(&str, u16, u16, bool)> = identities
        .iter()
        .map(|identity| {
            (
                identity.id.as_str(),
                identity.vendor,
                identity.product,
                identity.available,
            )
        })
        .collect();
    let available = uinput_available();
    assert_eq!(
        listed,
        [
            // Made through uhid, which a machine may not offer.
            (
                "dualsense",
                0x054c,
                0x0ce6,
                ctrlassist_test::uhid_available()
            ),
            ("generic", 0x1234, 0x5678, available),
            // Made through uhid for the kernel's own driver for the pad,
            // which a machine may not have loaded.
            (
                "steam-controller",
                0x28de,
                0x1302,
                ctrlassist_test::uhid_available()
                    && std::path::Path::new("/sys/bus/hid/drivers/hid-steam").exists()
            ),
            ("xbox", 0x045e, 0x0b12, available)
        ]
    );
    assert_eq!(identities[3].name, "Microsoft Xbox Series S|X Controller");

    // Gone with the daemon, as everything it published is.
    let mut watcher = StateWatcher::new(&client, None, StateWatcherConfig::default());
    daemon.stop().await;
    next_matching(&mut watcher, |e| *e == StateEvent::IdentitiesChanged).await;
    assert!(client.list_identities().unwrap().is_empty());
}

/// A daemon in use leaves the tests' fake pads alone: a test run beside
/// it neither fills its list of pads nor starts its profiles.
#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_that_was_not_asked_to_takes_no_fake_pad_for_a_pad() {
    const TEST: &str = "a_daemon_that_was_not_asked_to_takes_no_fake_pad_for_a_pad";
    if !can_make_pads(TEST) {
        return;
    }
    let _before = FakePad::new("Unseen Before", 0x1209, 0x8c01).unwrap();
    let (_dir, socket) = socket();
    let daemon = crate::client::start_daemon_taking(&socket, false).await;
    let client = hydrated(&socket).await;
    client
        .put_profile(
            "solo",
            "mode = \"multicast\"\npad = \"model:1209:8c01\"\nmanaged = true\n",
        )
        .await
        .unwrap();
    let _after = FakePad::new("Unseen After", 0x1209, 0x8c01).unwrap();
    let _rumble = RumblePad::new("Unseen Rumble", 0x1209, 0x8c02).unwrap();
    // Time for the pads to have been seen, had they been taken.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let unseen = |pads: Vec<Pad>| pads.iter().all(|pad| !pad.name.starts_with("Unseen"));
    assert!(unseen(client.list_pads().unwrap()));
    assert!(client.list_setups().unwrap().is_empty());
    daemon.stop().await;

    // One that was asked to takes them, under the names they had before
    // they were marked: where a fake pad says it is plugged is its
    // marker, and no part of what it is known by.
    let other = crate::client::socket();
    let daemon = start_daemon(&other.1).await;
    let client = hydrated(&other.1).await;
    eventually("the fake pads never were listed", async || {
        let pads = client.list_pads().unwrap();
        pads.iter()
            .any(|pad| pad.id == "model:1209:8c01:Unseen Before")
            && pads
                .iter()
                .any(|pad| pad.id == "model:1209:8c02:Unseen Rumble")
    })
    .await;
    daemon.stop().await;
}
