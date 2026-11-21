//! Controls, pad state and groups.

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

    pub const LEFT_X: Control = Control::Axis(0);
    pub const LEFT_Y: Control = Control::Axis(1);
    pub const RIGHT_X: Control = Control::Axis(2);
    pub const RIGHT_Y: Control = Control::Axis(3);
    pub const LEFT_TRIGGER: Control = Control::Axis(4);
    pub const RIGHT_TRIGGER: Control = Control::Axis(5);
}

/// Everything a pad holds at one instant.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PadState {
    buttons: ButtonBits,
    axes: [f32; AXES],
}

impl PadState {
    /// A pad at rest: nothing pressed, every axis at zero.
    pub const NEUTRAL: Self = Self {
        buttons: 0,
        axes: [0.0; AXES],
    };

    /// Sets one control. A button is pressed by any value above zero.
    pub fn set(&mut self, control: Control, value: f32) {
        match control {
            Control::Button(index) => {
                let bit = (1 as ButtonBits) << (usize::from(index) % BUTTONS);
                if value > 0.0 {
                    self.buttons |= bit;
                } else {
                    self.buttons &= !bit;
                }
            }
            Control::Axis(index) => self.axes[usize::from(index) % AXES] = value,
        }
    }

    /// Builder form of [`set`](Self::set).
    #[must_use]
    pub fn with(mut self, control: Control, value: f32) -> Self {
        self.set(control, value);
        self
    }

    /// One control's value; a button reads as 1 or 0.
    #[must_use]
    pub fn get(&self, control: Control) -> f32 {
        match control {
            Control::Button(index) => {
                if self.buttons & ((1 as ButtonBits) << (usize::from(index) % BUTTONS)) == 0 {
                    0.0
                } else {
                    1.0
                }
            }
            Control::Axis(index) => self.axes[usize::from(index) % AXES],
        }
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
    #[must_use]
    pub fn of(controls: &[Control]) -> Self {
        let mut group = Self::default();
        for control in controls {
            match *control {
                Control::Button(index) => {
                    group.buttons |= (1 as ButtonBits) << (usize::from(index) % BUTTONS);
                }
                Control::Axis(index) => {
                    group.axes |= (1 as AxisBits) << (usize::from(index) % AXES);
                }
            }
        }
        group
    }

    /// How far the group is from rest: 1 if any of its buttons is pressed,
    /// otherwise the distance of its axes from zero taken together, so a
    /// stick is judged by a circle rather than per axis.
    #[must_use]
    pub fn activity(&self, state: &PadState) -> f32 {
        if state.buttons & self.buttons != 0 {
            return 1.0;
        }
        let mut sum = 0.0;
        for (index, value) in state.axes.iter().enumerate() {
            if self.axes & (1 << index) != 0 {
                sum += value * value;
            }
        }
        sum.sqrt()
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
    /// The standard gamepad: two sticks, two triggers, a d-pad, and each
    /// remaining button alone.
    #[must_use]
    pub fn gamepad() -> Self {
        use gamepad::*;
        let mut layout = Self::default()
            .group(Kind::Stick, &[LEFT_X, LEFT_Y])
            .group(Kind::Stick, &[RIGHT_X, RIGHT_Y])
            .group(Kind::Trigger, &[LEFT_TRIGGER])
            .group(Kind::Trigger, &[RIGHT_TRIGGER])
            .group(Kind::Hat, &[DPAD_UP, DPAD_DOWN, DPAD_LEFT, DPAD_RIGHT]);
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
            layout = layout.group(Kind::Button, &[button]);
        }
        layout
    }

    /// Adds a group.
    #[must_use]
    pub fn group(mut self, kind: Kind, controls: &[Control]) -> Self {
        self.groups.push((Group::of(controls), kind));
        self
    }

    /// Zeroes each stick-like group that sits inside `deadzone`, as a
    /// whole, so rest noise on a worn stick never reaches a node.
    pub(crate) fn apply_deadzone(&self, state: &mut PadState, deadzone: f32) {
        for (group, _) in &self.groups {
            if group.axes != 0 && group.buttons == 0 && group.activity(state) < deadzone {
                for (index, axis) in state.axes.iter_mut().enumerate() {
                    if group.axes & (1 << index) != 0 {
                        *axis = 0.0;
                    }
                }
            }
        }
    }
}
