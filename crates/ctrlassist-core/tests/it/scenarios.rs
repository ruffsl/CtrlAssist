//! The routing behavior, written as scenarios before the nodes existed.
//!
//! Each test builds a graph the way a profile would, steps it with pad
//! states, and checks what the virtual pads hold.

use ctrlassist_core::gamepad::*;
use ctrlassist_core::{
    Contact, Control, DemuxConfig, DemuxFeedback, DemuxMode, EFFECTS, Effect, EffectKey,
    FeedbackError, Graph, GraphError, GraphSpec, Group, Input, Kind, Layout, Level, MAX_NODES,
    MAX_SINKS, Mode, Motion, MuxConfig, MuxFeedback, NodeId, NodeSpec, PadState, Router, Rule,
    SinkId, SourceId, SourceSet, SourceSpec, Switch, Touchpad,
};

const PRIMARY: SourceId = SourceId(0);
const ASSIST: SourceId = SourceId(1);

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
        sinks: vec![Input::Node(NodeId(0), 0)],
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
            input: Input::Source(SourceId(0)),
            config: DemuxConfig::new(mode, outputs),
        }],
        sinks: (0..outputs)
            .map(|port| Input::Node(NodeId(0), port))
            .collect(),
    })
    .unwrap()
}

fn step(graph: &mut Graph, source: SourceId, state: PadState) {
    graph.update(source, state, &mut |_| {});
}

fn pad() -> PadState {
    PadState::NEUTRAL
}

// --- priority ---------------------------------------------------------------

#[test]
fn priority_passes_the_primary_through_while_the_assist_rests() {
    let mut graph = mux(Mode::Priority);
    step(
        &mut graph,
        PRIMARY,
        pad().with(SOUTH, 1.0).with(LEFT_X, 0.8),
    );
    assert_eq!(graph.sink(SinkId(0)).get(SOUTH), 1.0);
    assert_eq!(graph.sink(SinkId(0)).get(LEFT_X), 0.8);
}

#[test]
fn priority_gives_a_stick_to_the_assist_as_a_whole() {
    let mut graph = mux(Mode::Priority);
    step(
        &mut graph,
        PRIMARY,
        pad().with(LEFT_X, 0.9).with(LEFT_Y, 0.9),
    );
    // The assist nudges only X; it takes Y too, so the output is the
    // assist's direction, not a blend of two people's.
    step(&mut graph, ASSIST, pad().with(LEFT_X, -0.5));
    assert_eq!(graph.sink(SinkId(0)).get(LEFT_X), -0.5);
    assert_eq!(graph.sink(SinkId(0)).get(LEFT_Y), 0.0);
    // The other stick is untouched by the assist and stays the primary's.
    step(
        &mut graph,
        PRIMARY,
        pad().with(LEFT_X, 0.9).with(LEFT_Y, 0.9).with(RIGHT_X, 0.4),
    );
    assert_eq!(graph.sink(SinkId(0)).get(RIGHT_X), 0.4);
    assert_eq!(graph.sink(SinkId(0)).get(LEFT_X), -0.5);
}

#[test]
fn priority_returns_a_stick_when_the_assist_lets_go() {
    let mut graph = mux(Mode::Priority);
    step(&mut graph, PRIMARY, pad().with(LEFT_X, 0.9));
    step(&mut graph, ASSIST, pad().with(LEFT_X, -0.5));
    step(&mut graph, ASSIST, pad());
    assert_eq!(graph.sink(SinkId(0)).get(LEFT_X), 0.9);
}

#[test]
fn priority_holds_a_button_for_the_assist_whatever_the_primary_does() {
    let mut graph = mux(Mode::Priority);
    step(&mut graph, ASSIST, pad().with(SOUTH, 1.0));
    step(&mut graph, PRIMARY, pad().with(SOUTH, 1.0));
    step(&mut graph, PRIMARY, pad());
    assert_eq!(
        graph.sink(SinkId(0)).get(SOUTH),
        1.0,
        "the primary's release must not reach the game"
    );
    step(&mut graph, ASSIST, pad());
    assert_eq!(graph.sink(SinkId(0)).get(SOUTH), 0.0);
}

#[test]
fn priority_takes_the_larger_trigger() {
    let mut graph = mux(Mode::Priority);
    step(&mut graph, PRIMARY, pad().with(RIGHT_TRIGGER, 0.8));
    step(&mut graph, ASSIST, pad().with(RIGHT_TRIGGER, 0.3));
    assert_eq!(graph.sink(SinkId(0)).get(RIGHT_TRIGGER), 0.8);
    step(&mut graph, ASSIST, pad().with(RIGHT_TRIGGER, 1.0));
    assert_eq!(graph.sink(SinkId(0)).get(RIGHT_TRIGGER), 1.0);
}

#[test]
fn an_owner_does_not_flicker_at_the_threshold() {
    let mut graph = mux(Mode::Priority);
    step(&mut graph, PRIMARY, pad().with(LEFT_X, 0.9));
    step(&mut graph, ASSIST, pad().with(LEFT_Y, 0.2));
    assert_eq!(graph.sink(SinkId(0)).get(LEFT_X), 0.0, "the assist engaged");
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
        sinks: vec![Input::Node(NodeId(0), 0)],
    })
    .unwrap();
    step(&mut graph, PRIMARY, pad().with(LEFT_X, 0.9));
    step(&mut graph, ASSIST, pad().with(LEFT_Y, 0.07));
    assert_eq!(
        graph.sink(SinkId(0)).get(LEFT_X),
        0.9,
        "0.07 is below engage"
    );
    step(&mut graph, ASSIST, pad().with(LEFT_Y, 0.2));
    step(&mut graph, ASSIST, pad().with(LEFT_Y, 0.07));
    assert_eq!(
        graph.sink(SinkId(0)).get(LEFT_Y),
        0.07,
        "0.07 is above release, so the assist keeps it"
    );
    step(&mut graph, ASSIST, pad().with(LEFT_Y, 0.02));
    assert_eq!(
        graph.sink(SinkId(0)).get(LEFT_X),
        0.9,
        "below release the primary has it back"
    );
}

#[test]
fn a_source_deadzone_hides_rest_noise_from_the_node() {
    let mut graph = mux(Mode::Priority);
    step(&mut graph, PRIMARY, pad().with(LEFT_X, 0.9));
    // A worn assist stick resting at 0.08 must not count as a touch, and
    // must not leak into the output either.
    step(
        &mut graph,
        ASSIST,
        pad().with(LEFT_X, 0.06).with(LEFT_Y, 0.05),
    );
    assert_eq!(graph.sink(SinkId(0)).get(LEFT_X), 0.9);
    assert_eq!(graph.sink(SinkId(0)).get(LEFT_Y), 0.0);
}

// --- average ----------------------------------------------------------------

#[test]
fn average_blends_sticks_both_are_using() {
    let mut graph = mux(Mode::Average);
    step(&mut graph, PRIMARY, pad().with(LEFT_X, 1.0));
    assert_eq!(
        graph.sink(SinkId(0)).get(LEFT_X),
        1.0,
        "alone, the primary is not halved"
    );
    step(&mut graph, ASSIST, pad().with(LEFT_X, -0.5));
    assert_eq!(graph.sink(SinkId(0)).get(LEFT_X), 0.25);
}

#[test]
fn average_presses_a_button_either_holds() {
    let mut graph = mux(Mode::Average);
    step(&mut graph, PRIMARY, pad().with(EAST, 1.0));
    step(&mut graph, ASSIST, pad().with(WEST, 1.0));
    assert_eq!(graph.sink(SinkId(0)).get(EAST), 1.0);
    assert_eq!(graph.sink(SinkId(0)).get(WEST), 1.0);
}

// --- toggle -----------------------------------------------------------------

