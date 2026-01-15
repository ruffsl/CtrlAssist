//! Node trait and related types for graph processing.
//!
//! Nodes are the processing units in the graph. They receive events
//! on input ports and emit events on output ports.

use crate::core::event::CtrlEvent;
use crate::core::state::EdgeState;
use std::collections::HashMap;

/// Unique identifier for a node within a graph
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub u32);

/// Unique identifier for a port on a node
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PortId(pub u32);

/// Common port IDs for standard nodes
pub mod ports {
    use super::PortId;

    // Mux node ports
    pub const MUX_PRIMARY_IN: PortId = PortId(0);
    pub const MUX_ASSIST_IN: PortId = PortId(1);
    pub const MUX_OUTPUT: PortId = PortId(0);

    // Demux node ports
    pub const DEMUX_INPUT: PortId = PortId(0);
    // Output ports: PortId(0), PortId(1), ...

    // Source/Sink ports
    pub const SOURCE_OUTPUT: PortId = PortId(0);
    pub const SINK_INPUT: PortId = PortId(0);
}

/// Describes the ports a node has
#[derive(Debug, Clone)]
pub struct NodePorts {
    /// Input port definitions
    pub inputs: Vec<PortDef>,
    /// Output port definitions
    pub outputs: Vec<PortDef>,
}

/// Definition of a single port
#[derive(Debug, Clone)]
pub struct PortDef {
    pub id: PortId,
    pub name: String,
}

impl PortDef {
    pub fn new(id: PortId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
        }
    }
}

/// Context passed to nodes during event processing.
///
/// Provides access to cached state on input edges, which nodes
/// can query to make decisions (e.g., Priority mode checking
/// if Assist stick is active).
pub struct ProcessContext<'a> {
    /// Cached state for each input port (updated automatically by executor)
    pub input_states: &'a HashMap<PortId, EdgeState>,

    /// Node's own persistent state (optional, for stateful nodes like Toggle)
    pub node_state: &'a mut dyn std::any::Any,
}

impl<'a> ProcessContext<'a> {
    /// Get state for a specific input port
    pub fn state(&self, port: PortId) -> Option<&EdgeState> {
        self.input_states.get(&port)
    }

    /// Convenience: check if a button is pressed on a specific input
    pub fn is_pressed(&self, port: PortId, button: crate::core::event::ButtonId) -> bool {
        self.input_states
            .get(&port)
            .is_some_and(|s| s.is_pressed(button))
    }

    /// Convenience: get axis value on a specific input
    pub fn axis_value(&self, port: PortId, axis: crate::core::event::AxisId) -> f32 {
        self.input_states
            .get(&port)
            .map(|s| s.axis_value(axis))
            .unwrap_or(0.0)
    }
}

/// The core trait all processing nodes must implement.
pub trait Node: Send + Sync {
    /// Human-readable name for this node
    fn name(&self) -> &str;

    /// Describe the ports this node has
    fn ports(&self) -> NodePorts;

    /// Process an event arriving on a specific input port.
    ///
    /// Returns a list of (output_port, event) pairs to emit.
    fn process(
        &mut self,
        input_port: PortId,
        event: CtrlEvent,
        ctx: &mut ProcessContext,
    ) -> Vec<(PortId, CtrlEvent)>;

    /// Optional: Called periodically for nodes that need to poll
    /// (e.g., source nodes polling physical devices).
    fn tick(&mut self) -> Vec<(PortId, CtrlEvent)> {
        vec![]
    }

    /// Optional: Check if this node has a specific capability
    fn has_capability(&self, cap: NodeCapability) -> bool {
        match cap {
            NodeCapability::ProcessesInput => true,
            NodeCapability::ProcessesFeedback => false,
            NodeCapability::Tickable => false,
        }
    }
}

/// Capabilities a node may have
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeCapability {
    /// Node processes input events (forward direction)
    ProcessesInput,
    /// Node processes feedback events (reverse direction)
    ProcessesFeedback,
    /// Node should be ticked periodically
    Tickable,
}

/// Boxed node type for storage in graph
pub type BoxedNode = Box<dyn Node>;

/// Empty state for stateless nodes
pub struct EmptyState;

impl Default for EmptyState {
    fn default() -> Self {
        Self
    }
}
