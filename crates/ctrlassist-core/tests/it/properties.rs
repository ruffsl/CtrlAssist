//! Invariants that must hold after any sequence of events.

use std::collections::HashMap;

use ctrlassist_core::gamepad::*;
use ctrlassist_core::{
    Contact, Control, Effect, EffectKey, Graph, GraphSpec, Input, Layout, Level, Mode, Motion,
    MuxConfig, NodeId, NodeSpec, PadState, Router, SinkId, SourceId, SourceSet, SourceSpec,
    Touchpad,
};
use proptest::prelude::*;

/// Every button but the D-pad's, the back buttons, the touchpads' clicks
/// and capacitive touch among them, which are in no group.
const BUTTONS: [Control; 21] = [
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
    TOUCHPAD,
    SECOND_TOUCHPAD,
    PADDLE_1,
    PADDLE_2,
    PADDLE_3,
    PADDLE_4,
    LEFT_STICK_TOUCH,
    RIGHT_STICK_TOUCH,
    LEFT_GRIP_TOUCH,
    RIGHT_GRIP_TOUCH,
    MODE,
];
const DPAD: [Control; 4] = [DPAD_UP, DPAD_DOWN, DPAD_LEFT, DPAD_RIGHT];
const STICKS: [[Control; 2]; 2] = [[LEFT_X, LEFT_Y], [RIGHT_X, RIGHT_Y]];
const TRIGGERS: [Control; 2] = [LEFT_TRIGGER, RIGHT_TRIGGER];

/// A reading of the sensors, its count telling one from another.
fn motion() -> impl Strategy<Value = Motion> {
    let rate = prop::sample::select(vec![-2000.0f32, -30.0, 0.0, 0.5, 400.0]);
    let pull = prop::sample::select(vec![-1.0f32, 0.0, 0.7, 1.0, 3.5]);
    (
        prop::array::uniform3(rate),
        prop::array::uniform3(pull),
        any::<u32>(),
        prop::sample::select(vec![0u32, 1000, 4000]),
    )
        .prop_map(|(gyro, accel, readings, interval)| Motion::new(gyro, accel, readings, interval))
}

/// A pad's touchpads, the first then the second.
fn touchpads(pad: &PadState) -> [Touchpad; 2] {
    [pad.touchpad(), pad.second_touchpad()]
}

/// Whether a pad has a finger on its touchpad `which`.
fn touching(pad: &PadState, which: usize) -> bool {
    touchpads(pad)[which].contacts().iter().any(Contact::down)
}

/// Whether a virtual pad's contacts on its touchpad `which` are `owner`'s,
/// or lifted, as they are for the step in which one pad's fingers give way
/// to another's.
fn shows(sink: &PadState, owner: &PadState, which: usize) -> bool {
    touchpads(sink)[which] == touchpads(owner)[which] || !touching(sink, which)
}

/// A pad state with neither touchpad touched, to compare the rest.
fn untouched(pad: PadState) -> PadState {
    pad.with_touchpad(Touchpad::UNTOUCHED)
        .with_second_touchpad(Touchpad::UNTOUCHED)
}

/// Fingers on a touchpad, or none.
fn touchpad() -> impl Strategy<Value = Touchpad> {
    let place = prop::sample::select(vec![0.0f32, 0.25, 0.5, 1.0]);
    let contact =
        (any::<bool>(), place.clone(), place).prop_map(|(down, x, y)| Contact::new(down, x, y));
    prop::array::uniform2(contact).prop_map(Touchpad::new)
}

/// A pad state whose axes are either at rest or well past every threshold,
/// so the model below needs no deadzone or hysteresis of its own.
fn pad_state(with_mode: bool) -> impl Strategy<Value = PadState> {
    let stick = prop::sample::select(vec![-1.0f32, -0.5, 0.0, 0.5, 1.0]);
    let trigger = prop::sample::select(vec![0.0f32, 0.3, 0.7, 1.0]);
    (
        prop::collection::vec(any::<bool>(), BUTTONS.len() + DPAD.len()),
        prop::collection::vec(stick, 4),
        prop::collection::vec(trigger, 2),
        motion(),
        touchpad(),
        touchpad(),
    )
        .prop_map(move |(buttons, sticks, triggers, motion, first, second)| {
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
                .with_motion(motion)
                .with_touchpad(first)
                .with_second_touchpad(second)
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
            inputs: vec![Input::Source(SourceId(0)), Input::Source(SourceId(1))],
            config: MuxConfig::new(mode, &Layout::gamepad()),
        }],
        sinks: vec![Input::Node(NodeId(0), 0)],
    })
    .unwrap()
}

