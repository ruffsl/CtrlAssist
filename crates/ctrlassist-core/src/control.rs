//! Controls, pad state and groups.

use crate::graph::GraphError;

/// Axes a pad state can hold.
pub const AXES: usize = 16;
/// Buttons a pad state can hold.
pub const BUTTONS: usize = 64;
/// One bit per button.
pub(crate) type ButtonBits = u64;
/// One bit per axis.
pub(crate) type AxisBits = u16;

/// One control on a pad, by kind and index.
///
/// The space is open-ended: a pad with thirty buttons and eight axes needs
/// no new vocabulary. The [`gamepad`] names are well-known positions in it,
/// which is what lets two different pads be merged control by control.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Control {
    /// A button, pressed or not.
    Button(u8),
    /// An axis. A stick axis rests at 0 and spans -1 to 1; a trigger rests
    /// at 0 and spans 0 to 1.
    Axis(u8),
}

impl Control {
    /// Whether the index fits in a pad state.
    #[must_use]
    pub const fn is_valid(self) -> bool {
        match self {
            Self::Button(index) => (index as usize) < BUTTONS,
            Self::Axis(index) => (index as usize) < AXES,
        }
    }
}

/// Well-known gamepad controls.
pub mod gamepad {
    use super::Control;

    pub const SOUTH: Control = Control::Button(0);
    pub const EAST: Control = Control::Button(1);
    pub const NORTH: Control = Control::Button(2);
    pub const WEST: Control = Control::Button(3);
    pub const LEFT_BUMPER: Control = Control::Button(4);
    pub const RIGHT_BUMPER: Control = Control::Button(5);
    pub const SELECT: Control = Control::Button(6);
    pub const START: Control = Control::Button(7);
    pub const MODE: Control = Control::Button(8);
    pub const LEFT_THUMB: Control = Control::Button(9);
    pub const RIGHT_THUMB: Control = Control::Button(10);
    pub const DPAD_UP: Control = Control::Button(11);
    pub const DPAD_DOWN: Control = Control::Button(12);
    pub const DPAD_LEFT: Control = Control::Button(13);
    pub const DPAD_RIGHT: Control = Control::Button(14);
    // The back buttons, at the places the controller database gives
    // `paddle1` to `paddle4`, in SDL's order. Like every button past the
    // D-pad they are in no group of the gamepad layout.
    /// The upper right back button: a Steam Controller's R4.
    pub const PADDLE_1: Control = Control::Button(16);
    /// The upper left back button: a Steam Controller's L4.
    pub const PADDLE_2: Control = Control::Button(17);
    /// The lower right back button: a Steam Controller's R5.
    pub const PADDLE_3: Control = Control::Button(18);
    /// The lower left back button: a Steam Controller's L5.
    pub const PADDLE_4: Control = Control::Button(19);
    /// The first touchpad's click, a button like any other, at the place
    /// the controller database gives `touchpad`. Where the finger is on
    /// the touchpad is no control: see [`Touchpad`](super::Touchpad).
    pub const TOUCHPAD: Control = Control::Button(20);
    // Capacitive touch: a hand resting on a stick or a grip without
    // moving or pressing it, as a Steam Controller senses it. Past the
    // controller database's last place, `misc6`'s, and in no group of the
    // gamepad layout, so a thumb resting on a stick never owns it.
    /// A thumb on the left stick.
    pub const LEFT_STICK_TOUCH: Control = Control::Button(26);
    /// A thumb on the right stick.
    pub const RIGHT_STICK_TOUCH: Control = Control::Button(27);
    /// A hand on the left grip.
    pub const LEFT_GRIP_TOUCH: Control = Control::Button(28);
    /// A hand on the right grip.
    pub const RIGHT_GRIP_TOUCH: Control = Control::Button(29);
    /// The second touchpad's click, as [`TOUCHPAD`] is the first's: a
    /// Steam Controller's left trackpad's.
    pub const SECOND_TOUCHPAD: Control = Control::Button(30);

