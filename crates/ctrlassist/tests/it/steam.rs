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
use ctrlassist_daemon::Daemon;
use ctrlassist_device::Pad;
use ctrlassist_protocol::{bootstrap, keys};
use ctrlassist_test::{
    FakeSteamController, Received, StateReport, SteamButton, SteamControllerBuilder, SteamState,
    SteamTransport, can_make_pads, can_make_steam_pads, may_open_once_udev_is_done,
};
use zenoh::Session;

use crate::client::{eventually, socket, start_daemon};
use crate::setups::{game, hydrated, in_phase};

/// A fake Steam Controller made by `builder` whose nodes are this user's
/// to open, or none, with why said, where the machine can't give one.
/// Whether they are is asked of the kernel without opening them: the raw
/// node opened would take the fake's gamepad away.
fn fake_made(test: &str, builder: SteamControllerBuilder) -> Option<FakeSteamController> {
    if !(can_make_steam_pads(test) && can_make_pads(test)) {
        return None;
    }
    let fake = builder.build().unwrap();
    let gamepad = fake.gamepad().expect("the driver makes the gamepad");
    let raw = fake.raw_node().expect("the driver makes a raw node");
    if !may_open_once_udev_is_done(&gamepad.event, false) || !may_open_once_udev_is_done(&raw, true)
    {
        let _ = writeln!(
            std::io::stderr(),
            "SKIPPED {test}: the fake's nodes aren't this user's to open here"
        );
        return None;
    }
    Some(fake)
}

/// A fake on a cable, as [`fake_made`].
fn fake(test: &str) -> Option<FakeSteamController> {
    fake_made(test, FakeSteamController::builder())
}

/// What the daemon calls the fake: its address, which its driver gives
/// as the controller's serial.
fn id_of(fake: &FakeSteamController) -> String {
    format!("address:{}", fake.serial())
}

/// A state of a pad as published: whether it was connected, and the
/// setup it was in.
type State = (bool, Option<String>);

/// Every state of the pad `id` the daemon publishes from now on: whether
/// it was connected in each, and the setup it was in.
struct Published {
    _session: Session,
    _subscriber: zenoh::pubsub::Subscriber<()>,
    _barrier: zenoh::pubsub::Subscriber<()>,
    states: Arc<Mutex<Vec<State>>>,
    /// How many times the profile `barrier` was heard put.
    barriers: Arc<std::sync::atomic::AtomicUsize>,
}

