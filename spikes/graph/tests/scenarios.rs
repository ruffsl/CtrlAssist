//! The routing behavior, written as scenarios before the nodes existed.
//!
//! Each test builds a graph the way a profile would, steps it with pad
//! states, and checks what the virtual pads hold.

#![expect(clippy::unwrap_used, reason = "tests fail loudly")]

use spike_graph::gamepad::*;
use spike_graph::{
    Command, Control, DemuxConfig, DemuxFeedback, DemuxMode, Effect, EffectKey, Graph, GraphError,
    GraphSpec, Input, Kind, Layout, Mode, MuxConfig, MuxFeedback, NodeSpec, PadState, Reply,
    Router, SourceSpec, Switch,
};

const PRIMARY: usize = 0;
const ASSIST: usize = 1;

fn source() -> SourceSpec {
    SourceSpec {
        deadzone: 0.1,
        layout: Layout::gamepad(),
    }
}

/// Two pads into one mux into one virtual pad: the assist preset.
fn mux_graph(config: MuxConfig) -> Graph {
    Graph::build(GraphSpec {
        sources: vec![source(), source()],
        nodes: vec![NodeSpec::Mux {
            inputs: vec![Input::Source(PRIMARY), Input::Source(ASSIST)],
            config,
        }],
        sinks: vec![Input::Node(0, 0)],
    })
    .unwrap()
}

fn mux(mode: Mode) -> Graph {
    mux_graph(MuxConfig::new(mode, &Layout::gamepad()))
}

/// One pad into one demux into `outputs` virtual pads: the split preset.
fn demux(mode: DemuxMode, outputs: usize) -> Graph {
    Graph::build(GraphSpec {
        sources: vec![source()],
        nodes: vec![NodeSpec::Demux {
            input: Input::Source(0),
            config: DemuxConfig::new(mode, outputs),
        }],
        sinks: (0..outputs).map(|port| Input::Node(0, port)).collect(),
    })
    .unwrap()
}

fn step(graph: &mut Graph, source: usize, state: PadState) {
    graph.update(source, state, &mut |_| {});
}

fn pad() -> PadState {
    PadState::NEUTRAL
}

// --- priority ---------------------------------------------------------------

#[test]
fn priority_passes_the_primary_through_while_the_assist_rests() {
    let mut graph = mux(Mode::Priority);
    step(&mut graph, PRIMARY, pad().with(SOUTH, 1.0).with(LEFT_X, 0.8));
    assert_eq!(graph.sink(0).get(SOUTH), 1.0);
    assert_eq!(graph.sink(0).get(LEFT_X), 0.8);
}

#[test]
fn priority_gives_a_stick_to_the_assist_as_a_whole() {
    let mut graph = mux(Mode::Priority);
    step(&mut graph, PRIMARY, pad().with(LEFT_X, 0.9).with(LEFT_Y, 0.9));
    // The assist nudges only X; it takes Y too, so the output is the
    // assist's direction, not a blend of two people's.
    step(&mut graph, ASSIST, pad().with(LEFT_X, -0.5));
    assert_eq!(graph.sink(0).get(LEFT_X), -0.5);
    assert_eq!(graph.sink(0).get(LEFT_Y), 0.0);
    // The other stick is untouched by the assist and stays the primary's.
    step(&mut graph, PRIMARY, pad().with(LEFT_X, 0.9).with(LEFT_Y, 0.9).with(RIGHT_X, 0.4));
    assert_eq!(graph.sink(0).get(RIGHT_X), 0.4);
    assert_eq!(graph.sink(0).get(LEFT_X), -0.5);
}

#[test]
fn priority_returns_a_stick_when_the_assist_lets_go() {
    let mut graph = mux(Mode::Priority);
    step(&mut graph, PRIMARY, pad().with(LEFT_X, 0.9));
    step(&mut graph, ASSIST, pad().with(LEFT_X, -0.5));
    step(&mut graph, ASSIST, pad());
    assert_eq!(graph.sink(0).get(LEFT_X), 0.9);
}

