//! Core types for the CtrlAssist graph-based input processing architecture.
//!
//! This module defines the foundational types used throughout the graph:
//! - [`CtrlEvent`]: The unified event packet type
//! - [`EdgeState`]: Cached gamepad state on graph edges
//! - [`Node`]: The trait all processing nodes implement
//! - [`Graph`]: The container for nodes and edges
//! - [`GraphExecutor`]: Runs the event processing loop

pub mod drivers;
pub mod event;
pub mod graph;
pub mod node;
pub mod nodes;
pub mod state;

pub use drivers::*;
pub use event::*;
pub use graph::*;
pub use node::*;
pub use nodes::*;
pub use state::*;