fn any_held(state: &PadState, controls: &[Control]) -> bool {
    controls.iter().any(|control| state.get(*control) != 0.0)
}

proptest! {
    /// Priority: the assist owns what it touches and nothing else, the
    /// changes reported always add up to the controls the virtual pad
    /// holds, and its motion is the primary's and each touchpad's contacts
    /// the assist's while they touch that touchpad, and the primary's
    /// otherwise. A capacitive touch, a loose button, owns no stick.
    #[test]
    fn priority_gives_the_assist_exactly_what_it_touches(
        steps in prop::collection::vec((0usize..2, pad_state(true)), 1..60)
    ) {
        let mut graph = mux(Mode::Priority);
        let mut pads = [PadState::NEUTRAL; 2];
        let mut shadow = PadState::NEUTRAL;
        for (source, state) in steps {
            pads[source] = state;
            graph.update(SourceId(source), state, &mut |change| shadow.set(change.control, change.value));
            let [primary, assist] = pads;
            let sink = *graph.sink(SinkId(0));
            prop_assert_eq!(sink.motion(), primary.motion());
            for which in 0..2 {
                let owner = if touching(&assist, which) { assist } else { primary };
                prop_assert!(
                    shows(&sink, &owner, which),
                    "the assist's touchpad {} while they touch it: {:?}",
                    which,
                    touchpads(&sink)
                );
            }
            // No change reports motion or the contacts: they are written
            // whole.
            let rebuilt = shadow
                .with_motion(sink.motion())
                .with_touchpad(sink.touchpad())
                .with_second_touchpad(sink.second_touchpad());
            prop_assert_eq!(rebuilt, sink, "the reported changes rebuild the virtual pad's controls");

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

    /// Average: every output axis lies between the two inputs, a button
    /// is down exactly when someone holds it, and nothing of motion or the
    /// contacts is blended: motion is the primary's, and each touchpad's
    /// contacts one pad's, whole: whoever touched it first, until they
    /// lift, whoever has the other.
    #[test]
    fn average_stays_between_its_inputs(
        steps in prop::collection::vec((0usize..2, pad_state(true)), 1..60)
    ) {
        let mut graph = mux(Mode::Average);
        let mut pads = [PadState::NEUTRAL; 2];
        // Whose each touchpad is, kept the obvious way.
        let mut holders = [0usize; 2];
        for (source, state) in steps {
            pads[source] = state;
            graph.update(SourceId(source), state, &mut |_| {});
            let [primary, assist] = pads;
            let sink = *graph.sink(SinkId(0));
            prop_assert_eq!(sink.motion(), primary.motion());
            for (which, holder) in holders.iter_mut().enumerate() {
                if !touching(&pads[*holder], which) {
                    *holder = (0..2).find(|pad| touching(&pads[*pad], which)).unwrap_or(*holder);
                }
                prop_assert!(
                    shows(&sink, &pads[*holder], which),
                    "touchpad {} is whoever touched it first, whole: {:?}",
                    which,
                    touchpads(&sink)
                );
            }
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
    /// exactly what the selected pad holds, motion and contacts with the
    /// rest, the fingers lifted for a step where a switch changes whose
    /// they are.
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
                    graph.update(SourceId(source), state, &mut |_| {});
                }
                None => {
                    selected = 1 - selected;
                    graph.switch(NodeId(0), &mut |_| {});
                }
            }
            let sink = *graph.sink(SinkId(0));
            prop_assert_eq!(untouched(sink), untouched(pads[selected]));
            for which in 0..2 {
                prop_assert!(shows(&sink, &pads[selected], which), "{}: {:?}", which, touchpads(&sink));
            }
            prop_assert_eq!(graph.feedback_targets(SinkId(0)), SourceSet::from(SourceId(selected)));
        }
    }
}

// --- feedback ---------------------------------------------------------------

const SOURCES: usize = 3;
const SINKS: usize = 2;

#[derive(Clone, Debug)]
enum Op {
    Upload(EffectKey, Effect),
    Play(EffectKey, u32),
    Erase(EffectKey),
    Targets(usize, u32),
    Gain(usize, u16),
    Clear(usize),
    Back(usize),
    /// Let this many milliseconds pass.
    Wait(u64),
}

fn key() -> impl Strategy<Value = EffectKey> {
    (0..SINKS, 0i16..3).prop_map(|(sink, id)| EffectKey {
        sink: SinkId(sink),
        id,
    })
}

