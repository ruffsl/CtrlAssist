//! The fake Steam Controller against the kernel's own `hid-steam`: what
//! the driver makes of it, and its raw node.
//!
//! Only Linux 7.3 and later know the Steam Controller, so each test skips
//! on an older kernel, CI's runner among them: these run only on a
//! developer's machine. The `device` virtual machine check drives the same
//! fake through the fixture on 7.3, and checks there what the driver makes
//! of it under both values of `lizard_mode`. None of these changes the
//! driver's `lizard_mode`, which is machine-wide and its owner's.

use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::fd::AsFd as _;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::Path;

use ctrlassist_test::{
    ChargeState, FakeSteamController, GAMEPAD, MOTION, Received, StateReport, SteamBattery,
    SteamButton, SteamState, SteamTransport, WIRELESS_GAMEPAD, can_make_steam_pads, can_open,
};
use evdev::{AbsoluteAxisCode, KeyCode};
use rustix::event::{PollFd, PollFlags, Timespec, poll};

// The back buttons, from linux/input-event-codes.h, which the input
// library has no names for.
const BTN_GRIPL: KeyCode = KeyCode(0x224);
const BTN_GRIPR: KeyCode = KeyCode(0x225);
const BTN_GRIPL2: KeyCode = KeyCode(0x226);

fn settle() {
    let _ = std::process::Command::new("udevadm").arg("settle").status();
}

fn skip(test: &str, why: &str) {
    let _ = writeln!(std::io::stderr(), "SKIPPED {test}: {why}");
}

/// Whether the commands the fake was set, in order, include `wanted` in
/// that order, each by its first bytes.
fn set_in_order(received: &[Received], wanted: &[&[u8]]) -> bool {
    let mut wanted = wanted.iter().peekable();
    for command in received.iter().filter_map(Received::command) {
        if wanted.peek().is_some_and(|next| command.starts_with(next)) {
            wanted.next();
        }
    }
    wanted.peek().is_none()
}

/// The driver's `lizard_mode`, as it is: whether the gamepad is silent
/// until the menu button is held.
fn lizard_mode() -> bool {
    std::fs::read_to_string("/sys/module/hid_steam/parameters/lizard_mode")
        .is_ok_and(|value| value.trim() == "Y")
}

/// The driver binds the fake as it binds a pad: it asks the
/// serial and the attributes, which the fake answers so nothing waits,
/// and makes a gamepad and a motion device that carry the fake's serial
/// and its `phys`. The daemon's own reading of what is connected takes
/// the gamepad for a test pad, named by the serial. Dropped, the fake
/// takes them with it.
#[test]
fn a_fake_steam_controller_is_bound_by_the_kernel_s_driver() {
    const TEST: &str = "a_fake_steam_controller_is_bound_by_the_kernel_s_driver";
    if !can_make_steam_pads(TEST) {
        return;
    }
    let fake = FakeSteamController::new().unwrap();
    let gamepad = fake.gamepad().expect("the driver makes the gamepad");
    let motion = fake.motion().expect("the driver makes the motion sensors");
    assert_eq!(gamepad.name, GAMEPAD);
    assert_eq!(motion.name, MOTION);
    assert_eq!(gamepad.uniq, fake.serial());
    assert_eq!(motion.uniq, fake.serial());
    assert!(fake.serial().starts_with("CATEST"), "{}", fake.serial());

    let received = fake.received();
    assert!(
        set_in_order(&received, &[&[0xae, 0x16, 0x01], &[0x83]]),
        "the serial, then the attributes: {received:?}"
    );
    assert!(
        received
            .iter()
            .filter(|what| **what == Received::GetFeature(1))
            .count()
            >= 2,
        "each asked as a GET of its answer: {received:?}"
    );

    settle();
    let found = ctrlassist_device::connected()
        .unwrap()
        .into_iter()
        .find(|found| found.node == gamepad.event)
        .expect("the gamepad is a pad the daemon would find");
    assert!(found.identifiers.is_test_pad(), "{:?}", found.identifiers);
    assert_eq!(found.identifiers.port.as_deref(), Some(fake.phys()));
    assert_eq!(found.identifiers.address.as_deref(), Some(fake.serial()));
    assert_eq!(
        (
            found.identifiers.bus,
            found.identifiers.vendor,
            found.identifiers.product
        ),
        (3, 0x28de, 0x1302)
    );

    // Closing uhid removes the device before the close returns.
    let phys = fake.phys().to_string();
    drop(fake);
    assert!(
        !ctrlassist_test::phys_connected(&phys),
        "the fake's gamepad went with it"
    );
}

