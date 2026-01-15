//! Node implementations for the graph architecture.
//!
//! This module contains the concrete node types:
//! - [`MuxNode`]: Multiplexes multiple inputs into one output
//! - [`DemuxNode`]: Demultiplexes one input into multiple outputs (TODO)
//! - [`GilrsSourceNode`]: Reads from physical gamepads via gilrs (TODO)
//! - [`SinkNode`]: Writes to virtual devices via evdev (TODO)

pub mod mux;
