//! From a pad's event codes to positions in a pad state.
//!
//! This is the layout the kernel's gamepad drivers agree on (the Xbox and
//! PlayStation families among them): face buttons by position, sticks on
//! X/Y and RX/RY, triggers on Z and RZ, the directional pad as a hat or
//! as four buttons. A pad that doesn't follow it needs a controller
//! database mapping instead.
//!
//! Everything here is plain data and arithmetic, so that it can be
//! applied on the input thread: applying an event neither allocates nor
//! can panic, whatever the event holds.

use ctrlassist_core::{AXES, BUTTONS, Control, PadState, gamepad};

/// Event types, from the kernel's `input-event-codes.h`.
const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;

/// One more than the highest key code and the highest absolute axis code.
const KEYS: usize = 0x300;
const ABSOLUTES: usize = 0x40;

const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_Z: u16 = 0x02;
const ABS_RX: u16 = 0x03;
const ABS_RY: u16 = 0x04;
const ABS_RZ: u16 = 0x05;
const ABS_GAS: u16 = 0x09;
const ABS_BRAKE: u16 = 0x0a;
const ABS_HAT0X: u16 = 0x10;
const ABS_HAT0Y: u16 = 0x11;

const BTN_SOUTH: u16 = 0x130;
const BTN_EAST: u16 = 0x131;
const BTN_NORTH: u16 = 0x133;
const BTN_WEST: u16 = 0x134;
const BTN_TL: u16 = 0x136;
const BTN_TR: u16 = 0x137;
const BTN_TL2: u16 = 0x138;
const BTN_TR2: u16 = 0x139;
const BTN_SELECT: u16 = 0x13a;
const BTN_START: u16 = 0x13b;
const BTN_MODE: u16 = 0x13c;
const BTN_THUMBL: u16 = 0x13d;
const BTN_THUMBR: u16 = 0x13e;
const BTN_DPAD_UP: u16 = 0x220;
const BTN_DPAD_DOWN: u16 = 0x221;
const BTN_DPAD_LEFT: u16 = 0x222;
const BTN_DPAD_RIGHT: u16 = 0x223;

/// The first positions past the gamepad's own, where a pad's other
/// buttons and axes go, in the order of their codes. A button passes by
/// every place with a name: see [`spare_button`].
const FIRST_SPARE_BUTTON: u8 = 15;
const FIRST_SPARE_AXIS: u8 = 6;

/// The first button place at or after `from` that no gamepad name has,
/// if one is left. A pad's other key never lands on a named place, so a
/// profile that names a back button means a back button, not whatever
/// key a pad with none happens to have.
fn spare_button(from: u8) -> Option<u8> {
    (from..=u8::MAX)
        .take_while(|place| usize::from(*place) < BUTTONS)
        .find(|place| !gamepad::NAMED_BUTTONS.contains(&Control::Button(*place)))
}

/// The values an absolute axis reports, as the pad declares them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AxisRange {
    /// The lowest value.
    pub min: i32,
    /// The highest value.
    pub max: i32,
}

impl AxisRange {
    /// A value on an axis that rests in the middle, as -1 through 1. The
    /// resting value reads as exactly zero: for a range with an even
    /// count of values, such as 0 through 255, the middle is the upper of
    /// the two central values, which is where such pads rest.
    fn centered(self, value: i32) -> f32 {
        let (min, max, value) = (i64::from(self.min), i64::from(self.max), i64::from(value));
        if max <= min {
            return 0.0;
        }
        let middle = (min + max + 1).div_euclid(2);
        let span = if value >= middle {
            max - middle
        } else {
            middle - min
        };
        if span == 0 {
            return 0.0;
        }
        ((value - middle) as f32 / span as f32).clamp(-1.0, 1.0)
    }

    /// A value on an axis that rests at its lowest, as 0 through 1.
    fn rested(self, value: i32) -> f32 {
        let (min, max, value) = (i64::from(self.min), i64::from(self.max), i64::from(value));
        if max <= min {
            return 0.0;
        }
        ((value - min) as f32 / (max - min) as f32).clamp(0.0, 1.0)
    }
}

