//! Invariants that must hold after any sequence of events.

use std::collections::HashMap;

use ctrlassist_core::gamepad::*;
use ctrlassist_core::{
    Command, Control, Effect, EffectKey, Graph, GraphSpec, Input, Layout, Mode, MuxConfig,
    NodeSpec, PadState, Reply, Router, SourceSpec,
};
use proptest::prelude::*;

const BUTTONS: [Control; 11] = [
    SOUTH,
    EAST,
    NORTH,
    WEST,
    LEFT_BUMPER,
    RIGHT_BUMPER,
    SELECT,
    START,
    LEFT_THUMB,
    RIGHT_THUMB,
    MODE,
];
const DPAD: [Control; 4] = [DPAD_UP, DPAD_DOWN, DPAD_LEFT, DPAD_RIGHT];
const STICKS: [[Control; 2]; 2] = [[LEFT_X, LEFT_Y], [RIGHT_X, RIGHT_Y]];
const TRIGGERS: [Control; 2] = [LEFT_TRIGGER, RIGHT_TRIGGER];

/// A pad state whose axes are either at rest or well past every threshold,
/// so the model below needs no deadzone or hysteresis of its own.
fn pad_state(with_mode: bool) -> impl Strategy<Value = PadState> {
    let stick = prop::sample::select(vec![-1.0f32, -0.5, 0.0, 0.5, 1.0]);
    let trigger = prop::sample::select(vec![0.0f32, 0.3, 0.7, 1.0]);
    (
        prop::collection::vec(any::<bool>(), 15),
        prop::collection::vec(stick, 4),
        prop::collection::vec(trigger, 2),
    )
        .prop_map(move |(buttons, sticks, triggers)| {
            let mut state = PadState::NEUTRAL;
            for (control, pressed) in BUTTONS.iter().chain(&DPAD).zip(buttons) {
                if *control != MODE || with_mode {
                    state.set(*control, f32::from(u8::from(pressed)));
                }
            }
            for (control, value) in STICKS.iter().flatten().zip(sticks) {
                state.set(*control, value);
            }
            for (control, value) in TRIGGERS.iter().zip(triggers) {
                state.set(*control, value);
            }
            state
        })
}

fn mux(mode: Mode) -> Graph {
    let source = || SourceSpec {
        deadzone: 0.1,
        layout: Layout::gamepad(),
    };
    Graph::build(GraphSpec {
        sources: vec![source(), source()],
        nodes: vec![NodeSpec::Mux {
            inputs: vec![Input::Source(0), Input::Source(1)],
            config: MuxConfig::new(mode, &Layout::gamepad()),
        }],
        sinks: vec![Input::Node(0, 0)],
    })
    .unwrap()
}

fn any_held(state: &PadState, controls: &[Control]) -> bool {
    controls.iter().any(|control| state.get(*control) != 0.0)
}

proptest! {
    /// Priority: the assist owns what it touches and nothing else, and the
    /// changes reported always add up to what the virtual pad holds.
    #[test]
    fn priority_gives_the_assist_exactly_what_it_touches(
        steps in prop::collection::vec((0usize..2, pad_state(true)), 1..60)
    ) {
        let mut graph = mux(Mode::Priority);
        let mut pads = [PadState::NEUTRAL; 2];
        let mut shadow = PadState::NEUTRAL;
        for (source, state) in steps {
            pads[source] = state;
            graph.update(source, state, &mut |change| shadow.set(change.control, change.value));
            let [primary, assist] = pads;
            let sink = *graph.sink(0);
            prop_assert_eq!(shadow, sink, "the reported changes rebuild the virtual pad");

            for button in BUTTONS {
                let expected = primary.get(button) != 0.0 || assist.get(button) != 0.0;
                prop_assert_eq!(sink.get(button) != 0.0, expected, "{:?}", button);
            }
            let dpad_owner = if any_held(&assist, &DPAD) { assist } else { primary };
            for button in DPAD {
                prop_assert_eq!(sink.get(button), dpad_owner.get(button));
            }
            for stick in STICKS {
                let owner = if any_held(&assist, &stick) { assist } else { primary };
                for axis in stick {
                    prop_assert_eq!(sink.get(axis), owner.get(axis));
                }
            }
            for trigger in TRIGGERS {
                prop_assert_eq!(sink.get(trigger), primary.get(trigger).max(assist.get(trigger)));
            }
        }
    }

    /// Average: every output axis lies between the two inputs, and a
    /// button is down exactly when someone holds it.
    #[test]
    fn average_stays_between_its_inputs(
        steps in prop::collection::vec((0usize..2, pad_state(true)), 1..60)
    ) {
        let mut graph = mux(Mode::Average);
        let mut pads = [PadState::NEUTRAL; 2];
        for (source, state) in steps {
            pads[source] = state;
            graph.update(source, state, &mut |_| {});
            let [primary, assist] = pads;
            let sink = *graph.sink(0);
            for axis in STICKS.iter().flatten().chain(&TRIGGERS) {
                let (low, high) = (
                    primary.get(*axis).min(assist.get(*axis)),
                    primary.get(*axis).max(assist.get(*axis)),
                );
                prop_assert!((low..=high).contains(&sink.get(*axis)), "{:?}", axis);
            }
            for button in BUTTONS.iter().chain(&DPAD) {
                let expected = primary.get(*button) != 0.0 || assist.get(*button) != 0.0;
                prop_assert_eq!(sink.get(*button) != 0.0, expected);
            }
        }
    }

    /// Toggle: after any mix of events and switches, the virtual pad holds
    /// exactly what the selected pad holds.
    #[test]
    fn toggle_always_equals_the_selected_pad(
        steps in prop::collection::vec(
            prop_oneof![
                (0usize..2, pad_state(false)).prop_map(Some),
                Just(None),
            ],
            1..80,
        )
    ) {
        let mut graph = mux(Mode::Toggle);
        let mut pads = [PadState::NEUTRAL; 2];
        let mut selected = 0;
        for step in steps {
            match step {
                Some((source, state)) => {
                    pads[source] = state;
                    graph.update(source, state, &mut |_| {});
                }
                None => {
                    selected = 1 - selected;
                    graph.switch(0, &mut |_| {});
                }
            }
            prop_assert_eq!(*graph.sink(0), pads[selected]);
            prop_assert_eq!(graph.feedback_targets(0), 1 << selected);
        }
    }
}

