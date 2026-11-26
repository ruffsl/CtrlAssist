//! SDL's controller database: layouts for pads whose kernel driver
//! doesn't present the gamepad layout.
//!
//! The format is the community's `gamecontrollerdb.txt`: one pad per
//! line, an identifier, a name, then `target:source` pairs. A pinned
//! snapshot of its Linux entries is built in, and a user's own file can
//! be laid over it.
//!
//! Every line is untrusted input, from the snapshot as much as from a
//! user. A line that doesn't parse is skipped and counted, never fatal,
//! and nothing a line says can make applying an event panic.

use ctrlassist_core::{AXES, BUTTONS, Control, gamepad};

use crate::layout::{Absolute, AxisRange, Key, Mapping, Out};
use crate::pad::Identifiers;

/// The built-in snapshot.
const BUNDLED: &str = include_str!("../data/gamecontrollerdb.txt");

/// The first hat axis, and how many hats there can be.
const ABS_HAT0X: u16 = 0x10;
const HATS: u16 = 4;
/// Where SDL starts numbering a pad's buttons.
const BTN_JOYSTICK: u16 = 0x120;

/// Hat directions, as the format's bit masks.
const UP: u8 = 1;
const RIGHT: u8 = 2;
const DOWN: u8 = 4;
const LEFT: u8 = 8;

/// Which part of a source axis or a target axis a binding means.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Part {
    Whole,
    Positive,
    Negative,
}

/// What a binding reads: SDL's own numbering of a pad's controls.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Source {
    Button(u16),
    Axis {
        index: u16,
        part: Part,
        inverted: bool,
    },
    Hat {
        hat: u16,
        direction: u8,
    },
}

#[derive(Clone, Copy, PartialEq, Debug)]
struct Binding {
    source: Source,
    out: Out,
}

/// One pad's line.
#[derive(Clone, PartialEq, Debug)]
struct Entry {
    bus: u16,
    vendor: u16,
    product: u16,
    version: u16,
    bindings: Vec<Binding>,
}

/// A set of layouts, searched by a pad's bus, vendor, product and version.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Database {
    /// Later entries outrank earlier ones.
    entries: Vec<Entry>,
    skipped: usize,
}

fn index(control: Control) -> u8 {
    match control {
        Control::Button(index) | Control::Axis(index) => index,
    }
}

/// Where a target name lands. The names past the gamepad's own take
/// fixed spare positions, so the same extra button is the same control
/// on every pad that has it.
fn target(name: &str) -> Option<Out> {
    let (part, name) = match name.as_bytes().first() {
        Some(b'+') => (Part::Positive, &name[1..]),
        Some(b'-') => (Part::Negative, &name[1..]),
        _ => (Part::Whole, name),
    };
    let button = |control: Control| Some(Out::Button(index(control)));
    let spare = |position: u8| (usize::from(position) < BUTTONS).then_some(Out::Button(position));
    let axis = |control: Control| {
        Some(match part {
            Part::Whole => Out::Axis(index(control)),
            Part::Positive => Out::Side(index(control), 1.0),
            Part::Negative => Out::Side(index(control), -1.0),
        })
    };
    let out = match name {
        "a" => button(gamepad::SOUTH),
        "b" => button(gamepad::EAST),
        "x" => button(gamepad::WEST),
        "y" => button(gamepad::NORTH),
        "back" => button(gamepad::SELECT),
        "guide" => button(gamepad::MODE),
        "start" => button(gamepad::START),
        "leftstick" => button(gamepad::LEFT_THUMB),
        "rightstick" => button(gamepad::RIGHT_THUMB),
        "leftshoulder" => button(gamepad::LEFT_BUMPER),
        "rightshoulder" => button(gamepad::RIGHT_BUMPER),
        "dpup" => button(gamepad::DPAD_UP),
        "dpdown" => button(gamepad::DPAD_DOWN),
        "dpleft" => button(gamepad::DPAD_LEFT),
        "dpright" => button(gamepad::DPAD_RIGHT),
        "leftx" => axis(gamepad::LEFT_X),
        "lefty" => axis(gamepad::LEFT_Y),
        "rightx" => axis(gamepad::RIGHT_X),
        "righty" => axis(gamepad::RIGHT_Y),
        "lefttrigger" => Some(Out::Trigger(index(gamepad::LEFT_TRIGGER))),
        "righttrigger" => Some(Out::Trigger(index(gamepad::RIGHT_TRIGGER))),
        "misc1" => spare(15),
        "paddle1" => spare(16),
        "paddle2" => spare(17),
        "paddle3" => spare(18),
        "paddle4" => spare(19),
        "touchpad" => button(gamepad::TOUCHPAD),
        "misc2" => spare(21),
        "misc3" => spare(22),
        "misc4" => spare(23),
        "misc5" => spare(24),
        "misc6" => spare(25),
        _ => None,
    }?;
    // A sign means something only before an axis.
    (part == Part::Whole || matches!(out, Out::Side(..))).then_some(out)
}