    /// Every button above, in the order of their places. A reader that
    /// hands a pad's other buttons the places left over passes these by,
    /// so that none of them stands in for a button with a name.
    pub const NAMED_BUTTONS: [Control; 25] = [
        SOUTH,
        EAST,
        NORTH,
        WEST,
        LEFT_BUMPER,
        RIGHT_BUMPER,
        SELECT,
        START,
        MODE,
        LEFT_THUMB,
        RIGHT_THUMB,
        DPAD_UP,
        DPAD_DOWN,
        DPAD_LEFT,
        DPAD_RIGHT,
        PADDLE_1,
        PADDLE_2,
        PADDLE_3,
        PADDLE_4,
        TOUCHPAD,
        LEFT_STICK_TOUCH,
        RIGHT_STICK_TOUCH,
        LEFT_GRIP_TOUCH,
        RIGHT_GRIP_TOUCH,
        SECOND_TOUCHPAD,
    ];

    pub const LEFT_X: Control = Control::Axis(0);
    pub const LEFT_Y: Control = Control::Axis(1);
    pub const RIGHT_X: Control = Control::Axis(2);
    pub const RIGHT_Y: Control = Control::Axis(3);
    pub const LEFT_TRIGGER: Control = Control::Axis(4);
    pub const RIGHT_TRIGGER: Control = Control::Axis(5);
}

/// What a pad's motion sensors last read: how fast the pad is turning,
/// and the acceleration it feels, gravity included.
///
/// The axes are the pad's own, as a DualSense reports them and SDL hands
/// them to a game: held in front of the player, x to their right, y up,
/// and z toward them. A reader of another kind of pad turns its readings
/// into these. A value is a physical one, the pad's own calibration
/// already applied: the writer of a virtual pad turns it into its
/// report's counts, and holds it to what they can carry (a virtual
/// DualSense's, under its neutral calibration, about ±2000 °/s and ±4 g).
///
/// It is not a control. No group holds it and no rule blends it: a node
/// takes it whole from one input, so a game always sees one pad's two
/// sensors, which agree.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Motion {
    gyro: [f32; 3],
    accel: [f32; 3],
    readings: u32,
    interval_us: u32,
}

impl Motion {
    /// A pad lying flat and still, as a virtual DualSense reports at
    /// rest: no turning, and gravity's 1 g straight up on y. It has had
    /// no reading.
    pub const STILL: Self = Self {
        gyro: [0.0; 3],
        accel: [0.0, 1.0, 0.0],
        readings: 0,
        interval_us: 0,
    };

    /// One reading of the sensors.
    ///
    /// - `gyro`: the rate of turning about x, y and z (pitch, yaw and
    ///   roll), in degrees a second.
    /// - `accel`: the acceleration along x, y and z, in g (9.80665 m/s²),
    ///   gravity included: a pad lying flat and still reads 1 on y.
    /// - `readings`: how many readings the pad has given, wrapping. A new
    ///   count is a fresh reading; the same count, the same reading again,
    ///   as when another pad's report steps the graph.
    /// - `interval_us`: the time from the pad's reading before this one
    ///   to this one, by the pad's own clock, in microseconds; 0 for its
    ///   first. A virtual pad advances a clock of its own by it.
    ///
    /// A reading with any rate or acceleration that isn't a finite
    /// number reads as the pad lying still, with its count and interval
    /// kept, so nothing a report says can make a step, or a writer after
    /// it, take a value that isn't one.
    #[must_use]
    pub fn new(gyro: [f32; 3], accel: [f32; 3], readings: u32, interval_us: u32) -> Self {
        let finite = gyro.iter().chain(&accel).all(|value| value.is_finite());
        let (gyro, accel) = if finite {
            (gyro, accel)
        } else {
            (Self::STILL.gyro, Self::STILL.accel)
        };
        Self {
            gyro,
            accel,
            readings,
            interval_us,
        }
    }

    /// The rate of turning about x, y and z, in degrees a second.
    #[must_use]
    pub fn gyro(&self) -> [f32; 3] {
        self.gyro
    }

    /// The acceleration along x, y and z, in g, gravity included.
    #[must_use]
    pub fn accel(&self) -> [f32; 3] {
        self.accel
    }

