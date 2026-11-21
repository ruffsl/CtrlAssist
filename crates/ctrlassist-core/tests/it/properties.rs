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
