//! The routing behavior, written as scenarios before the nodes existed.
//!
//! Each test builds a graph the way a profile would, steps it with pad
//! states, and checks what the virtual pads hold.

use ctrlassist_core::gamepad::*;
use ctrlassist_core::{
    Control, DemuxConfig, DemuxFeedback, DemuxMode, EFFECTS, Effect, EffectKey, FeedbackError,
    Graph, GraphError, GraphSpec, Input, Kind, Layout, Level, Mode, MuxConfig, MuxFeedback,
    NodeSpec, PadState, Router, SourceSpec, Switch,
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
    step(
        &mut graph,
        PRIMARY,
        pad().with(SOUTH, 1.0).with(LEFT_X, 0.8),
    );
    assert_eq!(graph.sink(0).get(SOUTH), 1.0);
    assert_eq!(graph.sink(0).get(LEFT_X), 0.8);
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
    assert_eq!(graph.sink(0).get(LEFT_X), -0.5);
    assert_eq!(graph.sink(0).get(LEFT_Y), 0.0);
    // The other stick is untouched by the assist and stays the primary's.
    step(
        &mut graph,
        PRIMARY,
        pad().with(LEFT_X, 0.9).with(LEFT_Y, 0.9).with(RIGHT_X, 0.4),
    );
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
    assert_eq!(
        graph.sink(0).get(SOUTH),
        1.0,
        "the primary's release must not reach the game"
    );
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
    assert_eq!(
        graph.sink(0).get(LEFT_Y),
        0.07,
        "0.07 is above release, so the assist keeps it"
    );
    step(&mut graph, ASSIST, pad().with(LEFT_Y, 0.02));
    assert_eq!(
        graph.sink(0).get(LEFT_X),
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
    assert_eq!(graph.sink(0).get(LEFT_X), 0.9);
    assert_eq!(graph.sink(0).get(LEFT_Y), 0.0);
}

// --- average ----------------------------------------------------------------

#[test]
fn average_blends_sticks_both_are_using() {
    let mut graph = mux(Mode::Average);
    step(&mut graph, PRIMARY, pad().with(LEFT_X, 1.0));
    assert_eq!(
        graph.sink(0).get(LEFT_X),
        1.0,
        "alone, the primary is not halved"
    );
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
        graph.sink(0).get(SOUTH),
        0.0,
        "held only by the outgoing pad: released"
    );
    assert_eq!(
        graph.sink(0).get(EAST),
        1.0,
        "held only by the incoming pad: pressed"
    );
    assert_eq!(graph.sink(0).get(LEFT_X), 0.0);
    assert_eq!(
        graph.sink(0).get(MODE),
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
        graph.sink(0).get(EAST),
        1.0,
        "holding the switch must not switch back"
    );
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
    assert_eq!(
        graph.sink(0).get(EAST),
        0.0,
        "Mode on the assist no longer switches"
    );
    step(&mut graph, PRIMARY, pad().with(SELECT, 1.0));
    assert_eq!(graph.sink(0).get(EAST), 1.0);
    assert_eq!(
        graph.sink(0).get(MODE),
        1.0,
        "Mode is an ordinary button again"
    );
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
    assert_eq!(
        graph.sink(1).get(MODE),
        0.0,
        "the switch button is consumed"
    );
    step(&mut graph, 0, held);
    step(&mut graph, 0, held.with(MODE, 1.0));
    assert_eq!(
        graph.sink(0).get(SOUTH),
        1.0,
        "past the last pad it wraps to the first"
    );
}