fn number(text: &str) -> Option<u16> {
    // No sign, no spaces, nothing a plain count wouldn't have.
    (!text.is_empty() && text.len() <= 4 && text.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| text.parse().ok())
        .flatten()
}

fn source(text: &str) -> Option<Source> {
    let (part, text) = match text.as_bytes().first() {
        Some(b'+') => (Part::Positive, &text[1..]),
        Some(b'-') => (Part::Negative, &text[1..]),
        _ => (Part::Whole, text),
    };
    let (kind, rest) = (text.as_bytes().first()?, text.get(1..)?);
    match kind {
        b'b' if part == Part::Whole => number(rest).map(Source::Button),
        b'a' => {
            let (rest, inverted) = match rest.strip_suffix('~') {
                Some(rest) => (rest, true),
                None => (rest, false),
            };
            Some(Source::Axis {
                index: number(rest)?,
                part,
                inverted,
            })
        }
        b'h' if part == Part::Whole => {
            let (hat, direction) = rest.split_once('.')?;
            let direction = u8::try_from(number(direction)?).ok()?;
            matches!(direction, UP | RIGHT | DOWN | LEFT).then_some(Source::Hat {
                hat: number(hat)?,
                direction,
            })
        }
        _ => None,
    }
}

/// The four numbers in an identifier: 32 hex digits holding bus, vendor,
/// product and version as little-endian words at fixed places. An
/// identifier of another shape (some carry a name's bytes) is not one.
fn identifier(text: &str) -> Option<(u16, u16, u16, u16)> {
    if text.len() != 32 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |at: usize| u8::from_str_radix(text.get(at * 2..at * 2 + 2)?, 16).ok();
    let word = |at: usize| Some(u16::from_le_bytes([byte(at)?, byte(at + 1)?]));
    let zero = |at: usize| Some(word(at)? == 0);
    // Bytes 2 and 3 are a checksum of the name in newer identifiers.
    (zero(6)? && zero(10)? && zero(14)?).then_some(())?;
    Some((word(0)?, word(4)?, word(8)?, word(12)?))
}

fn entry(line: &str) -> Option<Entry> {
    let mut fields = line.split(',');
    let (bus, vendor, product, version) = identifier(fields.next()?.trim())?;
    let _name = fields.next()?;
    let mut bindings = Vec::new();
    let mut is_linux = true;
    for field in fields {
        let Some((key, value)) = field.trim().split_once(':') else {
            continue;
        };
        if key == "platform" {
            is_linux = value == "Linux";
        } else if let (Some(out), Some(source)) = (target(key), source(value)) {
            bindings.push(Binding { source, out });
        }
        // Anything else is a hint for SDL, or a target this has no place
        // for; neither spoils the rest of the line.
    }
    (is_linux && !bindings.is_empty()).then_some(Entry {
        bus,
        vendor,
        product,
        version,
        bindings,
    })
}

