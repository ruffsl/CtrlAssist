//! CtrlAssist's kernel side.
//!
//! A pad is read through evdev from a descriptor, never a path: with
//! hiding on, the daemon can't open a pad's nodes itself and is handed
//! descriptors by the hiding service. A virtual pad is made through
//! uinput, or through uhid where it must pass as a pad games read raw,
//! and written the same level-based way pads are read.

mod database;
mod dualsense;
mod hidraw;
mod hotplug;
mod layout;
mod output;
mod pad;
mod realtime;
mod sink;
mod thread;

pub use database::Database;
pub use dualsense::DualSenseData;
pub use hotplug::{Change, Found, Monitor, connected};
pub use layout::{AxisRange, Mapping};
pub use pad::{Identifiers, Pad, PadError, PadId};
pub use sink::{
    Identity, OWN_ADDRESS, OWN_MARKER, Request, TEST_ADDRESS, TEST_MARKER, VirtualPad, own_address,
};
pub use thread::{Command, InputThread, NewGraph, Place, Report, SendError, Setup, SetupId, Tap};
