//! A Steam Controller in a setup on the input thread: the fake, bound by
//! the kernel's own `hid-steam`, read from its raw node for every control,
//! its rumble written there, and lizard mode kept off.
//!
//! Each test skips where the machine can't make the fake (a kernel older
//! than 7.3, Steam running, no uhid), and where the fake's raw node isn't
//! this user's to open, as on a machine with no rule like Steam's that
//! gives it to the seat.

use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::fd::OwnedFd;
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use ctrlassist_core::{
    Graph, GraphSpec, Input, Layout, Level, Mode, MuxConfig, NodeId, NodeSpec, PadState, SourceId,
    SourceSpec, Switch, gamepad,
};
use ctrlassist_device::{Command, Identity, InputThread, Pad, Report, Setup, VirtualPad};
use ctrlassist_test::{
    FakeSteamController, Received, StateReport, SteamButton, SteamState, SteamTransport, Trackpad,
    can_make_steam_pads, may_open_once_udev_is_done,
};

use crate::thread::{ID, PRIMARY, input_thread_switches, last, mux, sink};
use crate::{can_make_pads, read_until};

const ASSIST: SourceId = SourceId(1);

fn settle() {
    let _ = std::process::Command::new("udevadm").arg("settle").status();
}

fn skip(test: &str, why: &str) {
    let _ = writeln!(std::io::stderr(), "SKIPPED {test}: {why}");
}

/// Whether this process can make fake Steam Controllers and virtual pads
/// through uinput; says so where it can't.
fn can_make(test: &str) -> bool {
    can_make_steam_pads(test) && can_make_pads(test)
}

/// The fake's event node and raw node, once the driver has made both and
/// both are this user's to open.
fn nodes(
    test: &str,
    fake: &FakeSteamController,
) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    let gamepad = fake.gamepad().expect("the driver makes the gamepad");
    let raw = fake.raw_node().expect("the driver makes a raw node");
    settle();
    if !may_open_once_udev_is_done(&gamepad.event, false) || !may_open_once_udev_is_done(&raw, true)
    {
        skip(test, "the fake's nodes aren't this user's to open here");
        return None;
    }
    Some((gamepad.event, raw))
}

fn open_raw(raw: &Path) -> OwnedFd {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(raw)
        .unwrap()
        .into()
}

/// The fake as the daemon opens a Steam Controller: on its event node,
/// which says what it is, and then given its raw node, which takes the
/// event node away.
fn steam_pad(test: &str, fake: &FakeSteamController) -> Option<Pad> {
    let (event, raw) = nodes(test, fake)?;
    let mut pad = Pad::from_fd(File::open(event).unwrap().into()).unwrap();
    assert!(pad.identifiers().is_steam_controller());
    assert!(!pad.takes_output());
    pad.set_raw(open_raw(&raw)).unwrap();
    assert!(pad.can_rumble(), "rumble is the raw node's to send");
    assert!(
        !pad.takes_output(),
        "a virtual DualSense's reports are nothing for this pad"
    );
    Some(pad)
}

fn source() -> SourceSpec {
    SourceSpec {
        deadzone: 0.1,
        layout: Layout::gamepad(),
    }
}

/// One pad straight to the virtual pads.
fn straight(sinks: usize) -> Graph {
    Graph::build(GraphSpec {
        sources: vec![source()],
        nodes: Vec::new(),
        sinks: vec![Input::Source(PRIMARY); sinks],
    })
    .unwrap()
}

/// The fake sends `state`, numbered on from its last.
fn send(fake: &FakeSteamController, state: &SteamState) {
    fake.send_state(state, StateReport::Full).unwrap();
}

/// Sends `state` again and again until `seen` says the game has it: the
/// first state a newly started setup is sent may come before the input
/// thread has begun reading the pad, which reads away what came before
/// then, and nothing says when it has. Each look waits a little for what
/// it looks for; ten seconds in all at most, so that a test that fails
/// ends. Whether it was seen.
fn send_until(
    fake: &FakeSteamController,
    state: &SteamState,
    mut seen: impl FnMut() -> bool,
) -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(10) {
        send(fake, state);
        if seen() {
            return true;
        }
    }
    false
}

/// The frames the game's pad has now, waiting a little for one.
fn frames_now(game: &mut Pad) -> Vec<PadState> {
    let mut fds = [rustix::event::PollFd::new(
        &*game,
        rustix::event::PollFlags::IN,
    )];
    let wait = rustix::event::Timespec {
        tv_sec: 0,
        tv_nsec: 50_000_000,
    };
    let _ = rustix::event::poll(&mut fds, Some(&wait));
    let mut frames = Vec::new();
    game.read(|state| frames.push(*state)).unwrap();
    frames
}

/// Sends `state` until the game's last frame is `wanted`, and says every
/// frame the game had on the way.
fn played(
    fake: &FakeSteamController,
    state: &SteamState,
    game: &mut Pad,
    wanted: PadState,
) -> Vec<PadState> {
    let mut frames = Vec::new();
    let seen = send_until(fake, state, || {
        frames.extend(frames_now(game));
        frames.last() == Some(&wanted)
    });
    assert!(seen, "{wanted:?} never came: {frames:?}");
    frames
}

/// Every report until the setup comes back stopped.
fn until_stopped(thread: &InputThread, reports: &Receiver<Report>) -> Vec<Report> {
    thread.send(Command::Stop(ID)).unwrap();
    let mut seen = Vec::new();
    loop {
        let report = reports.recv().unwrap();
        let stopped = matches!(report, Report::Stopped(..));
        seen.push(report);
        if stopped {
            return seen;
        }
    }
}