#[test]
fn priority_holds_a_button_for_the_assist_whatever_the_primary_does() {
    let mut graph = mux(Mode::Priority);
    step(&mut graph, ASSIST, pad().with(SOUTH, 1.0));
    step(&mut graph, PRIMARY, pad().with(SOUTH, 1.0));
    step(&mut graph, PRIMARY, pad());
    assert_eq!(graph.sink(0).get(SOUTH), 1.0, "the primary's release must not reach the game");
    step(&mut graph, ASSIST, pad());
    assert_eq!(graph.sink(0).get(SOUTH), 0.0);
}

#[test]
fn priority_takes_the_larger_trigger() {
    let mut graph = mux(Mode::Priority);
    step(&mut graph, PRIMARY, pad().with(RIGHT_TRIGGER, 0.8));
    step(&mut graph, ASSIST, pad().with(RIGHT_TRIGGER, 0.3));
    assert_eq!(graph.sink(0).get(RIGHT_TRIGGER), 0.8);
    step(&mut graph, ASSIST, pad().with(RIGHT_TRIGGER, 1.0));
    assert_eq!(graph.sink(0).get(RIGHT_TRIGGER), 1.0);
}

#[test]
fn an_owner_does_not_flicker_at_the_threshold() {
    let mut graph = mux(Mode::Priority);
    step(&mut graph, PRIMARY, pad().with(LEFT_X, 0.9));
    step(&mut graph, ASSIST, pad().with(LEFT_Y, 0.2));
    assert_eq!(graph.sink(0).get(LEFT_X), 0.0, "the assist engaged");
    // Between release (0.05) and engage (0.1) the assist keeps the stick.
    // The source's deadzone is set to zero here so the value reaches the node.
    let mut graph = Graph::build(GraphSpec {
        sources: vec![
            source(),
            SourceSpec {
                deadzone: 0.0,
                layout: Layout::gamepad(),
            },
        ],
        nodes: vec![NodeSpec::Mux {
            inputs: vec![Input::Source(PRIMARY), Input::Source(ASSIST)],
            config: MuxConfig::new(Mode::Priority, &Layout::gamepad()),
        }],
        sinks: vec![Input::Node(0, 0)],
    })
    .unwrap();
    step(&mut graph, PRIMARY, pad().with(LEFT_X, 0.9));
    step(&mut graph, ASSIST, pad().with(LEFT_Y, 0.07));
    assert_eq!(graph.sink(0).get(LEFT_X), 0.9, "0.07 is below engage");
    step(&mut graph, ASSIST, pad().with(LEFT_Y, 0.2));
    step(&mut graph, ASSIST, pad().with(LEFT_Y, 0.07));
    assert_eq!(graph.sink(0).get(LEFT_Y), 0.07, "0.07 is above release, so the assist keeps it");
    step(&mut graph, ASSIST, pad().with(LEFT_Y, 0.02));
    assert_eq!(graph.sink(0).get(LEFT_X), 0.9, "below release the primary has it back");
}

#[test]
fn a_source_deadzone_hides_rest_noise_from_the_node() {
    let mut graph = mux(Mode::Priority);
    step(&mut graph, PRIMARY, pad().with(LEFT_X, 0.9));
    // A worn assist stick resting at 0.08 must not count as a touch, and
    // must not leak into the output either.
    step(&mut graph, ASSIST, pad().with(LEFT_X, 0.06).with(LEFT_Y, 0.05));
    assert_eq!(graph.sink(0).get(LEFT_X), 0.9);
    assert_eq!(graph.sink(0).get(LEFT_Y), 0.0);
}

// --- average ----------------------------------------------------------------