impl Published {
    async fn watch(socket: &std::path::Path, id: &str) -> Self {
        let session = zenoh::open(bootstrap::client_config(socket).unwrap())
            .await
            .unwrap();
        let states = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&states);
        let subscriber = session
            .declare_subscriber(keys::pad(&keys::pad_id(id)))
            .callback(move |sample| {
                let body: serde_json::Value =
                    serde_json::from_slice(&sample.payload().to_bytes()).unwrap_or_default();
                if let Some(connected) = body.get("connected").and_then(serde_json::Value::as_bool)
                {
                    let setup = body
                        .get("setup")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                    seen.lock().unwrap().push((connected, setup));
                }
            })
            .await
            .unwrap();
        let barriers = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let heard = Arc::clone(&barriers);
        let barrier = session
            .declare_subscriber(keys::profile("barrier"))
            .callback(move |_| {
                heard.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            })
            .await
            .unwrap();
        Self {
            _session: session,
            _subscriber: subscriber,
            _barrier: barrier,
            states,
            barriers,
        }
    }

    fn states(&self) -> Vec<State> {
        self.states.lock().unwrap().clone()
    }

    /// Has the daemon publish something after all it published of the
    /// pad, a profile put, and waits for that to arrive here, the daemon's
    /// samples reaching this session in the order they were put; then
    /// asserts the pad was published connected every time, at least
    /// `least` times, and last free.
    async fn never_left(&self, client: &DaemonClient, least: usize) {
        let before = self.barriers.load(std::sync::atomic::Ordering::Acquire);
        let toml = format!(
            "mode = \"unicast\"\npad = \"model:1209:{:04x}\"\n",
            before + 1
        );
        client.put_profile("barrier", &toml).await.unwrap();
        eventually("the barrier was never heard", async || {
            self.barriers.load(std::sync::atomic::Ordering::Acquire) > before
        })
        .await;
        assert_eq!(self.states().last(), Some(&(true, None)));
        let states = self.states();
        assert!(
            states.len() >= least && states.iter().all(|(connected, _)| *connected),
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
        published.never_left(&client, 2 * (round + 1)).await;
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

/// A daemon with `fake` listed, and a client of it, with `toml` put as
/// the profile `couch`.
async fn with_couch(
    fake: &FakeSteamController,
    toml: &str,
) -> (tempfile::TempDir, Daemon, DaemonClient) {
    let id = id_of(fake);
    let (dir, socket) = socket();
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
    client.put_profile("couch", toml).await.unwrap();
    (dir, daemon, client)
}

/// The profile `couch` over the pad `id` alone.
fn unicast(id: &str) -> String {
    format!("mode = \"unicast\"\npad = \"{id}\"\n")
}

/// Its setup taking the pad back and playing it once it is there again.
async fn plays_again(client: &DaemonClient, fake: &FakeSteamController) {
    let setup = in_phase(client, "couch", Phase::Running).await;
    assert!(setup.sources[0].present, "{setup:?}");
    assert!(fake.inputs_gone());
    let mut game = game(&setup).await;
    south_through(fake, &mut game, true).await;
    south_through(fake, &mut game, false).await;
}

/// A Steam Controller unplugged while its setup holds it is the pad gone,
/// as what reads it says, and the setup is held for it; plugged in again,
/// the same pad, it is a pad plugged in, and the setup takes it back.
#[tokio::test(flavor = "multi_thread")]
async fn a_steam_controller_unplugged_while_held_is_gone_and_taken_back() {
    const TEST: &str = "a_steam_controller_unplugged_while_held_is_gone_and_taken_back";
    let Some(fake) = fake(TEST) else {
        return;
    };
    let (id, serial) = (id_of(&fake), fake.serial().to_string());
    let (_dir, daemon, client) = with_couch(&fake, &unicast(&id)).await;
    client.start_setup("couch").await.unwrap();
    plays_again(&client, &fake).await;

    drop(fake);
    let setup = in_phase(&client, "couch", Phase::Held).await;
    assert!(!setup.sources[0].present);
    eventually("the pad was never said to have gone", async || {
        !listed(&client, &id).connected
    })
    .await;
    assert_eq!(listed(&client, &id).setup.as_deref(), Some("couch"));

    let again = FakeSteamController::builder()
        .same_pad_as(&serial)
        .build()
        .unwrap();
    plays_again(&client, &again).await;
    assert!(listed(&client, &id).connected);
    client.stop_setup("couch").await.unwrap();
    daemon.stop().await;
}

/// A Steam Controller in a Puck's slot that leaves the slot while its
/// setup holds it: the slot's HID device stays, so nothing udev says
/// tells the pad has gone, and what reads it, saying so, is what the book
/// acts on. The pad is gone and the setup held; back in the slot, it is a
/// pad plugged in, and the setup takes it back.
#[tokio::test(flavor = "multi_thread")]
async fn a_steam_controller_leaving_a_puck_while_held_is_gone_and_taken_back() {
    const TEST: &str = "a_steam_controller_leaving_a_puck_while_held_is_gone_and_taken_back";
    let builder = FakeSteamController::builder().transport(SteamTransport::Puck { joined: true });
    let Some(fake) = fake_made(TEST, builder) else {
        return;
    };
    let id = id_of(&fake);
    let (dir, daemon, client) = with_couch(&fake, &unicast(&id)).await;
    let published = Published::watch(&dir.path().join("zenoh.sock"), &id).await;
    client.start_setup("couch").await.unwrap();
    plays_again(&client, &fake).await;
    // Never said to have left as it was taken.
    let before = published.states().len();
    assert!(
        published.states().iter().all(|(connected, _)| *connected),
        "{:?}",
        published.states()
    );

    fake.send_wireless(false).unwrap();
    in_phase(&client, "couch", Phase::Held).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !published.states()[before..]
        .iter()
        .any(|(connected, _)| !connected)
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the pad was never said to have gone: {:?}, inputs {:?}",
            published.states(),
            fake.inputs()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    fake.send_wireless(true).unwrap();
    plays_again(&client, &fake).await;
    assert!(listed(&client, &id).connected);
    client.stop_setup("couch").await.unwrap();
    daemon.stop().await;
}

/// A profile that asks for its pads to be left to games, naming a Steam
/// Controller the daemon has never seen, starts held; the pad, plugged
/// in, is refused, and the setup says why, the pad left as it was.
#[tokio::test(flavor = "multi_thread")]
async fn a_steam_controller_left_to_games_is_refused_as_it_comes() {
    const TEST: &str = "a_steam_controller_left_to_games_is_refused_as_it_comes";
    let Some(first) = fake(TEST) else {
        return;
    };
    let serial = first.serial().to_string();
    let id = id_of(&first);
    drop(first);
    let (_dir, socket) = socket();
    let daemon = start_daemon(&socket).await;
    let client = hydrated(&socket).await;
    client
        .put_profile(
            "couch",
            &format!("mode = \"unicast\"\npad = \"{id}\"\nhide = \"none\"\n"),
        )
        .await
        .unwrap();
    client.start_setup("couch").await.unwrap();
    in_phase(&client, "couch", Phase::Held).await;

    let fake = FakeSteamController::builder()
        .same_pad_as(&serial)
        .build()
        .unwrap();
    eventually("the setup never said why", async || {
        client
            .setup("couch")
            .unwrap()
            .and_then(|setup| setup.reason)
            .is_some_and(|reason| reason.contains("hide"))
    })
    .await;
    let setup = client.setup("couch").unwrap().unwrap();
    assert_eq!(setup.phase, Phase::Held);
    assert!(!setup.sources[0].present);
    assert!(fake.gamepad().is_some(), "its gamepad is where it was");
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
    published.never_left(&client, 4).await;
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

/// A Steam Controller a setup uses can always be identified, buzzed
/// through its raw node, and is published as able to rumble; free, it
/// can be only where its kernel driver has force feedback, which a
/// kernel built without `CONFIG_STEAM_FF` doesn't give it, and the
/// refusal then says when it can.
#[tokio::test(flavor = "multi_thread")]
async fn a_steam_controller_is_identified_through_its_raw_node_while_a_setup_uses_it() {
    const TEST: &str =
        "a_steam_controller_is_identified_through_its_raw_node_while_a_setup_uses_it";
    let Some(fake) = fake(TEST) else {
        return;
    };
    let id = id_of(&fake);
    let gamepad = fake.gamepad().expect("the driver makes the gamepad");
    let driver_rumbles = Pad::from_fd(std::fs::File::open(&gamepad.event).unwrap().into())
        .unwrap()
        .can_rumble();
    let (_dir, daemon, client) = with_couch(&fake, &unicast(&id)).await;
    eventually("the daemon never learned whether it rumbles", async || {
        listed(&client, &id).rumble == Some(driver_rumbles)
    })
    .await;
    let free = client.identify_pad(&id, Duration::from_millis(1)).await;
    if driver_rumbles {
        free.unwrap();
    } else {
        let refused = free.unwrap_err();
        assert_eq!(refused.rpc_code(), Some(ErrorCode::PadWithoutRumble));
        assert!(
            refused
                .to_string()
                .contains("while a running profile is using it"),
            "{refused}"
        );
    }

    client.start_setup("couch").await.unwrap();
    in_phase(&client, "couch", Phase::Running).await;
    eventually(
        "the pad in a setup wasn't published as able to rumble",
        async || listed(&client, &id).rumble == Some(true),
    )
    .await;
    client
        .identify_pad(&id, Duration::from_millis(300))
        .await
        .unwrap();
    let buzzed = |received: &[Received]| {
        received.iter().any(|received| {
            matches!(received, Received::Output(report)
                if report.first() == Some(&0x80) && report.get(4..6) != Some(&[0, 0]))
        })
    };
    assert!(fake.wait_for(buzzed), "{:?}", fake.received());

    client.stop_setup("couch").await.unwrap();
    eventually("the pad let go wasn't published as it was", async || {
        listed(&client, &id).rumble == Some(driver_rumbles)
    })
    .await;
    daemon.stop().await;
}

/// A virtual DualSense's input report, read from its raw node as a game
/// reads it, `fake` sending `state` before each read, until `wanted` says
/// what is waited for, or twenty seconds at most; the last report.
fn dualsense_after(
    fake: &FakeSteamController,
    state: &SteamState,
    game: &mut std::fs::File,
    wanted: impl Fn(&[u8; 64]) -> bool,
) -> [u8; 64] {
    use std::io::Read as _;
    let started = std::time::Instant::now();
    let mut report = [0u8; 64];
    while started.elapsed() < Duration::from_secs(20) {
        fake.send_state(state, StateReport::Full).unwrap();
        // It reports every 10 ms whatever happens behind it.
        assert_eq!(game.read(&mut report).unwrap(), 64);
        if wanted(&report) {
            break;
        }
    }
    report
}

/// The gyroscope's pitch count of a DualSense's input report.
fn pitch(report: &[u8; 64]) -> i16 {
    i16::from_le_bytes([report[16], report[17]])
}

/// A profile of two Steam Controllers behind a virtual DualSense, with
/// the assist's first back button as its Gyro button: the game turns as
/// the primary does until the assist holds the paddle, and as the assist
/// does while they hold it, and never sees the paddle.
#[tokio::test(flavor = "multi_thread")]
async fn a_paddle_named_as_the_gyro_button_hands_the_motion_over() {
    const TEST: &str = "a_paddle_named_as_the_gyro_button_hands_the_motion_over";
    if !ctrlassist_test::can_make_hid_pads(TEST) {
        return;
    }
    let (Some(primary), Some(assist)) = (fake(TEST), fake(TEST)) else {
        return;
    };
    let (_dir, daemon, client) = with_couch(
        &primary,
        &format!(
            "mode = \"priority\"\nprimary = \"{}\"\nassist = \"{}\"\nidentity = \"dualsense\"\nassist-gyro = \"paddle-1\"\n",
            id_of(&primary),
            id_of(&assist)
        ),
    )
    .await;
    eventually("the daemon never listed the assist", async || {
        client
            .list_pads()
            .unwrap()
            .iter()
            .any(|pad| pad.id == id_of(&assist) && pad.connected)
    })
    .await;
    client.start_setup("couch").await.unwrap();
    let setup = in_phase(&client, "couch", Phase::Running).await;
    let event = std::path::PathBuf::from(setup.sinks[0].node.clone().unwrap());
    let raw = ctrlassist_test::raw_node_of(&event).expect("a virtual DualSense has a raw node");
    if !may_open_once_udev_is_done(&raw, false) {
        let _ = writeln!(
            std::io::stderr(),
            "SKIPPED {TEST}: a virtual DualSense's raw node isn't this user's to open here"
        );
        client.stop_setup("couch").await.unwrap();
        daemon.stop().await;
        return;
    }
    let mut game = std::fs::File::open(&raw).unwrap();

    // The primary turns one way, the assist the other, each a fresh
    // reading.
    let turning = |pitch: i16, clock: u32| SteamState {
        gyro: [pitch, 0, 0],
        accel: [0, 0, 16384],
        sensor_clock: clock,
        ..SteamState::default()
    };
    assist
        .send_state(&turning(-8192, 4000), StateReport::Full)
        .unwrap();
    let report = dualsense_after(&primary, &turning(8192, 4000), &mut game, |report| {
        pitch(report) > 0
    });
    assert!(pitch(&report) > 0, "the primary's motion: {report:?}");

    let held = turning(-8192, 8000).with(SteamButton::UpperRightBack);
    let report = dualsense_after(&assist, &held, &mut game, |report| pitch(report) < 0);
    assert!(pitch(&report) < 0, "the assist's motion: {report:?}");
    // No button down: the D-pad at none, and nothing else.
    assert_eq!(report[8..=10], [8, 0, 0], "the paddle shown: {report:?}");

    let report = dualsense_after(&assist, &turning(-8192, 12000), &mut game, |report| {
        pitch(report) > 0
    });
    assert!(pitch(&report) > 0, "the primary's again: {report:?}");

    client.stop_setup("couch").await.unwrap();
    daemon.stop().await;
}

/// A profile that asks for the Steam Controller identity makes a virtual
/// one, bound by the kernel's own driver, whose node is its raw node: a
/// Steam Controller behind it is read there whole, its back buttons and
/// both trackpads in the places the real one has them, and the virtual
/// pad is none of the daemon's pads.
#[tokio::test(flavor = "multi_thread")]
async fn a_steam_controller_behind_a_virtual_one_is_read_whole_at_its_raw_node() {
    use std::io::Read as _;

    const TEST: &str = "a_steam_controller_behind_a_virtual_one_is_read_whole_at_its_raw_node";
    let Some(fake) = fake(TEST) else {
        return;
    };
    let (_dir, daemon, client) = with_couch(
        &fake,
        &format!(
            "{}identity = \"steam-controller\"\n",
            unicast(&id_of(&fake))
        ),
    )
    .await;
    client.start_setup("couch").await.unwrap();
    let setup = in_phase(&client, "couch", Phase::Running).await;
    assert_eq!(setup.sinks[0].identity, "steam-controller");
    let raw = std::path::PathBuf::from(setup.sinks[0].node.clone().unwrap());
    assert!(raw.to_string_lossy().starts_with("/dev/hidraw"), "{raw:?}");
    // The daemon lists the pad behind it and not its own virtual one,
    // whose serial is `F` and twelve hex digits, as no fake's is.
    let pads = client.list_pads().unwrap();
    assert!(pads.iter().any(|pad| pad.id == id_of(&fake)), "{pads:?}");
    let own = |id: &str| {
        id.strip_prefix("address:F").is_some_and(|rest| {
            rest.len() == 12 && rest.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    };
    let listed: Vec<&str> = pads
        .iter()
        .map(|pad| pad.id.as_str())
        .filter(|id| own(id))
        .collect();
    assert!(listed.is_empty(), "{listed:?}");
    if !may_open_once_udev_is_done(&raw, false) {
        let _ = writeln!(
            std::io::stderr(),
            "SKIPPED {TEST}: a virtual Steam Controller's raw node isn't this user's to open here"
        );
        client.stop_setup("couch").await.unwrap();
        daemon.stop().await;
        return;
    }
    let mut game = std::fs::File::open(&raw).unwrap();

    let held = SteamState {
        left_pad: ctrlassist_test::Trackpad {
            touched: true,
            x: 16384,
            y: -16384,
            pressure: 0,
        },
        left_stick: [20000, 0],
        sensor_clock: 4000,
        ..SteamState::default()
    }
    .with(SteamButton::South)
    .with(SteamButton::UpperRightBack);
    // South, the upper right back button, and the left trackpad's touch.
    let wanted = 0x1 | 0x80 | 0x0200_0000;
    let started = std::time::Instant::now();
    let mut report = [0u8; 64];
    let mut buttons = 0;
    let mut clock = 4000;
    while buttons != wanted && started.elapsed() < Duration::from_secs(20) {
        // A fresh reading each time, so that each is one more report.
        clock += 4000;
        fake.send_state(
            &SteamState {
                sensor_clock: clock,
                ..held
            },
            StateReport::Full,
        )
        .unwrap();
        assert_eq!(game.read(&mut report).unwrap(), 54);
        assert_eq!(report[0], 0x42);
        buttons = u32::from_le_bytes([report[2], report[3], report[4], report[5]]);
    }
    assert_eq!(buttons, wanted, "{report:?}");
    let at = |place: usize| i16::from_le_bytes([report[place], report[place + 1]]);
    assert!(
        (at(10) - 20000).abs() <= 2,
        "the left stick's x: {}",
        at(10)
    );
    assert!(
        (at(18) - 16384).abs() <= 2,
        "the left trackpad's x: {}",
        at(18)
    );
    assert!(
        (at(20) + 16384).abs() <= 2,
        "the left trackpad's y: {}",
        at(20)
    );

    client.stop_setup("couch").await.unwrap();
    daemon.stop().await;
}

/// The haptic reports a reader writes a virtual Steam Controller, past
/// rumble, reach the Steam Controllers behind it that the profile's
/// Rumble choice names, each whole and as written, and no other: here
/// the assist's alone gets a pulse and a report of sound, and the
/// primary's neither.
#[tokio::test(flavor = "multi_thread")]
async fn a_virtual_steam_controller_s_haptics_go_where_its_rumble_goes() {
    use std::io::Write as _;

    const TEST: &str = "a_virtual_steam_controller_s_haptics_go_where_its_rumble_goes";
    let (Some(primary), Some(assist)) = (fake(TEST), fake(TEST)) else {
        return;
    };
    let (_dir, daemon, client) = with_couch(
        &primary,
        &format!(
            "mode = \"priority\"\nprimary = \"{}\"\nassist = \"{}\"\nidentity = \"steam-controller\"\nrumble = \"assist\"\n",
            id_of(&primary),
            id_of(&assist)
        ),
    )
    .await;
    eventually("the daemon never listed the assist", async || {
        client
            .list_pads()
            .unwrap()
            .iter()
            .any(|pad| pad.id == id_of(&assist) && pad.connected)
    })
    .await;
    client.start_setup("couch").await.unwrap();
    let setup = in_phase(&client, "couch", Phase::Running).await;
    let raw = std::path::PathBuf::from(setup.sinks[0].node.clone().unwrap());
    if !may_open_once_udev_is_done(&raw, true) {
        let _ = writeln!(
            std::io::stderr(),
            "SKIPPED {TEST}: a virtual Steam Controller's raw node isn't this user's to open here"
        );
        client.stop_setup("couch").await.unwrap();
        daemon.stop().await;
        return;
    }
    let mut game = std::fs::OpenOptions::new().write(true).open(&raw).unwrap();
    // A pulse on the left trackpad, and a report of sound.
    let pulse = vec![0x81, 1, 0x10, 0x27, 0x10, 0x27, 3, 0];
    let mut sound = vec![0x87];
    sound.extend((1..64u8).map(|byte| byte.wrapping_mul(3)));
    game.write_all(&pulse).unwrap();
    game.write_all(&sound).unwrap();
    // And a level, which the assist's gets too, after them.
    game.write_all(&[0x80, 0, 0, 0, 0x00, 0x80, 0, 0, 0, 0])
        .unwrap();

    let wanted = [Received::Output(pulse), Received::Output(sound)];
    let got = |received: &[Received]| wanted.iter().all(|wanted| received.contains(wanted));
    assert!(assist.wait_for(got), "{:?}", assist.received());
    let received = assist.received();
    let at = |wanted: &Received| received.iter().position(|sent| sent == wanted).unwrap();
    assert!(at(&wanted[0]) < at(&wanted[1]), "in the order written");
    // The assist has rumbled by now, so the primary has had its chance.
    assert!(
        assist.wait_for(|received| received.iter().any(
            |sent| matches!(sent, Received::Output(report) if report.first() == Some(&0x80)
                && report.get(5) == Some(&0x80))
        )),
        "{:?}",
        assist.received()
    );
    let stray: Vec<Received> = primary
        .received()
        .into_iter()
        .filter(|sent| wanted.contains(sent))
        .collect();
    assert!(stray.is_empty(), "{stray:?}");

    client.stop_setup("couch").await.unwrap();
    daemon.stop().await;
}