/// The driver makes the fake a battery, named by its serial, which
/// follows the fake's battery reports, in lizard mode too. A desktop's
/// battery applet may show it while the test runs.
#[test]
fn the_fake_s_battery_follows_its_battery_reports() {
    const TEST: &str = "the_fake_s_battery_follows_its_battery_reports";
    if !can_make_steam_pads(TEST) {
        return;
    }
    let fake = FakeSteamController::new().unwrap();
    let battery = fake.battery().expect("the driver makes the battery");
    let read = |name: &str| {
        std::fs::read_to_string(battery.join(name))
            .unwrap()
            .trim()
            .to_string()
    };
    for (charge, level, status) in [
        (ChargeState::Charging, 42, "Charging"),
        (ChargeState::Discharging, 77, "Discharging"),
        (ChargeState::Charged, 100, "Full"),
    ] {
        fake.send_battery(&SteamBattery {
            charge,
            level,
            battery_millivolts: 3900,
            system_millivolts: 3300,
            input_millivolts: 5000,
            battery_milliamps: 300,
            input_milliamps: 500,
            temperature: 28,
        })
        .unwrap();
        assert_eq!(read("capacity"), level.to_string());
        assert_eq!(read("status"), status);
    }
}

/// What the driver makes of the fake's state report, control by control,
/// which is the fake's reading of the layout held against the kernel's.
///
/// With the driver's `lizard_mode` on, its default, the gamepad says
/// nothing until the menu button is held: that is seen here, a report
/// changing no key, and then holding the menu button switching the
/// driver to gamepad mode for this fake alone, which it says to the fake
/// with lizard mode's settings off. With `lizard_mode` off, the driver
/// sets those as it binds, and events flow at once. A test can't see the
/// other setting's behavior without writing the parameter, which is the
/// machine's and its owner's, so one run sees one of the two.
#[test]
fn the_driver_reads_the_fake_s_state_as_its_layout_says() {
    const TEST: &str = "the_driver_reads_the_fake_s_state_as_its_layout_says";
    if !can_make_steam_pads(TEST) {
        return;
    }
    let fake = FakeSteamController::new().unwrap();
    let gamepad = fake.gamepad().expect("the driver makes the gamepad");
    let motion = fake.motion().expect("the driver makes the motion sensors");
    settle();
    if !can_open(&gamepad.event, false) || !can_open(&motion.event, false) {
        skip(
            TEST,
            "the fake's event nodes aren't this user's to open here",
        );
        return;
    }
    let gamepad_device = evdev::Device::open(&gamepad.event).unwrap();
    let motion_device = evdev::Device::open(&motion.event).unwrap();
    let held = |key: KeyCode| gamepad_device.get_key_state().unwrap().contains(key);

    // Lizard mode off, without the watchdog Steam keeps, as the driver
    // writes it.
    let gamepad_mode: &[u8] = &[0x87, 0x06, 0x09, 0x00, 0x00, 0x47, 0x00, 0x00];
    let state = SteamState::busy();
    if lizard_mode() {
        assert!(
            set_in_order(&fake.received(), &[&[0x85], &[0x8e]]),
            "the driver set lizard mode on as it bound the fake: {:?}",
            fake.received()
        );
        fake.send_state(&state, StateReport::Full).unwrap();
        assert!(
            !held(KeyCode::BTN_SOUTH),
            "in lizard mode the gamepad is silent"
        );
        let before = fake.received().len();
        fake.send_state(
            &SteamState::default().with(SteamButton::Menu),
            StateReport::Full,
        )
        .unwrap();
        assert!(
            fake.wait_for(|received| set_in_order(&received[before..], &[&[0x81], gamepad_mode])),
            "holding the menu button switched the driver to gamepad mode: {:?}",
            fake.received()
        );
        fake.send_state(&SteamState::default(), StateReport::Full)
            .unwrap();
    } else {
        assert!(
            set_in_order(&fake.received(), &[&[0x81], gamepad_mode]),
            "the driver set lizard mode off as it bound the fake: {:?}",
            fake.received()
        );
    }

    fake.send_state(&state, StateReport::Full).unwrap();
    assert!(held(KeyCode::BTN_SOUTH));
    assert!(held(BTN_GRIPR) && held(BTN_GRIPL2));
    assert!(!held(KeyCode::BTN_EAST) && !held(BTN_GRIPL));
    let axes = gamepad_device.get_abs_state().unwrap();
    let axis = |code: AbsoluteAxisCode| axes[usize::from(code.0)].value;
    // The sticks' Y is the report's, inverted; a trackpad is where the
    // report says only while it is touched; the triggers are as they are.
    assert_eq!(axis(AbsoluteAxisCode::ABS_X), -20000);
    assert_eq!(axis(AbsoluteAxisCode::ABS_Y), -12000);
    assert_eq!(axis(AbsoluteAxisCode::ABS_RX), 3000);
    assert_eq!(axis(AbsoluteAxisCode::ABS_RY), 3000);
    assert_eq!(axis(AbsoluteAxisCode::ABS_HAT0X), 1234);
    assert_eq!(axis(AbsoluteAxisCode::ABS_HAT0Y), -2345);
    assert_eq!(axis(AbsoluteAxisCode::ABS_HAT1X), 0);
    assert_eq!(axis(AbsoluteAxisCode::ABS_HAT1Y), 0);
    assert_eq!(axis(AbsoluteAxisCode::ABS_HAT2Y), 1000);
    assert_eq!(axis(AbsoluteAxisCode::ABS_HAT2X), 2000);

    // The accelerometer's second axis and the gyroscope's second are the
    // driver's Z, inverted; their third its Y.
    let axes = motion_device.get_abs_state().unwrap();
    let axis = |code: AbsoluteAxisCode| axes[usize::from(code.0)].value;
    assert_eq!(axis(AbsoluteAxisCode::ABS_X), 1000);
    assert_eq!(axis(AbsoluteAxisCode::ABS_Z), 2000);
    assert_eq!(axis(AbsoluteAxisCode::ABS_Y), 16384);
    assert_eq!(axis(AbsoluteAxisCode::ABS_RX), 16);
    assert_eq!(axis(AbsoluteAxisCode::ABS_RZ), 32);
    assert_eq!(axis(AbsoluteAxisCode::ABS_RY), 48);
}