fn effect() -> impl Strategy<Value = Effect> {
    (
        prop::sample::select(vec![0u16, 1, 20000, 50000, u16::MAX]),
        prop::sample::select(vec![0u16, 300, u16::MAX]),
        prop::sample::select(vec![0u16, 1, 40, 250]),
        prop::sample::select(vec![0u16, 0, 30]),
    )
        .prop_map(|(strong, weak, length_ms, delay_ms)| Effect {
            strong,
            weak,
            length_ms,
            delay_ms,
        })
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (key(), effect()).prop_map(|(key, effect)| Op::Upload(key, effect)),
        3 => (key(), 0u32..4).prop_map(|(key, count)| Op::Play(key, count)),
        1 => key().prop_map(Op::Erase),
        2 => (0..SINKS, 0u32..(1 << SOURCES)).prop_map(|(sink, targets)| Op::Targets(sink, targets)),
        1 => (0..SINKS, prop::sample::select(vec![0u16, 30000, u16::MAX])).prop_map(|(sink, gain)| Op::Gain(sink, gain)),
        1 => (0..SINKS).prop_map(Op::Clear),
        1 => (0..SOURCES).prop_map(Op::Back),
        4 => prop::sample::select(vec![1u64, 10, 39, 40, 41, 70, 250, 1000]).prop_map(Op::Wait),
    ]
}

/// The games' side of the story, kept the obvious way.
#[derive(Default)]
struct Model {
    effects: HashMap<EffectKey, (Effect, Option<(u64, u32)>)>,
    targets: [u32; SINKS],
    gain: [u16; SINKS],
}

impl Model {
    /// Whether an effect played at `started`, `count` times, is running at `now`.
    fn running(effect: Effect, started: u64, count: u32, now: u64) -> bool {
        let (delay, length) = (u64::from(effect.delay_ms), u64::from(effect.length_ms));
        let elapsed = now - started;
        if length == 0 {
            return elapsed >= delay;
        }
        // Walk the runs one at a time rather than dividing, so this isn't
        // the router's arithmetic written twice.
        let mut run_start = 0;
        for _ in 0..count {
            if (run_start + delay..run_start + delay + length).contains(&elapsed) {
                return true;
            }
            run_start += delay + length;
        }
        false
    }

    fn level(&self, source: usize, now: u64) -> Level {
        let mut level = Level::default();
        for sink in 0..SINKS {
            if self.targets[sink] & (1 << source) == 0 {
                continue;
            }
            let (mut strong, mut weak) = (0u64, 0u64);
            for (key, (effect, play)) in &self.effects {
                if let Some((started, count)) = play
                    && key.sink == SinkId(sink)
                    && Self::running(*effect, *started, *count, now)
                {
                    strong += u64::from(effect.strong) * u64::from(self.gain[sink]) / 65535;
                    weak += u64::from(effect.weak) * u64::from(self.gain[sink]) / 65535;
                }
            }
            level.strong = level.strong.max(strong.min(65535) as u16);
            level.weak = level.weak.max(weak.min(65535) as u16);
        }
        level
    }
}

