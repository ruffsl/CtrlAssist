//! Test support: fakes of the operating system's side of a pad.
//!
//! A [`FakePad`] is made through uinput and looks to the kernel like any
//! evdev gamepad. A [`FakeHidPad`] is made through uhid, so it also has a
//! HID device with a driver bound and a hidraw node, as a PlayStation pad
//! or any Bluetooth pad does. Both need access to their kernel interface,
//! which a clean machine gives only to root, so tests that use them skip
//! where it is missing and run for real in the virtual machine check.

mod hid;
mod inspect;
mod pad;
mod rumble;

pub use hid::FakeHidPad;
pub use inspect::{NodeKind, Pad, Reads, can_open, count_reads, pads};
pub use pad::FakePad;
pub use rumble::{Motors, RumblePad};

use std::path::Path;

/// Whether this process may create pads through uinput.
#[must_use]
pub fn uinput_available() -> bool {
    can_open(Path::new("/dev/uinput"), true)
}

/// Whether a test that can't make a fake pad must fail instead of
/// skipping: set where the machine is meant to allow it, as in CI, so a
/// machine that silently stopped allowing it is noticed.
#[must_use]
pub fn fake_pads_required() -> bool {
    std::env::var_os("CTRLASSIST_REQUIRE_FAKE_PADS").is_some()
}

/// Whether `test` can make fake pads here. Where it can't, this says so
/// on standard error and the test should return, unless fake pads are
/// required here.
///
/// # Panics
///
/// Panics if fake pads are required and can't be made.
#[must_use]
pub fn can_make_pads(test: &str) -> bool {
    use std::io::Write as _;
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

/// Whether this process may create pads through uhid.
#[must_use]
pub fn uhid_available() -> bool {
    can_open(Path::new("/dev/uhid"), true)
}