#[test]
fn average_blends_sticks_both_are_using() {
    let mut graph = mux(Mode::Average);
    step(&mut graph, PRIMARY, pad().with(LEFT_X, 1.0));
    assert_eq!(graph.sink(0).get(LEFT_X), 1.0, "alone, the primary is not halved");
    step(&mut graph, ASSIST, pad().with(LEFT_X, -0.5));
    assert_eq!(graph.sink(0).get(LEFT_X), 0.25);
}

#[test]
fn average_presses_a_button_either_holds() {
    let mut graph = mux(Mode::Average);
    step(&mut graph, PRIMARY, pad().with(EAST, 1.0));
    step(&mut graph, ASSIST, pad().with(WEST, 1.0));
    assert_eq!(graph.sink(0).get(EAST), 1.0);
    assert_eq!(graph.sink(0).get(WEST), 1.0);
}

// --- toggle -----------------------------------------------------------------

#[test]
fn toggle_follows_one_pad_until_the_switch() {
    let mut graph = mux(Mode::Toggle);
    step(&mut graph, PRIMARY, pad().with(SOUTH, 1.0));
    step(&mut graph, ASSIST, pad().with(EAST, 1.0));
    assert_eq!(graph.sink(0).get(SOUTH), 1.0);
    assert_eq!(graph.sink(0).get(EAST), 0.0, "the assist is not active yet");
}

#[test]
fn a_switch_takes_the_incoming_pads_state() {
    let mut graph = mux(Mode::Toggle);
    // The primary accelerates and steers; the assist already holds the
    // trigger at a different depth and one button of its own.
    step(
        &mut graph,
        PRIMARY,
        pad().with(RIGHT_TRIGGER, 0.8).with(LEFT_X, 0.7).with(SOUTH, 1.0),
    );
    let assist = pad().with(RIGHT_TRIGGER, 0.3).with(EAST, 1.0);
    step(&mut graph, ASSIST, assist);

    let mut trigger_values = Vec::new();
    graph.update(ASSIST, assist.with(MODE, 1.0), &mut |change| {
        if change.control == RIGHT_TRIGGER {
            trigger_values.push(change.value);
        }
    });

    assert_eq!(trigger_values, [0.3], "the trigger steps to the assist's value, never through zero");
    assert_eq!(graph.sink(0).get(SOUTH), 0.0, "held only by the outgoing pad: released");
    assert_eq!(graph.sink(0).get(EAST), 1.0, "held only by the incoming pad: pressed");
    assert_eq!(graph.sink(0).get(LEFT_X), 0.0);
    assert_eq!(graph.sink(0).get(MODE), 0.0, "the switch button is consumed");
}

#[test]
fn a_switch_happens_once_per_press() {
    let mut graph = mux(Mode::Toggle);
    step(&mut graph, PRIMARY, pad().with(SOUTH, 1.0));
    step(&mut graph, ASSIST, pad().with(MODE, 1.0));
    step(&mut graph, ASSIST, pad().with(MODE, 1.0).with(EAST, 1.0));
    assert_eq!(graph.sink(0).get(EAST), 1.0, "holding the switch must not switch back");
    step(&mut graph, ASSIST, pad());
    step(&mut graph, ASSIST, pad().with(MODE, 1.0));
    assert_eq!(graph.sink(0).get(SOUTH), 1.0, "a second press hands back");
}

#[test]
fn a_client_can_switch_without_a_button() {
    let mut graph = mux(Mode::Toggle);
    step(&mut graph, PRIMARY, pad().with(SOUTH, 1.0));
    step(&mut graph, ASSIST, pad().with(EAST, 1.0));
    graph.switch(0, &mut |_| {});
    assert_eq!(graph.sink(0).get(EAST), 1.0);
    assert_eq!(graph.sink(0).get(SOUTH), 0.0);
}