#[test]
fn multicast_copies_the_pad_to_every_virtual_pad() {
    let mut graph = demux(DemuxMode::Multicast, 2);
    step(&mut graph, 0, pad().with(SOUTH, 1.0).with(MODE, 1.0));
    for sink in 0..2 {
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
    assert_eq!(
        graph.sink(0).get(LEFT_X),
        -0.5,
        "the helper starts on player one"
    );
    assert_eq!(graph.sink(1).get(LEFT_X), 0.8);
    step(&mut graph, 2, pad().with(LEFT_X, -0.5).with(MODE, 1.0));
    assert_eq!(
        graph.sink(0).get(LEFT_X),
        0.9,
        "player one has the stick back"
    );
    assert_eq!(
        graph.sink(1).get(LEFT_X),
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
    step(
        &mut graph,
        ASSIST,
        pad().with(LEFT_X, -0.5).with(SOUTH, 1.0),
    );
    graph.set_present(ASSIST, false, &mut |_| {});
    assert_eq!(
        graph.sink(0).get(SOUTH),
        0.0,
        "nothing stays held by a pad that is gone"
    );
    assert_eq!(graph.sink(0).get(LEFT_X), 0.9);
    graph.set_present(ASSIST, true, &mut |_| {});
    step(&mut graph, ASSIST, pad().with(LEFT_X, -0.7));
    assert_eq!(
        graph.sink(0).get(LEFT_X),
        -0.7,
        "it is the assist again when it returns"
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
        sinks: vec![Input::Node(0, 0)],
    })
    .unwrap();
    let fire = Control::Button(0);
    let twist = Control::Axis(2);
    step(
        &mut graph,
        PRIMARY,
        pad().with(fire, 1.0).with(Control::Axis(0), 0.6),
    );
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
        pad()
            .with(fire, 1.0)
            .with(Control::Axis(0), 0.6)
            .with(Control::Axis(3), 0.9),
    );
    assert_eq!(graph.sink(0).get(Control::Axis(3)), 0.9);
}

// --- feedback: where it goes -------------------------------------------------

fn bit(source: usize) -> u32 {
    1 << source
}

#[test]
fn rumble_reaches_both_pads_of_a_priority_mux() {
    assert_eq!(
        mux(Mode::Priority).feedback_targets(0),
        bit(PRIMARY) | bit(ASSIST)
    );
}

#[test]
fn toggle_rumbles_only_the_active_pad() {
    let mut graph = mux(Mode::Toggle);
    assert_eq!(graph.feedback_targets(0), bit(PRIMARY));
    graph.switch(0, &mut |_| {});
    assert_eq!(graph.feedback_targets(0), bit(ASSIST));
}

#[test]
fn rumble_can_go_to_the_pad_that_is_not_providing_input() {
    let mut config = MuxConfig::new(Mode::Toggle, &Layout::gamepad());
    config.feedback = MuxFeedback::Inputs(bit(ASSIST));
    let graph = mux_graph(config);
    assert_eq!(
        graph.feedback_targets(0),
        bit(ASSIST),
        "the primary drives, the assist feels it"
    );
}

#[test]
fn rumble_can_be_off() {
    let mut config = MuxConfig::new(Mode::Priority, &Layout::gamepad());
    config.feedback = MuxFeedback::None;
    assert_eq!(mux_graph(config).feedback_targets(0), 0);
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

// --- feedback: how hard ------------------------------------------------------

const KEY: EffectKey = EffectKey { sink: 0, id: 0 };
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
    router.set_targets(0, bit(PRIMARY) | bit(ASSIST));
    router
}

/// Polls and returns what was reported, with when to ask again.
fn poll(router: &mut Router, now_ms: u64) -> (Vec<(usize, Level)>, Option<u64>) {
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
    router.set_targets(0, graph.feedback_targets(0));
    router.upload(KEY, rumble(30000, 0, 500), 0).unwrap();
    router.play(KEY, 1, 0).unwrap();
    assert_eq!(
        poll(&mut router, 0),
        (vec![(PRIMARY, level(30000, 0))], Some(500))
    );

    // 200 ms in, the assist takes over.
    graph.switch(0, &mut |_| {});
    router.set_targets(0, graph.feedback_targets(0));
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
    router.set_targets(0, graph.feedback_targets(0));
    router.upload(KEY, rumble(30000, 0, 500), 0).unwrap();
    router.play(KEY, 1, 0).unwrap();
    poll(&mut router, 0);
    poll(&mut router, 500);

    graph.switch(0, &mut |_| {});
    router.set_targets(0, graph.feedback_targets(0));
    assert_eq!(poll(&mut router, 10_000), (vec![], None));
}