/// Every control a generic virtual pad shows, from a Steam Controller,
/// each where the pad state has it: the face buttons by where they are,
/// the menu button as start and the view button as select, each pressed
/// alone so that the two can't pass swapped, the Steam
/// button as the mode button, the sticks with up as negative, and the
/// triggers by their axes.
#[test]
fn a_steam_controller_s_controls_reach_the_game_through_a_setup() {
    const TEST: &str = "a_steam_controller_s_controls_reach_the_game_through_a_setup";
    if !can_make(TEST) {
        return;
    }
    let fake = FakeSteamController::new().unwrap();
    let Some(pad) = steam_pad(TEST, &fake) else {
        return;
    };
    let (sink, mut game) = sink();
    let setup = Setup::new(straight(1), vec![Some(pad)], vec![sink]);
    let (thread, _reports) = InputThread::spawn().unwrap();
    thread.send(Command::Start(ID, Box::new(setup))).unwrap();

    // In several reports, as a generic virtual pad's frame holds only so
    // many changes at once.
    let buttons = SteamState::default()
        .with(SteamButton::South)
        .with(SteamButton::East)
        .with(SteamButton::West)
        .with(SteamButton::North)
        .with(SteamButton::LeftBumper)
        .with(SteamButton::RightBumper)
        .with(SteamButton::View)
        .with(SteamButton::Steam)
        .with(SteamButton::LeftStickClick)
        .with(SteamButton::RightStickClick);
    let pressed = PadState::NEUTRAL
        .with(gamepad::SOUTH, 1.0)
        .with(gamepad::EAST, 1.0)
        .with(gamepad::WEST, 1.0)
        .with(gamepad::NORTH, 1.0)
        .with(gamepad::LEFT_BUMPER, 1.0)
        .with(gamepad::RIGHT_BUMPER, 1.0)
        .with(gamepad::SELECT, 1.0)
        .with(gamepad::MODE, 1.0)
        .with(gamepad::LEFT_THUMB, 1.0)
        .with(gamepad::RIGHT_THUMB, 1.0);
    played(&fake, &buttons, &mut game, pressed);

    // The two middle buttons apart, so that one taken for the other shows.
    let menu = SteamState {
        buttons: (buttons.buttons & !SteamButton::View.bit()) | SteamButton::Menu.bit(),
        ..buttons
    };
    send(&fake, &menu);
    let pressed = pressed.with(gamepad::SELECT, 0.0).with(gamepad::START, 1.0);
    let frames = read_until(&mut game, |frames| frames.last() == Some(&pressed));
    assert_eq!(last(&frames), pressed);

    let axes = SteamState {
        left_trigger: 32767,
        right_trigger: 32767,
        // Right and up on the left stick, left and down on the right.
        left_stick: [32767, 32767],
        right_stick: [-32767, -32767],
        ..menu
    }
    .with(SteamButton::DpadUp)
    .with(SteamButton::DpadRight);
    send(&fake, &axes);
    let everything = pressed
        .with(gamepad::DPAD_UP, 1.0)
        .with(gamepad::DPAD_RIGHT, 1.0)
        .with(gamepad::LEFT_X, 1.0)
        .with(gamepad::LEFT_Y, -1.0)
        .with(gamepad::RIGHT_X, -1.0)
        .with(gamepad::RIGHT_Y, 1.0)
        .with(gamepad::LEFT_TRIGGER, 1.0)
        .with(gamepad::RIGHT_TRIGGER, 1.0);
    let frames = read_until(&mut game, |frames| frames.last() == Some(&everything));
    assert_eq!(last(&frames), everything);

    // The other half of the D-pad, and back to rest.
    send(
        &fake,
        &SteamState::default()
            .with(SteamButton::DpadDown)
            .with(SteamButton::DpadLeft),
    );
    let other = PadState::NEUTRAL
        .with(gamepad::DPAD_DOWN, 1.0)
        .with(gamepad::DPAD_LEFT, 1.0);
    let frames = read_until(&mut game, |frames| frames.last() == Some(&other));
    assert_eq!(last(&frames), other);
    send(&fake, &SteamState::default());
    let frames = read_until(&mut game, |frames| {
        frames.last() == Some(&PadState::NEUTRAL)
    });
    assert_eq!(last(&frames), PadState::NEUTRAL);
}

/// Two Steam Controllers merged in priority: the assist's stick outranks
/// the primary's, and each one's buttons reach the game.
#[test]
fn two_steam_controllers_merge_with_the_assist_outranking_the_primary() {
    const TEST: &str = "two_steam_controllers_merge_with_the_assist_outranking_the_primary";
    if !can_make(TEST) {
        return;
    }
    let primary = FakeSteamController::new().unwrap();
    let assist = FakeSteamController::new().unwrap();
    let (Some(primary_pad), Some(assist_pad)) =
        (steam_pad(TEST, &primary), steam_pad(TEST, &assist))
    else {
        return;
    };
    let (sink, mut game) = sink();
    let setup = Setup::new(
        mux(Mode::Priority),
        vec![Some(primary_pad), Some(assist_pad)],
        vec![sink],
    );
    let (thread, _reports) = InputThread::spawn().unwrap();
    thread.send(Command::Start(ID, Box::new(setup))).unwrap();

    let held = SteamState {
        left_stick: [32767, 0],
        ..SteamState::default()
    }
    .with(SteamButton::South);
    let primary_only = PadState::NEUTRAL
        .with(gamepad::SOUTH, 1.0)
        .with(gamepad::LEFT_X, 1.0);
    played(&primary, &held, &mut game, primary_only);

    send(
        &assist,
        &SteamState {
            left_stick: [-32767, 0],
            ..SteamState::default()
        }
        .with(SteamButton::East),
    );
    let taken = primary_only
        .with(gamepad::EAST, 1.0)
        .with(gamepad::LEFT_X, -1.0);
    let frames = read_until(&mut game, |frames| frames.last() == Some(&taken));
    assert_eq!(last(&frames), taken);

    send(&assist, &SteamState::default());
    let frames = read_until(&mut game, |frames| frames.last() == Some(&primary_only));
    assert_eq!(last(&frames), primary_only);
}