// --- feedback ---------------------------------------------------------------

const SOURCES: usize = 3;
const SINKS: usize = 2;

#[derive(Clone, Debug)]
enum Op {
    Upload {
        key: EffectKey,
        effect: Effect,
        accept: u32,
    },
    Play {
        key: EffectKey,
        playing: bool,
    },
    Erase {
        key: EffectKey,
    },
    Retarget {
        sink: usize,
        targets: u32,
    },
    Gone {
        source: usize,
    },
    Back {
        source: usize,
    },
}

fn key() -> impl Strategy<Value = EffectKey> {
    (0..SINKS, 0i16..3).prop_map(|(sink, id)| EffectKey { sink, id })
}

fn op() -> impl Strategy<Value = Op> {
    let mask = 0u32..(1 << SOURCES);
    prop_oneof![
        (key(), 1u16..4, mask.clone()).prop_map(|(key, strong, accept)| Op::Upload {
            key,
            effect: Effect { strong, weak: 0 },
            accept
        }),
        (key(), any::<bool>()).prop_map(|(key, playing)| Op::Play { key, playing }),
        key().prop_map(|key| Op::Erase { key }),
        (0..SINKS, mask).prop_map(|(sink, targets)| Op::Retarget { sink, targets }),
        (0..SOURCES).prop_map(|source| Op::Gone { source }),
        (0..SOURCES).prop_map(|source| Op::Back { source }),
    ]
}

/// A pad, as far as effects go: what it holds and whether each is playing.
type Device = HashMap<EffectKey, (Effect, bool)>;