/// Where a value lands in a pad state.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Out {
    /// A button: pressed when the value is past halfway.
    Button(u8),
    /// A whole axis that rests in the middle.
    Axis(u8),
    /// An axis that rests at zero: a trigger.
    Trigger(u8),
    /// One side of an axis that rests in the middle: the value pushes it
    /// toward this sign, and letting go returns it to rest only if it is
    /// on this side, so the control for the other side isn't undone.
    Side(u8, f32),
}

impl Out {
    /// Writes an amount from 0 through 1.
    fn amount(self, state: &mut PadState, amount: f32) {
        match self {
            Self::Button(index) => {
                state.set(Control::Button(index), if amount > 0.5 { 1.0 } else { 0.0 });
            }
            Self::Axis(index) | Self::Trigger(index) => state.set(Control::Axis(index), amount),
            Self::Side(index, sign) => {
                let control = Control::Axis(index);
                if amount > 0.0 {
                    state.set(control, sign * amount);
                } else if state.get(control) * sign > 0.0 {
                    state.set(control, 0.0);
                }
            }
        }
    }
}

/// What a key code does.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Key {
    Unmapped,
    To(Out),
}

/// What an absolute axis code does.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Absolute {
    Unmapped,
    /// The whole axis to one place.
    Whole {
        range: AxisRange,
        inverted: bool,
        out: Out,
    },
    /// Each side of the middle to its own place, as a hat's directions.
    Split {
        range: AxisRange,
        negative: Option<Out>,
        positive: Option<Out>,
    },
}

/// Where each of one pad's event codes lands in a pad state.
#[derive(Clone, Debug)]
pub struct Mapping {
    pub(crate) keys: Box<[Key; KEYS]>,
    pub(crate) absolutes: [Absolute; ABSOLUTES],
}

fn index(control: Control) -> u8 {
    match control {
        Control::Button(index) | Control::Axis(index) => index,
    }
}

impl Mapping {
    /// A mapping with no place for anything.
    pub(crate) fn empty() -> Self {
        Self {
            keys: Box::new([Key::Unmapped; KEYS]),
            absolutes: [Absolute::Unmapped; ABSOLUTES],
        }
    }

