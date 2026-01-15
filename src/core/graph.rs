//! Graph structure and executor for event processing.
//!
//! The graph contains nodes connected by edges. The executor
//! routes events through the graph, automatically updating
//! EdgeState as events flow.

use crate::core::event::CtrlEvent;
use crate::core::node::{BoxedNode, NodeId, PortId, ProcessContext};
use crate::core::state::EdgeState;
use std::collections::{HashMap, VecDeque};

/// An edge connecting two ports in the graph
#[derive(Debug, Clone)]
pub struct Edge {
    pub from_node: NodeId,
    pub from_port: PortId,
    pub to_node: NodeId,
    pub to_port: PortId,
}

/// Edge identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EdgeId(pub u32);

/// Graph structure holding nodes and edges
pub struct Graph {
    /// All nodes in the graph, indexed by NodeId
    nodes: HashMap<NodeId, NodeEntry>,

    /// All edges in the graph
    edges: Vec<Edge>,

    /// Next node ID to assign
    next_node_id: u32,

    /// Cached state for each edge (from_node, from_port) -> state
    edge_states: HashMap<(NodeId, PortId), EdgeState>,
}

struct NodeEntry {
    node: BoxedNode,
    state: Box<dyn std::any::Any + Send + Sync>,
}

impl Graph {
    /// Create a new empty graph
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            edges: Vec::new(),
            next_node_id: 0,
            edge_states: HashMap::new(),
        }
    }

    /// Add a node to the graph, returning its ID
    pub fn add_node<S: std::any::Any + Send + Sync + Default + 'static>(
        &mut self,
        node: BoxedNode,
    ) -> NodeId {
        let id = NodeId(self.next_node_id);
        self.next_node_id += 1;

        self.nodes.insert(
            id,
            NodeEntry {
                node,
                state: Box::new(S::default()),
            },
        );

        id
    }

    /// Add a node with custom initial state
    pub fn add_node_with_state(
        &mut self,
        node: BoxedNode,
        state: Box<dyn std::any::Any + Send + Sync>,
    ) -> NodeId {
        let id = NodeId(self.next_node_id);
        self.next_node_id += 1;

        self.nodes.insert(id, NodeEntry { node, state });

        id
    }

    /// Connect two nodes via their ports
    pub fn connect(
        &mut self,
        from_node: NodeId,
        from_port: PortId,
        to_node: NodeId,
        to_port: PortId,
    ) {
        // Initialize edge state
        self.edge_states
            .entry((from_node, from_port))
            .or_insert_with(EdgeState::new);

        self.edges.push(Edge {
            from_node,
            from_port,
            to_node,
            to_port,
        });
    }

    /// Get a reference to a node
    pub fn get_node(&self, id: NodeId) -> Option<&dyn crate::core::node::Node> {
        self.nodes.get(&id).map(|e| e.node.as_ref())
    }

    /// Get the number of nodes
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// Get the number of edges
    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
}

impl Default for Graph {
    fn default() -> Self {
        Self::new()
    }
}

/// Executor that runs event processing through the graph
pub struct GraphExecutor {
    graph: Graph,
    /// Queue of pending events to process
    event_queue: VecDeque<PendingEvent>,
}

struct PendingEvent {
    to_node: NodeId,
    to_port: PortId,
    event: CtrlEvent,
    /// Which edge produced this event (for state tracking)
    from_edge: Option<(NodeId, PortId)>,
}

impl GraphExecutor {
    /// Create a new executor for the given graph
    pub fn new(graph: Graph) -> Self {
        Self {
            graph,
            event_queue: VecDeque::new(),
        }
    }

    /// Inject an event into the graph at a specific node/port
    pub fn inject(&mut self, node: NodeId, port: PortId, event: CtrlEvent) {
        self.event_queue.push_back(PendingEvent {
            to_node: node,
            to_port: port,
            event,
            from_edge: None,
        });
    }

