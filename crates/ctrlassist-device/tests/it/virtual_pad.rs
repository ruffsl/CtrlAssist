//! Virtual pads, read back through the pad reader and driven by a
//! stand-in for a game.

use std::fs::File;
use std::os::fd::OwnedFd;
use std::path::Path;

use ctrlassist_core::{Effect, FeedbackError, Level, PadState, gamepad};
use ctrlassist_device::{Identity, Pad, Request, VirtualPad};
use evdev::{Device, FFEffectData, FFEffectKind, FFReplay, FFTrigger};
use rustix::event::{PollFd, PollFlags, poll};

use crate::{can_make_pads, read_until};

/// A virtual pad and a reader on its event node, once udev allows it.
fn pair(identity: Identity) -> (VirtualPad, Pad) {
    let sink = VirtualPad::create(identity).unwrap();
    let node = sink.event_node().to_path_buf();
    let _ = std::process::Command::new("udevadm").arg("settle").status();
    let fd: OwnedFd = File::open(node).unwrap().into();
    (sink, Pad::from_fd(fd).unwrap())
}

pub(crate) fn rumble(strong: u16, weak: u16, length: u16, delay: u16) -> FFEffectData {
    FFEffectData {
        direction: 0,
        trigger: FFTrigger::default(),
        replay: FFReplay { length, delay },
        kind: FFEffectKind::Rumble {
            strong_magnitude: strong,
            weak_magnitude: weak,
        },
    }
}

/// Serves the virtual pad until `game` has finished, answering each
/// request with `answer`, and returns the requests in order.
fn serve_while<T: Send + 'static>(
    sink: &mut VirtualPad,
    game: impl FnOnce() -> T + Send + 'static,
    mut answer: impl FnMut(Request) -> Result<(), FeedbackError>,
) -> (Vec<Request>, T) {
    let game = std::thread::spawn(game);
    let mut requests = Vec::new();
    while !game.is_finished() {
        let mut fds = [PollFd::new(&*sink, PollFlags::IN)];
        // The game's exit isn't a readable event, so look up now and then.
        let wait = rustix::event::Timespec {
            tv_sec: 0,
            tv_nsec: 20_000_000,
        };
        let _ = poll(&mut fds, Some(&wait));
        sink.serve(|request| {
            requests.push(request);
            answer(request)
        })
        .unwrap();
    }
    let outcome = game.join().unwrap();
    sink.serve(|request| {
        requests.push(request);
        answer(request)
    })
    .unwrap();
    (requests, outcome)
}

pub(crate) fn game_opens(node: &Path) -> Device {
    // A game writes to the node to play effects.
    Device::open(node).unwrap()
}