#[test]
fn the_switch_can_be_another_button_on_another_pad() {
    let mut config = MuxConfig::new(Mode::Toggle, &Layout::gamepad());
    config.switch = Some(Switch {
        input: Some(PRIMARY),
        button: SELECT,
    });
    let mut graph = mux_graph(config);
    step(&mut graph, ASSIST, pad().with(EAST, 1.0).with(MODE, 1.0));
    assert_eq!(graph.sink(0).get(EAST), 0.0, "Mode on the assist no longer switches");
    step(&mut graph, PRIMARY, pad().with(SELECT, 1.0));
    assert_eq!(graph.sink(0).get(EAST), 1.0);
    assert_eq!(graph.sink(0).get(MODE), 1.0, "Mode is an ordinary button again");
}

// --- demux ------------------------------------------------------------------

#[test]
fn unicast_drives_one_virtual_pad_and_rests_the_others() {
    let mut graph = demux(DemuxMode::Unicast, 3);
    step(&mut graph, 0, pad().with(SOUTH, 1.0).with(LEFT_X, 0.6));
    assert_eq!(graph.sink(0).get(SOUTH), 1.0);
    assert_eq!(*graph.sink(1), PadState::NEUTRAL);
    assert_eq!(*graph.sink(2), PadState::NEUTRAL);
}

#[test]
fn unicast_cycles_on_the_switch_and_neutralizes_the_pad_it_leaves() {
    let mut graph = demux(DemuxMode::Unicast, 2);
    let held = pad().with(SOUTH, 1.0).with(LEFT_X, 0.6);
    step(&mut graph, 0, held);
    step(&mut graph, 0, held.with(MODE, 1.0));
    assert_eq!(*graph.sink(0), PadState::NEUTRAL);
    assert_eq!(graph.sink(1).get(SOUTH), 1.0);
    assert_eq!(graph.sink(1).get(MODE), 0.0, "the switch button is consumed");
    step(&mut graph, 0, held);
    step(&mut graph, 0, held.with(MODE, 1.0));
    assert_eq!(graph.sink(0).get(SOUTH), 1.0, "past the last pad it wraps to the first");
}

#[test]
fn multicast_copies_the_pad_to_every_virtual_pad() {
    let mut graph = demux(DemuxMode::Multicast, 2);
    step(&mut graph, 0, pad().with(SOUTH, 1.0).with(MODE, 1.0));
    for sink in 0..2 {
        assert_eq!(graph.sink(sink).get(SOUTH), 1.0);
        assert_eq!(graph.sink(sink).get(MODE), 1.0, "nothing is reserved in multicast");
    }
}

// --- chained ----------------------------------------------------------------

/// The legacy cookbook's "double agent": one helper assists two players,
/// switching between them. Three pads, one demux, two muxes, two virtual pads.
fn double_agent() -> Graph {
    let layout = Layout::gamepad();
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
    .unwrap()
}

#[test]
fn one_helper_assists_two_players_in_one_graph() {
    let mut graph = double_agent();
    step(&mut graph, 0, pad().with(LEFT_X, 0.9));
    step(&mut graph, 1, pad().with(LEFT_X, 0.8));
    step(&mut graph, 2, pad().with(LEFT_X, -0.5));
    assert_eq!(graph.sink(0).get(LEFT_X), -0.5, "the helper starts on player one");
    assert_eq!(graph.sink(1).get(LEFT_X), 0.8);
    step(&mut graph, 2, pad().with(LEFT_X, -0.5).with(MODE, 1.0));
    assert_eq!(graph.sink(0).get(LEFT_X), 0.9, "player one has the stick back");
    assert_eq!(graph.sink(1).get(LEFT_X), -0.5, "the helper moved to player two");
}

#[test]
fn a_graph_with_a_cycle_is_refused() {
    let layout = Layout::gamepad();
    let result = Graph::build(GraphSpec {
        sources: vec![source()],
        nodes: vec![
            NodeSpec::Mux {
                inputs: vec![Input::Source(0), Input::Node(1, 0)],
                config: MuxConfig::new(Mode::Priority, &layout),
            },
            NodeSpec::Mux {
                inputs: vec![Input::Source(0), Input::Node(0, 0)],
                config: MuxConfig::new(Mode::Priority, &layout),
            },
        ],
        sinks: vec![Input::Node(0, 0)],
    });
    assert_eq!(result.err(), Some(GraphError::Cycle));
}