    /// Process all pending events, returning any events that exit the graph
    /// (i.e., events emitted to ports with no outgoing edges)
    pub fn process(&mut self) -> Vec<(NodeId, PortId, CtrlEvent)> {
        let mut outputs = Vec::new();

        while let Some(pending) = self.event_queue.pop_front() {
            // Update edge state if this event came from an edge
            if let Some((from_node, from_port)) = pending.from_edge {
                if let Some(state) = self.graph.edge_states.get_mut(&(from_node, from_port)) {
                    state.apply(&pending.event);
                }
            }

            // Get input states for the target node
            let input_states = self.collect_input_states(pending.to_node);

            // Get the node entry
            let Some(entry) = self.graph.nodes.get_mut(&pending.to_node) else {
                continue;
            };

            // Create context
            let mut ctx = ProcessContext {
                input_states: &input_states,
                node_state: entry.state.as_mut(),
            };

            // Process the event
            let result = entry.node.process(pending.to_port, pending.event, &mut ctx);

            // Route output events
            for (out_port, out_event) in result {
                let edges: Vec<_> = self
                    .graph
                    .edges
                    .iter()
                    .filter(|e| e.from_node == pending.to_node && e.from_port == out_port)
                    .cloned()
                    .collect();

                if edges.is_empty() {
                    // No outgoing edge - this is a graph output
                    outputs.push((pending.to_node, out_port, out_event));
                } else {
                    // Update edge state and queue for downstream nodes
                    if let Some(state) =
                        self.graph.edge_states.get_mut(&(pending.to_node, out_port))
                    {
                        state.apply(&out_event);
                    }

                    for edge in edges {
                        self.event_queue.push_back(PendingEvent {
                            to_node: edge.to_node,
                            to_port: edge.to_port,
                            event: out_event.clone(),
                            from_edge: Some((pending.to_node, out_port)),
                        });
                    }
                }
            }
        }

        outputs
    }

    /// Tick all tickable nodes and process resulting events
    pub fn tick(&mut self) -> Vec<(NodeId, PortId, CtrlEvent)> {
        // Collect tick results from all nodes
        let mut tick_events = Vec::new();

        for (&node_id, entry) in self.graph.nodes.iter_mut() {
            let events = entry.node.tick();
            for (port, event) in events {
                tick_events.push((node_id, port, event));
            }
        }

        // Inject tick events and process
        for (node, port, event) in tick_events {
            self.inject(node, port, event);
        }

        self.process()
    }

    /// Get a reference to the underlying graph
    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// Collect input states for a node from all incoming edges
    fn collect_input_states(&self, node: NodeId) -> HashMap<PortId, EdgeState> {
        let mut states = HashMap::new();

        for edge in &self.graph.edges {
            if edge.to_node == node {
                if let Some(state) = self
                    .graph
                    .edge_states
                    .get(&(edge.from_node, edge.from_port))
                {
                    states.insert(edge.to_port, state.clone());
                }
            }
        }

        states
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::event::{ButtonId, InputEvent};
    use crate::core::node::{EmptyState, Node, NodePorts, PortDef};

    /// Simple pass-through node for testing
    struct PassthroughNode;

    impl Node for PassthroughNode {
        fn name(&self) -> &str {
            "Passthrough"
        }

        fn ports(&self) -> NodePorts {
            NodePorts {
                inputs: vec![PortDef::new(PortId(0), "in")],
                outputs: vec![PortDef::new(PortId(0), "out")],
            }
        }

        fn process(
            &mut self,
            _port: PortId,
            event: CtrlEvent,
            _ctx: &mut ProcessContext,
        ) -> Vec<(PortId, CtrlEvent)> {
            // Pass through unchanged
            vec![(PortId(0), event)]
        }
    }

    #[test]
    fn test_single_node_graph() {
        let mut graph = Graph::new();
        let node = graph.add_node::<EmptyState>(Box::new(PassthroughNode));

        let mut executor = GraphExecutor::new(graph);

        let event = CtrlEvent::Input(InputEvent::button(ButtonId::South, true));
        executor.inject(node, PortId(0), event);

        let outputs = executor.process();
        assert_eq!(outputs.len(), 1);
    }

    #[test]
    fn test_connected_nodes() {
        let mut graph = Graph::new();
        let node1 = graph.add_node::<EmptyState>(Box::new(PassthroughNode));
        let node2 = graph.add_node::<EmptyState>(Box::new(PassthroughNode));

        graph.connect(node1, PortId(0), node2, PortId(0));

        let mut executor = GraphExecutor::new(graph);

        let event = CtrlEvent::Input(InputEvent::button(ButtonId::South, true));
        executor.inject(node1, PortId(0), event);

        let outputs = executor.process();
        // Event should exit from node2
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].0, node2);
    }
}