#[test]
fn what_is_written_reads_back_the_same_on_every_gamepad_control() {
    if !can_make_pads("what_is_written_reads_back_the_same_on_every_gamepad_control") {
        return;
    }
    for identity in [Identity::Generic, Identity::Xbox] {
        let (mut sink, mut reader) = pair(identity);
        // In two writes: the kernel ends a report frame itself once it
        // holds about as many changes as the pad has axes, so one frame
        // can't carry every control at once.
        let buttons = PadState::NEUTRAL
            .with(gamepad::SOUTH, 1.0)
            .with(gamepad::EAST, 1.0)
            .with(gamepad::NORTH, 1.0)
            .with(gamepad::WEST, 1.0)
            .with(gamepad::LEFT_BUMPER, 1.0)
            .with(gamepad::RIGHT_BUMPER, 1.0)
            .with(gamepad::SELECT, 1.0)
            .with(gamepad::START, 1.0)
            .with(gamepad::MODE, 1.0)
            .with(gamepad::LEFT_THUMB, 1.0)
            .with(gamepad::RIGHT_THUMB, 1.0);
        sink.write(&buttons).unwrap();
        let frames = read_until(&mut reader, |frames| !frames.is_empty());
        assert_eq!(frames, [buttons], "{identity:?}: one write is one frame");

        let everything = buttons
            .with(gamepad::DPAD_UP, 1.0)
            .with(gamepad::DPAD_RIGHT, 1.0)
            .with(gamepad::LEFT_X, 1.0)
            .with(gamepad::LEFT_Y, -1.0)
            .with(gamepad::RIGHT_X, -1.0)
            .with(gamepad::RIGHT_Y, 1.0)
            .with(gamepad::LEFT_TRIGGER, 1.0)
            .with(gamepad::RIGHT_TRIGGER, 1.0);
        sink.write(&everything).unwrap();
        let frames = read_until(&mut reader, |frames| !frames.is_empty());
        assert_eq!(frames, [everything], "{identity:?}");

        // Back to rest, in the same two steps: every control returns to
        // exactly zero.
        sink.write(&buttons).unwrap();
        sink.write(&PadState::NEUTRAL).unwrap();
        let frames = read_until(&mut reader, |frames| frames.len() >= 2);
        assert_eq!(frames, [buttons, PadState::NEUTRAL], "{identity:?}");

        // Writing what it already holds sends nothing, so the next real
        // change is the next frame.
        sink.write(&PadState::NEUTRAL).unwrap();
        sink.write(&PadState::NEUTRAL.with(gamepad::SOUTH, 1.0))
            .unwrap();
        let frames = read_until(&mut reader, |frames| !frames.is_empty());
        assert_eq!(
            frames,
            [PadState::NEUTRAL.with(gamepad::SOUTH, 1.0)],
            "{identity:?}"
        );
    }
}

#[test]
fn a_virtual_pad_is_marked_as_our_own_output_and_other_pads_are_not() {
    if !can_make_pads("a_virtual_pad_is_marked_as_our_own_output_and_other_pads_are_not") {
        return;
    }
    let (_first, reader) = pair(Identity::Generic);
    assert!(reader.is_own_output());
    let (_second, other) = pair(Identity::Xbox);
    assert!(other.is_own_output());
    // Each carries the process and a count of its own.
    let port = |pad: &Pad| pad.identifiers().port.clone().unwrap();
    assert!(port(&reader).starts_with(&format!("ctrlassist/virtual/{}/", std::process::id())));
    assert_ne!(port(&reader), port(&other));

    let fake = ctrlassist_test::FakePad::new("Not Ours", 0x1209, 0x5eed).unwrap();
    let fd: OwnedFd = File::open(fake.event_node()).unwrap().into();
    assert!(!Pad::from_fd(fd).unwrap().is_own_output());
}

#[test]
fn each_identity_reports_its_own_name_and_numbers() {
    if !can_make_pads("each_identity_reports_its_own_name_and_numbers") {
        return;
    }
    let (sink, reader) = pair(Identity::Xbox);
    assert_eq!(sink.identity(), Identity::Xbox);
    let ids = reader.identifiers();
    assert_eq!(ids.name, Identity::Xbox.name());
    assert_eq!((ids.vendor, ids.product), Identity::Xbox.model());
    assert_eq!((ids.bus, ids.vendor, ids.product), (3, 0x045e, 0x0b12));

    let (_sink, reader) = pair(Identity::Generic);
    let ids = reader.identifiers();
    assert_eq!(ids.name, "CtrlAssist Virtual Pad");
    assert_eq!((ids.vendor, ids.product), (0x1234, 0x5678));
}

