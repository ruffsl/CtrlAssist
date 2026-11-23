//! The graph: sources, nodes and sinks, validated once and then stepped.

use crate::control::{AXES, BUTTONS, Control, Layout, PadState};
use crate::demux::{Demux, DemuxConfig};
use crate::ids::{NodeId, SinkId, SourceId, SourceSet};
use crate::mux::{Mux, MuxConfig};

/// The most inputs one mux may have.
pub const MAX_INPUTS: usize = 8;
/// The most pads one graph may have: feedback targets are a 32-bit mask.
pub const MAX_SOURCES: usize = SourceSet::CAPACITY;
/// The most nodes one graph may have: two for each pad it may have, a
/// demux to share it out and a mux to merge it in, which is more than
/// any arrangement of people at one machine needs. The input thread
/// steps every node on every report, so the cap bounds that work.
pub const MAX_NODES: usize = 2 * MAX_SOURCES;
/// The most virtual pads one graph may have: as many as pads. Each is a
/// device the kernel and every program reading pads take on, and the
/// kernel has only so many input device numbers for the whole machine.
pub const MAX_SINKS: usize = MAX_SOURCES;

/// Where a node or sink takes its state from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Input {
    /// A pad.
    Source(SourceId),
    /// A node's output: the node, then which of its outputs.
    Node(NodeId, usize),
}

/// A pad as the graph sees it.
#[derive(Clone, PartialEq, Debug)]
pub struct SourceSpec {
    /// Stick-like groups resting inside this are zeroed before any node
    /// sees them.
    pub deadzone: f32,
    /// The pad's groups.
    pub layout: Layout,
}

/// A node and what feeds it.
#[derive(Clone, PartialEq, Debug)]
pub enum NodeSpec {
    /// Several inputs to one output. Later inputs outrank earlier ones.
    Mux {
        inputs: Vec<Input>,
        config: MuxConfig,
    },
    /// One input to several outputs.
    Demux { input: Input, config: DemuxConfig },
}

/// A whole graph, as a profile describes it.
#[derive(Clone, PartialEq, Debug)]
pub struct GraphSpec {
    /// The pads.
    pub sources: Vec<SourceSpec>,
    /// The nodes, in any order.
    pub nodes: Vec<NodeSpec>,
    /// What each virtual pad follows.
    pub sinks: Vec<Input>,
}

/// Why a graph was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum GraphError {
    /// A node depends on its own output.
    Cycle,
    /// An input names a source, node or output that doesn't exist.
    BadReference,
    /// Too many sources, nodes or sinks, or a node with no inputs or
    /// outputs or too many.
    BadArity,
    /// A control beyond a pad state's capacity, or a switch that isn't a
    /// button on one of the node's inputs.
    BadControl,
    /// A deadzone that isn't a number or is negative, or thresholds that
    /// aren't `0 < release <= engage <= 1`.
    BadThreshold,
}

impl std::fmt::Display for GraphError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Cycle => "a node depends on its own output",
            Self::BadReference => "an input names a source, node or output that doesn't exist",
            Self::BadArity => {
                "too many sources, nodes or sinks, or a node has no or too many inputs or outputs"
            }
            Self::BadControl => {
                "a control is out of range, or a switch is not a button on one of its node's inputs"
            }
            Self::BadThreshold => {
                "a deadzone is not a number or is negative, or thresholds are not 0 < release <= engage <= 1"
            }
        })
    }
}

impl std::error::Error for GraphError {}

/// One control changing on one virtual pad.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Change {
    /// The virtual pad.
    pub sink: SinkId,
    /// The control.
    pub control: Control,
    /// Its new value.
    pub value: f32,
}

enum Node {
    Mux(Mux),
    Demux(Demux),
}

