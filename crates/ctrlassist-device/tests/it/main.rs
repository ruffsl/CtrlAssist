//! The pad reader against fake pads made through uinput.
//!
//! A clean machine gives uinput only to root, so each test says so and
//! passes without running where it can't make a pad.

#![expect(clippy::unwrap_used, reason = "tests fail loudly")]

mod hotplug;
mod thread;
mod virtual_pad;

use std::fs::File;
use std::io::Write;
use std::os::fd::OwnedFd;

use ctrlassist_core::{PadState, gamepad};
use ctrlassist_device::{Pad, PadError};
use ctrlassist_test::{FakePad, fake_pads_required, uinput_available};
use rustix::event::{PollFd, PollFlags, Timespec, poll};

const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const ABS_X: u16 = 0x00;
const ABS_Z: u16 = 0x02;
const ABS_HAT0Y: u16 = 0x11;
const BTN_SOUTH: u16 = 0x130;
const BTN_EAST: u16 = 0x131;

/// Whether this process can make pads; says so, or fails where they are
/// required, when it can't.
pub(crate) fn can_make_pads(test: &str) -> bool {
    if uinput_available() {
        return true;
    }
    assert!(
        !fake_pads_required(),
        "{test}: /dev/uinput isn't writable, and fake pads are required here"
    );
    let _ = writeln!(
        std::io::stderr(),
        "SKIPPED {test}: /dev/uinput isn't writable here"
    );
    false
}

/// A fake pad, or none where this process can't make one.
fn fake(test: &str) -> Option<FakePad> {
    can_make_pads(test).then(|| FakePad::new("Device Test Pad", 0x1209, 0x5eed).unwrap())
}

fn open(fake: &FakePad) -> Pad {
    let node: OwnedFd = File::open(fake.event_node()).unwrap().into();
    Pad::from_fd(node).unwrap()
}

/// Reads until `enough` says the frames so far are what was waited for,
/// or gives up after a bounded number of waits so that a failing test
/// ends.
pub(crate) fn read_until(pad: &mut Pad, enough: impl Fn(&[PadState]) -> bool) -> Vec<PadState> {
    let mut frames = Vec::new();
    for _ in 0..200 {
        pad.read(|state| frames.push(*state)).unwrap();
        if enough(&frames) {
            return frames;
        }
        let mut fds = [PollFd::new(pad, PollFlags::IN)];
        let wait = Timespec {
            tv_sec: 0,
            tv_nsec: 50_000_000,
        };
        let _ = poll(&mut fds, Some(&wait));
    }
    // What was waited for never came; the caller's assertion says what.
    frames
}

#[test]
fn each_report_frame_that_changes_the_pad_is_one_state() {
    let Some(mut fake) = fake("each_report_frame_that_changes_the_pad_is_one_state") else {
        return;
    };
    let mut pad = open(&fake);
    assert_eq!(*pad.state(), PadState::NEUTRAL);

    // Two controls in one frame arrive together, not one after the other.
    fake.frame(&[(EV_KEY, BTN_SOUTH, 1), (EV_ABS, ABS_X, 32767)])
        .unwrap();
    fake.frame(&[(EV_ABS, ABS_Z, 255), (EV_ABS, ABS_HAT0Y, -1)])
        .unwrap();
    fake.frame(&[(EV_KEY, BTN_SOUTH, 0)]).unwrap();
    let frames = read_until(&mut pad, |frames| frames.len() >= 3);

    let first = PadState::NEUTRAL
        .with(gamepad::SOUTH, 1.0)
        .with(gamepad::LEFT_X, 1.0);
    let second = first
        .with(gamepad::LEFT_TRIGGER, 1.0)
        .with(gamepad::DPAD_UP, 1.0);
    let third = second.with(gamepad::SOUTH, 0.0);
    assert_eq!(frames, [first, second, third]);
    assert_eq!(*pad.state(), third);
}