#[test]
fn a_game_s_rumble_requests_arrive_in_order_with_what_it_asked() {
    if !can_make_pads("a_game_s_rumble_requests_arrive_in_order_with_what_it_asked") {
        return;
    }
    let mut sink = VirtualPad::create(Identity::Generic).unwrap();
    let node = sink.event_node().to_path_buf();
    let _ = std::process::Command::new("udevadm").arg("settle").status();

    let game = move || {
        let mut pad = game_opens(&node);
        let mut effect = pad
            .upload_ff_effect(rumble(0x8000, 0x4000, 250, 10))
            .unwrap();
        let id = effect.id();
        pad.set_ff_gain(0x7fff).unwrap();
        effect.play(3).unwrap();
        effect.stop().unwrap();
        // Dropping the effect erases it.
        drop(effect);
        id
    };
    let (requests, id) = serve_while(&mut sink, game, |_| Ok(()));
    let id = i16::try_from(id).unwrap();
    let effect = Effect {
        strong: 0x8000,
        weak: 0x4000,
        length_ms: 250,
        delay_ms: 10,
    };
    assert_eq!(
        requests,
        [
            Request::Upload { id, effect },
            Request::Gain(0x7fff),
            Request::Play { id, count: 3 },
            Request::Play { id, count: 0 },
            // The kernel stops an effect itself before erasing it.
            Request::Play { id, count: 0 },
            Request::Erase { id },
        ]
    );
}

#[test]
fn an_upload_that_is_refused_fails_in_the_game_and_does_not_hang_it() {
    if !can_make_pads("an_upload_that_is_refused_fails_in_the_game_and_does_not_hang_it") {
        return;
    }
    let mut sink = VirtualPad::create(Identity::Generic).unwrap();
    let node = sink.event_node().to_path_buf();
    let _ = std::process::Command::new("udevadm").arg("settle").status();

    let game = move || {
        let mut pad = game_opens(&node);
        pad.upload_ff_effect(rumble(1, 1, 0, 0))
            .map(|_| ())
            .map_err(|error| error.raw_os_error())
    };
    let (requests, outcome) = serve_while(&mut sink, game, |_| Err(FeedbackError::Full));
    assert_eq!(requests.len(), 1);
    // Out of space, as a real pad with no free effect answers.
    assert_eq!(outcome, Err(Some(28)));
}

/// A pad opened for writing as well, as the hiding service opens them.
pub(crate) fn writable(node: &Path) -> Pad {
    let fd: OwnedFd = File::options()
        .read(true)
        .write(true)
        .open(node)
        .unwrap()
        .into();
    Pad::from_fd(fd).unwrap()
}

#[test]
fn a_rumble_level_is_one_endless_effect_updated_in_place_and_stopped_at_zero() {
    if !can_make_pads("a_rumble_level_is_one_endless_effect_updated_in_place_and_stopped_at_zero") {
        return;
    }
    // A virtual pad stands in for a real pad with motors: it reports what
    // it is asked, as a real pad's driver would be.
    let mut motors = VirtualPad::create(Identity::Generic).unwrap();
    let node = motors.event_node().to_path_buf();
    let _ = std::process::Command::new("udevadm").arg("settle").status();

    let player = move || {
        let mut pad = writable(&node);
        assert!(pad.can_rumble());
        let first = Level {
            strong: 0x8000,
            weak: 0,
        };
        let second = Level {
            strong: 0x2000,
            weak: 0xffff,
        };
        pad.rumble(first).unwrap();
        // The same level again asks the pad for nothing.
        pad.rumble(first).unwrap();
        pad.rumble(second).unwrap();
        pad.rumble(Level::default()).unwrap();
    };
    let (requests, ()) = serve_while(&mut motors, player, |_| Ok(()));
    let endless = |strong, weak| Effect {
        strong,
        weak,
        length_ms: 0,
        delay_ms: 0,
    };
    assert_eq!(
        requests,
        [
            Request::Upload {
                id: 0,
                effect: endless(0x8000, 0)
            },
            Request::Play { id: 0, count: 1 },
            // The second level takes the first's place: one slot, and no
            // gap between them.
            Request::Upload {
                id: 0,
                effect: endless(0x2000, 0xffff)
            },
            Request::Play { id: 0, count: 1 },
            // Zero stops it, and it stays until the pad is dropped.
            Request::Play { id: 0, count: 0 },
            Request::Play { id: 0, count: 0 },
            Request::Erase { id: 0 },
        ]
    );
}