    /// How many readings the pad has given, wrapping.
    #[must_use]
    pub fn readings(&self) -> u32 {
        self.readings
    }

    /// The time since the pad's reading before this one, by its own
    /// clock, in microseconds.
    #[must_use]
    pub fn interval_us(&self) -> u32 {
        self.interval_us
    }
}

/// One finger's place on a touchpad.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Contact {
    down: bool,
    x: f32,
    y: f32,
}

impl Contact {
    /// No finger, and no place.
    pub const UP: Self = Self {
        down: false,
        x: 0.0,
        y: 0.0,
    };

    /// A contact, with a finger on it or not, and its place: `x` across
    /// from the touchpad's left edge at 0 to its right edge at 1, `y`
    /// down from its top edge at 0 to its bottom edge at 1. A DualSense's
    /// touchpad reports 0 to 1919 across and 0 to 1079 down. A lifted
    /// contact keeps the place its finger left, as a DualSense's report
    /// does.
    ///
    /// A place is held to 0 to 1, and one that isn't a finite number
    /// reads as [`UP`](Self::UP).
    #[must_use]
    pub fn new(down: bool, x: f32, y: f32) -> Self {
        if x.is_finite() && y.is_finite() {
            Self {
                down,
                x: x.clamp(0.0, 1.0),
                y: y.clamp(0.0, 1.0),
            }
        } else {
            Self::UP
        }
    }

    /// Whether a finger is on it.
    #[must_use]
    pub fn down(&self) -> bool {
        self.down
    }

    /// Its place across, from 0 at the left edge to 1 at the right.
    #[must_use]
    pub fn x(&self) -> f32 {
        self.x
    }

    /// Its place down, from 0 at the top edge to 1 at the bottom.
    #[must_use]
    pub fn y(&self) -> f32 {
        self.y
    }
}

/// Where fingers are on one of a pad's touchpads: two contacts, in the
/// order the pad numbers them. The first touchpad's click is a button,
/// [`gamepad::TOUCHPAD`], and the second's [`gamepad::SECOND_TOUCHPAD`].
///
/// It is not a control. No group holds it and no rule blends it: a node
/// takes it whole from one input, two fingers' places not averaging into
/// a third that means anything.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Touchpad {
    contacts: [Contact; Self::CONTACTS],
}

impl Touchpad {
    /// How many contacts a touchpad holds: a DualSense's two.
    pub const CONTACTS: usize = 2;

    /// No finger on it.
    pub const UNTOUCHED: Self = Self {
        contacts: [Contact::UP; Self::CONTACTS],
    };

    /// A touchpad with these contacts.
    #[must_use]
    pub fn new(contacts: [Contact; Self::CONTACTS]) -> Self {
        Self { contacts }
    }

    /// Its contacts.
    #[must_use]
    pub fn contacts(&self) -> [Contact; Self::CONTACTS] {
        self.contacts
    }

    /// Whether a finger is on it.
    pub(crate) fn touched(&self) -> bool {
        self.contacts.iter().any(Contact::down)
    }
}

/// How many touchpads a pad state holds: a DualSense's one and a Steam
/// Controller's two.
pub(crate) const TOUCHPADS: usize = 2;

/// Everything a pad holds at one instant: its controls, and beside them
/// its motion sensors' last reading and its two touchpads' contacts,
/// which are not controls. A pad without sensors or touchpads holds them
/// at rest.
///
/// The first touchpad is a DualSense's or a DualShock 4's, and a Steam
/// Controller's right trackpad; the second is a Steam Controller's left.
/// A pad with one touchpad leaves the second untouched.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PadState {
    buttons: ButtonBits,
    axes: [f32; AXES],
    motion: Motion,
    touchpads: [Touchpad; TOUCHPADS],
}

impl PadState {
    /// A pad at rest: nothing pressed, every axis at zero, lying flat and
    /// still, and no finger on either touchpad.
    pub const NEUTRAL: Self = Self {
        buttons: 0,
        axes: [0.0; AXES],
        motion: Motion::STILL,
        touchpads: [Touchpad::UNTOUCHED; TOUCHPADS],
    };

