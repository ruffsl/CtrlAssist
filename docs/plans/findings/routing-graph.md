# Finding: the routing graph

Built 2026-11-21 as `spikes/graph/`: a crate with no dependencies, about 900 lines, plus 36 tests. The scenario tests were written and committed before any node existed. Reproduce with `just spike-graph`.

## Verdict

**The model is kept, and the open-ended vocabulary with it.** The spike becomes the core crate.

The plan's rule was: every scenario passes through the executor without a special case, a step allocates nothing, and its cost is below the input loop's measurement noise; and the non-gamepad scenario needs no change to node code. All four hold.

| Check | Result |
|---|---|
| Scenarios through the executor | 32 of 32, with no node bypassed and no scenario-specific code |
| Property tests | 4, each also run at 20,000 random cases |
| Allocations while stepping | 0 in 2,000,000 steps, counted by the benchmark's allocator |
| Cost of a step | 154–199 ns for a two-pad mux; 325 ns for a demux feeding two muxes |
| Next to the input loop | The loop's end-to-end median is 26,000–34,000 ns, so a step is under 1% of it |
| Non-gamepad source | A flight stick whose grip is one group of three axes and seven buttons merged correctly with no change to the nodes |

## The model

**Level-based, not event-based.** A node computes its output state from its input states and a little state of its own. The graph evaluates the nodes in dependency order and reports the difference between what each virtual pad should now hold and what it last held. This is the main departure from the earlier refactor branch, which passed events between ports, and it is why several things the plan worried about stopped being separate cases:

- **A switch, a dropout, a reconnect and a resync are the same computation as an ordinary event.** The hand-off rule the maintainer chose (the virtual pad takes the incoming pad's state) is not code; it falls out of selecting a different input and diffing. The test that the trigger steps from 0.8 to 0.3 without passing through zero passes because nothing ever emits the intermediate state.
- **Nothing can stick.** A pad that disconnects reads as at rest, and the diff releases whatever only it held.
- **Report frames are natural.** One update yields the changes for one frame.

**Controls** are a kind and an index (`Button(n)`, `Axis(n)`), with the gamepad names as constants over that space. A pad state is a fixed-size value: 64 buttons as a bit set and 16 axes. That is what keeps a step allocation-free, and it is a capacity, not a vocabulary: raising it changes two constants.

**Groups and rules.** A layout arranges a pad's controls into groups, each with a kind (stick, trigger, button, hat). A mux applies one rule per group:

| Rule | Meaning |
|---|---|
| owned | the highest input active on the group owns every member; with none active, the first input |
| largest | each member takes the input furthest from rest; a button is down if anyone holds it |
| blended | each axis is the mean of the inputs active on the group |
| switched | the input the switch last selected owns the group |

and a mode is a table from kind to rule:

| Mode | Stick | Trigger | Button | Hat |
|---|---|---|---|---|
| priority | owned | largest | owned | owned |
| average | blended | blended | largest | largest |
| toggle | switched | switched | switched | switched |

The three modes are those three lines; there is no per-mode code. Legacy's separate cases (per button, per stick pair, triggers take the larger) are the priority row. A mux takes any number of inputs, later ones outranking earlier, so a second helper needs nothing new.

**Activity** is as decided: a source's deadzone zeroes a resting group before any node sees it, and a mux marks an input active on a group at the engage threshold and inactive below the release threshold.

**The switch** is a signal into a node. A configured button raises it on its press, once, and is removed from what flows on; a client raises the same signal through one call.

**Demux** has two modes with no table: the selected output follows the input and the others rest, or all follow.

## Feedback

Feedback is not input run backward, and the spike keeps it separate:

- **The graph answers one question:** which pads does this virtual pad's feedback go to, right now? It walks back from the sink: a mux passes it to a fixed set of inputs, to the inputs that can currently reach the output, or to none; a demux passes it only from outputs the input currently reaches. The answer changes when a switch does, and only then.
- **A router keeps the pads in line with the answer.** It turns a game's requests (upload, play, stop, erase) into commands per pad, and when the targets change it moves the effects: uploaded and started on a pad that joins, stopped and removed on one that leaves. So in toggle mode a playing rumble follows the hand-off, the bug the earlier refactor only fixed with a special case.

The cases the plan listed:

| Case | Outcome |
|---|---|
| Rumble to a pad not providing input | A mux's feedback setting is independent of its rules: the primary drives, the assist feels it |
| A pad rejecting an upload | The game is told the effect is accepted if any pad took it, and refused only if every pad asked refused it; a pad that refused is left alone afterward |
| Rumble off | The game is told accepted and nothing is sent, so a game that insists on rumble still runs |
| Two virtual pads driving one pad | An effect is keyed by virtual pad and the game's number, so two games' "effect 0" don't collide; each pad's kernel driver combines what is playing |
| A pad dropping out and returning | Nothing is sent while it is gone; on return it gets every effect meant for it, started if playing |
| One helper moving between two players | The helper's pad receives rumble from whichever player's game they are currently assisting |

**A bug the property test found.** Re-uploading an effect that is already playing, when a pad that had refused it now accepts, left that pad holding the effect but silent. The fix is that confirming an upload starts the effect on any pad that newly holds it while the game believes it is playing. No scenario I had written would have caught it.

**Not modeled yet:** gain and autocenter; a pad's limit on the number of effects (a pad refusing for lack of room is handled as any refusal, but the router doesn't budget slots); and the pass-through of whole output reports between like pads, which uses the same targets but is opaque bytes the device crate forwards.

