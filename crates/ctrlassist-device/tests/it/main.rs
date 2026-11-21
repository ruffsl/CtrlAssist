//! The pad reader against fake pads made through uinput.
//!
//! A clean machine gives uinput only to root, so each test says so and
//! passes without running where it can't make a pad.

#![expect(clippy::unwrap_used, reason = "tests fail loudly")]

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
