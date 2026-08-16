//! Node implementations for the graph architecture.
//!
//! This module contains the concrete node types:
//! - [`MuxNode`]: Multiplexes multiple inputs into one output
//! - [`SourceNode`]: Wraps physical gamepads, handles FF forwarding
//! - [`SinkNode`]: Wraps virtual devices, polls FF events
//! - [`DemuxNode`]: Demultiplexes one input into multiple outputs (TODO)

pub mod mux;
pub mod sink;
pub mod source;