impl Database {
    /// Parses a database. Lines that aren't a Linux pad's layout are
    /// skipped; [`skipped`](Self::skipped) says how many weren't even
    /// comments.
    #[must_use]
    pub fn parse(text: &str) -> Self {
        let mut database = Self::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match entry(line) {
                Some(entry) => database.entries.push(entry),
                None => database.skipped += 1,
            }
        }
        database
    }

    /// The snapshot built into this crate.
    #[must_use]
    pub fn bundled() -> Self {
        Self::parse(BUNDLED)
    }

    /// Lays another database over this one: where both have a pad, the
    /// other's layout is used.
    #[must_use]
    pub fn overlaid(mut self, other: Self) -> Self {
        self.entries.extend(other.entries);
        self.skipped += other.skipped;
        self
    }

    /// How many layouts it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether it holds none.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// How many lines were neither a comment nor a usable layout.
    #[must_use]
    pub fn skipped(&self) -> usize {
        self.skipped
    }

    /// The layout for a pad, if the database has one: for exactly this
    /// version of the pad, or failing that for any version of it on the
    /// same bus.
    fn find(&self, pad: &Identifiers) -> Option<&Entry> {
        let model = |entry: &&Entry| {
            (entry.bus, entry.vendor, entry.product) == (pad.bus, pad.vendor, pad.product)
        };
        let exact = self
            .entries
            .iter()
            .rev()
            .filter(model)
            .find(|entry| entry.version == pad.version);
        exact.or_else(|| self.entries.iter().rev().find(model))
    }

    /// The mapping for a pad with these key codes and absolute axes, if
    /// the database has a layout for it.
    #[must_use]
    pub fn mapping(
        &self,
        pad: &Identifiers,
        keys: &[u16],
        absolutes: &[(u16, AxisRange)],
    ) -> Option<Mapping> {
        let entry = self.find(pad)?;

        // SDL numbers a pad's controls by what the pad has: buttons from
        // the joystick codes up, then the codes below them; axes in code
        // order with the hats left out; hats by which pairs exist.
        let mut sorted: Vec<u16> = keys.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        let (low, high): (Vec<u16>, Vec<u16>) =
            sorted.iter().partition(|code| **code < BTN_JOYSTICK);
        let buttons: Vec<u16> = high.into_iter().chain(low).collect();

        let mut ranges: Vec<(u16, AxisRange)> = absolutes.to_vec();
        ranges.sort_unstable_by_key(|(code, _)| *code);
        ranges.dedup_by_key(|(code, _)| *code);
        let is_hat = |code: u16| (ABS_HAT0X..ABS_HAT0X + HATS * 2).contains(&code);
        let axes: Vec<(u16, AxisRange)> = ranges
            .iter()
            .copied()
            .filter(|(code, _)| !is_hat(*code))
            .collect();
        let range_of = |code: u16| {
            ranges
                .iter()
                .find(|(axis, _)| *axis == code)
                .map(|(_, range)| *range)
        };
        let hats: Vec<u16> = (0..HATS)
            .map(|hat| ABS_HAT0X + hat * 2)
            .filter(|x| range_of(*x).is_some() || range_of(*x + 1).is_some())
            .collect();

        let mut mapping = Mapping::empty();
        for binding in &entry.bindings {
            // An output past the capacity can't come from `target`, but a
            // mapping is only ever built from what was checked here.
            let fits = match binding.out {
                Out::Button(index) => usize::from(index) < BUTTONS,
                Out::Axis(index) | Out::Trigger(index) | Out::Side(index, _) => {
                    usize::from(index) < AXES
                }
            };
            if !fits {
                continue;
            }
            match binding.source {
                Source::Button(number) => {
                    let Some(code) = buttons.get(usize::from(number)) else {
                        continue;
                    };
                    if let Some(key) = mapping.keys.get_mut(usize::from(*code)) {
                        *key = Key::To(binding.out);
                    }
                }
                Source::Axis {
                    index,
                    part,
                    inverted,
                } => {
                    let Some((code, range)) = axes.get(usize::from(index)).copied() else {
                        continue;
                    };
                    let Some(slot) = mapping.absolutes.get_mut(usize::from(code)) else {
                        continue;
                    };
                    side(slot, range, part, inverted, binding.out);
                }
                Source::Hat { hat, direction } => {
                    let Some(x) = hats.get(usize::from(hat)).copied() else {
                        continue;
                    };
                    let (code, part) = match direction {
                        LEFT => (x, Part::Negative),
                        RIGHT => (x, Part::Positive),
                        UP => (x + 1, Part::Negative),
                        _ => (x + 1, Part::Positive),
                    };
                    let (Some(range), Some(slot)) =
                        (range_of(code), mapping.absolutes.get_mut(usize::from(code)))
                    else {
                        continue;
                    };
                    side(slot, range, part, false, binding.out);
                }
            }
        }
        Some(mapping)
    }
}