/// A validated graph and its running state. Stepping it allocates nothing.
pub struct Graph {
    sources: Vec<SourceSpec>,
    states: Vec<PadState>,
    present: Vec<bool>,
    nodes: Vec<Node>,
    inputs: Vec<Vec<Input>>,
    /// Where each node's outputs start in `outputs`.
    offsets: Vec<usize>,
    /// Node indices in dependency order.
    order: Vec<usize>,
    outputs: Vec<PadState>,
    /// The pads each node output's feedback goes to, beside `outputs`,
    /// worked out as each node steps.
    reach: Vec<SourceSet>,
    sinks: Vec<Input>,
    sink_states: Vec<PadState>,
}

impl std::fmt::Debug for Graph {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Graph")
            .field("sources", &self.sources.len())
            .field("nodes", &self.nodes.len())
            .field("sinks", &self.sinks.len())
            .finish_non_exhaustive()
    }
}

/// An input's state, and whether what feeds it is connected.
fn resolve(
    input: Input,
    states: &[PadState],
    present: &[bool],
    offsets: &[usize],
    outputs: &[PadState],
) -> (PadState, bool) {
    match input {
        Input::Source(SourceId(source)) if present[source] => (states[source], true),
        Input::Source(_) => (PadState::NEUTRAL, false),
        Input::Node(NodeId(node), output) => (outputs[offsets[node] + output], true),
    }
}

/// The pads an input's feedback goes to, from the nodes already stepped.
fn reach_of(input: Input, offsets: &[usize], reach: &[SourceSet]) -> SourceSet {
    match input {
        Input::Source(source) => source.into(),
        Input::Node(NodeId(node), output) => reach[offsets[node] + output],
    }
}

impl Graph {
    /// Validates a spec and builds its running state.
    ///
    /// # Errors
    ///
    /// Returns why the spec can't run. Everything a step relies on is
    /// checked here, so that no spec this accepts can make a step panic.
    pub fn build(spec: GraphSpec) -> Result<Self, GraphError> {
        // Before anything is walked or copied, so a spec of any size is
        // refused at the cost of reading three lengths.
        if spec.sources.len() > MAX_SOURCES
            || spec.nodes.len() > MAX_NODES
            || spec.sinks.len() > MAX_SINKS
        {
            return Err(GraphError::BadArity);
        }
        let output_counts: Vec<usize> = spec
            .nodes
            .iter()
            .map(|node| match node {
                NodeSpec::Mux { .. } => 1,
                NodeSpec::Demux { config, .. } => config.outputs,
            })
            .collect();
        let inputs: Vec<Vec<Input>> = spec
            .nodes
            .iter()
            .map(|node| match node {
                NodeSpec::Mux { inputs, .. } => inputs.clone(),
                NodeSpec::Demux { input, .. } => vec![*input],
            })
            .collect();

        let valid = |input: &Input| match *input {
            Input::Source(SourceId(source)) => source < spec.sources.len(),
            Input::Node(NodeId(node), output) => {
                output_counts.get(node).is_some_and(|count| output < *count)
            }
        };
        if !inputs.iter().flatten().chain(&spec.sinks).all(valid) {
            return Err(GraphError::BadReference);
        }
        if inputs
            .iter()
            .any(|inputs| inputs.is_empty() || inputs.len() > MAX_INPUTS)
        {
            return Err(GraphError::BadArity);
        }
        if spec
            .sources
            .iter()
            .any(|source| !(source.deadzone.is_finite() && source.deadzone >= 0.0))
        {
            return Err(GraphError::BadThreshold);
        }
        for node in &spec.nodes {
            match node {
                NodeSpec::Mux { inputs, config } => Mux::validate(config, inputs.len())?,
                NodeSpec::Demux { config, .. } => Demux::validate(config)?,
            }
        }

        // Dependency order: repeatedly take the nodes whose inputs are all
        // sources or nodes already taken. Anything left over is on a cycle.
        let mut order = Vec::with_capacity(spec.nodes.len());
        let mut placed = vec![false; spec.nodes.len()];
        while order.len() < spec.nodes.len() {
            let before = order.len();
            for node in 0..spec.nodes.len() {
                let ready = inputs[node].iter().all(|input| match *input {
                    Input::Source(_) => true,
                    Input::Node(NodeId(dependency), _) => placed[dependency],
                });
                if !placed[node] && ready {
                    order.push(node);
                }
            }
            for &node in &order[before..] {
                placed[node] = true;
            }
            if order.len() == before {
                return Err(GraphError::Cycle);
            }
        }

        let mut offsets = Vec::with_capacity(spec.nodes.len());
        let mut total = 0;
        for count in &output_counts {
            offsets.push(total);
            total += count;
        }
        let nodes = spec
            .nodes
            .into_iter()
            .map(|node| match node {
                NodeSpec::Mux { inputs, config } => Node::Mux(Mux::new(config, inputs.len())),
                NodeSpec::Demux { config, .. } => Node::Demux(Demux::new(config)),
            })
            .collect();

        let mut graph = Self {
            states: vec![PadState::NEUTRAL; spec.sources.len()],
            present: vec![true; spec.sources.len()],
            sources: spec.sources,
            nodes,
            inputs,
            offsets,
            order,
            outputs: vec![PadState::NEUTRAL; total],
            reach: vec![SourceSet::EMPTY; total],
            sink_states: vec![PadState::NEUTRAL; spec.sinks.len()],
            sinks: spec.sinks,
        };
        graph.evaluate(&mut |_| {});
        Ok(graph)
    }

