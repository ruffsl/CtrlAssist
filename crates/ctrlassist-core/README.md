# ctrlassist-core

CtrlAssist's routing model: pad states in, changes on virtual pads out, and where feedback goes on the way back. No device, async or serialization code, and no dependencies.

- `control`: controls, pad state, groups and layouts.
- `mux`, `demux`: the two nodes. A mux mode is a table of merge rules per group kind.
- `graph`: validation, dependency order, and the step.
- `feedback`: the games' rumble effects, turned into one level per pad.

The model and the reasons for it are in [the architecture](../../docs/architecture.md#the-core-model). `examples/bench.rs` times a step.