/// Binds the whole of an axis, or one side of it, keeping whatever the
/// other side is already bound to.
fn side(slot: &mut Absolute, range: AxisRange, part: Part, inverted: bool, out: Out) {
    let (mut negative, mut positive) = match *slot {
        Absolute::Split {
            negative, positive, ..
        } => (negative, positive),
        _ => (None, None),
    };
    match part {
        Part::Whole => {
            *slot = Absolute::Whole {
                range,
                inverted,
                out,
            };
            return;
        }
        Part::Negative => negative = Some(out),
        Part::Positive => positive = Some(out),
    }
    *slot = Absolute::Split {
        range,
        negative,
        positive,
    };
}

#[cfg(test)]
mod tests {
    use ctrlassist_core::PadState;
    use proptest::prelude::*;

    use super::*;

    const EV_KEY: u16 = 1;
    const EV_ABS: u16 = 3;
    const BYTE: AxisRange = AxisRange { min: 0, max: 255 };
    const HAT: AxisRange = AxisRange { min: -1, max: 1 };

    fn pad(bus: u16, vendor: u16, product: u16, version: u16) -> Identifiers {
        Identifiers {
            name: "Odd Pad".to_string(),
            bus,
            vendor,
            product,
            version,
            address: None,
            serial: None,
            port: None,
        }
    }

    /// A pad on USB, vendor 0x1209, product 0x0dd0, version 0x0111.
    const ODD: &str = "03000000091200ffd00d000011010000";

    fn line(bindings: &str) -> String {
        format!(
            "{},Odd Pad,{bindings},platform:Linux,",
            ODD.replace("ff", "00")
        )
    }

    /// A plain HID pad: twelve joystick buttons, four axes, a hat.
    fn caps() -> (Vec<u16>, Vec<(u16, AxisRange)>) {
        let keys = (0x120..0x12c).collect();
        let absolutes = vec![
            (0, BYTE),
            (1, BYTE),
            (2, BYTE),
            (5, BYTE),
            (0x10, HAT),
            (0x11, HAT),
        ];
        (keys, absolutes)
    }

    fn after(mapping: &Mapping, events: &[(u16, u16, i32)]) -> PadState {
        let mut state = PadState::NEUTRAL;
        for (kind, code, value) in events {
            mapping.apply(&mut state, *kind, *code, *value);
        }
        state
    }

    fn mapping(bindings: &str) -> Mapping {
        let (keys, absolutes) = caps();
        Database::parse(&line(bindings))
            .mapping(&pad(3, 0x1209, 0x0dd0, 0x0111), &keys, &absolutes)
            .unwrap()
    }