/// A virtual DualSense's input reports, read as a game reads them, until
/// one says what `wanted` waits for, or for four seconds at most; the
/// last report read.
fn dualsense_until(raw: &mut File, wanted: impl Fn(&[u8; 64]) -> bool) -> [u8; 64] {
    dualsense_within(raw, Duration::from_secs(4), wanted)
}

/// As [`dualsense_until`], for `bound` at most.
fn dualsense_within(
    raw: &mut File,
    bound: Duration,
    wanted: impl Fn(&[u8; 64]) -> bool,
) -> [u8; 64] {
    let started = Instant::now();
    let mut report = [0u8; 64];
    // It reports every 10 ms whatever happens behind it, so a read never
    // waits long.
    while started.elapsed() < bound {
        assert_eq!(raw.read(&mut report).unwrap(), 64);
        if wanted(&report) {
            break;
        }
    }
    report
}

/// Sends `state` until a report of the virtual DualSense says what
/// `wanted` waits for, as [`send_until`]; that report.
fn sent_to_dualsense(
    fake: &FakeSteamController,
    state: &SteamState,
    game: &mut File,
    wanted: impl Fn(&[u8; 64]) -> bool,
) -> [u8; 64] {
    let mut report = [0u8; 64];
    send_until(fake, state, || {
        report = dualsense_within(game, Duration::from_millis(50), &wanted);
        wanted(&report)
    });
    report
}

/// The first contact of a DualSense's input report: whether a finger is
/// on it, and its place in the touchpad's counts.
fn contact(report: &[u8; 64]) -> (bool, u16, u16) {
    let [id, low, middle, high] = [report[33], report[34], report[35], report[36]];
    let x = u16::from(low) | (u16::from(middle & 0x0f) << 8);
    let y = u16::from(middle >> 4) | (u16::from(high) << 4);
    (id & 0x80 == 0, x, y)
}

/// The gyroscope's pitch count of a DualSense's input report.
fn pitch(report: &[u8; 64]) -> i16 {
    i16::from_le_bytes([report[16], report[17]])
}

/// A virtual DualSense, made from a stand-in's data, with its raw node
/// open as a game's; none where that node isn't this user's.
fn virtual_dualsense(test: &str, last: u32) -> Option<(VirtualPad, File)> {
    let data = ctrlassist_test::synthetic_dualsense(true);
    let sink =
        VirtualPad::dualsense(&data, ctrlassist_device::own_address(0x7e_5c00 + last)).unwrap();
    settle();
    let raw = ctrlassist_test::raw_node_of(sink.event_node()).unwrap();
    if !may_open_once_udev_is_done(&raw, false) {
        skip(
            test,
            "a virtual DualSense's raw node isn't this user's to open here",
        );
        return None;
    }
    let reader = File::open(&raw).unwrap();
    Some((sink, reader))
}

/// The right trackpad is the virtual DualSense's touchpad, the finger
/// where it is on the trackpad and left where it lifted; and the motion
/// is the pad's.
#[test]
fn the_right_trackpad_and_the_motion_reach_a_virtual_dualsense() {
    const TEST: &str = "the_right_trackpad_and_the_motion_reach_a_virtual_dualsense";
    if !can_make_steam_pads(TEST) || !ctrlassist_test::can_make_hid_pads(TEST) {
        return;
    }
    let fake = FakeSteamController::new().unwrap();
    let Some(pad) = steam_pad(TEST, &fake) else {
        return;
    };
    let Some((sink, mut game)) = virtual_dualsense(TEST, 1) else {
        return;
    };
    let setup = Setup::new(straight(1), vec![Some(pad)], vec![sink]);
    let (thread, _reports) = InputThread::spawn().unwrap();
    thread.send(Command::Start(ID, Box::new(setup))).unwrap();

    // Three quarters across and halfway down the right trackpad; the left
    // touched too, which a DualSense has nowhere to show.
    let touched = SteamState {
        right_pad: Trackpad {
            touched: true,
            x: 16384,
            y: 0,
            pressure: 1000,
        },
        left_pad: Trackpad {
            touched: true,
            x: -16384,
            y: 16384,
            pressure: 0,
        },
        // Turning at 500 degrees a second about the pad's x.
        gyro: [8192, 0, 0],
        accel: [0, 0, 16384],
        ..SteamState::default()
    };
    let report = sent_to_dualsense(&fake, &touched, &mut game, |report| contact(report).0);
    let (down, x, y) = contact(&report);
    assert!(down, "{report:?}");
    // 0.75 of 1919 across, and half of 1079 down, within a count.
    assert!(x.abs_diff(1439) <= 1, "{x}");
    assert!(y.abs_diff(539) <= 1, "{y}");
    assert!(pitch(&report) > 0, "the pad's turning: {report:?}");

    // Lifted, with the report's fields elsewhere: where it was.
    let lifted = SteamState {
        right_pad: Trackpad {
            touched: false,
            x: -32767,
            y: -32767,
            pressure: 0,
        },
        ..touched
    };
    send(&fake, &lifted);
    let report = dualsense_until(&mut game, |report| !contact(report).0);
    let (down, x, y) = contact(&report);
    assert!(!down, "{report:?}");
    assert!(x.abs_diff(1439) <= 1 && y.abs_diff(539) <= 1, "{x} {y}");
}