    /// The mapping for a pad with these key codes and absolute axes,
    /// under the kernel's gamepad layout. Codes beyond the gamepad's own
    /// take the spare positions in code order until those run out,
    /// passing by every place a gamepad name has; the rest are dropped.
    #[must_use]
    pub fn kernel(keys: &[u16], absolutes: &[(u16, AxisRange)]) -> Self {
        let has = |code: u16| absolutes.iter().any(|(axis, _)| *axis == code);
        // With a right stick on RX/RY, Z and RZ are the triggers. Without
        // one, they are the right stick, as on plain HID pads.
        let z_is_trigger = has(ABS_RX) && has(ABS_RY);
        let analog_triggers =
            (z_is_trigger && (has(ABS_Z) || has(ABS_RZ))) || has(ABS_GAS) || has(ABS_BRAKE);

        let mut mapping = Self::empty();

        let mut spare = FIRST_SPARE_BUTTON;
        let mut sorted: Vec<u16> = keys.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        for code in sorted {
            let Some(slot) = mapping.keys.get_mut(usize::from(code)) else {
                continue;
            };
            let button = |control: Control| Key::To(Out::Button(index(control)));
            *slot = match code {
                BTN_SOUTH => button(gamepad::SOUTH),
                BTN_EAST => button(gamepad::EAST),
                BTN_NORTH => button(gamepad::NORTH),
                BTN_WEST => button(gamepad::WEST),
                BTN_TL => button(gamepad::LEFT_BUMPER),
                BTN_TR => button(gamepad::RIGHT_BUMPER),
                BTN_SELECT => button(gamepad::SELECT),
                BTN_START => button(gamepad::START),
                BTN_MODE => button(gamepad::MODE),
                BTN_THUMBL => button(gamepad::LEFT_THUMB),
                BTN_THUMBR => button(gamepad::RIGHT_THUMB),
                BTN_DPAD_UP => button(gamepad::DPAD_UP),
                BTN_DPAD_DOWN => button(gamepad::DPAD_DOWN),
                BTN_DPAD_LEFT => button(gamepad::DPAD_LEFT),
                BTN_DPAD_RIGHT => button(gamepad::DPAD_RIGHT),
                // A pad with analog triggers reports these clicks as
                // well; the axis already says everything they do.
                BTN_TL2 | BTN_TR2 if analog_triggers => Key::Unmapped,
                BTN_TL2 => Key::To(Out::Trigger(index(gamepad::LEFT_TRIGGER))),
                BTN_TR2 => Key::To(Out::Trigger(index(gamepad::RIGHT_TRIGGER))),
                _ => match spare_button(spare) {
                    Some(place) => {
                        spare = place + 1;
                        Key::To(Out::Button(place))
                    }
                    None => Key::Unmapped,
                },
            };
        }

        let mut spare = FIRST_SPARE_AXIS;
        let mut sorted: Vec<(u16, AxisRange)> = absolutes.to_vec();
        sorted.sort_unstable_by_key(|(code, _)| *code);
        sorted.dedup_by_key(|(code, _)| *code);
        for (code, range) in sorted {
            let Some(slot) = mapping.absolutes.get_mut(usize::from(code)) else {
                continue;
            };
            let whole = |out: Out| Absolute::Whole {
                range,
                inverted: false,
                out,
            };
            let centered = |control: Control| whole(Out::Axis(index(control)));
            let rested = |control: Control| whole(Out::Trigger(index(control)));
            let hat = |negative: Control, positive: Control| Absolute::Split {
                range,
                negative: Some(Out::Button(index(negative))),
                positive: Some(Out::Button(index(positive))),
            };
            *slot = match code {
                ABS_X => centered(gamepad::LEFT_X),
                ABS_Y => centered(gamepad::LEFT_Y),
                ABS_RX => centered(gamepad::RIGHT_X),
                ABS_RY => centered(gamepad::RIGHT_Y),
                ABS_Z if z_is_trigger => rested(gamepad::LEFT_TRIGGER),
                ABS_RZ if z_is_trigger => rested(gamepad::RIGHT_TRIGGER),
                ABS_Z => centered(gamepad::RIGHT_X),
                ABS_RZ => centered(gamepad::RIGHT_Y),
                ABS_BRAKE => rested(gamepad::LEFT_TRIGGER),
                ABS_GAS => rested(gamepad::RIGHT_TRIGGER),
                ABS_HAT0X => hat(gamepad::DPAD_LEFT, gamepad::DPAD_RIGHT),
                ABS_HAT0Y => hat(gamepad::DPAD_UP, gamepad::DPAD_DOWN),
                _ if usize::from(spare) < AXES => {
                    spare += 1;
                    whole(Out::Axis(spare - 1))
                }
                _ => Absolute::Unmapped,
            };
        }
        mapping
    }