// --- dropouts ---------------------------------------------------------------

#[test]
fn a_pad_that_drops_out_goes_neutral_and_the_other_carries_on() {
    let mut graph = mux(Mode::Priority);
    step(&mut graph, PRIMARY, pad().with(LEFT_X, 0.9));
    step(&mut graph, ASSIST, pad().with(LEFT_X, -0.5).with(SOUTH, 1.0));
    graph.set_present(ASSIST, false, &mut |_| {});
    assert_eq!(graph.sink(0).get(SOUTH), 0.0, "nothing stays held by a pad that is gone");
    assert_eq!(graph.sink(0).get(LEFT_X), 0.9);
    graph.set_present(ASSIST, true, &mut |_| {});
    step(&mut graph, ASSIST, pad().with(LEFT_X, -0.7));
    assert_eq!(graph.sink(0).get(LEFT_X), -0.7, "it is the assist again when it returns");
}

// --- a pad that is not gamepad-shaped ---------------------------------------

/// A flight stick: three joint axes, with a trigger, two thumb buttons and
/// a hat on the grip, all owned together; plus a separate throttle.
fn flight_stick() -> Layout {
    let grip: Vec<Control> = [0, 1, 2]
        .map(Control::Axis)
        .into_iter()
        .chain((0..7).map(Control::Button))
        .collect();
    Layout::default()
        .group(Kind::Stick, &grip)
        .group(Kind::Trigger, &[Control::Axis(3)])
}

#[test]
fn whoever_flies_the_stick_owns_its_trigger() {
    let layout = flight_stick();
    let stick = || SourceSpec {
        deadzone: 0.05,
        layout: flight_stick(),
    };
    let mut graph = Graph::build(GraphSpec {
        sources: vec![stick(), stick()],
        nodes: vec![NodeSpec::Mux {
            inputs: vec![Input::Source(PRIMARY), Input::Source(ASSIST)],
            config: MuxConfig::new(Mode::Priority, &layout),
        }],
        sinks: vec![Input::Node(0, 0)],
    })
    .unwrap();
    let fire = Control::Button(0);
    let twist = Control::Axis(2);
    step(&mut graph, PRIMARY, pad().with(fire, 1.0).with(Control::Axis(0), 0.6));
    assert_eq!(graph.sink(0).get(fire), 1.0);
    // The instructor takes the stick by twisting it: the student's trigger
    // goes with it, so nobody fires while someone else aims.
    step(&mut graph, ASSIST, pad().with(twist, 0.4));
    assert_eq!(graph.sink(0).get(fire), 0.0);
    assert_eq!(graph.sink(0).get(twist), 0.4);
    assert_eq!(graph.sink(0).get(Control::Axis(0)), 0.0);
    // The throttle is its own group and stays the student's.
    step(
        &mut graph,
        PRIMARY,
        pad().with(fire, 1.0).with(Control::Axis(0), 0.6).with(Control::Axis(3), 0.9),
    );
    assert_eq!(graph.sink(0).get(Control::Axis(3)), 0.9);
}

// --- feedback ---------------------------------------------------------------

const KEY: EffectKey = EffectKey { sink: 0, id: 0 };
const RUMBLE: Effect = Effect {
    strong: 40000,
    weak: 20000,
};

fn bit(source: usize) -> u32 {
    1 << source
}

/// A router following a graph's first sink.
fn router_for(graph: &Graph) -> (Router, Vec<Command>) {
    let mut router = Router::new(2);
    let mut commands = Vec::new();
    router.retarget(0, graph.feedback_targets(0), &mut commands);
    (router, commands)
}