/// A little-endian `i16` of a DualSense's input report.
fn i16_at(report: &[u8; 64], at: usize) -> i16 {
    i16::from_le_bytes([report[at], report[at + 1]])
}

/// The gyroscope's pitch, yaw and roll counts of a DualSense's input
/// report, then its accelerometer's x, y and z.
fn motion_of(report: &[u8; 64]) -> ([i16; 3], [i16; 3]) {
    (
        [16, 18, 20].map(|at| i16_at(report, at)),
        [22, 24, 26].map(|at| i16_at(report, at)),
    )
}

/// The sensor time of a DualSense's input report, in thirds of a
/// microsecond.
fn sensor_time(report: &[u8; 64]) -> u32 {
    u32::from_le_bytes([report[28], report[29], report[30], report[31]])
}

/// What a game reads of a DualSense's input report for its controls and
/// its touchpad: the sticks and triggers, the buttons, the motion and the
/// two contacts. The rest is the report's count and clocks, which move
/// whatever the pad does.
fn shown(report: &[u8; 64]) -> Vec<u8> {
    [1..=6, 8..=11, 16..=27, 33..=40]
        .into_iter()
        .flat_map(|bytes| report[bytes].to_vec())
        .collect()
}

/// Everything of a Steam Controller that a DualSense has no place for,
/// held on top of `state`: the left trackpad touched, pressed and
/// clicked, the four back buttons, the capacitive touches on both sticks
/// and grips, the quick access button and the triggers' clicks.
fn beyond_a_dualsense(state: SteamState) -> SteamState {
    SteamState {
        left_pad: Trackpad {
            touched: true,
            x: -12000,
            y: 9000,
            pressure: 20000,
        },
        ..state
    }
    .with(SteamButton::LeftPadClick)
    .with(SteamButton::UpperRightBack)
    .with(SteamButton::UpperLeftBack)
    .with(SteamButton::LowerRightBack)
    .with(SteamButton::LowerLeftBack)
    .with(SteamButton::LeftStickTouch)
    .with(SteamButton::RightStickTouch)
    .with(SteamButton::LeftGripTouch)
    .with(SteamButton::RightGripTouch)
    .with(SteamButton::QuickAccess)
    .with(SteamButton::LeftTriggerClick)
    .with(SteamButton::RightTriggerClick)
}

/// Behind a virtual DualSense, what a DualSense has no place for changes
/// nothing in its report: the left trackpad, the back buttons, the
/// capacitive touches, the quick access button and the triggers' clicks.
/// A pad lying flat and still reads as a DualSense at rest does.
#[test]
fn what_a_dualsense_has_no_place_for_changes_nothing_in_its_report() {
    const TEST: &str = "what_a_dualsense_has_no_place_for_changes_nothing_in_its_report";
    if !can_make_steam_pads(TEST) || !ctrlassist_test::can_make_hid_pads(TEST) {
        return;
    }
    let fake = FakeSteamController::new().unwrap();
    let Some(pad) = steam_pad(TEST, &fake) else {
        return;
    };
    let Some((sink, mut game)) = virtual_dualsense(TEST, 3) else {
        return;
    };
    let setup = Setup::new(straight(1), vec![Some(pad)], vec![sink]);
    let (thread, _reports) = InputThread::spawn().unwrap();
    thread.send(Command::Start(ID, Box::new(setup))).unwrap();

    // Flat and still: 1 g on the report's z, which is up.
    let flat = SteamState {
        accel: [0, 0, 16384],
        ..SteamState::default()
    };
    let rest = sent_to_dualsense(&fake, &flat, &mut game, |report| {
        motion_of(report).1[1] > 8000
    });
    assert_eq!(
        motion_of(&rest),
        ([0, 0, 0], [0, 8192, 0]),
        "a DualSense at rest: {rest:?}"
    );

    // All of it held, and south with it, which says the report was read.
    let south = beyond_a_dualsense(flat).with(SteamButton::South);
    let report = sent_to_dualsense(&fake, &south, &mut game, |report| report[8] & 0x20 != 0);
    let mut wanted = rest;
    wanted[8] |= 0x20;
    assert_eq!(shown(&report), shown(&wanted), "{report:?}");
}

