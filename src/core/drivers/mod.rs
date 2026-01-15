//! Driver nodes for interfacing with physical and virtual devices.
//!
//! These nodes bridge the graph architecture with the actual hardware:
//! - [`GilrsDriver`]: Reads from physical gamepads via gilrs
//! - [`SinkNode`]: Writes to virtual devices via evdev

pub mod gilrs_driver;
pub mod sink;