#[test]
fn a_pad_opened_mid_press_starts_with_what_is_held() {
    let Some(mut fake) = fake("a_pad_opened_mid_press_starts_with_what_is_held") else {
        return;
    };
    fake.frame(&[(EV_KEY, BTN_EAST, 1), (EV_ABS, ABS_X, -32768)])
        .unwrap();
    let pad = open(&fake);
    let held = PadState::NEUTRAL
        .with(gamepad::EAST, 1.0)
        .with(gamepad::LEFT_X, -1.0);
    assert_eq!(*pad.state(), held);
}

#[test]
fn events_the_kernel_dropped_cannot_leave_a_button_held() {
    let Some(mut fake) = fake("events_the_kernel_dropped_cannot_leave_a_button_held") else {
        return;
    };
    let mut pad = open(&fake);
    // Far more than the kernel keeps for a reader that isn't reading,
    // ending with everything released except one stick position.
    for round in 0..4000 {
        fake.frame(&[
            (EV_KEY, BTN_SOUTH, round % 2),
            (EV_ABS, ABS_X, 1000 + round),
        ])
        .unwrap();
    }
    fake.frame(&[
        (EV_KEY, BTN_SOUTH, 0),
        (EV_KEY, BTN_EAST, 1),
        (EV_ABS, ABS_X, 32767),
    ])
    .unwrap();

    let truth = PadState::NEUTRAL
        .with(gamepad::EAST, 1.0)
        .with(gamepad::LEFT_X, 1.0);
    let frames = read_until(&mut pad, |frames| frames.last() == Some(&truth));
    assert!(
        frames.len() < 4000,
        "nothing was dropped, so this tested nothing: {} frames",
        frames.len()
    );
    assert_eq!(*pad.state(), truth);
}

/// Whether a read of the kernel's raw events on `reader` holds the mark
/// the kernel leaves where it dropped events. Reads until nothing is
/// left.
fn saw_a_drop(reader: &mut File) -> bool {
    use std::io::Read as _;
    // An event is a time of two words, then its type, code and value.
    const EVENT: usize = 24;
    let mut buffer = [0u8; 64 * EVENT];
    let mut dropped = false;
    while let Ok(read) = reader.read(&mut buffer) {
        if read == 0 {
            break;
        }
        for event in buffer[..read].as_chunks::<EVENT>().0 {
            // EV_SYN, SYN_DROPPED.
            dropped |= event[16..20] == [0, 0, 3, 0];
        }
    }
    dropped
}

/// A stick that moves each time it is asked to, so that each frame it
/// sends is one the kernel passes on.
struct Stick(i32);

impl Stick {
    fn moved(&mut self, fake: &mut FakePad) {
        self.0 += 1;
        fake.frame(&[(EV_ABS, ABS_X, self.0)]).unwrap();
    }
}

/// How many frames of one event a reader can be sent unread before the
/// kernel drops some: its buffer, in events, is twice that, and the drop
/// comes with the last frame's report.
fn frames_before_a_drop(fake: &mut FakePad, stick: &mut Stick) -> usize {
    for frames in 1..=512 {
        let mut reader = File::open(fake.event_node()).unwrap();
        rustix::fs::fcntl_setfl(&reader, rustix::fs::OFlags::NONBLOCK).unwrap();
        for _ in 0..frames {
            stick.moved(fake);
        }
        if saw_a_drop(&mut reader) {
            return frames;
        }
    }
    unreachable!("the kernel's buffer is finite");
}

/// The kernel keeps the newest event with its mark of a drop, and the
/// frame it ends is never handed over whole. A press there is not lost:
/// the state fetched after the drop has it, and is told as a change from
/// what was last handed over, not from the dropped frame.
#[test]
fn a_press_in_the_frame_the_kernel_dropped_is_not_lost() {
    let Some(mut fake) = fake("a_press_in_the_frame_the_kernel_dropped_is_not_lost") else {
        return;
    };
    let mut stick = Stick(0);
    let before_a_drop = frames_before_a_drop(&mut fake, &mut stick);
    let mut pad = open(&fake);
    // One event short of the buffer, then the press as the event that
    // overflows it, kept with the mark.
    for _ in 0..before_a_drop - 2 {
        stick.moved(&mut fake);
    }
    fake.frame(&[(EV_ABS, ABS_X, -1), (EV_ABS, ABS_Z, 9)])
        .unwrap();
    fake.frame(&[(EV_KEY, BTN_SOUTH, 1)]).unwrap();

    let frames = read_until(&mut pad, |_| true);
    assert!(!frames.is_empty());
    assert_eq!(pad.state().get(gamepad::SOUTH), 1.0, "{frames:?}");
}