/// Behind a virtual DualSense, the right trackpad's click is the
/// touchpad's click, apart from its touch, and the left trackpad's is
/// nothing.
#[test]
fn the_right_trackpad_s_click_is_a_virtual_dualsense_s_touchpad_click() {
    const TEST: &str = "the_right_trackpad_s_click_is_a_virtual_dualsense_s_touchpad_click";
    if !can_make_steam_pads(TEST) || !ctrlassist_test::can_make_hid_pads(TEST) {
        return;
    }
    let fake = FakeSteamController::new().unwrap();
    let Some(pad) = steam_pad(TEST, &fake) else {
        return;
    };
    let Some((sink, mut game)) = virtual_dualsense(TEST, 4) else {
        return;
    };
    let setup = Setup::new(straight(1), vec![Some(pad)], vec![sink]);
    let (thread, _reports) = InputThread::spawn().unwrap();
    thread.send(Command::Start(ID, Box::new(setup))).unwrap();
    let clicked = |report: &[u8; 64]| report[10] & 0x02 != 0;

    // Touched in the middle, and clicked.
    let touched = SteamState {
        right_pad: Trackpad {
            touched: true,
            x: 0,
            y: 0,
            pressure: 30000,
        },
        ..SteamState::default()
    };
    let report = sent_to_dualsense(
        &fake,
        &touched.with(SteamButton::RightPadClick),
        &mut game,
        clicked,
    );
    assert!(clicked(&report), "{report:?}");
    let (down, x, y) = contact(&report);
    assert!(
        down && x.abs_diff(960) <= 1 && y.abs_diff(540) <= 1,
        "{x} {y}"
    );

    // Let up and still touched: the finger stays down.
    send(&fake, &touched);
    let report = dualsense_until(&mut game, |report| !clicked(report));
    assert!(!clicked(&report) && contact(&report).0, "{report:?}");

    // The left trackpad clicked instead, and the right lifted: nothing.
    send(
        &fake,
        &SteamState::default().with(SteamButton::LeftPadClick),
    );
    let report = dualsense_until(&mut game, |report| !contact(report).0);
    assert!(!contact(&report).0, "{report:?}");
    for _ in 0..20 {
        let report = dualsense_within(&mut game, Duration::from_millis(50), |_| true);
        assert!(!clicked(&report), "{report:?}");
    }
}

/// Behind a virtual DualSense, each of the pad's axes of turning and of
/// acceleration arrives on the one the pad state's axes put it on: the
/// report's x, z and minus y, as SDL and the kernel's driver both hand
/// them on, at the DualSense's own counts; and every report's sensor time
/// is later than the one before's.
#[test]
fn the_motion_reaches_a_virtual_dualsense_on_its_axes_with_a_moving_clock() {
    const TEST: &str = "the_motion_reaches_a_virtual_dualsense_on_its_axes_with_a_moving_clock";
    if !can_make_steam_pads(TEST) || !ctrlassist_test::can_make_hid_pads(TEST) {
        return;
    }
    let fake = FakeSteamController::new().unwrap();
    let Some(pad) = steam_pad(TEST, &fake) else {
        return;
    };
    let Some((sink, mut game)) = virtual_dualsense(TEST, 5) else {
        return;
    };
    let setup = Setup::new(straight(1), vec![Some(pad)], vec![sink]);
    let (thread, _reports) = InputThread::spawn().unwrap();
    thread.send(Command::Start(ID, Box::new(setup))).unwrap();

    // 500 degrees a second, and half a g, at the pad's full scales of
    // 2000 and 2; as the DualSense's counts, 17678 to 1080 degrees a
    // second and 8192 to the g.
    let rate = 8192;
    let dualsense_rate = 8184;
    let near = |read: i16, wanted: i16| read.abs_diff(wanted) <= wanted.unsigned_abs() / 100 + 1;
    let mut clock = 0;
    let mut reading = |gyro: [i16; 3], accel: [i16; 3]| {
        clock += 4000;
        SteamState {
            gyro,
            accel,
            sensor_clock: clock,
            ..SteamState::default()
        }
    };
    let cases = [
        // About the report's x: pitch.
        (
            [rate, 0, 0],
            [8192, 0, 16384],
            [dualsense_rate, 0, 0],
            [4096, 8192, 0],
        ),
        // About its z, which is up: yaw.
        (
            [0, 0, rate],
            [0, 0, 16384],
            [0, dualsense_rate, 0],
            [0, 8192, 0],
        ),
        // About its y, away from the player: roll, the other way.
        (
            [0, rate, 0],
            [0, 8192, 16384],
            [0, 0, -dualsense_rate],
            [0, 8192, -4096],
        ),
    ];
    for (gyro, accel, rates, gravity) in cases {
        let state = reading(gyro, accel);
        let report = sent_to_dualsense(&fake, &state, &mut game, |report| {
            let (read_rates, read_gravity) = motion_of(report);
            read_rates
                .iter()
                .zip(rates)
                .all(|(read, wanted)| near(*read, wanted))
                && read_gravity
                    .iter()
                    .zip(gravity)
                    .all(|(read, wanted)| near(*read, wanted))
        });
        let (read_rates, read_gravity) = motion_of(&report);
        assert!(
            read_rates
                .iter()
                .zip(rates)
                .all(|(read, wanted)| near(*read, wanted))
                && read_gravity
                    .iter()
                    .zip(gravity)
                    .all(|(read, wanted)| near(*read, wanted)),
            "{gyro:?} {accel:?} read as {read_rates:?} {read_gravity:?}"
        );
    }

    // Fresh readings and beats between them: each report later.
    let mut times = Vec::new();
    for _ in 0..30 {
        send(&fake, &reading([0, 0, 0], [0, 0, 16384]));
        let report = dualsense_within(&mut game, Duration::from_millis(50), |_| true);
        times.push(sensor_time(&report));
    }
    assert!(
        times.windows(2).all(|pair| pair[1] > pair[0]),
        "the sensor time stood still or went back: {times:?}"
    );
}