    /// Sets one control. A button is pressed by any value above zero. An
    /// axis is held to -1 through 1, and a value that isn't a number reads
    /// as rest. A control beyond the capacity is ignored.
    pub fn set(&mut self, control: Control, value: f32) {
        if !control.is_valid() {
            return;
        }
        match control {
            Control::Button(index) => {
                let bit = (1 as ButtonBits) << index;
                if value > 0.0 {
                    self.buttons |= bit;
                } else {
                    self.buttons &= !bit;
                }
            }
            Control::Axis(index) => {
                self.axes[usize::from(index)] = if value.is_finite() {
                    value.clamp(-1.0, 1.0)
                } else {
                    0.0
                };
            }
        }
    }

    /// Builder form of [`set`](Self::set).
    #[must_use]
    pub fn with(mut self, control: Control, value: f32) -> Self {
        self.set(control, value);
        self
    }

    /// One control's value; a button reads as 1 or 0. A control beyond the
    /// capacity reads as rest.
    #[must_use]
    pub fn get(&self, control: Control) -> f32 {
        if !control.is_valid() {
            return 0.0;
        }
        match control {
            Control::Button(index) => {
                if self.buttons & ((1 as ButtonBits) << index) == 0 {
                    0.0
                } else {
                    1.0
                }
            }
            Control::Axis(index) => self.axes[usize::from(index)],
        }
    }

    /// The motion sensors' last reading.
    #[must_use]
    pub fn motion(&self) -> Motion {
        self.motion
    }

    /// Sets the motion sensors' reading, checked as it was made.
    pub fn set_motion(&mut self, motion: Motion) {
        self.motion = motion;
    }

    /// Builder form of [`set_motion`](Self::set_motion).
    #[must_use]
    pub fn with_motion(mut self, motion: Motion) -> Self {
        self.motion = motion;
        self
    }

    /// Where fingers are on the first touchpad: a DualSense's, or a Steam
    /// Controller's right.
    #[must_use]
    pub fn touchpad(&self) -> Touchpad {
        self.touchpads[0]
    }

    /// Sets where fingers are on the first touchpad, checked as each
    /// contact was made.
    pub fn set_touchpad(&mut self, touchpad: Touchpad) {
        self.touchpads[0] = touchpad;
    }

    /// Builder form of [`set_touchpad`](Self::set_touchpad).
    #[must_use]
    pub fn with_touchpad(mut self, touchpad: Touchpad) -> Self {
        self.set_touchpad(touchpad);
        self
    }

    /// Where fingers are on the second touchpad: a Steam Controller's
    /// left.
    #[must_use]
    pub fn second_touchpad(&self) -> Touchpad {
        self.touchpads[1]
    }

    /// Sets where fingers are on the second touchpad, checked as each
    /// contact was made.
    pub fn set_second_touchpad(&mut self, touchpad: Touchpad) {
        self.touchpads[1] = touchpad;
    }

    /// Builder form of [`set_second_touchpad`](Self::set_second_touchpad).
    #[must_use]
    pub fn with_second_touchpad(mut self, touchpad: Touchpad) -> Self {
        self.set_second_touchpad(touchpad);
        self
    }

    /// Both touchpads, the first then the second.
    pub(crate) fn touchpads(&self) -> &[Touchpad; TOUCHPADS] {
        &self.touchpads
    }

    pub(crate) fn touchpads_mut(&mut self) -> &mut [Touchpad; TOUCHPADS] {
        &mut self.touchpads
    }

    pub(crate) fn buttons(&self) -> ButtonBits {
        self.buttons
    }

    pub(crate) fn buttons_mut(&mut self) -> &mut ButtonBits {
        &mut self.buttons
    }

    pub(crate) fn axes(&self) -> &[f32; AXES] {
        &self.axes
    }

    pub(crate) fn axes_mut(&mut self) -> &mut [f32; AXES] {
        &mut self.axes
    }
}

/// A set of controls that are owned together: a stick's two axes, a yoke's
/// three, a flight grip's axes with its trigger and hat, or one lone button.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Group {
    pub(crate) buttons: ButtonBits,
    pub(crate) axes: AxisBits,
}