#[test]
fn toggle_follows_one_pad_until_the_switch() {
    let mut graph = mux(Mode::Toggle);
    step(&mut graph, PRIMARY, pad().with(SOUTH, 1.0));
    step(&mut graph, ASSIST, pad().with(EAST, 1.0));
    assert_eq!(graph.sink(SinkId(0)).get(SOUTH), 1.0);
    assert_eq!(
        graph.sink(SinkId(0)).get(EAST),
        0.0,
        "the assist is not active yet"
    );
}

#[test]
fn a_switch_takes_the_incoming_pads_state() {
    let mut graph = mux(Mode::Toggle);
    // The primary accelerates and steers; the assist already holds the
    // trigger at a different depth and one button of its own.
    step(
        &mut graph,
        PRIMARY,
        pad()
            .with(RIGHT_TRIGGER, 0.8)
            .with(LEFT_X, 0.7)
            .with(SOUTH, 1.0),
    );
    let assist = pad().with(RIGHT_TRIGGER, 0.3).with(EAST, 1.0);
    step(&mut graph, ASSIST, assist);

    let mut trigger_values = Vec::new();
    graph.update(ASSIST, assist.with(MODE, 1.0), &mut |change| {
        if change.control == RIGHT_TRIGGER {
            trigger_values.push(change.value);
        }
    });

    assert_eq!(
        trigger_values,
        [0.3],
        "the trigger steps to the assist's value, never through zero"
    );
    assert_eq!(
        graph.sink(SinkId(0)).get(SOUTH),
        0.0,
        "held only by the outgoing pad: released"
    );
    assert_eq!(
        graph.sink(SinkId(0)).get(EAST),
        1.0,
        "held only by the incoming pad: pressed"
    );
    assert_eq!(graph.sink(SinkId(0)).get(LEFT_X), 0.0);
    assert_eq!(
        graph.sink(SinkId(0)).get(MODE),
        0.0,
        "the switch button is consumed"
    );
}

#[test]
fn a_switch_happens_once_per_press() {
    let mut graph = mux(Mode::Toggle);
    step(&mut graph, PRIMARY, pad().with(SOUTH, 1.0));
    step(&mut graph, ASSIST, pad().with(MODE, 1.0));
    step(&mut graph, ASSIST, pad().with(MODE, 1.0).with(EAST, 1.0));
    assert_eq!(
        graph.sink(SinkId(0)).get(EAST),
        1.0,
        "holding the switch must not switch back"
    );
    step(&mut graph, ASSIST, pad());
    step(&mut graph, ASSIST, pad().with(MODE, 1.0));
    assert_eq!(
        graph.sink(SinkId(0)).get(SOUTH),
        1.0,
        "a second press hands back"
    );
}

#[test]
fn a_client_can_switch_without_a_button() {
    let mut graph = mux(Mode::Toggle);
    step(&mut graph, PRIMARY, pad().with(SOUTH, 1.0));
    step(&mut graph, ASSIST, pad().with(EAST, 1.0));
    assert_eq!(graph.selected(NodeId(0)), Some(0));
    graph.switch(NodeId(0), &mut |_| {});
    assert_eq!(graph.sink(SinkId(0)).get(EAST), 1.0);
    assert_eq!(graph.sink(SinkId(0)).get(SOUTH), 0.0);
    assert_eq!(graph.selected(NodeId(0)), Some(1), "which is what it says");
    assert_eq!(graph.nodes(), 1);
    assert_eq!(graph.selected(NodeId(1)), None, "no such node");
}

#[test]
fn only_a_node_that_follows_a_switch_has_a_selection() {
    for mode in [Mode::Priority, Mode::Average] {
        assert_eq!(mux(mode).selected(NodeId(0)), None, "{mode:?}");
    }
}

#[test]
fn the_switch_can_be_another_button_on_another_pad() {
    let mut config = MuxConfig::new(Mode::Toggle, &Layout::gamepad());
    config.switch = Some(Switch {
        input: Some(0),
        button: SELECT,
    });
    let mut graph = mux_graph(config);
    step(&mut graph, ASSIST, pad().with(EAST, 1.0).with(MODE, 1.0));
    assert_eq!(
        graph.sink(SinkId(0)).get(EAST),
        0.0,
        "Mode on the assist no longer switches"
    );
    step(&mut graph, PRIMARY, pad().with(SELECT, 1.0));
    assert_eq!(graph.sink(SinkId(0)).get(EAST), 1.0);
    assert_eq!(
        graph.sink(SinkId(0)).get(MODE),
        1.0,
        "Mode is an ordinary button again"
    );
}

// --- demux ------------------------------------------------------------------

#[test]
fn unicast_drives_one_virtual_pad_and_rests_the_others() {
    let mut graph = demux(DemuxMode::Unicast, 3);
    step(
        &mut graph,
        SourceId(0),
        pad().with(SOUTH, 1.0).with(LEFT_X, 0.6),
    );
    assert_eq!(graph.sink(SinkId(0)).get(SOUTH), 1.0);
    assert_eq!(*graph.sink(SinkId(1)), PadState::NEUTRAL);
    assert_eq!(*graph.sink(SinkId(2)), PadState::NEUTRAL);
}

#[test]
fn unicast_cycles_on_the_switch_and_neutralizes_the_pad_it_leaves() {
    let mut graph = demux(DemuxMode::Unicast, 2);
    let held = pad().with(SOUTH, 1.0).with(LEFT_X, 0.6);
    step(&mut graph, SourceId(0), held);
    step(&mut graph, SourceId(0), held.with(MODE, 1.0));
    assert_eq!(*graph.sink(SinkId(0)), PadState::NEUTRAL);
    assert_eq!(graph.sink(SinkId(1)).get(SOUTH), 1.0);
    assert_eq!(
        graph.sink(SinkId(1)).get(MODE),
        0.0,
        "the switch button is consumed"
    );
    step(&mut graph, SourceId(0), held);
    step(&mut graph, SourceId(0), held.with(MODE, 1.0));
    assert_eq!(
        graph.sink(SinkId(0)).get(SOUTH),
        1.0,
        "past the last pad it wraps to the first"
    );
}

#[test]
fn multicast_copies_the_pad_to_every_virtual_pad() {
    let mut graph = demux(DemuxMode::Multicast, 2);
    step(
        &mut graph,
        SourceId(0),
        pad().with(SOUTH, 1.0).with(MODE, 1.0),
    );
    for sink in [SinkId(0), SinkId(1)] {
        assert_eq!(graph.sink(sink).get(SOUTH), 1.0);
        assert_eq!(
            graph.sink(sink).get(MODE),
            1.0,
            "nothing is reserved in multicast"
        );
    }
}

// --- chained ----------------------------------------------------------------

/// One helper assists two players, switching between them. Three pads, one demux, two muxes, two virtual pads.
fn double_agent() -> Graph {
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
                config: MuxConfig::new(Mode::Priority, &layout),
            },
        ],
        sinks: vec![Input::Node(NodeId(1), 0), Input::Node(NodeId(2), 0)],
    })
    .unwrap()
}

#[test]
fn one_helper_assists_two_players_in_one_graph() {
    let mut graph = double_agent();
    step(&mut graph, SourceId(0), pad().with(LEFT_X, 0.9));
    step(&mut graph, SourceId(1), pad().with(LEFT_X, 0.8));
    step(&mut graph, SourceId(2), pad().with(LEFT_X, -0.5));
    assert_eq!(
        graph.sink(SinkId(0)).get(LEFT_X),
        -0.5,
        "the helper starts on player one"
    );
    assert_eq!(graph.sink(SinkId(1)).get(LEFT_X), 0.8);
    step(
        &mut graph,
        SourceId(2),
        pad().with(LEFT_X, -0.5).with(MODE, 1.0),
    );
    assert_eq!(
        graph.sink(SinkId(0)).get(LEFT_X),
        0.9,
        "player one has the stick back"
    );
    assert_eq!(
        graph.sink(SinkId(1)).get(LEFT_X),
        -0.5,
        "the helper moved to player two"
    );
}