/// Behind a virtual pad of the generic or the Xbox identity, the
/// standard controls arrive and nothing else: neither trackpad, their
/// clicks, the motion, the back buttons, the capacitive touches, the
/// quick access button nor the triggers' clicks makes a frame.
#[test]
fn behind_a_generic_or_an_xbox_virtual_pad_only_the_standard_controls_arrive() {
    const TEST: &str = "behind_a_generic_or_an_xbox_virtual_pad_only_the_standard_controls_arrive";
    if !can_make(TEST) {
        return;
    }
    for identity in [Identity::Generic, Identity::Xbox] {
        let fake = FakeSteamController::new().unwrap();
        let Some(pad) = steam_pad(TEST, &fake) else {
            return;
        };
        let sink = VirtualPad::create(identity).unwrap();
        let mut game = Pad::from_fd(crate::opened(sink.event_node())).unwrap();
        let setup = Setup::new(straight(1), vec![Some(pad)], vec![sink]);
        let (thread, _reports) = InputThread::spawn().unwrap();
        thread.send(Command::Start(ID, Box::new(setup))).unwrap();

        let everything = beyond_a_dualsense(SteamState {
            right_pad: Trackpad {
                touched: true,
                x: 16384,
                y: -16384,
                pressure: 30000,
            },
            gyro: [8192, -8192, 8192],
            accel: [8192, 8192, 16384],
            sensor_clock: 4000,
            ..SteamState::default()
        })
        .with(SteamButton::RightPadClick);
        send(&fake, &everything);
        let south = PadState::NEUTRAL.with(gamepad::SOUTH, 1.0);
        let frames = played(
            &fake,
            &everything.with(SteamButton::South),
            &mut game,
            south,
        );
        assert_eq!(frames, [south], "{identity:?}");
        // And let go, back to rest with nothing between.
        let frames = played(&fake, &SteamState::default(), &mut game, PadState::NEUTRAL);
        assert_eq!(frames, [PadState::NEUTRAL], "{identity:?}");
    }
}

/// A back button held by the assist as the button that hands the motion
/// over: while it is held, the virtual DualSense turns as the assist's
/// pad does, and the game never sees the button.
#[test]
fn a_paddle_held_by_the_assist_hands_the_motion_over() {
    const TEST: &str = "a_paddle_held_by_the_assist_hands_the_motion_over";
    if !can_make_steam_pads(TEST) || !ctrlassist_test::can_make_hid_pads(TEST) {
        return;
    }
    let primary = FakeSteamController::new().unwrap();
    let assist = FakeSteamController::new().unwrap();
    let (Some(primary_pad), Some(assist_pad)) =
        (steam_pad(TEST, &primary), steam_pad(TEST, &assist))
    else {
        return;
    };
    let Some((sink, mut game)) = virtual_dualsense(TEST, 2) else {
        return;
    };
    let mut config = MuxConfig::new(Mode::Priority, &Layout::gamepad());
    config.gyro = Some(Switch {
        input: Some(1),
        button: gamepad::PADDLE_1,
    });
    let graph = Graph::build(GraphSpec {
        sources: vec![source(), source()],
        nodes: vec![NodeSpec::Mux {
            inputs: vec![Input::Source(PRIMARY), Input::Source(ASSIST)],
            config,
        }],
        sinks: vec![Input::Node(NodeId(0), 0)],
    })
    .unwrap();
    let setup = Setup::new(graph, vec![Some(primary_pad), Some(assist_pad)], vec![sink]);
    let (thread, _reports) = InputThread::spawn().unwrap();
    thread.send(Command::Start(ID, Box::new(setup))).unwrap();

    // The primary turns one way, the assist the other.
    let turning = |pitch: i16| SteamState {
        gyro: [pitch, 0, 0],
        accel: [0, 0, 16384],
        ..SteamState::default()
    };
    let report = sent_to_dualsense(&primary, &turning(8192), &mut game, |report| {
        pitch(report) > 0
    });
    send(&assist, &turning(-8192));
    assert!(pitch(&report) > 0, "the primary's motion: {report:?}");

    // R4 is the first paddle.
    send(&assist, &turning(-8192).with(SteamButton::UpperRightBack));
    let report = dualsense_until(&mut game, |report| pitch(report) < 0);
    assert!(pitch(&report) < 0, "the assist's motion: {report:?}");
    // No button down: the D-pad at none, and nothing else.
    assert_eq!(report[8..=10], [8, 0, 0], "the paddle shown: {report:?}");

    send(&assist, &turning(-8192));
    send(&primary, &turning(8192));
    let report = dualsense_until(&mut game, |report| pitch(report) > 0);
    assert!(pitch(&report) > 0, "the primary's again: {report:?}");
}

/// The settings frame that sets lizard mode off and asks for the motion
/// sensors' raw readings, as SDL sends it.
fn lizard_off() -> Received {
    let mut frame = vec![0x01, 0x87, 0x06, 0x09, 0x00, 0x00, 0x30, 0x18, 0x00];
    frame.resize(64, 0);
    Received::SetFeature(frame)
}

/// How many of `received` set lizard mode off.
fn lizard_offs(received: &[Received]) -> usize {
    let wanted = lizard_off();
    received
        .iter()
        .filter(|received| **received == wanted)
        .count()
}

/// Lizard mode is set off, with the motion sensors on, as the pad is
/// given its raw node, with no setup reading it; and again once the pad's
/// own interval of three seconds has passed, not in a burst before it.
/// By the fake's own times, as its thread read each, which a busy test
/// thread looking late moves neither of; the first read late only makes
/// the two look closer, so the bound is well under three seconds. This
/// waits for that interval.
#[test]
fn lizard_mode_is_set_off_at_once_and_again_after_the_pad_s_interval() {
    const TEST: &str = "lizard_mode_is_set_off_at_once_and_again_after_the_pad_s_interval";
    if !can_make_steam_pads(TEST) {
        return;
    }
    let fake = FakeSteamController::new().unwrap();
    let Some(pad) = steam_pad(TEST, &fake) else {
        return;
    };
    assert!(
        fake.wait_for(|received| lizard_offs(received) >= 2),
        "{:?}",
        fake.received()
    );
    let wanted = lizard_off();
    let at: Vec<Instant> = fake
        .received_at()
        .into_iter()
        .filter(|(_, received)| *received == wanted)
        .map(|(at, _)| at)
        .collect();
    assert_eq!(at.len(), 2, "no burst: {at:?}");
    let between = at[1] - at[0];
    assert!(between >= Duration::from_secs(2), "again after {between:?}");
    drop(pad);
}