proptest! {
    /// Whatever the games and the pads do, each connected pad holds
    /// exactly the effects meant for it, in the right state, and nothing
    /// is ever sent to a pad that is gone.
    #[test]
    fn every_pad_holds_exactly_the_effects_meant_for_it(ops in prop::collection::vec(op(), 1..120)) {
        let mut router = Router::new(SINKS);
        let mut devices: [Device; SOURCES] = Default::default();
        let mut present = [true; SOURCES];
        let mut targets = [0u32; SINKS];
        // What the games believe exists: effect and playing flag per key.
        let mut live: HashMap<EffectKey, (Effect, bool)> = HashMap::new();
        // Pads that refused a live effect, which stay without it.
        let mut refused: HashMap<EffectKey, u32> = HashMap::new();
        let mut commands = Vec::new();

        for op in ops {
            commands.clear();
            let mut rejected_by = 0;
            match op {
                Op::Upload { key, effect, accept } => {
                    let asked = router.upload(key, effect, &mut commands);
                    let accepted = asked & accept;
                    rejected_by = asked & !accept;
                    let playing = live.get(&key).is_some_and(|(_, playing)| *playing);
                    match router.confirm(key, accepted, &mut commands) {
                        Reply::Accepted => {
                            live.insert(key, (effect, playing));
                            refused.insert(key, rejected_by);
                        }
                        Reply::Rejected => {
                            prop_assert!(asked != 0 && accepted == 0);
                            live.remove(&key);
                            refused.remove(&key);
                        }
                    }
                }
                Op::Play { key, playing } => {
                    router.play(key, playing, &mut commands);
                    if let Some(entry) = live.get_mut(&key) {
                        entry.1 = playing;
                    }
                }
                Op::Erase { key } => {
                    router.erase(key, &mut commands);
                    live.remove(&key);
                    refused.remove(&key);
                }
                Op::Retarget { sink, targets: new } => {
                    router.retarget(sink, new, &mut commands);
                    // A pad that joins again gets another chance.
                    for (key, mask) in &mut refused {
                        if key.sink == sink {
                            *mask &= targets[sink] & new;
                        }
                    }
                    targets[sink] = new;
                }
                Op::Gone { source } => {
                    if present[source] {
                        present[source] = false;
                        devices[source].clear();
                        router.source_gone(source);
                    }
                }
                Op::Back { source } => {
                    if !present[source] {
                        present[source] = true;
                        router.source_back(source, &mut commands);
                        for mask in refused.values_mut() {
                            *mask &= !(1 << source);
                        }
                    }
                }
            }

            for command in &commands {
                let (Command::Upload { target, .. }
                | Command::Erase { target, .. }
                | Command::Play { target, .. }
                | Command::Stop { target, .. }) = *command;
                prop_assert!(present[target], "{command:?} was sent to a pad that is gone");
                match *command {
                    Command::Upload { target, key, effect } => {
                        if rejected_by & (1 << target) != 0 {
                            devices[target].remove(&key);
                        } else {
                            let playing = devices[target].get(&key).is_some_and(|(_, playing)| *playing);
                            devices[target].insert(key, (effect, playing));
                        }
                    }
                    Command::Erase { target, key } => {
                        prop_assert!(devices[target].remove(&key).is_some(), "erased what the pad never had");
                    }
                    Command::Play { target, key } => devices[target].get_mut(&key).unwrap().1 = true,
                    Command::Stop { target, key } => devices[target].get_mut(&key).unwrap().1 = false,
                }
            }

            for (source, device) in devices.iter().enumerate() {
                let mut expected = Device::new();
                if present[source] {
                    for (key, state) in &live {
                        let wanted = targets[key.sink] & (1 << source) != 0;
                        let refused = refused.get(key).is_some_and(|mask| mask & (1 << source) != 0);
                        if wanted && !refused {
                            expected.insert(*key, *state);
                        }
                    }
                }
                prop_assert_eq!(device, &expected, "pad {}", source);
            }
        }
    }
}

// --- nothing a profile or a pad can say panics --------------------------------

use ctrlassist_core::{
    DemuxConfig, DemuxFeedback, DemuxMode, Group, Kind, MuxFeedback, Rule, Switch,
};

/// Mostly controls that exist, sometimes ones that don't.
fn any_control() -> impl Strategy<Value = Control> {
    prop_oneof![
        4 => (0u8..64).prop_map(Control::Button),
        4 => (0u8..16).prop_map(Control::Axis),
        1 => any::<u8>().prop_map(Control::Button),
        1 => any::<u8>().prop_map(Control::Axis),
    ]
}

/// Mostly a plausible threshold, sometimes anything a float can be.
fn any_threshold() -> impl Strategy<Value = f32> {
    prop_oneof![9 => 0.0f32..0.5, 1 => any::<f32>()]
}

/// Mostly a source, sometimes a node's output, either of which may not exist.
fn any_input() -> impl Strategy<Value = Input> {
    prop_oneof![
        6 => (0usize..3).prop_map(Input::Source),
        2 => (0usize..3, 0usize..2).prop_map(|(node, output)| Input::Node(node, output)),
        1 => (0usize..40).prop_map(Input::Source),
    ]
}

fn any_rule() -> impl Strategy<Value = Rule> {
    prop::sample::select(vec![
        Rule::Owned,
        Rule::Largest,
        Rule::Blended,
        Rule::Switched,
    ])
}

/// A mux configuration with nothing checked: any thresholds, any switch,
/// any groups that can be built at all.
fn any_mux() -> impl Strategy<Value = MuxConfig> {
    (
        prop::collection::vec(
            (prop::collection::vec(any_control(), 0..5), any_rule()),
            0..6,
        ),
        any_rule(),
        any_rule(),
        any_threshold(),
        any_threshold(),
        prop::option::of((prop::option::of(0usize..3), any_control())),
        prop_oneof![
            Just(MuxFeedback::None),
            Just(MuxFeedback::Active),
            any::<u32>().prop_map(MuxFeedback::Inputs),
        ],
    )
        .prop_map(
            |(groups, ungrouped_buttons, ungrouped_axes, engage, release, switch, feedback)| {
                MuxConfig {
                    rules: groups
                        .into_iter()
                        .filter_map(|(controls, rule)| Some((Group::of(&controls).ok()?, rule)))
                        .collect(),
                    ungrouped_buttons,
                    ungrouped_axes,
                    // Usually in order, as a real profile's would be.
                    engage: if engage.is_finite() && release.is_finite() {
                        engage.max(release)
                    } else {
                        engage
                    },
                    release: if engage.is_finite() && release.is_finite() {
                        engage.min(release)
                    } else {
                        release
                    },
                    switch: switch.map(|(input, button)| Switch { input, button }),
                    feedback,
                }
            },
        )
}