#[test]
fn a_graph_with_a_cycle_is_refused() {
    let layout = Layout::gamepad();
    let result = Graph::build(GraphSpec {
        sources: vec![source()],
        nodes: vec![
            NodeSpec::Mux {
                inputs: vec![Input::Source(SourceId(0)), Input::Node(NodeId(1), 0)],
                config: MuxConfig::new(Mode::Priority, &layout),
            },
            NodeSpec::Mux {
                inputs: vec![Input::Source(SourceId(0)), Input::Node(NodeId(0), 0)],
                config: MuxConfig::new(Mode::Priority, &layout),
            },
        ],
        sinks: vec![Input::Node(NodeId(0), 0)],
    });
    assert_eq!(result.err(), Some(GraphError::Cycle));
}

// --- dropouts ---------------------------------------------------------------

#[test]
fn a_pad_that_drops_out_goes_neutral_and_the_other_carries_on() {
    let mut graph = mux(Mode::Priority);
    step(&mut graph, PRIMARY, pad().with(LEFT_X, 0.9));
    step(
        &mut graph,
        ASSIST,
        pad().with(LEFT_X, -0.5).with(SOUTH, 1.0),
    );
    graph.set_present(ASSIST, false, &mut |_| {});
    assert_eq!(
        graph.sink(SinkId(0)).get(SOUTH),
        0.0,
        "nothing stays held by a pad that is gone"
    );
    assert_eq!(graph.sink(SinkId(0)).get(LEFT_X), 0.9);
    graph.set_present(ASSIST, true, &mut |_| {});
    step(&mut graph, ASSIST, pad().with(LEFT_X, -0.7));
    assert_eq!(
        graph.sink(SinkId(0)).get(LEFT_X),
        -0.7,
        "it is the assist again when it returns"
    );
}

// --- motion and the touchpad ------------------------------------------------

/// The `readings`th reading of a pad turning at `yaw` degrees a second,
/// tipped to one side.
fn turning(yaw: f32, readings: u32) -> Motion {
    Motion::new([0.0, yaw, 0.0], [0.5, 0.85, 0.0], readings, 4000)
}

/// One finger down, at a place.
fn finger(x: f32, y: f32) -> Touchpad {
    Touchpad::new([Contact::new(true, x, y), Contact::UP])
}

#[test]
fn a_pad_at_rest_lies_flat_and_still_with_no_finger_down() {
    let motion = PadState::NEUTRAL.motion();
    assert_eq!(motion, Motion::STILL);
    assert_eq!(motion.gyro(), [0.0; 3]);
    assert_eq!(motion.accel(), [0.0, 1.0, 0.0], "1 g, straight up");
    assert_eq!(motion.readings(), 0);
    assert_eq!(PadState::NEUTRAL.touchpad(), Touchpad::UNTOUCHED);
    assert!(
        Touchpad::UNTOUCHED
            .contacts()
            .iter()
            .all(|contact| !contact.down())
    );
}

#[test]
fn priority_and_average_carry_the_primarys_motion_and_contacts_whatever_the_assist_does() {
    for mode in [Mode::Priority, Mode::Average] {
        let mut graph = mux(mode);
        let primary = pad()
            .with_motion(turning(30.0, 1))
            .with_touchpad(finger(0.2, 0.3));
        step(&mut graph, PRIMARY, primary);
        // The assist turns hard, touches, and takes the stick.
        let assist = |readings| {
            pad()
                .with(LEFT_X, 0.9)
                .with(SOUTH, 1.0)
                .with_motion(turning(-200.0, readings))
                .with_touchpad(finger(0.9, 0.9))
        };
        step(&mut graph, ASSIST, assist(50));
        let sink = *graph.sink(SinkId(0));
        assert_eq!(sink.get(LEFT_X), 0.9, "{mode:?}: the stick is the assist's");
        assert_eq!(sink.motion(), primary.motion(), "{mode:?}");
        assert_eq!(sink.touchpad(), primary.touchpad(), "{mode:?}");
        // The assist's next report is no fresh reading of the primary's.
        step(&mut graph, ASSIST, assist(51));
        assert_eq!(graph.sink(SinkId(0)).motion().readings(), 1, "{mode:?}");
        // The primary's is.
        step(&mut graph, PRIMARY, primary.with_motion(turning(35.0, 2)));
        assert_eq!(graph.sink(SinkId(0)).motion(), turning(35.0, 2), "{mode:?}");
    }
}

#[test]
fn the_assist_does_not_stand_in_for_a_primary_that_goes() {
    let mut graph = mux(Mode::Priority);
    step(
        &mut graph,
        PRIMARY,
        pad()
            .with_motion(turning(30.0, 1))
            .with_touchpad(finger(0.2, 0.3)),
    );
    step(
        &mut graph,
        ASSIST,
        pad()
            .with_motion(turning(-200.0, 9))
            .with_touchpad(finger(0.9, 0.9)),
    );
    graph.set_present(PRIMARY, false, &mut |_| {});
    let sink = *graph.sink(SinkId(0));
    assert_eq!(sink.motion(), Motion::STILL, "the primary's pad is gone");
    assert_eq!(sink.touchpad(), Touchpad::UNTOUCHED);
}

#[test]
fn toggle_carries_the_selected_pads_motion_and_contacts_following_a_switch() {
    let mut graph = mux(Mode::Toggle);
    let primary = pad()
        .with_motion(turning(30.0, 1))
        .with_touchpad(finger(0.2, 0.3));
    let assist = pad()
        .with_motion(turning(-200.0, 9))
        .with_touchpad(finger(0.9, 0.9));
    step(&mut graph, PRIMARY, primary);
    step(&mut graph, ASSIST, assist);
    assert_eq!(graph.sink(SinkId(0)).motion(), primary.motion());
    assert_eq!(graph.sink(SinkId(0)).touchpad(), primary.touchpad());

    // By the switch's button: a finger held down moves to the assist's
    // place in one report, as a stick steps.
    step(&mut graph, ASSIST, assist.with(MODE, 1.0));
    assert_eq!(graph.sink(SinkId(0)).motion(), assist.motion());
    assert_eq!(graph.sink(SinkId(0)).touchpad(), assist.touchpad());

    // By a client.
    graph.select(NodeId(0), 0, &mut |_| {});
    assert_eq!(graph.sink(SinkId(0)).motion(), primary.motion());
    assert_eq!(graph.sink(SinkId(0)).touchpad(), primary.touchpad());
}

#[test]
fn toggle_carries_the_next_pads_motion_when_the_selected_one_goes() {
    let mut graph = mux(Mode::Toggle);
    let assist = pad().with_motion(turning(-200.0, 9));
    step(&mut graph, PRIMARY, pad().with_motion(turning(30.0, 1)));
    step(&mut graph, ASSIST, assist);
    graph.set_present(PRIMARY, false, &mut |_| {});
    assert_eq!(graph.selected(NodeId(0)), Some(1));
    assert_eq!(graph.sink(SinkId(0)).motion(), assist.motion());
}

#[test]
fn the_touchpads_click_follows_the_modes_rule_for_buttons() {
    for mode in [Mode::Priority, Mode::Average] {
        let mut graph = mux(mode);
        step(&mut graph, ASSIST, pad().with(TOUCHPAD, 1.0));
        assert_eq!(
            graph.sink(SinkId(0)).get(TOUCHPAD),
            1.0,
            "{mode:?}: either pad's click presses it"
        );
    }
    let mut graph = mux(Mode::Toggle);
    step(&mut graph, ASSIST, pad().with(TOUCHPAD, 1.0));
    assert_eq!(
        graph.sink(SinkId(0)).get(TOUCHPAD),
        0.0,
        "toggle: only the selected pad's"
    );
    graph.switch(NodeId(0), &mut |_| {});
    assert_eq!(graph.sink(SinkId(0)).get(TOUCHPAD), 1.0);
}

