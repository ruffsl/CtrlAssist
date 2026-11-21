# Spike: the routing graph

The routing model as a pure crate: no device, async or serialization code, and no dependencies. Unlike the other spikes this one is meant to be promoted, tests and all, into the core crate.

- `src/control.rs`: controls, pad state, groups and layouts.
- `src/mux.rs`, `src/demux.rs`: the two nodes. A mux mode is a table of merge rules per group.
- `src/graph.rs`: validation, dependency order, and the step that turns pad states into changes on the virtual pads.
- `src/feedback.rs`: where a virtual pad's feedback goes, and keeping each pad's effects in line with that.
- `tests/scenarios.rs`: the behavior, written before the nodes.
- `tests/properties.rs`: invariants under random sequences.
- `examples/bench.rs`: time per step, and a check that stepping allocates nothing.

`just spike-graph` runs the tests and the benchmark. The results are in `docs/architecture.md`, under "What the platform does".