#[test]
fn rumble_reaches_both_pads_of_a_priority_mux() {
    let graph = mux(Mode::Priority);
    let (mut router, mut commands) = router_for(&graph);
    let asked = router.upload(KEY, RUMBLE, &mut commands);
    assert_eq!(asked, bit(PRIMARY) | bit(ASSIST));
    assert_eq!(router.confirm(KEY, asked, &mut commands), Reply::Accepted);
    router.play(KEY, true, &mut commands);
    router.play(KEY, false, &mut commands);
    router.erase(KEY, &mut commands);
    let upload = |target| Command::Upload {
        target,
        key: KEY,
        effect: RUMBLE,
    };
    assert_eq!(
        commands,
        [
            upload(PRIMARY),
            upload(ASSIST),
            Command::Play { target: PRIMARY, key: KEY },
            Command::Play { target: ASSIST, key: KEY },
            Command::Stop { target: PRIMARY, key: KEY },
            Command::Stop { target: ASSIST, key: KEY },
            Command::Erase { target: PRIMARY, key: KEY },
            Command::Erase { target: ASSIST, key: KEY },
        ]
    );
}

#[test]
fn toggle_rumbles_only_the_active_pad_and_moves_with_the_switch() {
    let mut graph = mux(Mode::Toggle);
    let (mut router, mut commands) = router_for(&graph);
    assert_eq!(graph.feedback_targets(0), bit(PRIMARY));
    let asked = router.upload(KEY, RUMBLE, &mut commands);
    router.confirm(KEY, asked, &mut commands);
    router.play(KEY, true, &mut commands);
    commands.clear();

    graph.switch(0, &mut |_| {});
    router.retarget(0, graph.feedback_targets(0), &mut commands);
    // The playing effect follows the hand-off: started on the pad that
    // now has control, stopped and removed on the one that gave it up.
    assert_eq!(
        commands,
        [
            Command::Upload {
                target: ASSIST,
                key: KEY,
                effect: RUMBLE
            },
            Command::Play { target: ASSIST, key: KEY },
            Command::Stop { target: PRIMARY, key: KEY },
            Command::Erase { target: PRIMARY, key: KEY },
        ]
    );
}

#[test]
fn rumble_can_go_to_the_pad_that_is_not_providing_input() {
    let mut config = MuxConfig::new(Mode::Toggle, &Layout::gamepad());
    config.feedback = MuxFeedback::Inputs(bit(ASSIST));
    let graph = mux_graph(config);
    assert_eq!(graph.feedback_targets(0), bit(ASSIST), "the primary drives, the assist feels it");
}

#[test]
fn rumble_can_be_off() {
    let mut config = MuxConfig::new(Mode::Priority, &Layout::gamepad());
    config.feedback = MuxFeedback::None;
    let graph = mux_graph(config);
    let (mut router, mut commands) = router_for(&graph);
    let asked = router.upload(KEY, RUMBLE, &mut commands);
    assert_eq!(asked, 0);
    assert_eq!(router.confirm(KEY, asked, &mut commands), Reply::Accepted, "the game is not told its rumble goes nowhere");
    router.play(KEY, true, &mut commands);
    assert!(commands.is_empty());
}

#[test]
fn an_upload_is_accepted_if_any_pad_takes_it() {
    let graph = mux(Mode::Priority);
    let (mut router, mut commands) = router_for(&graph);
    let asked = router.upload(KEY, RUMBLE, &mut commands);
    assert_eq!(router.confirm(KEY, asked & !bit(ASSIST), &mut commands), Reply::Accepted);
    commands.clear();
    router.play(KEY, true, &mut commands);
    assert_eq!(commands, [Command::Play { target: PRIMARY, key: KEY }], "the pad that refused is left alone");
}

#[test]
fn an_upload_every_pad_refuses_is_refused() {
    let graph = mux(Mode::Priority);
    let (mut router, mut commands) = router_for(&graph);
    let asked = router.upload(KEY, RUMBLE, &mut commands);
    assert_eq!(router.confirm(KEY, 0, &mut commands), Reply::Rejected);
    commands.clear();
    router.play(KEY, true, &mut commands);
    assert!(commands.is_empty(), "a refused effect is forgotten");
    let _ = asked;
}