#[test]
fn motion_and_contacts_change_no_control() {
    let mut graph = mux(Mode::Priority);
    let mut changes = Vec::new();
    graph.update(
        PRIMARY,
        pad()
            .with_motion(turning(30.0, 1))
            .with_touchpad(finger(0.2, 0.3)),
        &mut |change| changes.push(change),
    );
    assert_eq!(changes, [], "a virtual pad is written its whole state");
    assert_eq!(graph.sink(SinkId(0)).motion(), turning(30.0, 1));
}

#[test]
fn unicast_carries_motion_and_contacts_to_the_selected_virtual_pad_alone() {
    let mut graph = demux(DemuxMode::Unicast, 2);
    let held = pad()
        .with_motion(turning(30.0, 1))
        .with_touchpad(finger(0.2, 0.3));
    step(&mut graph, SourceId(0), held);
    assert_eq!(graph.sink(SinkId(0)).motion(), held.motion());
    assert_eq!(graph.sink(SinkId(0)).touchpad(), held.touchpad());
    assert_eq!(*graph.sink(SinkId(1)), PadState::NEUTRAL);
    step(&mut graph, SourceId(0), held.with(MODE, 1.0));
    assert_eq!(
        *graph.sink(SinkId(0)),
        PadState::NEUTRAL,
        "the one it leaves lies still, untouched"
    );
    assert_eq!(graph.sink(SinkId(1)).motion(), held.motion());
    assert_eq!(graph.sink(SinkId(1)).touchpad(), held.touchpad());
}

#[test]
fn multicast_carries_motion_and_contacts_to_every_virtual_pad() {
    let mut graph = demux(DemuxMode::Multicast, 2);
    let held = pad()
        .with_motion(turning(30.0, 1))
        .with_touchpad(finger(0.2, 0.3));
    step(&mut graph, SourceId(0), held);
    for sink in [SinkId(0), SinkId(1)] {
        assert_eq!(*graph.sink(sink), held);
    }
}

#[test]
fn a_helper_shared_between_two_players_moves_neither_ones_motion() {
    let mut graph = double_agent();
    step(&mut graph, SourceId(0), pad().with_motion(turning(10.0, 1)));
    step(&mut graph, SourceId(1), pad().with_motion(turning(20.0, 1)));
    step(
        &mut graph,
        SourceId(2),
        pad().with_motion(turning(-200.0, 1)),
    );
    assert_eq!(graph.sink(SinkId(0)).motion(), turning(10.0, 1));
    assert_eq!(graph.sink(SinkId(1)).motion(), turning(20.0, 1));
}

#[test]
fn a_reading_that_is_not_a_number_lies_still_and_a_place_stays_on_the_touchpad() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let motion = Motion::new([0.0, bad, 0.0], [0.5, 0.85, 0.0], 7, 4000);
        assert_eq!(motion.gyro(), Motion::STILL.gyro(), "{bad}");
        assert_eq!(
            motion.accel(),
            Motion::STILL.accel(),
            "{bad}: the whole reading"
        );
        assert_eq!(motion.readings(), 7, "it is still a reading");
        assert_eq!(motion.interval_us(), 4000);
        let motion = Motion::new([0.0; 3], [bad, 1.0, 0.0], 7, 4000);
        assert_eq!(motion.accel(), Motion::STILL.accel(), "{bad}");
        assert_eq!(Contact::new(true, bad, 0.5), Contact::UP, "{bad}");
        assert_eq!(Contact::new(true, 0.5, bad), Contact::UP, "{bad}");
    }
    let past = Contact::new(true, 1.5, -0.25);
    assert!(past.down());
    assert_eq!((past.x(), past.y()), (1.0, 0.0));
    let lifted = Contact::new(false, 0.25, 0.75);
    assert!(!lifted.down());
    assert_eq!(
        (lifted.x(), lifted.y()),
        (0.25, 0.75),
        "a lifted finger's place is kept"
    );
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
        .unwrap()
        .group(Kind::Trigger, &[Control::Axis(3)])
        .unwrap()
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
        sinks: vec![Input::Node(NodeId(0), 0)],
    })
    .unwrap();
    let fire = Control::Button(0);
    let twist = Control::Axis(2);
    step(
        &mut graph,
        PRIMARY,
        pad().with(fire, 1.0).with(Control::Axis(0), 0.6),
    );
    assert_eq!(graph.sink(SinkId(0)).get(fire), 1.0);
    // The instructor takes the stick by twisting it: the student's trigger
    // goes with it, so nobody fires while someone else aims.
    step(&mut graph, ASSIST, pad().with(twist, 0.4));
    assert_eq!(graph.sink(SinkId(0)).get(fire), 0.0);
    assert_eq!(graph.sink(SinkId(0)).get(twist), 0.4);
    assert_eq!(graph.sink(SinkId(0)).get(Control::Axis(0)), 0.0);
    // The throttle is its own group and stays the student's.
    step(
        &mut graph,
        PRIMARY,
        pad()
            .with(fire, 1.0)
            .with(Control::Axis(0), 0.6)
            .with(Control::Axis(3), 0.9),
    );
    assert_eq!(graph.sink(SinkId(0)).get(Control::Axis(3)), 0.9);
}

// --- feedback: where it goes -------------------------------------------------

fn bit(source: SourceId) -> SourceSet {
    source.into()
}

#[test]
fn rumble_reaches_both_pads_of_a_priority_mux() {
    assert_eq!(
        mux(Mode::Priority).feedback_targets(SinkId(0)),
        bit(PRIMARY) | bit(ASSIST)
    );
}

#[test]
fn toggle_rumbles_only_the_active_pad() {
    let mut graph = mux(Mode::Toggle);
    assert_eq!(graph.feedback_targets(SinkId(0)), bit(PRIMARY));
    graph.switch(NodeId(0), &mut |_| {});
    assert_eq!(graph.feedback_targets(SinkId(0)), bit(ASSIST));
}

#[test]
fn rumble_can_go_to_the_pad_that_is_not_providing_input() {
    let mut config = MuxConfig::new(Mode::Toggle, &Layout::gamepad());
    config.feedback = MuxFeedback::Inputs(1 << 1);
    let graph = mux_graph(config);
    assert_eq!(
        graph.feedback_targets(SinkId(0)),
        bit(ASSIST),
        "the primary drives, the assist feels it"
    );
}

#[test]
fn rumble_can_be_off() {
    let mut config = MuxConfig::new(Mode::Priority, &Layout::gamepad());
    config.feedback = MuxFeedback::None;
    assert_eq!(
        mux_graph(config).feedback_targets(SinkId(0)),
        SourceSet::EMPTY
    );
}

#[test]
fn unicast_takes_rumble_only_from_the_active_virtual_pad() {
    let mut graph = demux(DemuxMode::Unicast, 2);
    assert_eq!(graph.feedback_targets(SinkId(0)), bit(SourceId(0)));
    assert_eq!(graph.feedback_targets(SinkId(1)), SourceSet::EMPTY);
    step(&mut graph, SourceId(0), pad().with(MODE, 1.0));
    assert_eq!(graph.feedback_targets(SinkId(0)), SourceSet::EMPTY);
    assert_eq!(graph.feedback_targets(SinkId(1)), bit(SourceId(0)));
}

#[test]
fn demux_rumble_can_be_off() {
    let mut config = DemuxConfig::new(DemuxMode::Multicast, 2);
    config.feedback = DemuxFeedback::None;
    let graph = Graph::build(GraphSpec {
        sources: vec![source()],
        nodes: vec![NodeSpec::Demux {
            input: Input::Source(SourceId(0)),
            config,
        }],
        sinks: vec![Input::Node(NodeId(0), 0), Input::Node(NodeId(0), 1)],
    })
    .unwrap();
    assert_eq!(graph.feedback_targets(SinkId(0)), SourceSet::EMPTY);
}