    #[test]
    fn an_identifier_holds_bus_vendor_product_and_version() {
        assert_eq!(
            identifier("030000004c050000c405000011010000"),
            Some((3, 0x054c, 0x05c4, 0x0111))
        );
        // The name's checksum, in newer identifiers, is not part of it.
        assert_eq!(
            identifier("0300abcd4c050000c405000011010000"),
            Some((3, 0x054c, 0x05c4, 0x0111))
        );
        for not_one in [
            "",
            "030000004c050000c40500001101000",
            "030000004c050000c4050000110100000",
            "030000004c05ffffc405000011010000",
            "0300000058626f782047616d65706164",
            "0300000g4c050000c405000011010000",
            "é30000004c050000c40500001101000",
        ] {
            assert_eq!(identifier(not_one), None, "{not_one:?}");
        }
    }

    #[test]
    fn buttons_are_numbered_from_the_joystick_codes_then_the_ones_below() {
        let database = Database::parse(&line("a:b0,b:b1,x:b12"));
        // Twelve joystick buttons, and one code below them, which SDL
        // numbers after them.
        let keys: Vec<u16> = (0x120..0x12c).chain([0x100]).collect();
        let mapping = database
            .mapping(&pad(3, 0x1209, 0x0dd0, 0x0111), &keys, &[])
            .unwrap();
        let state = after(&mapping, &[(EV_KEY, 0x120, 1), (EV_KEY, 0x100, 1)]);
        assert_eq!(
            state,
            PadState::NEUTRAL
                .with(gamepad::SOUTH, 1.0)
                .with(gamepad::WEST, 1.0)
        );
    }

    #[test]
    fn axes_are_numbered_without_the_hats_and_hats_by_which_exist() {
        let mapping = mapping(
            "leftx:a0,lefty:a1,rightx:a2,righty:a3,dpup:h0.1,dpright:h0.2,dpdown:h0.4,dpleft:h0.8",
        );
        // The fourth axis is code 5: numbering follows what the pad has.
        let state = after(&mapping, &[(EV_ABS, 5, 255), (EV_ABS, 0, 0)]);
        assert_eq!(
            state,
            PadState::NEUTRAL
                .with(gamepad::RIGHT_Y, 1.0)
                .with(gamepad::LEFT_X, -1.0)
        );
        let state = after(&mapping, &[(EV_ABS, 0x10, -1), (EV_ABS, 0x11, 1)]);
        assert_eq!(
            state,
            PadState::NEUTRAL
                .with(gamepad::DPAD_LEFT, 1.0)
                .with(gamepad::DPAD_DOWN, 1.0)
        );
        let state = after(
            &mapping,
            &[(EV_ABS, 0x10, -1), (EV_ABS, 0x10, 1), (EV_ABS, 0x10, 0)],
        );
        assert_eq!(state, PadState::NEUTRAL);
    }

    #[test]
    fn a_button_can_be_a_trigger_and_an_axis_can_be_one_too() {
        let mapping = mapping("lefttrigger:b6,righttrigger:a2");
        let state = after(&mapping, &[(EV_KEY, 0x126, 1), (EV_ABS, 2, 255)]);
        let both = PadState::NEUTRAL
            .with(gamepad::LEFT_TRIGGER, 1.0)
            .with(gamepad::RIGHT_TRIGGER, 1.0);
        assert_eq!(state, both);
        // An axis used as a trigger rests at its lowest, not its middle.
        assert_eq!(after(&mapping, &[(EV_ABS, 2, 0)]), PadState::NEUTRAL);
    }

    #[test]
    fn half_an_axis_can_be_a_button_or_a_trigger_and_an_axis_can_be_inverted() {
        let mapping = mapping("dpup:-a1,dpdown:+a1,lefttrigger:+a2,leftx:a0~");
        assert_eq!(
            after(&mapping, &[(EV_ABS, 1, 0)]),
            PadState::NEUTRAL.with(gamepad::DPAD_UP, 1.0)
        );
        assert_eq!(
            after(&mapping, &[(EV_ABS, 1, 255)]),
            PadState::NEUTRAL.with(gamepad::DPAD_DOWN, 1.0)
        );
        assert_eq!(
            after(&mapping, &[(EV_ABS, 1, 0), (EV_ABS, 1, 128)]),
            PadState::NEUTRAL
        );
        assert_eq!(
            after(&mapping, &[(EV_ABS, 2, 255)]),
            PadState::NEUTRAL.with(gamepad::LEFT_TRIGGER, 1.0)
        );
        assert_eq!(
            after(&mapping, &[(EV_ABS, 2, 0)]),
            PadState::NEUTRAL,
            "the other half is nothing"
        );
        assert_eq!(
            after(&mapping, &[(EV_ABS, 0, 255)]),
            PadState::NEUTRAL.with(gamepad::LEFT_X, -1.0)
        );
    }

