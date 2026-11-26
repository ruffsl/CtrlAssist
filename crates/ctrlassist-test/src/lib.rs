//! Test support: fakes of the operating system's side of a pad.
//!
//! A [`FakePad`] is made through uinput and looks to the kernel like any
//! evdev gamepad. A [`FakeHidPad`] is made through uhid, so it also has a
//! HID device with a driver bound and a hidraw node, as a PlayStation pad
//! or any Bluetooth pad does. A [`FakeSteamController`] is made through
//! uhid for the kernel's own `hid-steam` to bind, which only Linux 7.3 and
//! later can. Each needs access to its kernel interface,
//! which a clean machine gives only to root, so tests that use them skip
//! where it is missing and run for real in the virtual machine check.

mod hid;
mod hider;
mod inspect;
mod pad;
mod raw;
mod rumble;
mod steam;

pub use hid::{
    FakeBluetoothDualSense, FakeDualSense, FakeHidPad, playstation_crc32, synthetic_dualsense,
};
pub use hider::{Asked, FakeHider, Script, raw_node_of};
pub use inspect::{NodeKind, Pad, Reads, can_open, count_reads, pads};
pub use pad::FakePad;
pub use raw::{get_feature, set_feature};
pub use rumble::{Motors, RumblePad};
pub use steam::{
    ChargeState, FakeSteamController, GAMEPAD, InVirtualMachineCheck, MOTION, Received,
    StateReport, SteamBattery, SteamButton, SteamControllerBuilder, SteamInput, SteamState,
    SteamTransport, Trackpad, VALVE, VM_CHECK_MARKER, WIRELESS_GAMEPAD,
    kernel_knows_steam_controller, steam_is_running, wireless_report,
};

use std::path::Path;

/// The `phys` a fake pad made through uinput carries, which marks it as
/// the tests': a daemon takes no such pad unless it is asked to.
fn marker() -> std::ffi::CString {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let number = NEXT.fetch_add(1, Ordering::Relaxed);
    let text = format!(
        "{}{}/{number}",
        ctrlassist_device::TEST_MARKER,
        std::process::id()
    );
    std::ffi::CString::new(text).expect("the marker and two numbers hold no zero byte")
}

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
/// writable and the kernel's PlayStation driver is loaded. Where it
/// can't, this says so on standard error and the test should return,
/// unless fake pads are required here.
///
/// # Panics
///
/// Panics if fake pads are required and can't be made.
#[must_use]
pub fn can_make_hid_pads(test: &str) -> bool {
    use std::io::Write as _;
    let driver = Path::new("/sys/bus/hid/drivers/playstation").exists();
    if uhid_available() && uinput_available() && driver {
        return true;
    }
    assert!(
        !fake_pads_required(),
        "{test}: /dev/uhid isn't writable or the kernel's PlayStation driver isn't loaded, and fake pads are required here"
    );
    let _ = writeln!(
        std::io::stderr(),
        "SKIPPED {test}: /dev/uhid isn't writable here, or the kernel's PlayStation driver isn't loaded"
    );
    false
}

/// Whether `test` can make a [`FakeSteamController`] here: the running
/// kernel's `hid-steam` knows the 2026 controller, Steam isn't running,
/// and uhid is writable. Where it can't, this says so on standard error
/// and the test should return.
///
/// Unlike the other fakes, a kernel older than 7.3, which has no driver
/// for the controller, is a skip even where fake pads are required: CI's
/// runner has such a kernel, and the virtual machine checks are where the
/// fake always runs. So is a running Steam, which would take the fake
/// for a controller of its own.
///
/// # Panics
///
/// Panics if fake pads are required, the kernel knows the controller and
/// uhid isn't writable.
#[must_use]
pub fn can_make_steam_pads(test: &str) -> bool {
    use std::io::Write as _;
    let skip = |why: &str| {
        let _ = writeln!(std::io::stderr(), "SKIPPED {test}: {why}");
        false
    };
    if let Err(why) = kernel_knows_steam_controller() {
        return skip(&why);
    }
    if steam_is_running() {
        return skip("Steam is running, and would take a fake Steam Controller for its own");
    }
    if !uhid_available() {
        assert!(
            !fake_pads_required(),
            "{test}: /dev/uhid isn't writable, and fake pads are required here"
        );
        return skip("/dev/uhid isn't writable here");
    }
    if hid_steam_binds() {
        return true;
    }
    assert!(
        !fake_pads_required(),
        "{test}: the kernel's hid-steam doesn't bind a fake Steam Controller, and fake pads are required here"
    );
    skip("the kernel's hid-steam doesn't bind a fake Steam Controller here: blacklisted, say")
}

/// Whether the kernel's `hid-steam` binds a fake Steam Controller here:
/// it is loaded, or loads for one and binds it. A kernel that knows the
/// controller may still have the module blacklisted, as some do to keep
/// lizard mode's own pointer. Found once in a process, by making one fake
/// if the module isn't loaded yet.
fn hid_steam_binds() -> bool {
    static BINDS: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *BINDS.get_or_init(|| {
        Path::new("/sys/bus/hid/drivers/hid-steam").exists()
            || FakeSteamController::new().is_ok_and(|fake| fake.bound_by_hid_steam())
    })
}

/// Whether this process may create pads through uhid.
#[must_use]
pub fn uhid_available() -> bool {
    can_open(Path::new("/dev/uhid"), true)
}
