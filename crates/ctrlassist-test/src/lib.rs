//! Test support: fakes of the operating system's side of a pad.
//!
//! A [`FakePad`] is made through uinput and looks to the kernel like any
//! evdev gamepad. A [`FakeHidPad`] is made through uhid, so it also has a
//! HID device with a driver bound and a hidraw node, as a PlayStation pad
//! or any Bluetooth pad does. Both need access to their kernel interface,
//! which a clean machine gives only to root, so tests that use them skip
//! where it is missing and run for real in the virtual machine check.

mod hid;
mod hider;
mod inspect;
mod pad;
mod rumble;

pub use hid::{FakeHidPad, synthetic_dualsense};
pub use hider::{Asked, FakeHider};
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

/// Where the pad on `node` says it is plugged in, the kernel's `phys`.
/// A virtual pad's is its alone, so it tells that pad from another that
/// was later given the same node.
#[must_use]
pub fn phys_of(node: &Path) -> Option<String> {
    ctrlassist_device::connected()
        .ok()?
        .into_iter()
        .find(|found| found.node == node)?
        .identifiers
        .port
}

/// Whether a pad that says it is plugged in at `phys` is connected. A
/// node's path isn't the test for a pad having gone: the kernel hands a
/// freed node number to the next pad, which may be another test's.
#[must_use]
pub fn phys_connected(phys: &str) -> bool {
    ctrlassist_device::connected().is_ok_and(|pads| {
        pads.iter()
            .any(|found| found.identifiers.port.as_deref() == Some(phys))
    })
}

/// Whether `test` can make a virtual DualSense here: uhid and uinput are
/// writable. Where it can't, this says so on standard error and the test
/// should return, unless fake pads are required here.
///
/// # Panics
///
/// Panics if fake pads are required and can't be made.
#[must_use]
pub fn can_make_hid_pads(test: &str) -> bool {
    use std::io::Write as _;
    if uhid_available() && uinput_available() {
        return true;
    }
    assert!(
        !fake_pads_required(),
        "{test}: /dev/uhid isn't writable, and fake pads are required here"
    );
    let _ = writeln!(
        std::io::stderr(),
        "SKIPPED {test}: /dev/uhid isn't writable here"
    );
    false
}

/// Whether this process may create pads through uhid.
#[must_use]
pub fn uhid_available() -> bool {
    can_open(Path::new("/dev/uhid"), true)
}