/// Reads what the raw node has, until `count` reports have come or a
/// read has waited a second for one.
fn read_reports(raw: &mut File, count: usize) -> Vec<Vec<u8>> {
    let mut reports = Vec::new();
    let mut buffer = [0u8; 256];
    while reports.len() < count {
        let mut fds = [PollFd::new(&*raw, PollFlags::IN)];
        let wait = Timespec {
            tv_sec: 1,
            tv_nsec: 0,
        };
        if poll(&mut fds, Some(&wait)).unwrap() == 0 {
            break;
        }
        while let Ok(read) = raw.read(&mut buffer) {
            reports.push(buffer[..read].to_vec());
        }
    }
    reports
}

/// Opening the raw node, as Steam and SDL do, takes the driver's input
/// devices away; the three state reports and the battery report the fake
/// sends, the newer state report declared for it, are read from it byte
/// for byte; and an output report and a feature report written to it
/// reach the fake as they were written. Closed, the driver makes its
/// devices anew and asks the serial again.
///
/// The node is root's on a machine with no rule like Steam's to give it
/// to the seat, so this skips there; the `device` virtual machine check
/// opens it as root through the fixture.
#[test]
fn the_raw_node_takes_the_input_devices_and_passes_reports_both_ways() {
    const TEST: &str = "the_raw_node_takes_the_input_devices_and_passes_reports_both_ways";
    if !can_make_steam_pads(TEST) {
        return;
    }
    let fake = FakeSteamController::builder()
        .declaring_report_47()
        .build()
        .unwrap();
    fake.gamepad().expect("the driver makes the gamepad");
    let node = fake.raw_node().expect("the driver makes a raw node");
    settle();
    if !can_open(&node, true) {
        skip(TEST, "the fake's raw node isn't this user's to open here");
        return;
    }
    let mut raw = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
        .open(&node)
        .unwrap();
    assert!(fake.inputs_gone(), "{:?}", fake.inputs());
    let serials_asked = |received: &[Received]| {
        received
            .iter()
            .filter_map(Received::command)
            .filter(|command| command.first() == Some(&0xae))
            .count()
    };
    let asked_before = serials_asked(&fake.received());

    let state = SteamState::busy();
    let battery = SteamBattery {
        charge: ChargeState::Charging,
        level: 42,
        battery_millivolts: 3900,
        system_millivolts: 3300,
        input_millivolts: 5000,
        battery_milliamps: 300,
        input_milliamps: 500,
        temperature: 28,
    };
    let sent = vec![
        state.report(StateReport::Full, 0),
        state.report(StateReport::WithoutOrientation, 1),
        state.report(StateReport::WithShortClocks, 2),
        battery.report(),
    ];
    for report in &sent {
        fake.send(report).unwrap();
    }
    assert_eq!(read_reports(&mut raw, sent.len()), sent);

    let rumble = [0x80, 0x00, 0x00, 0x40, 0x00, 0x10, 0x00, 0x00, 0x20, 0x00];
    raw.write_all(&rumble).unwrap();
    assert!(
        fake.wait_for(|received| received.contains(&Received::Output(rumble.to_vec()))),
        "{:?}",
        fake.received()
    );
    let mut settings = vec![0x01, 0x87, 0x03, 0x09, 0x00, 0x00];
    settings.resize(64, 0);
    ctrlassist_test::set_feature(raw.as_fd(), &settings).unwrap();
    assert!(
        fake.wait_for(|received| received.contains(&Received::SetFeature(settings.clone()))),
        "{:?}",
        fake.received()
    );
    // The fake acknowledges it by its echo, asked for as a GET.
    let reply = ctrlassist_test::get_feature(raw.as_fd(), 1, 64).unwrap();
    assert_eq!(reply, settings);

    drop(raw);
    fake.gamepad().expect("the driver makes the gamepad again");
    assert!(
        fake.wait_for(|received| serials_asked(received) > asked_before),
        "the driver asked the serial again: {:?}",
        fake.received()
    );
}