#[test]
fn unicast_takes_rumble_only_from_the_active_virtual_pad() {
    let mut graph = demux(DemuxMode::Unicast, 2);
    assert_eq!(graph.feedback_targets(0), bit(0));
    assert_eq!(graph.feedback_targets(1), 0);
    step(&mut graph, 0, pad().with(MODE, 1.0));
    assert_eq!(graph.feedback_targets(0), 0);
    assert_eq!(graph.feedback_targets(1), bit(0));
}

#[test]
fn demux_rumble_can_be_off() {
    let mut config = DemuxConfig::new(DemuxMode::Multicast, 2);
    config.feedback = DemuxFeedback::None;
    let graph = Graph::build(GraphSpec {
        sources: vec![source()],
        nodes: vec![NodeSpec::Demux {
            input: Input::Source(0),
            config,
        }],
        sinks: vec![Input::Node(0, 0), Input::Node(0, 1)],
    })
    .unwrap();
    assert_eq!(graph.feedback_targets(0), 0);
}

#[test]
fn two_virtual_pads_can_rumble_one_pad_without_colliding() {
    let graph = demux(DemuxMode::Multicast, 2);
    let mut router = Router::new(2);
    let mut commands = Vec::new();
    router.retarget(0, graph.feedback_targets(0), &mut commands);
    router.retarget(1, graph.feedback_targets(1), &mut commands);
    // Two games each upload their own effect 0.
    let first = EffectKey { sink: 0, id: 0 };
    let second = EffectKey { sink: 1, id: 0 };
    router.upload(first, RUMBLE, &mut commands);
    router.upload(second, Effect { strong: 1, weak: 1 }, &mut commands);
    router.confirm(first, bit(0), &mut commands);
    router.confirm(second, bit(0), &mut commands);
    commands.clear();
    router.erase(first, &mut commands);
    router.play(second, true, &mut commands);
    assert_eq!(
        commands,
        [
            Command::Erase { target: 0, key: first },
            Command::Play { target: 0, key: second },
        ],
        "one game erasing its effect leaves the other's alone"
    );
}

#[test]
fn feedback_follows_one_helper_between_two_players() {
    let mut graph = double_agent();
    // Player one's game rumbles player one and, while the helper is with
    // them, the helper.
    assert_eq!(graph.feedback_targets(0), bit(0) | bit(2));
    assert_eq!(graph.feedback_targets(1), bit(1));
    step(&mut graph, 2, pad().with(MODE, 1.0));
    assert_eq!(graph.feedback_targets(0), bit(0));
    assert_eq!(graph.feedback_targets(1), bit(1) | bit(2));
}

#[test]
fn a_returning_pad_gets_its_effects_back() {
    let graph = mux(Mode::Priority);
    let (mut router, mut commands) = router_for(&graph);
    let asked = router.upload(KEY, RUMBLE, &mut commands);
    router.confirm(KEY, asked, &mut commands);
    router.play(KEY, true, &mut commands);
    commands.clear();

    router.source_gone(ASSIST);
    router.play(KEY, false, &mut commands);
    router.play(KEY, true, &mut commands);
    assert_eq!(
        commands,
        [
            Command::Stop { target: PRIMARY, key: KEY },
            Command::Play { target: PRIMARY, key: KEY },
        ],
        "nothing is sent to a pad that is gone"
    );
    commands.clear();

    // Its effects died with its connection, so they are uploaded again,
    // and the one that is playing is started.
    router.source_back(ASSIST, &mut commands);
    assert_eq!(
        commands,
        [
            Command::Upload {
                target: ASSIST,
                key: KEY,
                effect: RUMBLE
            },
            Command::Play { target: ASSIST, key: KEY },
        ]
    );
}
