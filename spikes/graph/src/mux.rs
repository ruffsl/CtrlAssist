//! The mux node: several pads in, one out.

use crate::control::{AXES, Control, Group, Kind, Layout, PadState, gamepad};

/// How a group's output is chosen from the inputs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rule {
    /// The highest input that is active on the group owns every member of
    /// it; with none active, the first input does.
    Owned,
    /// Each member takes the input furthest from rest; a button is pressed
    /// if any input holds it.
    Largest,
    /// Each axis is the mean of the inputs active on the group; a button
    /// is pressed if any input holds it.
    Blended,
    /// The input the switch last selected owns the group.
    Switched,
}

/// The mux modes, each a table of rules by group kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// The assist overrides what it touches; the larger trigger wins.
    Priority,
    /// Sticks and triggers blend; buttons combine.
    Average,
    /// One pad at a time, handed over by the switch.
    Toggle,
}

impl Mode {
    /// The rule this mode applies to a group of the given kind.
    #[must_use]
    pub fn rule(self, kind: Kind) -> Rule {
        match (self, kind) {
            (Self::Toggle, _) => Rule::Switched,
            (Self::Priority, Kind::Trigger) => Rule::Largest,
            (Self::Priority, _) => Rule::Owned,
            (Self::Average, Kind::Stick | Kind::Trigger) => Rule::Blended,
            (Self::Average, Kind::Button | Kind::Hat) => Rule::Largest,
        }
    }
}

/// The button that raises a mux's switch. It is consumed: the game never
/// sees it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Switch {
    /// The input whose button it is, or any input.
    pub input: Option<usize>,
    /// The button.
    pub button: Control,
}

/// Which inputs a mux passes feedback back to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MuxFeedback {
    /// None of them.
    None,
    /// A fixed set, as a mask over the inputs, whoever is providing input.
    Inputs(u32),
    /// The inputs that can currently reach the output: the selected one if
    /// any group is switched, otherwise all of them.
    Active,
}

/// A mux node's settings.
#[derive(Clone, PartialEq, Debug)]
pub struct MuxConfig {
    /// The rule for each group. Controls in no group follow the first input.
    pub rules: Vec<(Group, Rule)>,
    /// An input becomes active on a group at or above this.
    pub engage: f32,
    /// An active input stops being active on a group below this.
    pub release: f32,
    /// What raises the switch, besides a client asking.
    pub switch: Option<Switch>,
    /// Where feedback goes.
    pub feedback: MuxFeedback,
}

impl MuxConfig {
    /// A mode's rules over a layout, with the default thresholds. Toggle
    /// switches on the second input's Mode button.
    #[must_use]
    pub fn new(mode: Mode, layout: &Layout) -> Self {
        Self {
            rules: layout
                .groups
                .iter()
                .map(|(group, kind)| (*group, mode.rule(*kind)))
                .collect(),
            engage: 0.1,
            release: 0.05,
            switch: (mode == Mode::Toggle).then_some(Switch {
                input: Some(1),
                button: gamepad::MODE,
            }),
            feedback: MuxFeedback::Active,
        }
    }
}

pub(crate) struct Mux {
    config: MuxConfig,
    inputs: usize,
    /// Per group, which inputs are active on it, as a mask.
    active: Vec<u32>,
    selected: usize,
    switch_down: bool,
}

fn copy_group(out: &mut PadState, from: &PadState, group: &Group) {
    let buttons = out.buttons_mut();
    *buttons = (*buttons & !group.buttons) | (from.buttons() & group.buttons);
    for index in 0..AXES {
        if group.axes & (1 << index) != 0 {
            out.axes_mut()[index] = from.axes()[index];
        }
    }
}

impl Mux {
    pub(crate) fn new(config: MuxConfig, inputs: usize) -> Self {
        Self {
            active: vec![0; config.rules.len()],
            config,
            inputs,
            selected: 0,
            switch_down: false,
        }
    }

    /// Raises the switch, as a client would.
    pub(crate) fn switch(&mut self) {
        self.selected = (self.selected + 1) % self.inputs;
    }

    /// The inputs feedback goes to, as a mask.
    pub(crate) fn feedback_inputs(&self) -> u32 {
        match self.config.feedback {
            MuxFeedback::None => 0,
            MuxFeedback::Inputs(mask) => mask & ((1 << self.inputs) - 1),
            MuxFeedback::Active => {
                if self.config.rules.iter().any(|(_, rule)| *rule == Rule::Switched) {
                    1 << self.selected
                } else {
                    (1 << self.inputs) - 1
                }
            }
        }
    }

    pub(crate) fn step(&mut self, inputs: &mut [PadState], out: &mut PadState) {
        // The switch button raises the switch on its press, once, and is
        // then removed so nothing downstream sees it.
        if let Some(Switch {
            input,
            button: Control::Button(button),
        }) = self.config.switch
        {
            let bit = 1u64 << button;
            let mut down = false;
            for (index, state) in inputs.iter_mut().enumerate() {
                if input.is_none_or(|input| input == index) {
                    down |= state.buttons() & bit != 0;
                    *state.buttons_mut() &= !bit;
                }
            }
            if down && !self.switch_down {
                self.switch();
            }
            self.switch_down = down;
        }

        *out = inputs[0];
        for (index, (group, rule)) in self.config.rules.iter().enumerate() {
            let active = &mut self.active[index];
            for (input, state) in inputs.iter().enumerate() {
                let activity = group.activity(state);
                if activity >= self.config.engage {
                    *active |= 1 << input;
                } else if activity < self.config.release {
                    *active &= !(1 << input);
                }
            }
            match rule {
                Rule::Owned => {
                    let owner = if *active == 0 { 0 } else { (31 - active.leading_zeros()) as usize };
                    copy_group(out, &inputs[owner], group);
                }
                Rule::Switched => copy_group(out, &inputs[self.selected], group),
                Rule::Largest | Rule::Blended => {
                    let mut buttons = 0;
                    for state in inputs.iter() {
                        buttons |= state.buttons() & group.buttons;
                    }
                    *out.buttons_mut() = (out.buttons() & !group.buttons) | buttons;
                    for axis in 0..AXES {
                        if group.axes & (1 << axis) == 0 {
                            continue;
                        }
                        out.axes_mut()[axis] = if *rule == Rule::Largest {
                            inputs
                                .iter()
                                .map(|state| state.axes()[axis])
                                .fold(0.0, |best: f32, value| if value.abs() > best.abs() { value } else { best })
                        } else if *active == 0 {
                            inputs[0].axes()[axis]
                        } else {
                            let mut sum = 0.0;
                            for (input, state) in inputs.iter().enumerate() {
                                if *active & (1 << input) != 0 {
                                    sum += state.axes()[axis];
                                }
                            }
                            sum / active.count_ones() as f32
                        };
                    }
                }
            }
        }
    }
}
