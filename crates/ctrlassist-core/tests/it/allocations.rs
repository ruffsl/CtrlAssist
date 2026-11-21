//! Stepping a graph must not allocate: the step runs on the input thread.
//!
//! The counter is per thread, so the test holds whether tests run as
//! threads of one process or each in its own.

#![expect(
    unsafe_code,
    reason = "a counting global allocator can only be written with unsafe"
)]

use std::alloc::{GlobalAlloc, Layout as AllocLayout, System};
use std::cell::Cell;

use ctrlassist_core::gamepad::*;
use ctrlassist_core::{
    DemuxConfig, DemuxMode, Graph, GraphSpec, Input, Layout, Mode, MuxConfig, NodeId, NodeSpec,
    PadState, SourceId, SourceSpec,
};

thread_local! {
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

struct Counting;

// SAFETY: every call is forwarded unchanged to the system allocator, which
// upholds the GlobalAlloc contract. The counter is a thread-local `Cell`
// with a const initializer and no destructor, so touching it never
// allocates and never runs during thread teardown.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: AllocLayout) -> *mut u8 {
        ALLOCATIONS.with(|count| count.set(count.get() + 1));
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

/// One helper assisting two players: a demux feeding two muxes.
fn shared_helper() -> Graph {
    let layout = Layout::gamepad();
    Graph::build(GraphSpec {
        sources: vec![source(), source(), source()],
        nodes: vec![
            NodeSpec::Demux {
                input: Input::Source(SourceId(2)),
                config: DemuxConfig::new(DemuxMode::Unicast, 2),
            },
            NodeSpec::Mux {
                inputs: vec![Input::Source(SourceId(0)), Input::Node(NodeId(0), 0)],
                config: MuxConfig::new(Mode::Priority, &layout),
            },
            NodeSpec::Mux {
                inputs: vec![Input::Source(SourceId(1)), Input::Node(NodeId(0), 1)],
                config: MuxConfig::new(Mode::Average, &layout),
            },
        ],
        sinks: vec![Input::Node(NodeId(1), 0), Input::Node(NodeId(2), 0)],
    })
    .unwrap()
}

#[test]
fn stepping_a_graph_allocates_nothing() {
    let mut graph = shared_helper();
    let values = [0.25f32, 0.5, 0.75, 1.0, -0.25, -0.5, -0.75, -1.0];
    let mut changes = 0u64;

    let before = ALLOCATIONS.with(Cell::get);
    for step in 0..10_000usize {
        let value = values[step % 8];
        let state = PadState::NEUTRAL
            .with(LEFT_X, value)
            .with(RIGHT_TRIGGER, value.abs())
            .with(SOUTH, (step % 2) as f32)
            .with(MODE, ((step / 100) % 2) as f32);
        graph.update(SourceId(step % 3), state, &mut |_| changes += 1);
        if step % 1000 == 0 {
            graph.set_present(SourceId(1), step % 2000 == 0, &mut |_| changes += 1);
            graph.switch(NodeId(0), &mut |_| changes += 1);
        }
    }
    let allocations = ALLOCATIONS.with(Cell::get) - before;

    assert!(changes > 10_000, "the steps did real work");
    assert_eq!(allocations, 0);
}
