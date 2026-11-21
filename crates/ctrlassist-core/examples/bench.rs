//! Times one step of the graph. That a step allocates nothing is a test;
//! this only reports how long one takes.
//!
//! `cargo run --release -p ctrlassist-core --example bench`

#![expect(
    clippy::print_stdout,
    reason = "an example's job is to print its result"
)]

use std::time::Instant;

use ctrlassist_core::gamepad::*;
use ctrlassist_core::{
    DemuxConfig, DemuxMode, Graph, GraphSpec, Input, Layout, Mode, MuxConfig, NodeSpec, PadState,
    SourceSpec,
};

fn source() -> SourceSpec {
    SourceSpec {
        deadzone: 0.1,
        layout: Layout::gamepad(),
    }
}

fn measure(name: &str, mut graph: Graph, sources: usize) {
    const STEPS: usize = 2_000_000;
    let values = [0.25f32, 0.5, 0.75, 1.0, -0.25, -0.5, -0.75, -1.0];
    let mut changes = 0u64;
    let started = Instant::now();
    for step in 0..STEPS {
        let value = values[step % 8];
        let state = PadState::NEUTRAL
            .with(LEFT_X, value)
            .with(RIGHT_TRIGGER, value.abs())
            .with(SOUTH, (step % 2) as f32);
        graph.update(step % sources, state, &mut |_| changes += 1);
    }
    let nanos = started.elapsed().as_nanos() as f64 / STEPS as f64;
    println!("{name}: {nanos:.0} ns per step ({changes} changes in {STEPS} steps)");
}

fn main() {
    let layout = Layout::gamepad();
    let mux = |mode| {
        Graph::build(GraphSpec {
            sources: vec![source(), source()],
            nodes: vec![NodeSpec::Mux {
                inputs: vec![Input::Source(0), Input::Source(1)],
                config: MuxConfig::new(mode, &layout),
            }],
            sinks: vec![Input::Node(0, 0)],
        })
        .expect("the preset is a valid graph because it is written here")
    };
    measure("priority mux, two pads", mux(Mode::Priority), 2);
    measure("average mux, two pads", mux(Mode::Average), 2);
    measure("toggle mux, two pads", mux(Mode::Toggle), 2);
    measure(
        "one helper, two players (demux and two muxes)",
        Graph::build(GraphSpec {
            sources: vec![source(), source(), source()],
            nodes: vec![
                NodeSpec::Demux {
                    input: Input::Source(2),
                    config: DemuxConfig::new(DemuxMode::Unicast, 2),
                },
                NodeSpec::Mux {
                    inputs: vec![Input::Source(0), Input::Node(0, 0)],
                    config: MuxConfig::new(Mode::Priority, &layout),
                },
                NodeSpec::Mux {
                    inputs: vec![Input::Source(1), Input::Node(0, 1)],
                    config: MuxConfig::new(Mode::Priority, &layout),
                },
            ],
            sinks: vec![Input::Node(1, 0), Input::Node(2, 0)],
        })
        .expect("the preset is a valid graph because it is written here"),
        3,
    );
}