/// After the kernel drops events, the batch read with its mark can hold
/// frames from after it: older than the state then fetched, which takes
/// the key events still queued out of the kernel's buffer. Played after
/// that state, a press among them would stay held with no release to
/// follow it.
#[test]
fn events_read_beside_a_drop_cannot_undo_the_state_fetched_after_it() {
    let Some(mut fake) = fake("events_read_beside_a_drop_cannot_undo_the_state_fetched_after_it")
    else {
        return;
    };
    let mut stick = Stick(0);
    let before_a_drop = frames_before_a_drop(&mut fake, &mut stick);
    let mut stick = |fake: &mut FakePad| stick.moved(fake);
    // The pad reads up to 64 events at a time; the press must fall within
    // the first read after the drop, and the release after it, with no
    // second drop.
    let between = 31;
    if 2 * before_a_drop < 2 + 2 + 2 * between + 2 + 4 + 2 {
        let _ = writeln!(
            std::io::stderr(),
            "SKIPPED: the kernel's buffer for this pad is too small to leave events past one read"
        );
        return;
    }
    let mut pad = open(&fake);
    for _ in 0..before_a_drop {
        stick(&mut fake);
    }
    fake.frame(&[(EV_KEY, BTN_SOUTH, 1)]).unwrap();
    for _ in 0..between {
        stick(&mut fake);
    }
    fake.frame(&[(EV_KEY, BTN_SOUTH, 0)]).unwrap();
    stick(&mut fake);
    stick(&mut fake);

    let frames = read_until(&mut pad, |_| true);
    assert!(!frames.is_empty());
    assert_eq!(
        pad.state().get(gamepad::SOUTH),
        0.0,
        "the press and the release both came before the state was fetched"
    );
}

#[test]
fn a_frame_that_changes_nothing_mapped_is_not_reported() {
    let Some(mut fake) = fake("a_frame_that_changes_nothing_mapped_is_not_reported") else {
        return;
    };
    let mut pad = open(&fake);
    fake.frame(&[(EV_KEY, BTN_SOUTH, 1)]).unwrap();
    // The same press again: the kernel itself drops a repeat of a value.
    fake.frame(&[(EV_KEY, BTN_SOUTH, 1)]).unwrap();
    fake.frame(&[(EV_KEY, BTN_SOUTH, 0)]).unwrap();
    let frames = read_until(&mut pad, |frames| frames.len() >= 2);
    assert_eq!(
        frames,
        [
            PadState::NEUTRAL.with(gamepad::SOUTH, 1.0),
            PadState::NEUTRAL
        ]
    );
}

#[test]
fn a_pad_that_is_unplugged_reads_as_gone_and_keeps_its_last_state() {
    let Some(mut fake) = fake("a_pad_that_is_unplugged_reads_as_gone_and_keeps_its_last_state")
    else {
        return;
    };
    let mut pad = open(&fake);
    fake.frame(&[(EV_KEY, BTN_SOUTH, 1)]).unwrap();
    read_until(&mut pad, |frames| !frames.is_empty());
    drop(fake);

    let mut outcome = Ok(());
    for _ in 0..200 {
        outcome = pad.read(|_| {});
        if outcome.is_err() {
            break;
        }
        let mut fds = [PollFd::new(&pad, PollFlags::IN)];
        let _ = poll(&mut fds, None);
    }
    let error = outcome.unwrap_err();
    assert!(matches!(error, PadError::Gone), "{error:?}");
    assert_eq!(error.to_string(), "the pad has gone");
    assert_eq!(pad.state().get(gamepad::SOUTH), 1.0);
}