/// A Puck's slot binds with no pad in it, and has a gamepad only
/// while one is: the driver asks the receiver's wireless state as it
/// binds, and follows the slot's word that a pad joined or left.
#[test]
fn a_puck_s_slot_has_a_gamepad_only_while_a_pad_is_in_it() {
    const TEST: &str = "a_puck_s_slot_has_a_gamepad_only_while_a_pad_is_in_it";
    if !can_make_steam_pads(TEST) {
        return;
    }
    let fake = FakeSteamController::builder()
        .transport(SteamTransport::Puck { joined: false })
        .build()
        .unwrap();
    // The raw node is made once the driver has bound the slot.
    fake.raw_node().expect("the driver makes a raw node");
    let received = fake.received();
    assert!(
        set_in_order(&received, &[&[0xb4]]) && received.contains(&Received::GetFeature(2)),
        "the driver asked the receiver's wireless state: {received:?}"
    );
    assert!(fake.inputs().is_empty(), "{:?}", fake.inputs());

    fake.send_wireless(true).unwrap();
    let gamepad = fake.gamepad().expect("a pad joining makes the gamepad");
    assert_eq!(gamepad.name, WIRELESS_GAMEPAD);
    assert_eq!(gamepad.uniq, fake.serial());
    fake.send_wireless(false).unwrap();
    assert!(fake.inputs_gone(), "{:?}", fake.inputs());
}

/// Over Bluetooth the fake is on that bus with the Steam Controller's number
/// there, and bound as one.
#[test]
fn a_fake_on_bluetooth_is_bound_as_a_steam_controller() {
    const TEST: &str = "a_fake_on_bluetooth_is_bound_as_a_steam_controller";
    if !can_make_steam_pads(TEST) {
        return;
    }
    let fake = FakeSteamController::builder()
        .transport(SteamTransport::Bluetooth)
        .build()
        .unwrap();
    let gamepad = fake.gamepad().expect("the driver makes the gamepad");
    assert_eq!(gamepad.name, GAMEPAD);
    settle();
    let found = ctrlassist_device::connected()
        .unwrap()
        .into_iter()
        .find(|found| found.node == gamepad.event)
        .expect("the gamepad is a pad the daemon would find");
    assert!(found.identifiers.is_test_pad());
    assert_eq!(
        (found.identifiers.bus, found.identifiers.product),
        (5, 0x1303)
    );
    assert!(Path::new(&gamepad.event).exists());
}