    /// Gives a pad's new state and reports what changes on the virtual
    /// pads. A source that doesn't exist is ignored.
    pub fn update(&mut self, source: SourceId, mut state: PadState, emit: &mut impl FnMut(Change)) {
        let source = source.0;
        let Some(spec) = self.sources.get(source) else {
            return;
        };
        spec.layout.apply_deadzone(&mut state, spec.deadzone);
        self.states[source] = state;
        self.evaluate(emit);
    }

    /// Marks a pad as gone or back. A pad that is gone reads as at rest,
    /// so nothing it held stays held. A source that doesn't exist is
    /// ignored.
    pub fn set_present(&mut self, source: SourceId, present: bool, emit: &mut impl FnMut(Change)) {
        let source = source.0;
        let Some(slot) = self.present.get_mut(source) else {
            return;
        };
        *slot = present;
        if !present {
            self.states[source] = PadState::NEUTRAL;
        }
        self.evaluate(emit);
    }

    /// Raises a node's switch, as its button would. A node that doesn't
    /// exist is ignored.
    pub fn switch(&mut self, node: NodeId, emit: &mut impl FnMut(Change)) {
        match self.nodes.get_mut(node.0) {
            Some(Node::Mux(mux)) => mux.switch(),
            Some(Node::Demux(demux)) => demux.switch(),
            None => return,
        }
        self.evaluate(emit);
    }

    /// Selects a mux's input or a demux's output by index, as a client
    /// asks. A repeated request, or one out of range, changes nothing.
    pub fn select(&mut self, node: NodeId, index: usize, emit: &mut impl FnMut(Change)) {
        match self.nodes.get_mut(node.0) {
            Some(Node::Mux(mux)) => mux.select(index),
            Some(Node::Demux(demux)) => demux.select(index),
            None => return,
        }
        self.evaluate(emit);
    }

    /// How many nodes the graph has.
    #[must_use]
    pub fn nodes(&self) -> usize {
        self.nodes.len()
    }

    /// How many pads the graph has.
    #[must_use]
    pub fn sources(&self) -> usize {
        self.sources.len()
    }

    /// How many virtual pads the graph has.
    #[must_use]
    pub fn sinks(&self) -> usize {
        self.sinks.len()
    }