#[test]
fn a_pad_s_identity_is_what_the_kernel_reports() {
    let Some(fake) = fake("a_pad_s_identity_is_what_the_kernel_reports") else {
        return;
    };
    let pad = open(&fake);
    let identity = pad.identifiers();
    assert_eq!(identity.name, "Device Test Pad");
    assert_eq!(
        (identity.bus, identity.vendor, identity.product),
        (3, 0x1209, 0x5eed)
    );
    // A uinput pad has no address of its own and no USB serial number.
    assert_eq!(identity.address, None);
    assert_eq!(identity.serial, None);
}

#[test]
fn a_descriptor_that_is_not_an_event_device_is_refused() {
    let null: OwnedFd = File::open("/dev/null").unwrap().into();
    let error = Pad::from_fd(null).unwrap_err();
    assert!(matches!(error, PadError::Io(_)), "{error:?}");
    assert!(std::error::Error::source(&error).is_some());
    assert!(error.to_string().contains("couldn't be read"));
}

/// A pad that doesn't follow the gamepad layout: twelve plain joystick
/// buttons, four axes of which the last two aren't where a stick's are,
/// and a hat.
fn odd_pad(test: &str) -> Option<FakePad> {
    let keys: Vec<u16> = (0x120..0x12c).collect();
    let axes = [
        (0, 0, 255),
        (1, 0, 255),
        (2, 0, 255),
        (5, 0, 255),
        (0x10, -1, 1),
        (0x11, -1, 1),
    ];
    can_make_pads(test).then(|| {
        FakePad::with_controls("Odd Layout Pad", (0x1209, 0x0dd0, 0x0111), &keys, &axes).unwrap()
    })
}

/// A database line for the odd pad, as a user's own file would hold.
const ODD_LAYOUT: &str = "03000000091200000d0d000011010000,Odd Layout Pad,a:b2,b:b1,x:b3,y:b0,leftshoulder:b4,\
    rightshoulder:b5,lefttrigger:b6,righttrigger:b7,back:b8,start:b9,leftx:a0,lefty:a1,rightx:a2,righty:a3,\
    dpup:h0.1,dpright:h0.2,dpdown:h0.4,dpleft:h0.8,platform:Linux,";

#[test]
fn a_pad_with_an_odd_layout_is_read_through_its_database_line() {
    let Some(mut fake) = odd_pad("a_pad_with_an_odd_layout_is_read_through_its_database_line")
    else {
        return;
    };
    // The identifier's product is little-endian: 0x0dd0 is `d00d`.
    let database = ctrlassist_device::Database::parse(&ODD_LAYOUT.replace("0d0d0000", "d00d0000"));
    assert_eq!((database.len(), database.skipped()), (1, 0));
    let node: OwnedFd = File::open(fake.event_node()).unwrap().into();
    let mut pad = Pad::with_database(node, &database).unwrap();

    // Its third button is the south one; its fourth axis, code 5, is the
    // right stick's Y; its seventh button is a trigger that only clicks.
    fake.frame(&[
        (EV_KEY, 0x122, 1),
        (EV_ABS, 5, 255),
        (EV_KEY, 0x126, 1),
        (EV_ABS, 0x11, -1),
    ])
    .unwrap();
    let frames = read_until(&mut pad, |frames| !frames.is_empty());
    let expected = PadState::NEUTRAL
        .with(gamepad::SOUTH, 1.0)
        .with(gamepad::RIGHT_Y, 1.0)
        .with(gamepad::LEFT_TRIGGER, 1.0)
        .with(gamepad::DPAD_UP, 1.0);
    assert_eq!(frames, [expected]);
}

#[test]
fn a_pad_the_database_does_not_know_is_read_as_a_gamepad() {
    let Some(mut fake) = fake("a_pad_the_database_does_not_know_is_read_as_a_gamepad") else {
        return;
    };
    let database = ctrlassist_device::Database::bundled();
    let node: OwnedFd = File::open(fake.event_node()).unwrap().into();
    let mut pad = Pad::with_database(node, &database).unwrap();
    fake.frame(&[(EV_KEY, BTN_SOUTH, 1)]).unwrap();
    let frames = read_until(&mut pad, |frames| !frames.is_empty());
    assert_eq!(frames, [PadState::NEUTRAL.with(gamepad::SOUTH, 1.0)]);
}
