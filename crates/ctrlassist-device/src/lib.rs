//! CtrlAssist's kernel side.
//!
//! A pad is read through evdev from a descriptor, never a path: with
//! hiding on, the daemon can't open a pad's nodes itself and is handed
//! descriptors by the hiding service. A virtual pad is made through
//! uinput and written the same level-based way pads are read.

mod database;
mod hotplug;
mod layout;
mod pad;
mod realtime;
mod sink;
mod thread;

pub use database::Database;
pub use hotplug::{Change, Found, Monitor, connected};
pub use layout::{AxisRange, Mapping};
pub use pad::{Identifiers, Pad, PadError, PadId};
pub use sink::{Identity, OWN_MARKER, Request, VirtualPad};
pub use thread::{Command, InputThread, Report, SendError, Setup, SetupId};