impl Group {
    /// A group of the given controls.
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::BadControl`] if a control is beyond the
    /// capacity of a pad state.
    pub fn of(controls: &[Control]) -> Result<Self, GraphError> {
        if controls.iter().all(|control| control.is_valid()) {
            Ok(Self::of_valid(controls))
        } else {
            Err(GraphError::BadControl)
        }
    }

    /// A group of controls already known to be within capacity.
    fn of_valid(controls: &[Control]) -> Self {
        let mut group = Self::default();
        for control in controls {
            match *control {
                Control::Button(index) => group.buttons |= (1 as ButtonBits) << index,
                Control::Axis(index) => group.axes |= (1 as AxisBits) << index,
            }
        }
        group
    }

    /// How far the group's axes are from rest, taken together, so a stick
    /// is judged by a circle rather than per axis.
    #[must_use]
    pub fn deflection(&self, state: &PadState) -> f32 {
        let mut sum = 0.0;
        for (index, value) in state.axes.iter().enumerate() {
            if self.axes & (1 << index) != 0 {
                sum += value * value;
            }
        }
        sum.sqrt()
    }

    /// How far the group is from rest: 1 if any of its buttons is pressed,
    /// otherwise its axes' [`deflection`](Self::deflection).
    #[must_use]
    pub fn activity(&self, state: &PadState) -> f32 {
        if state.buttons & self.buttons != 0 {
            1.0
        } else {
            self.deflection(state)
        }
    }
}

/// What a group is, which is what a mode's rule table is keyed by.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Axes that together give a direction.
    Stick,
    /// One axis that rests at zero and is squeezed.
    Trigger,
    /// One button.
    Button,
    /// Direction buttons.
    Hat,
}

/// A pad's controls arranged into groups.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Layout {
    pub(crate) groups: Vec<(Group, Kind)>,
}

impl Layout {
    /// The groups and what kind each is.
    #[must_use]
    pub fn groups(&self) -> &[(Group, Kind)] {
        &self.groups
    }

    /// The standard gamepad: two sticks, two triggers, a d-pad, and each
    /// face button, bumper, Select, Start, Mode and stick click alone. The
    /// touchpads' clicks, the back buttons and capacitive touch are in no
    /// group.
    #[must_use]
    pub fn gamepad() -> Self {
        use gamepad::*;
        let mut groups = vec![
            (Group::of_valid(&[LEFT_X, LEFT_Y]), Kind::Stick),
            (Group::of_valid(&[RIGHT_X, RIGHT_Y]), Kind::Stick),
            (Group::of_valid(&[LEFT_TRIGGER]), Kind::Trigger),
            (Group::of_valid(&[RIGHT_TRIGGER]), Kind::Trigger),
            (
                Group::of_valid(&[DPAD_UP, DPAD_DOWN, DPAD_LEFT, DPAD_RIGHT]),
                Kind::Hat,
            ),
        ];
        for button in [
            SOUTH,
            EAST,
            NORTH,
            WEST,
            LEFT_BUMPER,
            RIGHT_BUMPER,
            SELECT,
            START,
            MODE,
            LEFT_THUMB,
            RIGHT_THUMB,
        ] {
            groups.push((Group::of_valid(&[button]), Kind::Button));
        }
        Self { groups }
    }

    /// Adds a group.
    ///
    /// # Errors
    ///
    /// Returns [`GraphError::BadControl`] if a control is beyond the
    /// capacity of a pad state.
    pub fn group(mut self, kind: Kind, controls: &[Control]) -> Result<Self, GraphError> {
        self.groups.push((Group::of(controls)?, kind));
        Ok(self)
    }

    /// Zeroes the axes of each group while they rest inside `deadzone`,
    /// as a whole, so rest noise on a worn stick never reaches a node.
    pub(crate) fn apply_deadzone(&self, state: &mut PadState, deadzone: f32) {
        for (group, _) in &self.groups {
            if group.axes != 0 && group.deflection(state) < deadzone {
                for (index, axis) in state.axes.iter_mut().enumerate() {
                    if group.axes & (1 << index) != 0 {
                        *axis = 0.0;
                    }
                }
            }
        }
    }
}