#[test]
fn effects_on_one_virtual_pad_add_and_clamp() {
    let mut router = router();
    let second = EffectKey { sink: 0, id: 1 };
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
    router.set_targets(0, bit(0));
    router.set_targets(1, bit(0));
    let first = EffectKey { sink: 0, id: 0 };
    let second = EffectKey { sink: 1, id: 0 };
    router.upload(first, rumble(50000, 1000, 0), 0).unwrap();
    router.upload(second, rumble(20000, 9000, 0), 0).unwrap();
    router.play(first, 1, 0).unwrap();
    router.play(second, 1, 0).unwrap();
    assert_eq!(
        poll(&mut router, 0).0,
        vec![(0, level(50000, 9000))],
        "each motor takes the larger request"
    );
    // One game erasing its effect 0 leaves the other game's effect 0 alone.
    router.erase(first).unwrap();
    assert_eq!(poll(&mut router, 1).0, vec![(0, level(20000, 9000))]);
}

#[test]
fn gain_scales_a_virtual_pads_rumble() {
    let mut router = router();
    router.upload(KEY, rumble(FULL, FULL, 0), 0).unwrap();
    router.play(KEY, 1, 0).unwrap();
    router.set_gain(0, FULL / 2);
    let (reported, _) = poll(&mut router, 0);
    assert_eq!(reported[0].1, level(FULL / 2, FULL / 2));
    router.set_gain(0, 0);
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
            .upload(EffectKey { sink: 0, id }, rumble(1, 1, 0), 0)
            .unwrap();
    }
    let extra = EffectKey { sink: 0, id: 99 };
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
    let nowhere = EffectKey { sink: 7, id: 0 };
    assert_eq!(
        router.upload(nowhere, rumble(1, 1, 0), 0),
        Err(FeedbackError::NoSuchSink)
    );
    assert_eq!(router.play(KEY, 1, 0), Err(FeedbackError::NoSuchEffect));
    assert_eq!(router.erase(KEY), Err(FeedbackError::NoSuchEffect));
    router.set_targets(7, 1);
    router.set_gain(7, 1);
    router.clear(7);
    router.source_back(7);
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
    router.clear(0);
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
        sinks: vec![Input::Node(0, 0)],
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
            input: Input::Source(0),
            config,
        }],
        sinks: vec![Input::Node(0, 0)],
    });
    assert_eq!(result.err(), Some(GraphError::BadControl));
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
    for deadzone in [f32::NAN, -0.1, f32::INFINITY] {
        let result = Graph::build(GraphSpec {
            sources: vec![SourceSpec {
                deadzone,
                layout: Layout::gamepad(),
            }],
            nodes: vec![],
            sinks: vec![Input::Source(0)],
        });
        assert_eq!(result.err(), Some(GraphError::BadThreshold), "{deadzone}");
    }
}