#[test]
fn feedback_follows_one_helper_between_two_players() {
    let mut graph = double_agent();
    // Player one's game rumbles player one and, while the helper is with
    // them, the helper.
    assert_eq!(
        graph.feedback_targets(SinkId(0)),
        bit(SourceId(0)) | bit(SourceId(2))
    );
    assert_eq!(graph.feedback_targets(SinkId(1)), bit(SourceId(1)));
    step(&mut graph, SourceId(2), pad().with(MODE, 1.0));
    assert_eq!(graph.feedback_targets(SinkId(0)), bit(SourceId(0)));
    assert_eq!(
        graph.feedback_targets(SinkId(1)),
        bit(SourceId(1)) | bit(SourceId(2))
    );
}

/// Where feedback goes is found at once however many ways a pad reaches
/// a virtual pad: here by 8^20 and 2^32 paths, more than could ever be
/// walked one by one on the input thread.
#[test]
fn feedback_is_found_however_many_ways_a_pad_reaches_a_virtual_pad() {
    let layout = Layout::gamepad();
    let node = |inputs: Vec<Input>| NodeSpec::Mux {
        inputs,
        config: MuxConfig::new(Mode::Priority, &layout),
    };
    // A chain of muxes, each taking the one before eight times over.
    let chain = GraphSpec {
        sources: vec![source()],
        nodes: (0..20)
            .map(|at| {
                node(vec![
                    if at == 0 {
                        Input::Source(SourceId(0))
                    } else {
                        Input::Node(NodeId(at - 1), 0)
                    };
                    8
                ])
            })
            .collect(),
        sinks: vec![Input::Node(NodeId(19), 0)],
    };
    // A lattice two wide: each layer's two muxes take both of the layer
    // before, in either order.
    let layers = 32;
    let lattice = GraphSpec {
        sources: vec![source(), source()],
        nodes: (0..layers * 2)
            .map(|at| {
                let (layer, side) = (at / 2, at % 2);
                let before = |side: usize| {
                    if layer == 0 {
                        Input::Source(SourceId(side))
                    } else {
                        Input::Node(NodeId((layer - 1) * 2 + side), 0)
                    }
                };
                node(vec![before(side), before(1 - side)])
            })
            .collect(),
        sinks: vec![
            Input::Node(NodeId(layers * 2 - 2), 0),
            Input::Node(NodeId(layers * 2 - 1), 0),
        ],
    };

    let started = std::time::Instant::now();
    let mut chain = Graph::build(chain).unwrap();
    let mut lattice = Graph::build(lattice).unwrap();
    for at in 0..100 {
        let state = pad().with(SOUTH, (at % 2) as f32);
        step(&mut chain, SourceId(0), state);
        step(&mut lattice, SourceId(at % 2), state);
        assert_eq!(chain.feedback_targets(SinkId(0)), bit(SourceId(0)));
        for sink in [SinkId(0), SinkId(1)] {
            assert_eq!(
                lattice.feedback_targets(sink),
                bit(SourceId(0)) | bit(SourceId(1))
            );
        }
    }
    // Generous for a debug build on a busy machine; a walk path by path
    // would take years.
    let took = started.elapsed();
    assert!(took < std::time::Duration::from_secs(2), "{took:?}");
}

// --- feedback: how hard ------------------------------------------------------

const KEY: EffectKey = EffectKey {
    sink: SinkId(0),
    id: 0,
};
const FULL: u16 = u16::MAX;

fn rumble(strong: u16, weak: u16, length_ms: u16) -> Effect {
    Effect {
        strong,
        weak,
        length_ms,
        delay_ms: 0,
    }
}

fn level(strong: u16, weak: u16) -> Level {
    Level { strong, weak }
}

/// A router for one virtual pad over the two pads of a mux.
fn router() -> Router {
    let mut router = Router::new(1, 2);
    router.set_targets(SinkId(0), bit(PRIMARY) | bit(ASSIST));
    router
}

/// Polls and returns what was reported, with when to ask again.
fn poll(router: &mut Router, now_ms: u64) -> (Vec<(SourceId, Level)>, Option<u64>) {
    let mut reported = Vec::new();
    let next = router.poll(now_ms, &mut |source, level| reported.push((source, level)));
    (reported, next)
}

#[test]
fn a_rumble_runs_for_its_length_then_stops_by_itself() {
    let mut router = router();
    router.upload(KEY, rumble(40000, 20000, 500), 0).unwrap();
    assert_eq!(
        poll(&mut router, 0),
        (vec![], None),
        "an effect that isn't playing does nothing"
    );

    router.play(KEY, 1, 1000).unwrap();
    let on = level(40000, 20000);
    assert_eq!(
        poll(&mut router, 1000),
        (vec![(PRIMARY, on), (ASSIST, on)], Some(1500))
    );
    assert_eq!(
        poll(&mut router, 1200),
        (vec![], Some(1500)),
        "nothing to say until it ends"
    );
    let off = level(0, 0);
    assert_eq!(
        poll(&mut router, 1500),
        (vec![(PRIMARY, off), (ASSIST, off)], None)
    );
}

#[test]
fn a_rumble_without_a_length_runs_until_stopped() {
    let mut router = router();
    router.upload(KEY, rumble(FULL, 0, 0), 0).unwrap();
    router.play(KEY, 1, 0).unwrap();
    assert_eq!(poll(&mut router, 0).1, None, "there is no time to wait for");
    assert_eq!(poll(&mut router, 3_600_000).0, vec![]);
    router.play(KEY, 0, 3_600_000).unwrap();
    assert_eq!(
        poll(&mut router, 3_600_000).0,
        vec![(PRIMARY, level(0, 0)), (ASSIST, level(0, 0))]
    );
}

#[test]
fn a_delay_and_a_repeat_count_are_honored() {
    let mut router = router();
    let effect = Effect {
        strong: 1000,
        weak: 0,
        length_ms: 100,
        delay_ms: 50,
    };
    router.upload(KEY, effect, 0).unwrap();
    router.play(KEY, 2, 0).unwrap();
    let on = level(1000, 0);
    let off = level(0, 0);
    assert_eq!(
        poll(&mut router, 0),
        (vec![], Some(50)),
        "waiting out the delay"
    );
    assert_eq!(
        poll(&mut router, 50),
        (vec![(PRIMARY, on), (ASSIST, on)], Some(150))
    );
    assert_eq!(
        poll(&mut router, 150),
        (vec![(PRIMARY, off), (ASSIST, off)], Some(200)),
        "second delay"
    );
    assert_eq!(
        poll(&mut router, 200),
        (vec![(PRIMARY, on), (ASSIST, on)], Some(300))
    );
    assert_eq!(
        poll(&mut router, 300),
        (vec![(PRIMARY, off), (ASSIST, off)], None)
    );
}

#[test]
fn a_hand_off_mid_rumble_gives_the_new_pad_the_level_and_it_ends_on_time() {
    let mut graph = mux(Mode::Toggle);
    let mut router = Router::new(1, 2);
    router.set_targets(SinkId(0), graph.feedback_targets(SinkId(0)));
    router.upload(KEY, rumble(30000, 0, 500), 0).unwrap();
    router.play(KEY, 1, 0).unwrap();
    assert_eq!(
        poll(&mut router, 0),
        (vec![(PRIMARY, level(30000, 0))], Some(500))
    );

    // 200 ms in, the assist takes over.
    graph.switch(NodeId(0), &mut |_| {});
    router.set_targets(SinkId(0), graph.feedback_targets(SinkId(0)));
    assert_eq!(
        poll(&mut router, 200),
        (
            vec![(PRIMARY, level(0, 0)), (ASSIST, level(30000, 0))],
            Some(500)
        ),
        "the rumble moves, and still ends when the game meant it to"
    );
    assert_eq!(poll(&mut router, 500), (vec![(ASSIST, level(0, 0))], None));
}

