//! The routing model: pads in, virtual pads out, feedback back.
//!
//! Nothing here touches a device. A [`Graph`] is built from a [`GraphSpec`]
//! and stepped with pad states; it reports what each virtual pad should
//! change, and says which pads a virtual pad's feedback goes to. A
//! [`Router`] turns the games' rumble effects into one level per pad.
//!
//! The model is level-based: a node computes its output state from its
//! input states and a little state of its own, and the graph emits the
//! difference from what the virtual pad last held. That makes resyncs,
//! dropouts and switches the same computation as an ordinary event.

mod control;
mod demux;
mod feedback;
mod graph;
mod ids;
mod mux;

pub use control::{AXES, BUTTONS, Control, Group, Kind, Layout, PadState, gamepad};
pub use demux::{DemuxConfig, DemuxFeedback, DemuxMode};
pub use feedback::{EFFECTS, Effect, EffectKey, FeedbackError, Level, Router};
pub use graph::{
    Change, Graph, GraphError, GraphSpec, Input, MAX_INPUTS, MAX_NODES, MAX_SINKS, MAX_SOURCES,
    NodeSpec, SourceSpec,
};
pub use ids::{NodeId, SinkId, SourceId, SourceSet};
pub use mux::{Mode, MuxConfig, MuxFeedback, Rule, Switch};