    #[test]
    fn two_buttons_can_push_one_axis_each_way_without_undoing_each_other() {
        let mapping = mapping("+rightx:b3,-rightx:b0");
        assert_eq!(
            after(&mapping, &[(EV_KEY, 0x123, 1)]),
            PadState::NEUTRAL.with(gamepad::RIGHT_X, 1.0)
        );
        assert_eq!(
            after(&mapping, &[(EV_KEY, 0x120, 1)]),
            PadState::NEUTRAL.with(gamepad::RIGHT_X, -1.0)
        );
        // Letting go of one side while the other is held leaves the other.
        let state = after(
            &mapping,
            &[(EV_KEY, 0x123, 1), (EV_KEY, 0x120, 1), (EV_KEY, 0x123, 0)],
        );
        assert_eq!(state, PadState::NEUTRAL.with(gamepad::RIGHT_X, -1.0));
        let state = after(&mapping, &[(EV_KEY, 0x123, 1), (EV_KEY, 0x123, 0)]);
        assert_eq!(state, PadState::NEUTRAL);
    }

    #[test]
    fn extra_buttons_take_the_same_spare_positions_on_every_pad() {
        let mapping = mapping("misc1:b0,paddle1:b1,touchpad:b2");
        let state = after(
            &mapping,
            &[(EV_KEY, 0x120, 1), (EV_KEY, 0x121, 1), (EV_KEY, 0x122, 1)],
        );
        let expected = PadState::NEUTRAL
            .with(Control::Button(15), 1.0)
            .with(Control::Button(16), 1.0)
            .with(Control::Button(20), 1.0);
        assert_eq!(state, expected);
    }

    #[test]
    fn a_binding_to_a_control_the_pad_lacks_is_dropped_and_the_rest_kept() {
        let mapping = mapping("a:b0,b:b99,leftx:a9,dpup:h3.1,x:b1");
        let state = after(&mapping, &[(EV_KEY, 0x120, 1), (EV_KEY, 0x121, 1)]);
        assert_eq!(
            state,
            PadState::NEUTRAL
                .with(gamepad::SOUTH, 1.0)
                .with(gamepad::WEST, 1.0)
        );
    }

    #[test]
    fn what_is_not_a_binding_is_ignored_and_what_is_not_a_layout_is_counted() {
        let text = format!(
            "# a comment\n\n{}\nnot a line at all\n{ODD},Odd Pad,a:b0,platform:Windows,\n{ODD},Odd Pad,platform:Linux,\n",
            line("a:b0,crc:1234,hint:SDL_GAMECONTROLLER_USE_BUTTON_LABELS:=1,sdk>=:31,+a:b1,b:+b2,x:h0.3,y:a1~~,back:b-1,start:b1e2,guide:b")
        )
        .replace("ff", "00");
        let database = Database::parse(&text);
        assert_eq!((database.len(), database.skipped()), (1, 3));
        let (keys, absolutes) = caps();
        let mapping = database
            .mapping(&pad(3, 0x1209, 0x0dd0, 0x0111), &keys, &absolutes)
            .unwrap();
        // Only `a:b0` was a binding.
        let all: Vec<(u16, u16, i32)> = (0x120..0x12c).map(|code| (EV_KEY, code, 1)).collect();
        assert_eq!(
            after(&mapping, &all),
            PadState::NEUTRAL.with(gamepad::SOUTH, 1.0)
        );
    }