fn any_node() -> impl Strategy<Value = NodeSpec> {
    prop_oneof![
        (prop::collection::vec(any_input(), 1..5), any_mux())
            .prop_map(|(inputs, config)| NodeSpec::Mux { inputs, config }),
        (
            any_input(),
            prop::sample::select(vec![DemuxMode::Unicast, DemuxMode::Multicast]),
            prop_oneof![9 => 1usize..4, 1 => 0usize..20],
            prop::option::of(any_control()),
            prop::sample::select(vec![DemuxFeedback::None, DemuxFeedback::Active]),
        )
            .prop_map(|(input, mode, outputs, switch, feedback)| NodeSpec::Demux {
                input,
                config: DemuxConfig {
                    mode,
                    outputs,
                    switch,
                    feedback
                },
            }),
    ]
}

fn any_source() -> impl Strategy<Value = SourceSpec> {
    (
        any_threshold(),
        prop::collection::vec(prop::collection::vec(any_control(), 0..5), 0..4),
    )
        .prop_map(|(deadzone, groups)| {
            let mut layout = Layout::default();
            for controls in groups {
                if let Ok(next) = layout.clone().group(Kind::Stick, &controls) {
                    layout = next;
                }
            }
            SourceSpec { deadzone, layout }
        })
}

/// A pad state with nothing checked either: any control, any value.
fn any_state() -> impl Strategy<Value = PadState> {
    prop::collection::vec((any_control(), any::<f32>()), 0..12).prop_map(|controls| {
        let mut state = PadState::NEUTRAL;
        for (control, value) in controls {
            state.set(control, value);
        }
        state
    })
}

#[derive(Clone, Debug)]
enum Poke {
    Update(usize, PadState),
    Present(usize, bool),
    Switch(usize),
    Select(usize, usize),
}

fn any_poke() -> impl Strategy<Value = Poke> {
    prop_oneof![
        (0usize..6, any_state()).prop_map(|(source, state)| Poke::Update(source, state)),
        (0usize..6, any::<bool>()).prop_map(|(source, present)| Poke::Present(source, present)),
        (0usize..6).prop_map(Poke::Switch),
        (0usize..6, 0usize..20).prop_map(|(node, index)| Poke::Select(node, index)),
    ]
}

/// Whatever a profile says, building it either refuses or yields a graph
/// that no pad state, dropout or request can make panic, and whose virtual
/// pads only ever hold values in range.
#[test]
fn nothing_a_profile_or_a_pad_says_can_panic_a_step() {
    use proptest::test_runner::{Config, TestRunner};
    use std::cell::Cell;

    const CASES: u32 = 4000;
    let built = Cell::new(0u32);
    let strategy = (
        prop::collection::vec(any_source(), 3..5),
        prop::collection::vec(any_node(), 0..4),
        prop::collection::vec(any_input(), 0..4),
        prop::collection::vec(any_poke(), 0..40),
    );
    TestRunner::new(Config {
        cases: CASES,
        ..Config::default()
    })
    .run(&strategy, |(sources, nodes, sinks, pokes)| {
        let sink_count = sinks.len();
        let Ok(mut graph) = Graph::build(GraphSpec {
            sources,
            nodes,
            sinks,
        }) else {
            return Ok(());
        };
        built.set(built.get() + 1);
        for poke in pokes {
            match poke {
                Poke::Update(source, state) => graph.update(source, state, &mut |_| {}),
                Poke::Present(source, present) => graph.set_present(source, present, &mut |_| {}),
                Poke::Switch(node) => graph.switch(node, &mut |_| {}),
                Poke::Select(node, index) => graph.select(node, index, &mut |_| {}),
            }
            for sink in 0..=sink_count {
                let state = graph.sink(sink);
                for axis in 0..16 {
                    let value = state.get(Control::Axis(axis));
                    prop_assert!(
                        (-1.0..=1.0).contains(&value),
                        "axis {axis} of sink {sink} is {value}"
                    );
                }
                let _ = graph.feedback_targets(sink);
            }
        }
        Ok(())
    })
    .unwrap();
    // The generators lean toward profiles that build; if they stop doing
    // so, this property is only testing refusals.
    assert!(
        built.get() > CASES / 5,
        "only {} of {CASES} generated profiles were accepted",
        built.get()
    );
}