/// The output report a level is, as SDL sends it.
fn rumble(level: Level) -> Received {
    let [strong_low, strong_high] = level.strong.to_le_bytes();
    let [weak_low, weak_high] = level.weak.to_le_bytes();
    Received::Output(vec![
        0x80,
        0,
        0,
        0,
        strong_low,
        strong_high,
        0,
        weak_low,
        weak_high,
        0,
    ])
}

/// A rumble reaches the pad as its rumble report, written again while it
/// lasts, since the pad stops by itself shortly after the last; and once
/// it ends, the report at rest follows. The pad is let go with its motors
/// at rest too.
#[test]
fn a_rumble_reaches_the_pad_again_while_it_lasts_and_a_stop_follows() {
    const TEST: &str = "a_rumble_reaches_the_pad_again_while_it_lasts_and_a_stop_follows";
    if !can_make(TEST) {
        return;
    }
    let fake = FakeSteamController::new().unwrap();
    let Some(pad) = steam_pad(TEST, &fake) else {
        return;
    };
    let (sink, _game) = sink();
    let setup = Setup::new(straight(1), vec![Some(pad)], vec![sink]);
    let (thread, reports) = InputThread::spawn().unwrap();
    thread.send(Command::Start(ID, Box::new(setup))).unwrap();

    let level = Level {
        strong: 0x8000,
        weak: 0x4000,
    };
    thread
        .send(Command::Buzz {
            setup: ID,
            source: PRIMARY,
            level,
            millis: 400,
        })
        .unwrap();
    let on = rumble(level);
    let off = rumble(Level::default());
    let stopped = |received: &[Received]| {
        received
            .iter()
            .position(|sent| *sent == on)
            .is_some_and(|at| received[at..].contains(&off))
    };
    assert!(
        fake.wait_for(stopped),
        "the rumble and then its stop: {:?}",
        fake.received()
    );
    let received = fake.received();
    let start = received.iter().position(|sent| *sent == on).unwrap();
    let end = start
        + received[start..]
            .iter()
            .position(|sent| *sent == off)
            .unwrap();
    let again = received[start..end]
        .iter()
        .filter(|sent| **sent == on)
        .count();
    // Every 40 ms for 400: ten, fewer on a busy machine, and never a
    // writer that sends as fast as it can.
    assert!(
        (3..=20).contains(&again),
        "{again} rumble reports in 400 ms"
    );
    assert!(
        !received[end..].contains(&on),
        "nothing more of the rumble after its stop"
    );

    let seen = until_stopped(&thread, &reports);
    let stops = |received: &[Received]| received.iter().filter(|sent| **sent == off).count();
    let before = stops(&fake.received());
    drop(seen);
    assert!(
        fake.wait_for(|received| stops(received) == before + 1),
        "let go, its motors are at rest: {:?}",
        fake.received()
    );
}

/// The pad going, its fake dropped as a pad is unplugged, is reported,
/// and the setup carries on with the other.
#[test]
fn a_steam_controller_that_goes_is_reported_and_the_setup_carries_on() {
    const TEST: &str = "a_steam_controller_that_goes_is_reported_and_the_setup_carries_on";
    if !can_make(TEST) {
        return;
    }
    let primary = FakeSteamController::new().unwrap();
    let assist = FakeSteamController::new().unwrap();
    let (Some(primary_pad), Some(assist_pad)) =
        (steam_pad(TEST, &primary), steam_pad(TEST, &assist))
    else {
        return;
    };
    let (sink, mut game) = sink();
    let setup = Setup::new(
        mux(Mode::Priority),
        vec![Some(primary_pad), Some(assist_pad)],
        vec![sink],
    );
    let (thread, reports) = InputThread::spawn().unwrap();
    thread.send(Command::Start(ID, Box::new(setup))).unwrap();
    let held = PadState::NEUTRAL.with(gamepad::EAST, 1.0);
    played(
        &assist,
        &SteamState::default().with(SteamButton::East),
        &mut game,
        held,
    );

    drop(assist);
    let gone = loop {
        match reports.recv().unwrap() {
            Report::PadGone { setup, source, .. } => break (setup, source),
            Report::Selected { .. } => {}
            other => unreachable!("expected the pad to be reported gone, got {other:?}"),
        }
    };
    assert_eq!(gone, (ID, ASSIST));
    // What it held is let go.
    let frames = read_until(&mut game, |frames| {
        frames.last() == Some(&PadState::NEUTRAL)
    });
    assert_eq!(last(&frames), PadState::NEUTRAL);
    let south = PadState::NEUTRAL.with(gamepad::SOUTH, 1.0);
    played(
        &primary,
        &SteamState::default().with(SteamButton::South),
        &mut game,
        south,
    );
}

