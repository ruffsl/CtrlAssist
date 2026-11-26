//! CtrlAssist's kernel side.
//!
//! A pad is read through evdev from a descriptor, never a path: with
//! hiding on, the daemon can't open a pad's nodes itself and is handed
//! descriptors by the hiding service. A virtual pad is made through
//! uinput, or through uhid where it must pass as a pad games read raw,
//! and written the same level-based way pads are read.

// The input thread catches a panic in one setup's handling and takes
// that setup down alone; aborting instead would end the daemon and every
// virtual pad with it. Cargo builds tests to unwind whatever a profile
// says, so no test can show what a shipped build does: this refuses to
// build one that aborts.
#[cfg(panic = "abort")]
compile_error!(
    "ctrlassist-device must be built to unwind on panic: the input thread keeps one setup's panic from taking the others down"
);

mod anchor;
mod database;
mod descriptor;
mod dualsense;
mod hidraw;
mod hotplug;
mod layout;
mod motion;
mod output;
mod pad;
mod queue;
mod realtime;
mod sink;
mod steam;
mod thread;

pub use anchor::Anchor;
pub use database::Database;
pub use dualsense::DualSenseData;
pub use hotplug::{Change, Found, Monitor, anchors, connected, open_node};
pub use layout::{AxisRange, Mapping};
pub use output::{Rested, Writes};
pub use pad::{Identifiers, Pad, PadError, PadId};
pub use sink::{
    Identity, OWN_ADDRESS, OWN_MARKER, Request, TEST_ADDRESS, TEST_MARKER, VirtualPad, own_address,
};
pub use thread::{Command, InputThread, NewGraph, Place, Report, SendError, Setup, SetupId, Tap};