    /// Which input a mux has selected, or which output a demux has. None
    /// for a node that doesn't select, one that blends its inputs or
    /// sends to every output, and for a node that doesn't exist.
    #[must_use]
    pub fn selected(&self, node: NodeId) -> Option<usize> {
        match self.nodes.get(node.0)? {
            Node::Mux(mux) => mux.selected(),
            Node::Demux(demux) => demux.selected(),
        }
    }

    /// What a virtual pad currently holds; a pad at rest for a sink that
    /// doesn't exist.
    #[must_use]
    pub fn sink(&self, sink: SinkId) -> &PadState {
        self.sink_states.get(sink.0).unwrap_or(&PadState::NEUTRAL)
    }

    /// The pads a virtual pad's feedback goes to; none for a sink that
    /// doesn't exist. Each step works this out for every node's outputs,
    /// so asking costs nothing however the graph is wired.
    #[must_use]
    pub fn feedback_targets(&self, sink: SinkId) -> SourceSet {
        self.sinks.get(sink.0).map_or(SourceSet::EMPTY, |input| {
            reach_of(*input, &self.offsets, &self.reach)
        })
    }

    fn evaluate(&mut self, emit: &mut impl FnMut(Change)) {
        let Self {
            states,
            present,
            nodes,
            inputs,
            offsets,
            order,
            outputs,
            reach,
            sinks,
            sink_states,
            ..
        } = self;
        for &node in order.iter() {
            let mut gathered = [PadState::NEUTRAL; MAX_INPUTS];
            let count = inputs[node].len();
            let mut connected = 0u32;
            for (index, (slot, input)) in gathered.iter_mut().zip(&inputs[node]).enumerate() {
                let (state, is_present) = resolve(*input, states, present, offsets, outputs);
                *slot = state;
                connected |= u32::from(is_present) << index;
            }
            match &mut nodes[node] {
                Node::Mux(mux) => {
                    let mut out = PadState::NEUTRAL;
                    mux.step(&mut gathered[..count], connected, &mut out);
                    outputs[offsets[node]] = out;
                }
                Node::Demux(demux) => {
                    let end = offsets.get(node + 1).copied().unwrap_or(outputs.len());
                    demux.step(gathered[0], &mut outputs[offsets[node]..end]);
                }
            }
            // Where this node's feedback goes, from where its inputs'
            // goes. The order steps every input first, so each node
            // output is worked out once, and this is linear in the
            // graph's size however many ways a pad reaches a sink.
            let start = offsets[node];
            match &nodes[node] {
                Node::Mux(mux) => {
                    let ports = mux.feedback_inputs();
                    let mut targets = SourceSet::EMPTY;
                    for (port, input) in inputs[node].iter().enumerate() {
                        if ports & (1 << port) != 0 {
                            targets |= reach_of(*input, offsets, reach);
                        }
                    }
                    reach[start] = targets;
                }
                Node::Demux(demux) => {
                    let end = offsets.get(node + 1).copied().unwrap_or(reach.len());
                    let from = reach_of(inputs[node][0], offsets, reach);
                    for (output, slot) in reach[start..end].iter_mut().enumerate() {
                        *slot = if demux.feedback_from(output) {
                            from
                        } else {
                            SourceSet::EMPTY
                        };
                    }
                }
            }
        }
        for (sink, input) in sinks.iter().enumerate() {
            let (new, _) = resolve(*input, states, present, offsets, outputs);
            let old = &mut sink_states[sink];
            let changed = old.buttons() ^ new.buttons();
            for index in 0..BUTTONS {
                if changed & (1 << index) != 0 {
                    let control = Control::Button(index as u8);
                    emit(Change {
                        sink: SinkId(sink),
                        control,
                        value: new.get(control),
                    });
                }
            }
            for index in 0..AXES {
                if old.axes()[index] != new.axes()[index] {
                    emit(Change {
                        sink: SinkId(sink),
                        control: Control::Axis(index as u8),
                        value: new.axes()[index],
                    });
                }
            }
            *old = new;
        }
    }
}