#[test]
fn a_graph_has_limits_on_its_size() {
    let layout = Layout::gamepad();
    let nine = Graph::build(GraphSpec {
        sources: (0..9).map(|_| source()).collect(),
        nodes: vec![NodeSpec::Mux {
            inputs: (0..9).map(Input::Source).collect(),
            config: MuxConfig::new(Mode::Priority, &layout),
        }],
        sinks: vec![Input::Node(0, 0)],
    });
    assert_eq!(
        nine.err(),
        Some(GraphError::BadArity),
        "a mux takes at most eight inputs"
    );
    let many = Graph::build(GraphSpec {
        sources: (0..33).map(|_| source()).collect(),
        nodes: vec![],
        sinks: vec![Input::Source(0)],
    });
    assert_eq!(
        many.err(),
        Some(GraphError::BadArity),
        "a graph takes at most 32 pads"
    );
    let none = Graph::build(GraphSpec {
        sources: vec![source()],
        nodes: vec![NodeSpec::Demux {
            input: Input::Source(0),
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
    assert_eq!(graph.sink(0).get(LEFT_X), 0.0);
    assert_eq!(graph.sink(0).get(LEFT_Y), 0.0);
    assert_eq!(
        graph.sink(0).get(RIGHT_X),
        1.0,
        "an axis is held to its range"
    );
}

#[test]
fn asking_about_what_does_not_exist_is_harmless() {
    let mut graph = mux(Mode::Toggle);
    step(&mut graph, 9, pad().with(SOUTH, 1.0));
    graph.set_present(9, false, &mut |_| {});
    graph.switch(9, &mut |_| {});
    graph.select(9, 0, &mut |_| {});
    graph.select(0, 9, &mut |_| {});
    assert_eq!(*graph.sink(9), PadState::NEUTRAL);
    assert_eq!(graph.feedback_targets(9), 0);
    assert_eq!(*graph.sink(0), PadState::NEUTRAL);
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
    assert_eq!(graph.sink(0).get(EXTRA_BUTTON), 1.0);
    assert_eq!(graph.sink(0).get(EXTRA_AXIS), 0.5);
    graph.switch(0, &mut |_| {});
    assert_eq!(
        graph.sink(0).get(EXTRA_BUTTON),
        0.0,
        "the primary's extra button went with it"
    );
    assert_eq!(graph.sink(0).get(Control::Button(21)), 1.0);
    assert_eq!(graph.sink(0).get(EXTRA_AXIS), -0.5);
}

#[test]
fn ungrouped_controls_obey_priority_as_groups_of_one() {
    let mut graph = mux(Mode::Priority);
    step(&mut graph, PRIMARY, pad().with(EXTRA_AXIS, 0.9));
    assert_eq!(graph.sink(0).get(EXTRA_AXIS), 0.9);
    step(
        &mut graph,
        ASSIST,
        pad().with(EXTRA_AXIS, -0.4).with(EXTRA_BUTTON, 1.0),
    );
    assert_eq!(
        graph.sink(0).get(EXTRA_AXIS),
        -0.4,
        "the assist owns an axis it moves"
    );
    assert_eq!(graph.sink(0).get(EXTRA_BUTTON), 1.0);
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
        sinks: vec![Input::Source(0)],
    })
    .unwrap();
    step(
        &mut graph,
        0,
        pad()
            .with(Control::Axis(0), 0.05)
            .with(Control::Button(0), 1.0),
    );
    assert_eq!(
        graph.sink(0).get(Control::Axis(0)),
        0.0,
        "rest noise on the grip is zeroed"
    );
    assert_eq!(
        graph.sink(0).get(Control::Button(0)),
        1.0,
        "its buttons are untouched"
    );
}

#[test]
fn a_selection_never_rests_on_a_pad_that_is_gone() {
    let mut graph = mux(Mode::Toggle);
    step(&mut graph, PRIMARY, pad().with(SOUTH, 1.0));
    step(&mut graph, ASSIST, pad().with(EAST, 1.0).with(MODE, 1.0));
    assert_eq!(graph.sink(0).get(EAST), 1.0, "the assist has control");
    // The assist's battery dies. Control must not be stranded with it,
    // along with the only button that could hand it back.
    graph.set_present(ASSIST, false, &mut |_| {});
    assert_eq!(
        graph.sink(0).get(SOUTH),
        1.0,
        "control returned to the pad that is there"
    );
    assert_eq!(graph.feedback_targets(0), bit(PRIMARY));
    graph.set_present(ASSIST, true, &mut |_| {});
    assert_eq!(
        graph.sink(0).get(SOUTH),
        1.0,
        "and stays there until someone switches"
    );
}

#[test]
fn a_client_selects_an_input_by_name_so_a_repeat_is_harmless() {
    let mut graph = mux(Mode::Toggle);
    step(&mut graph, PRIMARY, pad().with(SOUTH, 1.0));
    step(&mut graph, ASSIST, pad().with(EAST, 1.0));
    graph.select(0, ASSIST, &mut |_| {});
    graph.select(0, ASSIST, &mut |_| {});
    assert_eq!(
        graph.sink(0).get(EAST),
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
            inputs: vec![Input::Source(0), Input::Source(1), Input::Source(2)],
            config: MuxConfig::new(Mode::Priority, &layout),
        }],
        sinks: vec![Input::Node(0, 0)],
    })
    .unwrap();
    step(&mut graph, 0, pad().with(LEFT_X, 0.3));
    step(&mut graph, 1, pad().with(LEFT_X, 0.6));
    assert_eq!(graph.sink(0).get(LEFT_X), 0.6);
    step(&mut graph, 2, pad().with(LEFT_X, -0.9));
    assert_eq!(
        graph.sink(0).get(LEFT_X),
        -0.9,
        "later inputs outrank earlier ones"
    );
    step(&mut graph, 2, pad());
    assert_eq!(graph.sink(0).get(LEFT_X), 0.6);
}