#[test]
fn a_pad_without_motors_ignores_rumble() {
    if !can_make_pads("a_pad_without_motors_ignores_rumble") {
        return;
    }
    let fake = ctrlassist_test::FakePad::new("No Motors", 0x1209, 0x5eed).unwrap();
    let mut pad = writable(fake.event_node());
    assert!(!pad.can_rumble());
    pad.rumble(Level { strong: 1, weak: 1 }).unwrap();
}

/// A virtual DualSense is bound by the kernel's PlayStation driver as a
/// real one is: what is written reads back through the driver's own pad,
/// and a game's rumble comes back as levels, whichever way the driver
/// marks it for the pad's firmware.
#[test]
fn a_virtual_dualsense_is_a_dualsense_to_the_kernel_s_driver() {
    const TEST: &str = "a_virtual_dualsense_is_a_dualsense_to_the_kernel_s_driver";
    if !ctrlassist_test::can_make_hid_pads(TEST) {
        return;
    }
    // Made from a copied pad's data and nothing else.
    assert!(VirtualPad::create(Identity::DualSense).is_err());
    for (newer, last) in [(false, 0x01u32), (true, 0x02)] {
        let data = ctrlassist_test::synthetic_dualsense(newer);
        let address = ctrlassist_device::own_address(0x7e_5700 + last);
        let mut sink = VirtualPad::dualsense(&data, address).unwrap();
        assert_eq!(sink.identity(), Identity::DualSense);
        let node = sink.event_node().to_path_buf();
        let _ = std::process::Command::new("udevadm").arg("settle").status();
        let fd: OwnedFd = File::open(&node).unwrap().into();
        let mut reader = Pad::from_fd(fd).unwrap();
        let identifiers = reader.identifiers().clone();
        assert_eq!(
            (identifiers.vendor, identifiers.product),
            Identity::DualSense.model()
        );
        assert_eq!(
            identifiers.address.as_deref(),
            Some(format!("02:ca:55:7e:57:{last:02x}").as_str()),
            "the driver names the pad by the address it was told"
        );
        assert!(reader.is_own_output(), "{identifiers:?}");

        let held = PadState::NEUTRAL
            .with(gamepad::SOUTH, 1.0)
            .with(gamepad::NORTH, 1.0)
            .with(gamepad::LEFT_BUMPER, 1.0)
            .with(gamepad::START, 1.0)
            .with(gamepad::MODE, 1.0)
            .with(gamepad::RIGHT_THUMB, 1.0)
            .with(gamepad::DPAD_UP, 1.0)
            .with(gamepad::DPAD_RIGHT, 1.0)
            .with(gamepad::LEFT_X, 1.0)
            .with(gamepad::RIGHT_Y, -1.0)
            .with(gamepad::RIGHT_TRIGGER, 1.0);
        sink.write(&held).unwrap();
        let arrived = |frames: &[PadState]| {
            frames.last().is_some_and(|state| {
                state.get(gamepad::SOUTH) > 0.0 && state.get(gamepad::RIGHT_TRIGGER) > 0.99
            })
        };
        let frames = read_until(&mut reader, arrived);
        let read = *frames.last().unwrap();
        for control in [
            gamepad::SOUTH,
            gamepad::NORTH,
            gamepad::LEFT_BUMPER,
            gamepad::START,
            gamepad::MODE,
            gamepad::RIGHT_THUMB,
            gamepad::DPAD_UP,
            gamepad::DPAD_RIGHT,
            gamepad::LEFT_X,
            gamepad::RIGHT_TRIGGER,
        ] {
            assert!(read.get(control) > 0.99, "{control:?} in {read:?}");
        }
        assert!(read.get(gamepad::RIGHT_Y) < -0.99, "{read:?}");
        for control in [
            gamepad::EAST,
            gamepad::WEST,
            gamepad::DPAD_DOWN,
            gamepad::SELECT,
        ] {
            assert!(read.get(control) <= 0.0, "{control:?} in {read:?}");
        }
        assert!(
            read.get(gamepad::LEFT_Y).abs() < 0.01,
            "a stick at rest: {read:?}"
        );
        // The same state again is nothing new to send.
        sink.write(&held).unwrap();
        // A second pad with its address is refused by the driver, which
        // is an error and not a wait without end.
        let twice = VirtualPad::dualsense(&data, address).unwrap_err();
        assert_eq!(twice.kind(), std::io::ErrorKind::AlreadyExists, "{twice}");

        // A game rumbles the pad through the driver, and stops.
        let game = move || {
            let mut pad = game_opens(&node);
            let mut effect = pad.upload_ff_effect(rumble(0x8000, 0x4000, 0, 0)).unwrap();
            effect.play(1).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(200));
            effect.stop().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(200));
        };
        let (mut requests, ()) = serve_while(&mut sink, game, |_| Ok(()));
        // Whatever else has the pad open, as a running Steam does, may
        // ask for its feature reports meanwhile.
        requests.retain(|request| !matches!(request, Request::Feature { .. }));
        let on = Level {
            strong: 0x8080,
            weak: 0x4040,
        };
        assert_eq!(
            requests,
            [Request::Level(on), Request::Level(Level::default())],
            "newer firmware: {newer}"
        );
    }
}

