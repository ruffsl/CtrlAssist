//! CtrlAssist's kernel side.
//!
//! A pad is read through evdev from a descriptor, never a path: with
//! hiding on, the daemon can't open a pad's nodes itself and is handed
//! descriptors by the hiding service.

mod layout;
mod pad;

pub use layout::{AxisRange, Mapping};
pub use pad::{Identity, Pad, PadError};