#[test]
fn a_rumble_that_ended_is_not_replayed_by_a_later_hand_off() {
    let mut graph = mux(Mode::Toggle);
    let mut router = Router::new(1, 2);
    router.set_targets(SinkId(0), graph.feedback_targets(SinkId(0)));
    router.upload(KEY, rumble(30000, 0, 500), 0).unwrap();
    router.play(KEY, 1, 0).unwrap();
    poll(&mut router, 0);
    poll(&mut router, 500);

    graph.switch(NodeId(0), &mut |_| {});
    router.set_targets(SinkId(0), graph.feedback_targets(SinkId(0)));
    assert_eq!(poll(&mut router, 10_000), (vec![], None));
}

#[test]
fn effects_on_one_virtual_pad_add_and_clamp() {
    let mut router = router();
    let second = EffectKey {
        sink: SinkId(0),
        id: 1,
    };
    router.upload(KEY, rumble(40000, 100, 0), 0).unwrap();
    router.upload(second, rumble(40000, 200, 0), 0).unwrap();
    router.play(KEY, 1, 0).unwrap();
    router.play(second, 1, 0).unwrap();
    let (reported, _) = poll(&mut router, 0);
    assert_eq!(reported[0], (PRIMARY, level(FULL, 300)));
}

#[test]
fn the_strongest_virtual_pad_wins_on_a_shared_pad() {
    // Two games, each on its own virtual pad, rumbling one pad.
    let mut router = Router::new(2, 1);
    router.set_targets(SinkId(0), bit(SourceId(0)));
    router.set_targets(SinkId(1), bit(SourceId(0)));
    let first = EffectKey {
        sink: SinkId(0),
        id: 0,
    };
    let second = EffectKey {
        sink: SinkId(1),
        id: 0,
    };
    router.upload(first, rumble(50000, 1000, 0), 0).unwrap();
    router.upload(second, rumble(20000, 9000, 0), 0).unwrap();
    router.play(first, 1, 0).unwrap();
    router.play(second, 1, 0).unwrap();
    assert_eq!(
        poll(&mut router, 0).0,
        vec![(SourceId(0), level(50000, 9000))],
        "each motor takes the larger request"
    );
    // One game erasing its effect 0 leaves the other game's effect 0 alone.
    router.erase(first).unwrap();
    assert_eq!(
        poll(&mut router, 1).0,
        vec![(SourceId(0), level(20000, 9000))]
    );
}

#[test]
fn gain_scales_a_virtual_pads_rumble() {
    let mut router = router();
    router.upload(KEY, rumble(FULL, FULL, 0), 0).unwrap();
    router.play(KEY, 1, 0).unwrap();
    router.set_gain(SinkId(0), FULL / 2);
    let (reported, _) = poll(&mut router, 0);
    assert_eq!(reported[0].1, level(FULL / 2, FULL / 2));
    router.set_gain(SinkId(0), 0);
    assert_eq!(poll(&mut router, 1).0[0].1, level(0, 0));
}

#[test]
fn replacing_a_playing_effect_changes_it_in_place() {
    let mut router = router();
    router.upload(KEY, rumble(10000, 0, 0), 0).unwrap();
    router.play(KEY, 1, 0).unwrap();
    poll(&mut router, 0);
    router.upload(KEY, rumble(60000, 0, 0), 1).unwrap();
    assert_eq!(
        poll(&mut router, 1).0[0].1,
        level(60000, 0),
        "still playing, at its new strength"
    );
}

#[test]
fn replacing_a_playing_effect_restarts_its_run_as_the_kernel_does() {
    let mut router = router();
    router.upload(KEY, rumble(10000, 0, 100), 0).unwrap();
    router.play(KEY, 1, 0).unwrap();
    assert_eq!(poll(&mut router, 0).1, Some(100));
    // 60 ms in, the game updates it: the run starts over from here.
    router.upload(KEY, rumble(10000, 0, 100), 60).unwrap();
    assert_eq!(poll(&mut router, 60).1, Some(160));
}

#[test]
fn replacing_an_effect_that_finished_does_not_start_it_again() {
    let mut router = router();
    router.upload(KEY, rumble(10000, 0, 100), 0).unwrap();
    router.play(KEY, 1, 0).unwrap();
    poll(&mut router, 0);
    router.upload(KEY, rumble(10000, 0, 5000), 100).unwrap();
    assert_eq!(
        poll(&mut router, 100),
        (vec![(PRIMARY, level(0, 0)), (ASSIST, level(0, 0))], None)
    );
}

#[test]
fn a_game_is_not_told_its_rumble_goes_nowhere() {
    let mut router = Router::new(1, 2);
    // No targets at all: rumble is off, or no pad can rumble.
    assert_eq!(router.upload(KEY, rumble(FULL, FULL, 0), 0), Ok(()));
    assert_eq!(router.play(KEY, 1, 0), Ok(()));
    assert_eq!(poll(&mut router, 0), (vec![], None));
}

#[test]
fn a_virtual_pad_holds_as_many_effects_as_the_kernel_offers() {
    let mut router = router();
    for id in 0..EFFECTS as i16 {
        router
            .upload(
                EffectKey {
                    sink: SinkId(0),
                    id,
                },
                rumble(1, 1, 0),
                0,
            )
            .unwrap();
    }
    let extra = EffectKey {
        sink: SinkId(0),
        id: 99,
    };
    assert_eq!(
        router.upload(extra, rumble(1, 1, 0), 0),
        Err(FeedbackError::Full)
    );
    router.erase(KEY).unwrap();
    assert_eq!(
        router.upload(extra, rumble(1, 1, 0), 0),
        Ok(()),
        "erasing one frees its place"
    );
}

#[test]
fn requests_about_what_does_not_exist_are_refused_not_fatal() {
    let mut router = router();
    let nowhere = EffectKey {
        sink: SinkId(7),
        id: 0,
    };
    assert_eq!(
        router.upload(nowhere, rumble(1, 1, 0), 0),
        Err(FeedbackError::NoSuchSink)
    );
    assert_eq!(router.play(KEY, 1, 0), Err(FeedbackError::NoSuchEffect));
    assert_eq!(router.erase(KEY), Err(FeedbackError::NoSuchEffect));
    router.set_targets(SinkId(7), bit(PRIMARY));
    router.set_gain(SinkId(7), 1);
    router.clear(SinkId(7));
    router.source_back(SourceId(7));
    assert_eq!(poll(&mut router, 0), (vec![], None));
}

#[test]
fn a_returning_pad_is_told_the_current_level_again() {
    let mut router = router();
    router.upload(KEY, rumble(30000, 0, 0), 0).unwrap();
    router.play(KEY, 1, 0).unwrap();
    poll(&mut router, 0);
    // The assist drops out and comes back: whatever it held is gone.
    router.source_back(ASSIST);
    assert_eq!(
        poll(&mut router, 5).0,
        vec![(ASSIST, level(30000, 0))],
        "only the pad that returned"
    );
    // A pad that returns while nothing plays needs no telling.
    router.play(KEY, 0, 6).unwrap();
    poll(&mut router, 6);
    router.source_back(ASSIST);
    assert_eq!(poll(&mut router, 7).0, vec![]);
}

#[test]
fn a_closed_virtual_pad_stops_rumbling() {
    let mut router = router();
    router.upload(KEY, rumble(30000, 0, 0), 0).unwrap();
    router.play(KEY, 1, 0).unwrap();
    poll(&mut router, 0);
    router.clear(SinkId(0));
    assert_eq!(
        poll(&mut router, 1).0,
        vec![(PRIMARY, level(0, 0)), (ASSIST, level(0, 0))]
    );
}

// --- what a profile may not say ---------------------------------------------

fn build_mux(config: MuxConfig) -> Result<Graph, GraphError> {
    Graph::build(GraphSpec {
        sources: vec![source(), source()],
        nodes: vec![NodeSpec::Mux {
            inputs: vec![Input::Source(PRIMARY), Input::Source(ASSIST)],
            config,
        }],
        sinks: vec![Input::Node(NodeId(0), 0)],
    })
}

