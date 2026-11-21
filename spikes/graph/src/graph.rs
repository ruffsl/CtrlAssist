//! The graph: sources, nodes and sinks, validated once and then stepped.

use crate::control::{AXES, BUTTONS, Control, Layout, PadState};
use crate::demux::{Demux, DemuxConfig};
use crate::mux::{Mux, MuxConfig};

/// The most inputs one node may have.
const MAX_INPUTS: usize = 8;

/// Where a node or sink takes its state from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Input {
    /// A pad, by index.
    Source(usize),
    /// A node's output: the node's index, then which of its outputs.
    Node(usize, usize),
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
    Mux { inputs: Vec<Input>, config: MuxConfig },
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
    /// A node has no inputs or outputs, or more inputs than supported.
    BadArity,
}

/// One control changing on one virtual pad.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Change {
    /// The virtual pad.
    pub sink: usize,
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

fn resolve(input: Input, states: &[PadState], present: &[bool], offsets: &[usize], outputs: &[PadState]) -> PadState {
    match input {
        Input::Source(source) if present[source] => states[source],
        Input::Source(_) => PadState::NEUTRAL,
        Input::Node(node, output) => outputs[offsets[node] + output],
    }
}

impl Graph {
    /// Validates a spec and builds its running state.
    ///
    /// # Errors
    ///
    /// Returns why the spec can't run: a dangling reference, a node with
    /// an unusable number of inputs or outputs, or a cycle.
    pub fn build(spec: GraphSpec) -> Result<Self, GraphError> {
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
            Input::Source(source) => source < spec.sources.len(),
            Input::Node(node, output) => output_counts.get(node).is_some_and(|count| output < *count),
        };
        if !inputs.iter().flatten().chain(&spec.sinks).all(valid) {
            return Err(GraphError::BadReference);
        }
        if inputs.iter().any(|inputs| inputs.is_empty() || inputs.len() > MAX_INPUTS)
            || output_counts.contains(&0)
        {
            return Err(GraphError::BadArity);
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
                    Input::Node(dependency, _) => placed[dependency],
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
            sink_states: vec![PadState::NEUTRAL; spec.sinks.len()],
            sinks: spec.sinks,
        };
        graph.evaluate(&mut |_| {});
        Ok(graph)
    }

    /// Gives a pad's new state and reports what changes on the virtual pads.
    pub fn update(&mut self, source: usize, mut state: PadState, emit: &mut impl FnMut(Change)) {
        let spec = &self.sources[source];
        spec.layout.apply_deadzone(&mut state, spec.deadzone);
        self.states[source] = state;
        self.evaluate(emit);
    }

    /// Marks a pad as gone or back. A pad that is gone reads as at rest,
    /// so nothing it held stays held.
    pub fn set_present(&mut self, source: usize, present: bool, emit: &mut impl FnMut(Change)) {
        self.present[source] = present;
        if !present {
            self.states[source] = PadState::NEUTRAL;
        }
        self.evaluate(emit);
    }

    /// Raises a node's switch, as a client asking would.
    pub fn switch(&mut self, node: usize, emit: &mut impl FnMut(Change)) {
        match &mut self.nodes[node] {
            Node::Mux(mux) => mux.switch(),
            Node::Demux(demux) => demux.switch(),
        }
        self.evaluate(emit);
    }

    /// What a virtual pad currently holds.
    #[must_use]
    pub fn sink(&self, sink: usize) -> &PadState {
        &self.sink_states[sink]
    }

    /// The pads a virtual pad's feedback goes to, as a mask over sources.
    #[must_use]
    pub fn feedback_targets(&self, sink: usize) -> u32 {
        self.targets_of(self.sinks[sink])
    }

    fn targets_of(&self, input: Input) -> u32 {
        match input {
            Input::Source(source) => 1 << source,
            Input::Node(node, output) => match &self.nodes[node] {
                Node::Mux(mux) => {
                    let ports = mux.feedback_inputs();
                    let mut targets = 0;
                    for (port, input) in self.inputs[node].iter().enumerate() {
                        if ports & (1 << port) != 0 {
                            targets |= self.targets_of(*input);
                        }
                    }
                    targets
                }
                Node::Demux(demux) if demux.feedback_from(output) => self.targets_of(self.inputs[node][0]),
                Node::Demux(_) => 0,
            },
        }
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
            sinks,
            sink_states,
            ..
        } = self;
        for &node in order.iter() {
            let mut gathered = [PadState::NEUTRAL; MAX_INPUTS];
            let count = inputs[node].len();
            for (slot, input) in gathered.iter_mut().zip(&inputs[node]) {
                *slot = resolve(*input, states, present, offsets, outputs);
            }
            match &mut nodes[node] {
                Node::Mux(mux) => {
                    let mut out = PadState::NEUTRAL;
                    mux.step(&mut gathered[..count], &mut out);
                    outputs[offsets[node]] = out;
                }
                Node::Demux(demux) => {
                    let end = offsets.get(node + 1).copied().unwrap_or(outputs.len());
                    demux.step(gathered[0], &mut outputs[offsets[node]..end]);
                }
            }
        }
        for (sink, input) in sinks.iter().enumerate() {
            let new = resolve(*input, states, present, offsets, outputs);
            let old = &mut sink_states[sink];
            let changed = old.buttons() ^ new.buttons();
            for index in 0..BUTTONS {
                if changed & (1 << index) != 0 {
                    let control = Control::Button(index as u8);
                    emit(Change {
                        sink,
                        control,
                        value: new.get(control),
                    });
                }
            }
            for index in 0..AXES {
                if old.axes()[index] != new.axes()[index] {
                    emit(Change {
                        sink,
                        control: Control::Axis(index as u8),
                        value: new.axes()[index],
                    });
                }
            }
            *old = new;
        }
    }
}