    #[test]
    fn a_pad_is_found_by_its_exact_version_then_by_any_version_on_its_bus() {
        let other_version = line("a:b1").replace("11010000", "22020000");
        let database = Database::parse(&format!("{}\n{other_version}\n", line("a:b0")));
        let (keys, absolutes) = caps();
        let south = |version: u16, bus: u16| {
            let mapping =
                database.mapping(&pad(bus, 0x1209, 0x0dd0, version), &keys, &absolutes)?;
            Some(
                (0x120..0x12c)
                    .find(|code| after(&mapping, &[(EV_KEY, *code, 1)]).get(gamepad::SOUTH) > 0.0),
            )
        };
        assert_eq!(south(0x0111, 3), Some(Some(0x120)));
        assert_eq!(south(0x0222, 3), Some(Some(0x121)));
        assert_eq!(
            south(0x0333, 3),
            Some(Some(0x121)),
            "any version: the later line"
        );
        assert_eq!(south(0x0111, 5), None, "another bus is another layout");
        assert!(
            database
                .mapping(&pad(3, 0x1209, 0x0001, 0x0111), &keys, &absolutes)
                .is_none()
        );
    }

    #[test]
    fn a_user_s_layout_outranks_the_built_in_one_for_the_same_pad() {
        let built_in = Database::parse(&line("a:b0"));
        let users = Database::parse(&line("a:b5"));
        let database = built_in.overlaid(users);
        assert_eq!(database.len(), 2);
        let (keys, absolutes) = caps();
        let mapping = database
            .mapping(&pad(3, 0x1209, 0x0dd0, 0x0111), &keys, &absolutes)
            .unwrap();
        assert_eq!(
            after(&mapping, &[(EV_KEY, 0x125, 1)]),
            PadState::NEUTRAL.with(gamepad::SOUTH, 1.0)
        );
        assert_eq!(after(&mapping, &[(EV_KEY, 0x120, 1)]), PadState::NEUTRAL);
    }

    #[test]
    fn the_built_in_snapshot_parses_whole() {
        let database = Database::bundled();
        assert!(!database.is_empty());
        assert!(database.len() > 600, "{} layouts", database.len());
        // Identifiers that hold a name's bytes, not numbers, are the only
        // lines expected to be skipped.
        assert!(
            database.skipped() < database.len() / 20,
            "{} skipped",
            database.skipped()
        );
    }

    proptest! {
        /// No text can make parsing panic, and no layout parsed from any
        /// text can make applying an event panic.
        #[test]
        fn nothing_a_database_says_can_panic(
            fields in prop::collection::vec("[-+]?[a-z0-9]{0,12}:[-+]?[abh]?[0-9.~]{0,6}", 0..24),
            junk in ".{0,60}",
            events in prop::collection::vec((0u16..5, any::<u16>(), any::<i32>()), 0..40),
            keys in prop::collection::vec(any::<u16>(), 0..40),
            axes in prop::collection::vec((any::<u16>(), any::<i32>(), any::<i32>()), 0..20),
        ) {
            let text = format!("{}\n{junk}\n{junk},{junk},a:b0,platform:Linux,\n", line(&fields.join(",")));
            let database = Database::parse(&text);
            let absolutes: Vec<(u16, AxisRange)> = axes.iter().map(|(code, min, max)| (*code, AxisRange { min: *min, max: *max })).collect();
            if let Some(mapping) = database.mapping(&pad(3, 0x1209, 0x0dd0, 0x0111), &keys, &absolutes) {
                let mut state = PadState::NEUTRAL;
                for (kind, code, value) in events {
                    mapping.apply(&mut state, kind, code, value);
                }
            }
        }

        /// Arbitrary bytes, as text, never panic the parser.
        #[test]
        fn no_text_can_panic_the_parser(text in "\\PC{0,400}") {
            let _ = Database::parse(&text);
        }
    }
}