#[test]
fn a_control_beyond_the_capacity_is_refused_not_wrapped() {
    assert_eq!(
        Layout::default()
            .group(Kind::Button, &[Control::Button(64)])
            .err(),
        Some(GraphError::BadControl)
    );
    assert_eq!(
        Layout::default()
            .group(Kind::Stick, &[Control::Axis(16)])
            .err(),
        Some(GraphError::BadControl)
    );
    // And a pad state neither stores nor aliases one.
    let state = pad()
        .with(Control::Button(64), 1.0)
        .with(Control::Axis(16), 1.0);
    assert_eq!(state, PadState::NEUTRAL);
    assert_eq!(pad().with(SOUTH, 1.0).get(Control::Button(64)), 0.0);
}

#[test]
fn a_switch_must_be_a_button_on_one_of_the_inputs() {
    let layout = Layout::gamepad();
    for switch in [
        Switch {
            input: Some(1),
            button: LEFT_X,
        },
        Switch {
            input: Some(1),
            button: Control::Button(200),
        },
        Switch {
            input: Some(2),
            button: MODE,
        },
    ] {
        let mut config = MuxConfig::new(Mode::Toggle, &layout);
        config.switch = Some(switch);
        assert_eq!(
            build_mux(config).err(),
            Some(GraphError::BadControl),
            "{switch:?}"
        );
    }
    let mut config = DemuxConfig::new(DemuxMode::Unicast, 2);
    config.switch = Some(LEFT_X);
    let result = Graph::build(GraphSpec {
        sources: vec![source()],
        nodes: vec![NodeSpec::Demux {
            input: Input::Source(SourceId(0)),
            config,
        }],
        sinks: vec![Input::Node(NodeId(0), 0)],
    });
    assert_eq!(result.err(), Some(GraphError::BadControl));
}

/// A switch is taken out of the input whatever it does, so one on a node
/// that never selects would only keep a button from the game.
#[test]
fn a_switch_is_refused_on_a_node_that_never_selects() {
    let layout = Layout::gamepad();
    let switch = Some(Switch {
        input: None,
        button: SELECT,
    });
    for mode in [Mode::Priority, Mode::Average] {
        let mut config = MuxConfig::new(mode, &layout);
        assert!(!config.selects(), "{mode:?}");
        config.switch = switch;
        assert_eq!(
            build_mux(config).err(),
            Some(GraphError::BadControl),
            "{mode:?}"
        );
    }
    // One switched group is enough: that group follows the selection.
    let mut config = MuxConfig::new(Mode::Priority, &layout);
    for (_, rule) in config.rules.iter_mut().take(1) {
        *rule = Rule::Switched;
    }
    config.switch = switch;
    assert!(config.selects());
    let mut graph = build_mux(config).unwrap();
    assert_eq!(graph.selected(NodeId(0)), Some(0));
    step(&mut graph, PRIMARY, pad().with(SELECT, 1.0));
    assert_eq!(graph.selected(NodeId(0)), Some(1));
    // So is a control in no group, where there is one.
    let mut config = MuxConfig::new(Mode::Priority, &layout);
    config.ungrouped_axes = Rule::Switched;
    assert!(config.selects(), "the gamepad leaves axes in no group");
    config.rules = vec![(
        Group::of(&(0..16).map(Control::Axis).collect::<Vec<_>>()).unwrap(),
        Rule::Owned,
    )];
    assert!(!config.selects(), "every axis is in a group");

    let demux = |mode: DemuxMode| {
        let mut config = DemuxConfig::new(mode, 2);
        config.switch = Some(SELECT);
        Graph::build(GraphSpec {
            sources: vec![source()],
            nodes: vec![NodeSpec::Demux {
                input: Input::Source(SourceId(0)),
                config,
            }],
            sinks: vec![Input::Node(NodeId(0), 0)],
        })
    };
    assert_eq!(
        demux(DemuxMode::Multicast).err(),
        Some(GraphError::BadControl)
    );
    assert!(demux(DemuxMode::Unicast).is_ok());
}

#[test]
fn thresholds_must_be_numbers_in_order() {
    let layout = Layout::gamepad();
    for (engage, release) in [
        (f32::NAN, 0.05),
        (0.1, f32::NAN),
        (0.1, 0.2),
        (0.1, -0.1),
        (f32::INFINITY, 0.0),
        // Never reached: activity is at most 1.
        (1.5, 0.5),
        // Never gone below: an input would hold a group for good.
        (0.1, 0.0),
        (0.0, 0.0),
    ] {
        let mut config = MuxConfig::new(Mode::Priority, &layout);
        config.engage = engage;
        config.release = release;
        assert_eq!(
            build_mux(config).err(),
            Some(GraphError::BadThreshold),
            "{engage} {release}"
        );
    }
    for (engage, release) in [(1.0, 1.0), (1.0, 0.01), (0.1, 0.1)] {
        let mut config = MuxConfig::new(Mode::Priority, &layout);
        config.engage = engage;
        config.release = release;
        assert!(build_mux(config).is_ok(), "{engage} {release}");
    }
    let with_deadzone = |deadzone| {
        Graph::build(GraphSpec {
            sources: vec![SourceSpec {
                deadzone,
                layout: Layout::gamepad(),
            }],
            nodes: vec![],
            sinks: vec![Input::Source(SourceId(0))],
        })
    };
    // At 1 or more, a stick short of all the way would read as at rest.
    for deadzone in [f32::NAN, -0.1, f32::INFINITY, 1.0, 1.5] {
        let result = with_deadzone(deadzone);
        assert_eq!(result.err(), Some(GraphError::BadThreshold), "{deadzone}");
    }
    for deadzone in [0.0, 0.1, 0.99] {
        assert!(with_deadzone(deadzone).is_ok(), "{deadzone}");
    }
}

#[test]
fn a_graph_has_limits_on_its_size() {
    let layout = Layout::gamepad();
    let nine = Graph::build(GraphSpec {
        sources: (0..9).map(|_| source()).collect(),
        nodes: vec![NodeSpec::Mux {
            inputs: (0..9).map(|pad| Input::Source(SourceId(pad))).collect(),
            config: MuxConfig::new(Mode::Priority, &layout),
        }],
        sinks: vec![Input::Node(NodeId(0), 0)],
    });
    assert_eq!(
        nine.err(),
        Some(GraphError::BadArity),
        "a mux takes at most eight inputs"
    );
    let many = Graph::build(GraphSpec {
        sources: (0..33).map(|_| source()).collect(),
        nodes: vec![],
        sinks: vec![Input::Source(SourceId(0))],
    });
    assert_eq!(
        many.err(),
        Some(GraphError::BadArity),
        "a graph takes at most 32 pads"
    );
    // At most 64 nodes and 32 virtual pads, and exactly that many build.
    let chain = |nodes: usize, sinks: usize| {
        Graph::build(GraphSpec {
            sources: vec![source()],
            nodes: (0..nodes)
                .map(|at| NodeSpec::Mux {
                    inputs: vec![if at == 0 {
                        Input::Source(SourceId(0))
                    } else {
                        Input::Node(NodeId(at - 1), 0)
                    }],
                    config: MuxConfig::new(Mode::Priority, &layout),
                })
                .collect(),
            sinks: vec![Input::Source(SourceId(0)); sinks],
        })
    };
    assert_eq!((MAX_NODES, MAX_SINKS), (64, 32));
    assert!(chain(MAX_NODES, MAX_SINKS).is_ok());
    assert_eq!(chain(MAX_NODES + 1, 1).err(), Some(GraphError::BadArity));
    assert_eq!(chain(1, MAX_SINKS + 1).err(), Some(GraphError::BadArity));
    let none = Graph::build(GraphSpec {
        sources: vec![source()],
        nodes: vec![NodeSpec::Demux {
            input: Input::Source(SourceId(0)),
            config: DemuxConfig::new(DemuxMode::Multicast, 0),
        }],
        sinks: vec![],
    });
    assert_eq!(none.err(), Some(GraphError::BadArity));
}