/// A pad in a Puck's slot is gone when the slot says its pad left, though
/// the slot itself, and its raw node, stay.
#[test]
fn a_puck_s_slot_saying_its_pad_left_is_the_pad_going() {
    const TEST: &str = "a_puck_s_slot_saying_its_pad_left_is_the_pad_going";
    if !can_make(TEST) {
        return;
    }
    let fake = FakeSteamController::builder()
        .transport(SteamTransport::Puck { joined: true })
        .build()
        .unwrap();
    let Some(pad) = steam_pad(TEST, &fake) else {
        return;
    };
    let (sink, mut game) = sink();
    let setup = Setup::new(straight(1), vec![Some(pad)], vec![sink]);
    let (thread, reports) = InputThread::spawn().unwrap();
    thread.send(Command::Start(ID, Box::new(setup))).unwrap();
    let held = PadState::NEUTRAL.with(gamepad::NORTH, 1.0);
    played(
        &fake,
        &SteamState::default().with(SteamButton::North),
        &mut game,
        held,
    );

    fake.send_wireless(false).unwrap();
    match reports.recv().unwrap() {
        Report::PadGone { setup, source, .. } => assert_eq!((setup, source), (ID, PRIMARY)),
        other => unreachable!("expected the pad to be reported gone, got {other:?}"),
    }
    let frames = read_until(&mut game, |frames| {
        frames.last() == Some(&PadState::NEUTRAL)
    });
    assert_eq!(last(&frames), PadState::NEUTRAL);
}

/// What the raw node kept from before the pad was read is not played:
/// neither what came before the pad was given its raw node, nor what came
/// after, while its setup wasn't yet running. The game sees what the pad
/// sent once it was read, and nothing else.
#[test]
fn reports_kept_before_the_pad_was_read_are_not_played() {
    const TEST: &str = "reports_kept_before_the_pad_was_read_are_not_played";
    if !can_make(TEST) {
        return;
    }
    let fake = FakeSteamController::new().unwrap();
    let Some((event, raw)) = nodes(TEST, &fake) else {
        return;
    };
    let mut pad = Pad::from_fd(File::open(event).unwrap().into()).unwrap();
    let raw = open_raw(&raw);
    assert!(fake.inputs_gone(), "{:?}", fake.inputs());
    send(&fake, &SteamState::default().with(SteamButton::South));
    send(&fake, &SteamState::default().with(SteamButton::West));
    pad.set_raw(raw).unwrap();
    assert_eq!(*pad.state(), PadState::NEUTRAL);
    // After, as a pad that reports every few milliseconds does while its
    // setup is made.
    send(&fake, &SteamState::default().with(SteamButton::North));
    send(&fake, &SteamState::default().with(SteamButton::RightBumper));

    let (sink, mut game) = sink();
    let setup = Setup::new(straight(1), vec![Some(pad)], vec![sink]);
    let (thread, _reports) = InputThread::spawn().unwrap();
    thread.send(Command::Start(ID, Box::new(setup))).unwrap();
    let east = PadState::NEUTRAL.with(gamepad::EAST, 1.0);
    let frames = played(
        &fake,
        &SteamState::default().with(SteamButton::East),
        &mut game,
        east,
    );
    assert_eq!(frames, [east], "only what came once it was read");
}

/// The pad's event node is gone once its raw node is open, its readers
/// told `ENODEV`: that is not the pad going. The setup reads it, and
/// says nothing of a pad gone, until it is stopped with the pad still in
/// it; and it leaves the input thread asleep while the pad says nothing.
#[test]
fn the_event_node_s_going_is_not_the_pad_s() {
    const TEST: &str = "the_event_node_s_going_is_not_the_pad_s";
    if !can_make(TEST) {
        return;
    }
    let fake = FakeSteamController::new().unwrap();
    let Some(pad) = steam_pad(TEST, &fake) else {
        return;
    };
    assert!(fake.inputs_gone(), "{:?}", fake.inputs());
    let (sink, mut game) = sink();
    let setup = Setup::new(straight(1), vec![Some(pad)], vec![sink]);
    let (thread, reports) = InputThread::spawn().unwrap();
    thread.send(Command::Start(ID, Box::new(setup))).unwrap();
    let south = PadState::NEUTRAL.with(gamepad::SOUTH, 1.0);
    played(
        &fake,
        &SteamState::default().with(SteamButton::South),
        &mut game,
        south,
    );

    // While the pad sends nothing, the thread sleeps: a node gone that it
    // listened to would wake it at every wait.
    assert!(
        asleep_within(&thread, Duration::from_secs(4)),
        "the input thread is kept awake"
    );
    let before = input_thread_switches(&thread);
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        input_thread_switches(&thread),
        before,
        "the input thread woke with nothing to do"
    );

    let seen = until_stopped(&thread, &reports);
    assert!(
        !seen
            .iter()
            .any(|report| matches!(report, Report::PadGone { .. })),
        "{seen:?}"
    );
    let Some(Report::Stopped(_, setup)) = seen.last() else {
        unreachable!("{seen:?}");
    };
    assert!(setup.pads()[0].is_some(), "the pad is still the setup's");
}

/// Whether `thread` is seen asleep, the kernel's own word for it, within
/// `bound`: one kept awake never is.
fn asleep_within(thread: &InputThread, bound: Duration) -> bool {
    let task = thread.task().unwrap();
    let started = Instant::now();
    while started.elapsed() < bound {
        let stat = std::fs::read_to_string(format!("/proc/self/task/{task}/stat")).unwrap();
        // The name is in parentheses and may hold anything; the state is
        // the field after the last of them.
        let state = stat.rsplit_once(')').unwrap().1.trim_start().chars().next();
        if state == Some('S') {
            return true;
        }
        std::thread::yield_now();
    }
    false
}
