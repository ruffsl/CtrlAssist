//! A Steam Controller in a setup, against a live daemon and the fake
//! the kernel's own `hid-steam` binds: read through its raw node, which
//! takes its event node away while the setup holds it, and never said to
//! have left for that.
//!
//! Each test skips where the machine can't make the fake (a kernel older
//! than 7.3, Steam running, no uhid), and where the fake's raw node isn't
//! this user's to open, as on a machine with neither CtrlAssist's udev
//! rule nor Steam's.

use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ctrlassist::{DaemonClient, ErrorCode, Phase};
use ctrlassist_core::gamepad;
use ctrlassist_device::Pad;
use ctrlassist_protocol::{bootstrap, keys};
use ctrlassist_test::{
    FakeSteamController, StateReport, SteamButton, SteamState, can_make_pads, can_make_steam_pads,
    can_open,
};
use zenoh::Session;

use crate::client::{eventually, socket, start_daemon};
use crate::setups::{game, hydrated, in_phase};

/// A fake Steam Controller whose nodes are this user's to open, or none,
/// with why said, where the machine can't give one.
fn fake(test: &str) -> Option<FakeSteamController> {
    if !(can_make_steam_pads(test) && can_make_pads(test)) {
        return None;
    }
    let fake = FakeSteamController::new().unwrap();
    let gamepad = fake.gamepad().expect("the driver makes the gamepad");
    let raw = fake.raw_node().expect("the driver makes a raw node");
    let open = |node: &std::path::Path, write: bool| {
        let deadline = std::time::Instant::now() + Duration::from_secs(4);
        while !can_open(node, write) {
            if std::time::Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    };
    if !open(&gamepad.event, false) || !open(&raw, true) {
        let _ = writeln!(
            std::io::stderr(),
            "SKIPPED {test}: the fake's nodes aren't this user's to open here"
        );
        return None;
    }
    Some(fake)
}

/// What the daemon calls the fake: its address, which its driver gives
/// as the controller's serial.
fn id_of(fake: &FakeSteamController) -> String {
    format!("address:{}", fake.serial())
}

/// Every state of the pad `id` the daemon publishes from now on, whether
/// it was connected in each.
struct Published {
    _session: Session,
    _subscriber: zenoh::pubsub::Subscriber<()>,
    connected: Arc<Mutex<Vec<bool>>>,
}

impl Published {
    async fn watch(socket: &std::path::Path, id: &str) -> Self {
        let session = zenoh::open(bootstrap::client_config(socket).unwrap())
            .await
            .unwrap();
        let connected = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&connected);
        let subscriber = session
            .declare_subscriber(keys::pad(&keys::pad_id(id)))
            .callback(move |sample| {
                let body: serde_json::Value =
                    serde_json::from_slice(&sample.payload().to_bytes()).unwrap_or_default();
                if let Some(connected) = body.get("connected").and_then(serde_json::Value::as_bool)
                {
                    seen.lock().unwrap().push(connected);
                }
            })
            .await
            .unwrap();
        Self {
            _session: session,
            _subscriber: subscriber,
            connected,
        }
    }

    fn states(&self) -> Vec<bool> {
        self.connected.lock().unwrap().clone()
    }

    /// Asserts the pad was published connected every time, and at least
    /// `least` times.
    fn never_left(&self, least: usize) {
        let states = self.states();
        assert!(
            states.len() >= least && states.iter().all(|connected| *connected),
            "{states:?}"
        );
    }
}

/// The pad `id` as the daemon lists it, once and only once.
fn listed(client: &DaemonClient, id: &str) -> ctrlassist::Pad {
    let pads: Vec<_> = client
        .list_pads()
        .unwrap()
        .into_iter()
        .filter(|pad| pad.id == id)
        .collect();
    assert_eq!(pads.len(), 1, "{pads:?}");
    pads.into_iter().next().unwrap()
}

