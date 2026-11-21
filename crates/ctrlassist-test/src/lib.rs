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

pub use hid::FakeHidPad;
pub use inspect::{NodeKind, Pad, Reads, can_open, count_reads, pads};
pub use pad::FakePad;

use std::path::Path;

/// Whether this process may create pads through uinput.
#[must_use]
pub fn uinput_available() -> bool {
    can_open(Path::new("/dev/uinput"), true)
}

/// Whether this process may create pads through uhid.
#[must_use]
pub fn uhid_available() -> bool {
    can_open(Path::new("/dev/uhid"), true)
}