## Profile format, drafted

Not implemented in the spike; drafted from what the graph needed to be told. A preset is a profile whose graph is implied.

The assist preset, with nothing overridden:

```toml
preset = "assist"            # two pads, one priority mux, one virtual pad
primary = "0c:27:56:4f:bd:14"
assist  = "1c:96:5a:a1:35:da"
```

Taking turns, with the switch moved and rumble sent to both:

```toml
preset = "toggle"
primary = "0c:27:56:4f:bd:14"
assist  = "1c:96:5a:a1:35:da"

[mux]
switch   = { button = "select", on = "primary" }
feedback = ["primary", "assist"]
```

One pad split across two players' muxes:

```toml
preset = "split"
pad = "1c:96:5a:a1:35:da"
virtual-pads = 2
```

A hand-written graph, the legacy cookbook's helper for two players:

```toml
[source.one]
pad = "0c:27:56:4f:bd:14"

[source.two]
pad = "a4:5e:60:c1:22:07"

[source.helper]
pad = "1c:96:5a:a1:35:da"
deadzone = 0.15                    # this one's stick is worn

[node.share]
demux = "unicast"
input = "helper"
outputs = 2

[node.first]
mux = "priority"
inputs = ["one", "share.0"]        # later inputs outrank earlier ones

[node.second]
mux = "priority"
inputs = ["two", "share.1"]
engage = 0.2
release = 0.1

[sink.player-one]
input = "first"
identity = "generic"

[sink.player-two]
input = "second"
identity = "xbox"
auto-start = true
```

Groups appear only when a pad isn't a gamepad or a mode's table is overridden:

```toml
[source.stick.layout]
grip     = { kind = "stick",   axes = [0, 1, 2], buttons = [0, 1, 2, 3, 4, 5, 6] }
throttle = { kind = "trigger", axes = [3] }

[node.first.rules]
trigger = "blended"                # priority everywhere else
```

Open in the format: how pads are named when a profile is shared between machines (an address is specific to one pad), and whether a preset's implied graph should be shown expanded when a profile is saved.

## What this changes in the plan

- The core crate is this spike, promoted: its vocabulary, nodes, graph, router and tests.
- "Feedback flows backward along the same edges" is replaced by: the graph says where a sink's feedback goes, and a router moves effects to match.
- The hand-off, dropout and resync rules need no code of their own; they are consequences of the level-based step and are pinned by tests.
- Mux modes are tables, so a per-group override in a profile is nearly free, and a custom mode needs no code (#17).
- A mux takes any number of inputs.
- The device crate's job toward the core is narrow: turn evdev events into pad states per frame, apply the changes a step reports, and execute the router's commands, reporting which pads accepted an upload.