/// A DualSense given its raw node is sent whole output reports: what is
/// passed to it arrives with the rumble it is at, and its rumble alone
/// arrives marked the way its firmware takes it.
#[test]
fn a_dualsense_with_its_raw_node_is_sent_whole_output_reports() {
    const TEST: &str = "a_dualsense_with_its_raw_node_is_sent_whole_output_reports";
    if !ctrlassist_test::can_make_hid_pads(TEST) {
        return;
    }
    let real = ctrlassist_test::FakeDualSense::new(0x31).unwrap();
    let _ = std::process::Command::new("udevadm").arg("settle").status();
    let raw = ctrlassist_test::raw_node_of(real.event_node()).unwrap();
    if !ctrlassist_test::can_open(&raw, true) {
        use std::io::Write as _;
        let _ = writeln!(
            std::io::stderr(),
            "SKIPPED {TEST}: a DualSense's raw node isn't this user's to open here"
        );
        return;
    }
    let event: OwnedFd = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(real.event_node())
        .unwrap()
        .into();
    let mut pad = Pad::from_fd(event).unwrap();
    assert!(!pad.takes_output());
    // Something that is no raw node leaves the pad as it was.
    let not_raw: OwnedFd = File::open("/dev/null").unwrap().into();
    assert!(pad.set_raw(not_raw).is_err());
    assert!(!pad.takes_output());
    let raw: OwnedFd = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(raw)
        .unwrap()
        .into();
    pad.set_raw(raw).unwrap();
    assert!(pad.takes_output());

    let arrived = |wanted: &dyn Fn(&[u8]) -> bool| {
        for _ in 0..400 {
            if real.outputs().iter().any(|sent| wanted(sent)) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        false
    };
    let level = Level {
        strong: 0x8000,
        weak: 0x4000,
    };
    pad.rumble(level).unwrap();
    // The stand-in's firmware takes the newer rumble.
    assert!(arrived(&|sent| sent[1] == 0x02
        && sent[39] == 0x04
        && sent[3..5] == [0x40, 0x80]));
    let mut lights = [0u8; 48];
    lights[0] = 0x02;
    (lights[2], lights[45]) = (0x04, 0xee);
    pad.pass(&lights).unwrap();
    assert!(
        arrived(&|sent| sent[45] == 0xee && sent[39] == 0x04 && sent[3..5] == [0x40, 0x80]),
        "the lights arrive with the rumble kept: {:?}",
        real.outputs()
    );
    pad.rumble(Level::default()).unwrap();
    assert!(arrived(&|sent| sent[1] == 0
        && sent[39] == 0
        && sent[45] == 0));
}

/// A DualSense on Bluetooth is sent whole output reports too, as the
/// report it takes there: another number, a sequence number, a tag, the
/// same settings, and a checksum. The kernel's driver writes the pad
/// such reports of its own as it binds, which say what one looks like.
#[test]
fn a_dualsense_on_bluetooth_is_sent_whole_output_reports() {
    const TEST: &str = "a_dualsense_on_bluetooth_is_sent_whole_output_reports";
    if !ctrlassist_test::can_make_hid_pads(TEST) {
        return;
    }
    let real = ctrlassist_test::FakeBluetoothDualSense::new(0x41).unwrap();
    let node = real
        .event_node()
        .expect("the kernel's driver takes the pad as a DualSense on Bluetooth");
    let _ = std::process::Command::new("udevadm").arg("settle").status();
    let raw = ctrlassist_test::raw_node_of(&node).unwrap();
    if !ctrlassist_test::can_open(&raw, true) {
        use std::io::Write as _;
        let _ = writeln!(
            std::io::stderr(),
            "SKIPPED {TEST}: a DualSense's raw node isn't this user's to open here"
        );
        return;
    }
    let whole = |sent: &[u8]| {
        sent.len() == 78
            && sent[0] == 0x31
            && sent[2] == 0x10
            && sent[74..] == ctrlassist_test::playstation_crc32(0xa2, &sent[..74]).to_le_bytes()
    };
    // The driver's own, from binding the pad: the shape to match.
    let theirs = real.outputs();
    assert!(
        !theirs.is_empty(),
        "the driver set the pad's lights as it bound it"
    );
    assert!(theirs.iter().all(|sent| whole(sent)), "{theirs:?}");

    let event: OwnedFd = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&node)
        .unwrap()
        .into();
    let mut pad = Pad::from_fd(event).unwrap();
    assert!(pad.could_take_output() && !pad.takes_output());
    let raw: OwnedFd = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(raw)
        .unwrap()
        .into();
    pad.set_raw(raw).unwrap();
    assert!(pad.takes_output());

    let ours = || real.outputs().split_off(theirs.len());
    let arrived = |wanted: &dyn Fn(&[u8]) -> bool| {
        for _ in 0..400 {
            if ours().iter().any(|sent| wanted(sent)) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        false
    };
    // The settings sit two bytes further on than over USB.
    pad.rumble(Level {
        strong: 0x8000,
        weak: 0x4000,
    })
    .unwrap();
    // The stand-in's firmware takes the newer rumble.
    assert!(
        arrived(&|sent| whole(sent)
            && sent[3] == 0x02
            && sent[41] == 0x04
            && sent[5..7] == [0x40, 0x80]),
        "{:?}",
        ours()
    );
    let mut lights = [0u8; 48];
    lights[0] = 0x02;
    (lights[2], lights[45], lights[46], lights[47]) = (0x04, 0xee, 0xdd, 0xcc);
    pad.pass(&lights).unwrap();
    assert!(
        arrived(&|sent| whole(sent)
            && sent[47..50] == [0xee, 0xdd, 0xcc]
            && sent[41] == 0x04
            && sent[5..7] == [0x40, 0x80]),
        "the lights arrive with the rumble kept: {:?}",
        ours()
    );
    pad.rumble(Level::default()).unwrap();
    assert!(arrived(&|sent| whole(sent)
        && sent[3] == 0
        && sent[41] == 0));
    // Each report is numbered one on from the last, round from 15.
    let numbers: Vec<u8> = ours().iter().map(|sent| sent[1] >> 4).collect();
    assert_eq!(numbers, [0, 1, 2], "{:?}", ours());
}