#[test]
fn a_value_that_is_not_a_number_reads_as_rest() {
    let mut graph = mux(Mode::Priority);
    step(
        &mut graph,
        PRIMARY,
        pad()
            .with(LEFT_X, f32::NAN)
            .with(LEFT_Y, f32::INFINITY)
            .with(RIGHT_X, 7.0),
    );
    assert_eq!(graph.sink(SinkId(0)).get(LEFT_X), 0.0);
    assert_eq!(graph.sink(SinkId(0)).get(LEFT_Y), 0.0);
    assert_eq!(
        graph.sink(SinkId(0)).get(RIGHT_X),
        1.0,
        "an axis is held to its range"
    );
}

#[test]
fn asking_about_what_does_not_exist_is_harmless() {
    let mut graph = mux(Mode::Toggle);
    step(&mut graph, SourceId(9), pad().with(SOUTH, 1.0));
    graph.set_present(SourceId(9), false, &mut |_| {});
    graph.switch(NodeId(9), &mut |_| {});
    graph.select(NodeId(9), 0, &mut |_| {});
    graph.select(NodeId(0), 9, &mut |_| {});
    assert_eq!(*graph.sink(SinkId(9)), PadState::NEUTRAL);
    assert_eq!(graph.feedback_targets(SinkId(9)), SourceSet::EMPTY);
    assert_eq!(*graph.sink(SinkId(0)), PadState::NEUTRAL);
}

// --- controls in no group ---------------------------------------------------

/// Controls the gamepad layout doesn't name.
const EXTRA_BUTTON: Control = Control::Button(20);
const EXTRA_AXIS: Control = Control::Axis(8);

#[test]
fn ungrouped_controls_follow_the_selection_in_toggle() {
    let mut graph = mux(Mode::Toggle);
    step(
        &mut graph,
        PRIMARY,
        pad().with(EXTRA_BUTTON, 1.0).with(EXTRA_AXIS, 0.5),
    );
    step(
        &mut graph,
        ASSIST,
        pad().with(Control::Button(21), 1.0).with(EXTRA_AXIS, -0.5),
    );
    assert_eq!(graph.sink(SinkId(0)).get(EXTRA_BUTTON), 1.0);
    assert_eq!(graph.sink(SinkId(0)).get(EXTRA_AXIS), 0.5);
    graph.switch(NodeId(0), &mut |_| {});
    assert_eq!(
        graph.sink(SinkId(0)).get(EXTRA_BUTTON),
        0.0,
        "the primary's extra button went with it"
    );
    assert_eq!(graph.sink(SinkId(0)).get(Control::Button(21)), 1.0);
    assert_eq!(graph.sink(SinkId(0)).get(EXTRA_AXIS), -0.5);
}

#[test]
fn ungrouped_controls_obey_priority_as_groups_of_one() {
    let mut graph = mux(Mode::Priority);
    step(&mut graph, PRIMARY, pad().with(EXTRA_AXIS, 0.9));
    assert_eq!(graph.sink(SinkId(0)).get(EXTRA_AXIS), 0.9);
    step(
        &mut graph,
        ASSIST,
        pad().with(EXTRA_AXIS, -0.4).with(EXTRA_BUTTON, 1.0),
    );
    assert_eq!(
        graph.sink(SinkId(0)).get(EXTRA_AXIS),
        -0.4,
        "the assist owns an axis it moves"
    );
    assert_eq!(graph.sink(SinkId(0)).get(EXTRA_BUTTON), 1.0);
}

// --- more of the review's cases ---------------------------------------------

#[test]
fn a_deadzone_applies_to_a_group_that_also_has_buttons() {
    let stick = SourceSpec {
        deadzone: 0.15,
        layout: flight_stick(),
    };
    let mut graph = Graph::build(GraphSpec {
        sources: vec![stick],
        nodes: vec![],
        sinks: vec![Input::Source(SourceId(0))],
    })
    .unwrap();
    step(
        &mut graph,
        SourceId(0),
        pad()
            .with(Control::Axis(0), 0.05)
            .with(Control::Button(0), 1.0),
    );
    assert_eq!(
        graph.sink(SinkId(0)).get(Control::Axis(0)),
        0.0,
        "rest noise on the grip is zeroed"
    );
    assert_eq!(
        graph.sink(SinkId(0)).get(Control::Button(0)),
        1.0,
        "its buttons are untouched"
    );
}

#[test]
fn a_selection_never_rests_on_a_pad_that_is_gone() {
    let mut graph = mux(Mode::Toggle);
    step(&mut graph, PRIMARY, pad().with(SOUTH, 1.0));
    step(&mut graph, ASSIST, pad().with(EAST, 1.0).with(MODE, 1.0));
    assert_eq!(
        graph.sink(SinkId(0)).get(EAST),
        1.0,
        "the assist has control"
    );
    // The assist's battery dies. Control must not be stranded with it,
    // along with the only button that could hand it back.
    graph.set_present(ASSIST, false, &mut |_| {});
    assert_eq!(
        graph.sink(SinkId(0)).get(SOUTH),
        1.0,
        "control returned to the pad that is there"
    );
    assert_eq!(graph.feedback_targets(SinkId(0)), bit(PRIMARY));
    graph.set_present(ASSIST, true, &mut |_| {});
    assert_eq!(
        graph.sink(SinkId(0)).get(SOUTH),
        1.0,
        "and stays there until someone switches"
    );
}

#[test]
fn a_client_selects_an_input_by_name_so_a_repeat_is_harmless() {
    let mut graph = mux(Mode::Toggle);
    step(&mut graph, PRIMARY, pad().with(SOUTH, 1.0));
    step(&mut graph, ASSIST, pad().with(EAST, 1.0));
    graph.select(NodeId(0), 1, &mut |_| {});
    graph.select(NodeId(0), 1, &mut |_| {});
    assert_eq!(
        graph.sink(SinkId(0)).get(EAST),
        1.0,
        "selecting twice is not switching twice"
    );
}

#[test]
fn a_mux_takes_more_than_two_pads() {
    let layout = Layout::gamepad();
    let mut graph = Graph::build(GraphSpec {
        sources: vec![source(), source(), source()],
        nodes: vec![NodeSpec::Mux {
            inputs: vec![
                Input::Source(SourceId(0)),
                Input::Source(SourceId(1)),
                Input::Source(SourceId(2)),
            ],
            config: MuxConfig::new(Mode::Priority, &layout),
        }],
        sinks: vec![Input::Node(NodeId(0), 0)],
    })
    .unwrap();
    step(&mut graph, SourceId(0), pad().with(LEFT_X, 0.3));
    step(&mut graph, SourceId(1), pad().with(LEFT_X, 0.6));
    assert_eq!(graph.sink(SinkId(0)).get(LEFT_X), 0.6);
    step(&mut graph, SourceId(2), pad().with(LEFT_X, -0.9));
    assert_eq!(
        graph.sink(SinkId(0)).get(LEFT_X),
        -0.9,
        "later inputs outrank earlier ones"
    );
    step(&mut graph, SourceId(2), pad());
    assert_eq!(graph.sink(SinkId(0)).get(LEFT_X), 0.6);
}

// --- naming things ----------------------------------------------------------

#[test]
fn a_set_of_pads_holds_what_it_is_given_and_nothing_past_its_capacity() {
    let set = SourceSet::of(&[
        SourceId(0),
        SourceId(5),
        SourceId(31),
        SourceId(32),
        SourceId(900),
    ]);
    assert_eq!(
        set.iter().collect::<Vec<_>>(),
        [SourceId(0), SourceId(5), SourceId(31)]
    );
    assert!(set.contains(SourceId(5)));
    assert!(
        !set.contains(SourceId(32)),
        "a pad past the capacity is never a member"
    );
    assert!(!set.contains(SourceId(900)));
    assert!(SourceSet::EMPTY.is_empty());
    assert_eq!(
        SourceSet::from(SourceId(2)) | SourceId(4).into(),
        SourceSet::of(&[SourceId(2), SourceId(4)])
    );
}