proptest! {
    /// Whatever the games do and however time passes, each pad has been
    /// told exactly the level the games' effects add up to; and when the
    /// router says nothing changes until a given time, nothing does.
    #[test]
    fn every_pad_is_told_exactly_the_level_the_effects_add_up_to(ops in prop::collection::vec(op(), 1..120)) {
        let mut router = Router::new(SINKS, SOURCES);
        let mut model = Model { gain: [u16::MAX; SINKS], ..Model::default() };
        // What each pad was last told.
        let mut told = [Level::default(); SOURCES];
        let mut now = 0u64;

        for op in ops {
            match op {
                Op::Upload(key, effect) => {
                    prop_assert_eq!(router.upload(key, effect, now), Ok(()));
                    let entry = model.effects.entry(key).or_insert((effect, None));
                    // Replacing a playing effect starts its current run
                    // over with the runs it has left; a finished one stays
                    // finished.
                    if let (old, Some((started, count))) = *entry {
                        let period = u64::from(old.delay_ms) + u64::from(old.length_ms);
                        let mut done = 0;
                        while old.length_ms != 0 && started + (done + 1) * period <= now {
                            done += 1;
                        }
                        entry.1 = (done < u64::from(count)).then(|| (now, count - done as u32));
                    }
                    entry.0 = effect;
                }
                Op::Play(key, count) => {
                    let known = model.effects.get_mut(&key);
                    prop_assert_eq!(router.play(key, count, now).is_ok(), known.is_some());
                    if let Some(entry) = known {
                        entry.1 = (count > 0).then_some((now, count));
                    }
                }
                Op::Erase(key) => {
                    prop_assert_eq!(router.erase(key).is_ok(), model.effects.remove(&key).is_some());
                }
                Op::Targets(sink, targets) => {
                    let set: Vec<SourceId> = (0..SOURCES).filter(|source| targets & (1 << source) != 0).map(SourceId).collect();
                    router.set_targets(SinkId(sink), SourceSet::of(&set));
                    model.targets[sink] = targets;
                }
                Op::Gain(sink, gain) => {
                    router.set_gain(SinkId(sink), gain);
                    model.gain[sink] = gain;
                }
                Op::Clear(sink) => {
                    router.clear(SinkId(sink));
                    model.effects.retain(|key, _| key.sink != SinkId(sink));
                }
                Op::Back(source) => {
                    router.source_back(SourceId(source));
                    // The pad forgot what it was told.
                    told[source] = Level::default();
                }
                Op::Wait(millis) => now += millis,
            }

            let next = router.poll(now, &mut |source, level| told[source.0] = level);
            for (source, told) in told.iter().enumerate() {
                prop_assert_eq!(*told, model.level(source, now), "pad {} at {} ms", source, now);
            }
            match next {
                Some(next) => {
                    prop_assert!(next > now, "asked to poll again at {next}, which is not after {now}");
                    for source in 0..SOURCES {
                        prop_assert_eq!(model.level(source, next - 1), model.level(source, now), "pad {} changed before {}", source, next);
                    }
                }
                None => {
                    for source in 0..SOURCES {
                        prop_assert_eq!(model.level(source, now + 1_000_000), model.level(source, now), "pad {} changed with no poll due", source);
                    }
                }
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
        6 => (0usize..3).prop_map(|pad| Input::Source(SourceId(pad))),
        2 => (0usize..3, 0usize..2).prop_map(|(node, output)| Input::Node(NodeId(node), output)),
        1 => (0usize..40).prop_map(|pad| Input::Source(SourceId(pad))),
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
        any_rule(),
        // Mostly within the 0 to 1 a mux takes, so that most build.
        prop_oneof![19 => 0.01f32..=1.0, 1 => any::<f32>()],
        prop_oneof![19 => 0.01f32..=1.0, 1 => any::<f32>()],
        prop::option::of((prop::option::of(0usize..3), any_control())),
        prop_oneof![
            Just(MuxFeedback::None),
            Just(MuxFeedback::Active),
            any::<u32>().prop_map(MuxFeedback::Inputs),
        ],
        prop::bool::weighted(0.25),
    )
        .prop_map(
            |(
                groups,
                ungrouped_buttons,
                ungrouped_axes,
                touchpad,
                engage,
                release,
                switch,
                feedback,
                stray,
            )| {
                let mut config = MuxConfig {
                    rules: groups
                        .into_iter()
                        .filter_map(|(controls, rule)| Some((Group::of(&controls).ok()?, rule)))
                        .collect(),
                    ungrouped_buttons,
                    ungrouped_axes,
                    touchpad,
                    gyro: None,
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
                };
                // Usually a switch only where something follows it, as
                // in a real profile; sometimes one that is refused.
                if !stray && !config.selects() {
                    config.switch = None;
                }
                config
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
            prop::bool::weighted(0.25),
        )
            .prop_map(|(input, mode, outputs, switch, feedback, stray)| {
                NodeSpec::Demux {
                    input,
                    config: DemuxConfig {
                        mode,
                        outputs,
                        // Usually a switch only on a unicast, which
                        // selects; sometimes one that is refused.
                        switch: switch.filter(|_| stray || mode == DemuxMode::Unicast),
                        feedback,
                    },
                }
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

/// Mostly a value in a sensor's or a touchpad's range, sometimes anything
/// a float can be.
fn any_value() -> impl Strategy<Value = f32> {
    prop_oneof![3 => -4.0f32..4.0, 1 => any::<f32>()]
}

/// A pad state with nothing checked either: any control, any value, and
/// any reading and contacts.
fn any_state() -> impl Strategy<Value = PadState> {
    let contact =
        (any::<bool>(), any_value(), any_value()).prop_map(|(down, x, y)| Contact::new(down, x, y));
    (
        prop::collection::vec((any_control(), any::<f32>()), 0..12),
        prop::array::uniform3(any_value()),
        prop::array::uniform3(any_value()),
        any::<u32>(),
        any::<u32>(),
        prop::array::uniform2(prop::array::uniform2(contact)),
    )
        .prop_map(
            |(controls, gyro, accel, readings, interval, [first, second])| {
                let mut state = PadState::NEUTRAL;
                for (control, value) in controls {
                    state.set(control, value);
                }
                state
                    .with_motion(Motion::new(gyro, accel, readings, interval))
                    .with_touchpad(Touchpad::new(first))
                    .with_second_touchpad(Touchpad::new(second))
            },
        )
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
/// pads only ever hold values in range: every motion value a finite
/// number, and every place on its touchpad.
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
                Poke::Update(source, state) => graph.update(SourceId(source), state, &mut |_| {}),
                Poke::Present(source, present) => {
                    graph.set_present(SourceId(source), present, &mut |_| {});
                }
                Poke::Switch(node) => graph.switch(NodeId(node), &mut |_| {}),
                Poke::Select(node, index) => graph.select(NodeId(node), index, &mut |_| {}),
            }
            for sink in 0..=sink_count {
                let state = graph.sink(SinkId(sink));
                for axis in 0..16 {
                    let value = state.get(Control::Axis(axis));
                    prop_assert!(
                        (-1.0..=1.0).contains(&value),
                        "axis {axis} of sink {sink} is {value}"
                    );
                }
                let motion = state.motion();
                for value in motion.gyro().iter().chain(&motion.accel()) {
                    prop_assert!(value.is_finite(), "sink {sink}'s motion is {motion:?}");
                }
                for contact in touchpads(state).iter().flat_map(Touchpad::contacts) {
                    let on = |place: f32| (0.0..=1.0).contains(&place);
                    prop_assert!(
                        on(contact.x()) && on(contact.y()),
                        "sink {sink}'s contact is {contact:?}"
                    );
                }
                let _ = graph.feedback_targets(SinkId(sink));
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

/// Where a virtual pad's feedback goes, found the slow way: walking back
/// from it along every path, as the architecture describes it.
fn walk_back(spec: &GraphSpec, graph: &Graph, input: Input) -> SourceSet {
    match input {
        Input::Source(source) => source.into(),
        Input::Node(node, output) => match &spec.nodes[node.0] {
            NodeSpec::Mux { inputs, config } => {
                let all = (1u32 << inputs.len()) - 1;
                let ports = match config.feedback {
                    MuxFeedback::None => 0,
                    MuxFeedback::Inputs(mask) => mask & all,
                    MuxFeedback::Active => {
                        graph.selected(node).map_or(all, |selected| 1 << selected)
                    }
                };
                let mut targets = SourceSet::EMPTY;
                for (port, input) in inputs.iter().enumerate() {
                    if ports & (1 << port) != 0 {
                        targets |= walk_back(spec, graph, *input);
                    }
                }
                targets
            }
            NodeSpec::Demux { input, config } => {
                let reached = config.feedback == DemuxFeedback::Active
                    && graph
                        .selected(node)
                        .is_none_or(|selected| selected == output);
                if reached {
                    walk_back(spec, graph, *input)
                } else {
                    SourceSet::EMPTY
                }
            }
        },
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]

    /// The feedback each step works out, node by node, is where a walk
    /// back from the virtual pad along every path would end, after any
    /// sequence of events.
    #[test]
    fn feedback_goes_where_a_walk_back_from_the_virtual_pad_ends(
        sources in prop::collection::vec(any_source(), 3..5),
        nodes in prop::collection::vec(any_node(), 0..4),
        sinks in prop::collection::vec(any_input(), 0..4),
        pokes in prop::collection::vec(any_poke(), 0..40),
    ) {
        let spec = GraphSpec { sources, nodes, sinks };
        let Ok(mut graph) = Graph::build(spec.clone()) else {
            return Ok(());
        };
        for poke in pokes {
            match poke {
                Poke::Update(source, state) => graph.update(SourceId(source), state, &mut |_| {}),
                Poke::Present(source, present) => {
                    graph.set_present(SourceId(source), present, &mut |_| {});
                }
                Poke::Switch(node) => graph.switch(NodeId(node), &mut |_| {}),
                Poke::Select(node, index) => graph.select(NodeId(node), index, &mut |_| {}),
            }
            for (sink, input) in spec.sinks.iter().enumerate() {
                prop_assert_eq!(
                    graph.feedback_targets(SinkId(sink)),
                    walk_back(&spec, &graph, *input),
                    "virtual pad {}", sink
                );
            }
        }
    }
}
