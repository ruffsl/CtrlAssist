//! Times one step of the graph and checks that stepping allocates nothing.

use std::alloc::{GlobalAlloc, Layout as AllocLayout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use spike_graph::gamepad::*;
use spike_graph::{
    DemuxConfig, DemuxMode, Graph, GraphSpec, Input, Layout, Mode, MuxConfig, NodeSpec, PadState,
    SourceSpec,
};

struct Counting;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

// SAFETY: every call is forwarded unchanged to the system allocator, which
// upholds the GlobalAlloc contract; the counter has no effect on memory.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: AllocLayout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the caller's obligations are passed straight through.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: AllocLayout) {
        // SAFETY: the caller's obligations are passed straight through.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn source() -> SourceSpec {
    SourceSpec {
        deadzone: 0.1,
        layout: Layout::gamepad(),
    }
}

fn measure(name: &str, mut graph: Graph, sources: usize) {
    const STEPS: u64 = 2_000_000;
    let values = [0.25f32, 0.5, 0.75, 1.0, -0.25, -0.5, -0.75, -1.0];
    let mut changes = 0u64;
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    let started = Instant::now();
    for step in 0..STEPS {
        let value = values[(step % 8) as usize];
        let state = PadState::NEUTRAL
            .with(LEFT_X, value)
            .with(RIGHT_TRIGGER, value.abs())
            .with(SOUTH, (step % 2) as f32);
        graph.update((step as usize) % sources, state, &mut |_| changes += 1);
    }
    let elapsed = started.elapsed();
    let allocations = ALLOCATIONS.load(Ordering::Relaxed) - before;
    println!(
        "{name}: {:.0} ns per step, {allocations} allocations in {STEPS} steps, {changes} changes",
        elapsed.as_nanos() as f64 / STEPS as f64,
    );
    assert_eq!(allocations, 0, "stepping the graph must not allocate");
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