    /// Applies one event to a pad state. An event this mapping has no
    /// place for changes nothing.
    pub fn apply(&self, state: &mut PadState, kind: u16, code: u16, value: i32) {
        match kind {
            EV_KEY => {
                if let Some(Key::To(out)) = self.keys.get(usize::from(code)) {
                    out.amount(state, if value == 0 { 0.0 } else { 1.0 });
                }
            }
            EV_ABS => match self.absolutes.get(usize::from(code)) {
                Some(Absolute::Whole {
                    range,
                    inverted,
                    out,
                }) => match out {
                    Out::Axis(index) => {
                        let value = range.centered(value);
                        state.set(
                            Control::Axis(*index),
                            if *inverted { -value } else { value },
                        );
                    }
                    out => {
                        let amount = range.rested(value);
                        out.amount(state, if *inverted { 1.0 - amount } else { amount });
                    }
                },
                Some(Absolute::Split {
                    range,
                    negative,
                    positive,
                }) => {
                    let value = range.centered(value);
                    if let Some(out) = negative {
                        out.amount(state, (-value).max(0.0));
                    }
                    if let Some(out) = positive {
                        out.amount(state, value.max(0.0));
                    }
                }
                Some(Absolute::Unmapped) | None => {}
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STICK: AxisRange = AxisRange {
        min: -32768,
        max: 32767,
    };
    const BYTE: AxisRange = AxisRange { min: 0, max: 255 };
    const HAT: AxisRange = AxisRange { min: -1, max: 1 };

    /// What an Xbox pad's driver reports.
    fn xbox() -> Mapping {
        Mapping::kernel(
            &[
                BTN_SOUTH, BTN_EAST, BTN_NORTH, BTN_WEST, BTN_TL, BTN_TR, BTN_SELECT, BTN_START,
                BTN_MODE, BTN_THUMBL, BTN_THUMBR,
            ],
            &[
                (ABS_X, STICK),
                (ABS_Y, STICK),
                (ABS_RX, STICK),
                (ABS_RY, STICK),
                (ABS_Z, AxisRange { min: 0, max: 1023 }),
                (ABS_RZ, AxisRange { min: 0, max: 1023 }),
                (ABS_HAT0X, HAT),
                (ABS_HAT0Y, HAT),
            ],
        )
    }

    fn after(mapping: &Mapping, events: &[(u16, u16, i32)]) -> PadState {
        let mut state = PadState::NEUTRAL;
        for (kind, code, value) in events {
            mapping.apply(&mut state, *kind, *code, *value);
        }
        state
    }

    #[test]
    fn a_stick_at_rest_reads_exactly_zero_and_its_ends_exactly_one() {
        for (range, rest) in [
            (STICK, 0),
            (BYTE, 128),
            (AxisRange { min: 0, max: 65535 }, 32768),
            (HAT, 0),
        ] {
            assert_eq!(range.centered(rest), 0.0, "{range:?}");
            assert_eq!(range.centered(range.max), 1.0, "{range:?}");
            assert_eq!(range.centered(range.min), -1.0, "{range:?}");
        }
        assert!((STICK.centered(16384) - 0.5).abs() < 0.001);
        assert!((BYTE.centered(64) + 0.5).abs() < 0.001);
    }

    #[test]
    fn a_trigger_reads_zero_at_rest_and_one_when_pulled() {
        assert_eq!(BYTE.rested(0), 0.0);
        assert_eq!(BYTE.rested(255), 1.0);
        assert!((BYTE.rested(51) - 0.2).abs() < 0.001);
    }

    #[test]
    fn a_value_outside_its_range_or_a_range_that_makes_no_sense_stays_in_bounds() {
        assert_eq!(BYTE.centered(1000), 1.0);
        assert_eq!(BYTE.centered(-1000), -1.0);
        assert_eq!(BYTE.rested(-5), 0.0);
        assert_eq!(BYTE.rested(i32::MAX), 1.0);
        for broken in [
            AxisRange { min: 0, max: 0 },
            AxisRange { min: 5, max: -5 },
            AxisRange {
                min: i32::MIN,
                max: i32::MIN,
            },
        ] {
            assert_eq!(broken.centered(3), 0.0);
            assert_eq!(broken.rested(3), 0.0);
        }
        let widest = AxisRange {
            min: i32::MIN,
            max: i32::MAX,
        };
        assert_eq!(widest.centered(i32::MAX), 1.0);
        assert_eq!(widest.centered(i32::MIN), -1.0);
        assert_eq!(widest.rested(i32::MIN), 0.0);
    }

    #[test]
    fn a_gamepad_s_controls_land_on_their_named_positions() {
        let state = after(
            &xbox(),
            &[
                (EV_KEY, BTN_SOUTH, 1),
                (EV_KEY, BTN_NORTH, 1),
                (EV_KEY, BTN_THUMBR, 1),
                (EV_ABS, ABS_X, 32767),
                (EV_ABS, ABS_RY, -32768),
                (EV_ABS, ABS_Z, 1023),
            ],
        );
        let expected = PadState::NEUTRAL
            .with(gamepad::SOUTH, 1.0)
            .with(gamepad::NORTH, 1.0)
            .with(gamepad::RIGHT_THUMB, 1.0)
            .with(gamepad::LEFT_X, 1.0)
            .with(gamepad::RIGHT_Y, -1.0)
            .with(gamepad::LEFT_TRIGGER, 1.0);
        assert_eq!(state, expected);
        assert_eq!(
            after(&xbox(), &[(EV_KEY, BTN_SOUTH, 1), (EV_KEY, BTN_SOUTH, 0)]),
            PadState::NEUTRAL
        );
    }

    #[test]
    fn a_hat_presses_and_releases_the_directional_buttons() {
        let mapping = xbox();
        let left_up = after(
            &mapping,
            &[(EV_ABS, ABS_HAT0X, -1), (EV_ABS, ABS_HAT0Y, -1)],
        );
        assert_eq!(
            left_up,
            PadState::NEUTRAL
                .with(gamepad::DPAD_LEFT, 1.0)
                .with(gamepad::DPAD_UP, 1.0)
        );
        // Straight from left to right, with no rest between.
        let right = after(&mapping, &[(EV_ABS, ABS_HAT0X, -1), (EV_ABS, ABS_HAT0X, 1)]);
        assert_eq!(right, PadState::NEUTRAL.with(gamepad::DPAD_RIGHT, 1.0));
        let rest = after(&mapping, &[(EV_ABS, ABS_HAT0Y, 1), (EV_ABS, ABS_HAT0Y, 0)]);
        assert_eq!(rest, PadState::NEUTRAL);
    }

    #[test]
    fn directional_buttons_work_where_there_is_no_hat() {
        let mapping = Mapping::kernel(&[BTN_DPAD_UP, BTN_DPAD_RIGHT], &[]);
        let state = after(
            &mapping,
            &[(EV_KEY, BTN_DPAD_UP, 1), (EV_KEY, BTN_DPAD_RIGHT, 1)],
        );
        assert_eq!(
            state,
            PadState::NEUTRAL
                .with(gamepad::DPAD_UP, 1.0)
                .with(gamepad::DPAD_RIGHT, 1.0)
        );
    }

    #[test]
    fn a_trigger_s_click_is_ignored_beside_its_axis_and_drives_the_axis_without_one() {
        // A PlayStation pad reports the click as well as the pull.
        let analog = Mapping::kernel(
            &[BTN_TL2, BTN_TR2],
            &[
                (ABS_RX, BYTE),
                (ABS_RY, BYTE),
                (ABS_Z, BYTE),
                (ABS_RZ, BYTE),
            ],
        );
        assert_eq!(after(&analog, &[(EV_KEY, BTN_TL2, 1)]), PadState::NEUTRAL);
        assert_eq!(
            after(&analog, &[(EV_ABS, ABS_RZ, 255)]),
            PadState::NEUTRAL.with(gamepad::RIGHT_TRIGGER, 1.0)
        );

        let digital = Mapping::kernel(&[BTN_TL2, BTN_TR2], &[(ABS_X, BYTE), (ABS_Y, BYTE)]);
        assert_eq!(
            after(&digital, &[(EV_KEY, BTN_TL2, 1)]),
            PadState::NEUTRAL.with(gamepad::LEFT_TRIGGER, 1.0)
        );
        assert_eq!(
            after(&digital, &[(EV_KEY, BTN_TR2, 1), (EV_KEY, BTN_TR2, 0)]),
            PadState::NEUTRAL
        );
    }

    #[test]
    fn without_a_right_stick_on_rx_and_ry_the_z_axes_are_the_right_stick() {
        let mapping = Mapping::kernel(
            &[],
            &[(ABS_X, BYTE), (ABS_Y, BYTE), (ABS_Z, BYTE), (ABS_RZ, BYTE)],
        );
        let state = after(&mapping, &[(EV_ABS, ABS_Z, 255), (EV_ABS, ABS_RZ, 0)]);
        assert_eq!(
            state,
            PadState::NEUTRAL
                .with(gamepad::RIGHT_X, 1.0)
                .with(gamepad::RIGHT_Y, -1.0)
        );
        // At rest in the middle, as a stick is, not pulled halfway.
        assert_eq!(after(&mapping, &[(EV_ABS, ABS_Z, 128)]), PadState::NEUTRAL);
    }

    #[test]
    fn pedal_axes_are_triggers() {
        let mapping = Mapping::kernel(&[], &[(ABS_GAS, BYTE), (ABS_BRAKE, BYTE)]);
        let state = after(
            &mapping,
            &[(EV_ABS, ABS_GAS, 255), (EV_ABS, ABS_BRAKE, 255)],
        );
        assert_eq!(
            state,
            PadState::NEUTRAL
                .with(gamepad::RIGHT_TRIGGER, 1.0)
                .with(gamepad::LEFT_TRIGGER, 1.0)
        );
    }

    #[test]
    fn other_controls_take_the_spare_positions_in_code_order() {
        // Given out of order and twice over, to show neither matters.
        let mapping = Mapping::kernel(
            &[0x2c1, BTN_SOUTH, 0x2c0, 0x2c0],
            &[(0x28, BYTE), (ABS_X, STICK), (0x07, BYTE)],
        );
        let state = after(&mapping, &[(EV_KEY, 0x2c0, 1), (EV_ABS, 0x28, 255)]);
        assert_eq!(
            state,
            PadState::NEUTRAL
                .with(Control::Button(15), 1.0)
                .with(Control::Axis(7), 1.0)
        );
        // The next free place: 16 to 20 are the back buttons' and the
        // touchpad's click.
        let state = after(&mapping, &[(EV_KEY, 0x2c1, 1), (EV_ABS, 0x07, 0)]);
        assert_eq!(
            state,
            PadState::NEUTRAL
                .with(Control::Button(21), 1.0)
                .with(Control::Axis(6), -1.0)
        );
    }

    #[test]
    fn other_keys_pass_by_every_named_place() {
        // Twelve keys that aren't a gamepad's own: enough to cross the
        // back buttons' and the touchpad's click's places, and capacitive
        // touch's and the second touchpad's click's.
        let keys: Vec<u16> = (0x2c0..0x2cc).collect();
        let mapping = Mapping::kernel(&keys, &[]);
        let places: Vec<u8> = keys
            .iter()
            .map(|code| {
                let state = after(&mapping, &[(EV_KEY, *code, 1)]);
                let held: Vec<u8> = (0..BUTTONS as u8)
                    .filter(|index| state.get(Control::Button(*index)) != 0.0)
                    .collect();
                assert_eq!(held.len(), 1, "{code:#x}: {held:?}");
                held[0]
            })
            .collect();
        assert_eq!(places, [15, 21, 22, 23, 24, 25, 31, 32, 33, 34, 35, 36]);
        for named in gamepad::NAMED_BUTTONS {
            assert!(
                !places.iter().any(|place| Control::Button(*place) == named),
                "{named:?} was given to another key"
            );
        }
    }

    #[test]
    fn controls_beyond_the_capacity_are_dropped_not_wrapped() {
        // Enough codes that aren't a gamepad's own to overfill the spares.
        let keys: Vec<u16> = (0x100..0x130).chain(0x2c0..0x2e8).collect();
        let absolutes: Vec<(u16, AxisRange)> = (0x20..0x40).map(|code| (code, BYTE)).collect();
        let mapping = Mapping::kernel(&keys, &absolutes);
        let mut all = Vec::new();
        all.extend(keys.iter().map(|code| (EV_KEY, *code, 1)));
        all.extend(absolutes.iter().map(|(code, _)| (EV_ABS, *code, 255)));
        let state = after(&mapping, &all);
        // Every spare position is taken, and no named position was.
        for index in 0..BUTTONS as u8 {
            let named = gamepad::NAMED_BUTTONS.contains(&Control::Button(index));
            let expected = if index >= FIRST_SPARE_BUTTON && !named {
                1.0
            } else {
                0.0
            };
            assert_eq!(
                state.get(Control::Button(index)),
                expected,
                "button {index}"
            );
        }
        for index in 0..AXES as u8 {
            let expected = if index >= FIRST_SPARE_AXIS { 1.0 } else { 0.0 };
            assert_eq!(state.get(Control::Axis(index)), expected, "axis {index}");
        }
    }

    #[test]
    fn an_event_with_no_place_changes_nothing() {
        let mapping = xbox();
        let events = [
            (EV_KEY, 0x1ff, 1),
            (EV_KEY, u16::MAX, 1),
            (EV_ABS, 0x3f, 5),
            (EV_ABS, u16::MAX, i32::MIN),
            (0x02, 0, 5),
            (u16::MAX, u16::MAX, i32::MAX),
            (0x00, 0, 0),
        ];
        assert_eq!(after(&mapping, &events), PadState::NEUTRAL);
        // Codes outside the tables are refused when the mapping is built, too.
        let odd = Mapping::kernel(&[u16::MAX, 0x300], &[(u16::MAX, BYTE), (0x40, BYTE)]);
        assert_eq!(
            after(&odd, &[(EV_KEY, u16::MAX, 1), (EV_ABS, 0x40, 9)]),
            PadState::NEUTRAL
        );
    }
}
