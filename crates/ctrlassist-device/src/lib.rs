//! CtrlAssist's kernel side.
//!
//! A pad is read through evdev from a descriptor, never a path: with
//! hiding on, the daemon can't open a pad's nodes itself and is handed
//! descriptors by the hiding service. A virtual pad is made through
//! uinput and written the same level-based way pads are read.

mod layout;
mod pad;
mod sink;

pub use layout::{AxisRange, Mapping};
pub use pad::{Identifiers, Pad, PadError};
pub use sink::{Identity, Request, VirtualPad};