/// Presses or releases south on the fake until the virtual pad a game
/// reads holds it so: a report is read once, and the reader may start
/// after the first is sent.
async fn south_through(fake: &FakeSteamController, game: &mut Pad, pressed: bool) {
    let state = if pressed {
        SteamState::default().with(SteamButton::South)
    } else {
        SteamState::default()
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        fake.send_state(&state, StateReport::Full).unwrap();
        game.read(|_| {}).unwrap();
        if (game.state().get(gamepad::SOUTH) > 0.5) == pressed {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the virtual pad never held south as {pressed}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Whether the daemon has the pad on a node again, as an `identify`
/// says: one away, with no node to buzz it through, is refused as away.
async fn on_a_node(client: &DaemonClient, id: &str) -> bool {
    match client.identify_pad(id, Duration::from_millis(1)).await {
        Ok(()) => true,
        Err(error) => !error.to_string().contains("is away"),
    }
}

/// A profile that has a fake Steam Controller starts, and its presses
/// reach the virtual pad, read through its raw node, which takes its
/// joystick away from games; the pad is in the setup, connected and
/// listed once throughout, and nothing ever says it left. Stopped, its
/// joystick is back, and it is free, one pad still. Started again, and
/// stopped again, the same.
#[tokio::test(flavor = "multi_thread")]
async fn a_steam_controller_plays_through_a_setup_and_never_leaves() {
    const TEST: &str = "a_steam_controller_plays_through_a_setup_and_never_leaves";
    let Some(fake) = fake(TEST) else {
        return;
    };
    let id = id_of(&fake);
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    eventually("the daemon never listed the fake", async || {
        client
            .list_pads()
            .unwrap()
            .iter()
            .any(|pad| pad.id == id && pad.connected)
    })
    .await;
    let pad = listed(&client, &id);
    assert_eq!((pad.vendor, pad.product), (0x28de, 0x1302));
    let published = Published::watch(&socket, &id).await;
    // Named by its id, which is its serial and no MAC address.
    client
        .put_profile("couch", &format!("mode = \"unicast\"\npad = \"{id}\"\n"))
        .await
        .unwrap();

    for round in 0..2 {
        client.start_setup("couch").await.unwrap();
        let setup = in_phase(&client, "couch", Phase::Running).await;
        assert_eq!(setup.sources[0].pad.as_deref(), Some(id.as_str()));
        assert!(setup.sources[0].present, "{setup:?}");
        // Its driver has taken its gamepad away: games that read event
        // nodes see nothing of it, as of a pad grabbed.
        assert!(fake.inputs_gone(), "round {round}");
        let pad = listed(&client, &id);
        assert!(pad.connected && !pad.hidden, "{pad:?}");
        assert_eq!(pad.setup.as_deref(), Some("couch"));

        let mut game = game(&setup).await;
        south_through(&fake, &mut game, true).await;
        south_through(&fake, &mut game, false).await;

        client.stop_setup("couch").await.unwrap();
        // Given back to its driver, which makes its gamepad again.
        assert!(fake.gamepad().is_some(), "round {round}");
        eventually("the daemon never had the pad on a node again", async || {
            on_a_node(&client, &id).await
        })
        .await;
        let pad = listed(&client, &id);
        assert!(pad.connected && pad.setup.is_none(), "{pad:?}");
        published.never_left(2 * (round + 1));
    }
    daemon.stop().await;
}

/// A profile that starts by itself, once its Steam Controller is there,
/// and is stopped by hand, stays stopped as the pad's node comes back:
/// that is no pad plugged in.
#[tokio::test(flavor = "multi_thread")]
async fn a_managed_profile_stopped_by_hand_stays_stopped_as_the_node_returns() {
    const TEST: &str = "a_managed_profile_stopped_by_hand_stays_stopped_as_the_node_returns";
    let Some(fake) = fake(TEST) else {
        return;
    };
    let id = id_of(&fake);
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    eventually("the daemon never listed the fake", async || {
        client.list_pads().unwrap().iter().any(|pad| pad.id == id)
    })
    .await;
    client
        .put_profile(
            "couch",
            &format!("mode = \"unicast\"\npad = \"{id}\"\nmanaged = true\n"),
        )
        .await
        .unwrap();
    in_phase(&client, "couch", Phase::Running).await;
    assert!(fake.inputs_gone());

    client.stop_setup("couch").await.unwrap();
    assert!(fake.gamepad().is_some());
    eventually("the daemon never had the pad on a node again", async || {
        on_a_node(&client, &id).await
    })
    .await;
    // Asked of the reactor, which has taken in the node's return by now:
    // a start then would have made the setup at once.
    let stopped = client.update_setup("couch", false).await.unwrap_err();
    assert_eq!(stopped.rpc_code(), Some(ErrorCode::UnknownSetup));
    assert!(client.setup("couch").unwrap().is_none());
    daemon.stop().await;
}

/// A Steam Controller unplugged while its setup holds it is the pad gone,
/// as what reads it says, and the setup is held for it; plugged in again
/// it is a pad plugged in, and the setup takes it back.
#[tokio::test(flavor = "multi_thread")]
async fn a_steam_controller_unplugged_while_held_is_gone_and_its_setup_held() {
    const TEST: &str = "a_steam_controller_unplugged_while_held_is_gone_and_its_setup_held";
    let Some(fake) = fake(TEST) else {
        return;
    };
    let id = id_of(&fake);
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    eventually("the daemon never listed the fake", async || {
        client.list_pads().unwrap().iter().any(|pad| pad.id == id)
    })
    .await;
    client
        .put_profile("couch", &format!("mode = \"unicast\"\npad = \"{id}\"\n"))
        .await
        .unwrap();
    client.start_setup("couch").await.unwrap();
    in_phase(&client, "couch", Phase::Running).await;
    assert!(fake.inputs_gone());

    drop(fake);
    let setup = in_phase(&client, "couch", Phase::Held).await;
    assert!(!setup.sources[0].present);
    eventually("the pad was never said to have gone", async || {
        !listed(&client, &id).connected
    })
    .await;
    assert_eq!(listed(&client, &id).setup.as_deref(), Some("couch"));
    client.stop_setup("couch").await.unwrap();
    daemon.stop().await;
}

/// A Steam Controller and a DualSense in one setup: the one read through
/// its raw node, the other grabbed, there being no hiding service here;
/// each one's press reaches the virtual pad. A change that gives the
/// Steam Controller up for another pad gives it back to its driver as the
/// setup runs, and one that brings it takes it again, its node going and
/// coming back each time with nothing said of it.
#[tokio::test(flavor = "multi_thread")]
async fn a_steam_controller_beside_a_dualsense_and_swapped_in_and_out() {
    const TEST: &str = "a_steam_controller_beside_a_dualsense_and_swapped_in_and_out";
    if !ctrlassist_test::can_make_hid_pads(TEST) {
        return;
    }
    let Some(fake) = fake(TEST) else {
        return;
    };
    let dualsense = ctrlassist_test::FakeDualSense::new(0x5c).unwrap();
    let _plain = ctrlassist_test::FakePad::new("Steam Swap", 0x1209, 0x8f31).unwrap();
    let id = id_of(&fake);
    let other = "address:02:ca:7e:00:51:5c";
    let plain = "model:1209:8f31:Steam Swap";
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    eventually("the daemon never listed both", async || {
        let pads = client.list_pads().unwrap();
        [id.as_str(), other, plain]
            .iter()
            .all(|wanted| pads.iter().any(|pad| pad.id == *wanted && pad.connected))
    })
    .await;
    let published = Published::watch(&socket, &id).await;
    let both = format!("mode = \"priority\"\nprimary = \"{id}\"\nassist = \"{other}\"\n");
    client.put_profile("couch", &both).await.unwrap();
    client.start_setup("couch").await.unwrap();
    let setup = in_phase(&client, "couch", Phase::Running).await;
    assert!(fake.inputs_gone());
    let mut game = game(&setup).await;
    south_through(&fake, &mut game, true).await;
    south_through(&fake, &mut game, false).await;
    dualsense.south(true);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        game.read(|_| {}).unwrap();
        if game.state().get(gamepad::SOUTH) > 0.5 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the DualSense's press never reached the game"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    dualsense.south(false);

    // Another pad in its place: the Steam Controller is given back.
    client
        .put_profile(
            "couch",
            &format!("mode = \"priority\"\nprimary = \"{plain}\"\nassist = \"{other}\"\n"),
        )
        .await
        .unwrap();
    let updated = client.update_setup("couch", true).await.unwrap();
    assert!(!updated.disruptive, "{updated:?}");
    assert!(fake.gamepad().is_some());
    eventually("the Steam Controller was never let go", async || {
        listed(&client, &id).setup.is_none() && on_a_node(&client, &id).await
    })
    .await;

    // And both again: taken as the setup runs.
    client.put_profile("couch", &both).await.unwrap();
    let updated = client.update_setup("couch", true).await.unwrap();
    assert!(!updated.disruptive, "{updated:?}");
    assert!(fake.inputs_gone());
    eventually("the Steam Controller was never taken", async || {
        client
            .setup("couch")
            .unwrap()
            .is_some_and(|setup| setup.sources.iter().all(|source| source.present))
    })
    .await;
    let setup = client.setup("couch").unwrap().unwrap();
    let mut game = crate::setups::game(&setup).await;
    south_through(&fake, &mut game, true).await;
    south_through(&fake, &mut game, false).await;
    client.stop_setup("couch").await.unwrap();
    assert!(fake.gamepad().is_some());
    published.never_left(4);
    daemon.stop().await;
}

/// A profile that asks for its pads to be left to games can't have a
/// Steam Controller: reading it takes it away from them. The start is
/// refused with why, and the pad is left as it was.
#[tokio::test(flavor = "multi_thread")]
async fn a_steam_controller_can_t_be_left_to_games() {
    const TEST: &str = "a_steam_controller_can_t_be_left_to_games";
    let Some(fake) = fake(TEST) else {
        return;
    };
    let id = id_of(&fake);
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    eventually("the daemon never listed the fake", async || {
        client.list_pads().unwrap().iter().any(|pad| pad.id == id)
    })
    .await;
    client
        .put_profile(
            "couch",
            &format!("mode = \"unicast\"\npad = \"{id}\"\nhide = \"none\"\n"),
        )
        .await
        .unwrap();
    let refused = client.start_setup("couch").await.unwrap_err();
    assert_eq!(refused.rpc_code(), Some(ErrorCode::SetupFailed));
    assert!(refused.to_string().contains("hide"), "{refused}");
    // Published as starting, and then withdrawn, which the mirror may
    // hear after the refusal.
    eventually("the setup stayed listed", async || {
        client.setup("couch").unwrap().is_none()
    })
    .await;
    assert!(fake.gamepad().is_some(), "its gamepad is where it was");
    eventually("the pad stayed taken", async || {
        let pad = listed(&client, &id);
        pad.connected && pad.setup.is_none()
    })
    .await;
    assert!(on_a_node(&client, &id).await);
    daemon.stop().await;
}
