//! The mux node: several pads in, one out.

use crate::control::{AXES, AxisBits, ButtonBits, Control, Group, Kind, Layout, PadState, gamepad};
use crate::graph::GraphError;

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
    /// The rule for each group.
    pub rules: Vec<(Group, Rule)>,
    /// The rule for a button in no group, as a group of one.
    pub ungrouped_buttons: Rule,
    /// The rule for an axis in no group, as a group of one.
    pub ungrouped_axes: Rule,
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
            ungrouped_buttons: mode.rule(Kind::Button),
            ungrouped_axes: mode.rule(Kind::Stick),
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
    /// The configured groups, then one group per axis that is in none.
    rules: Vec<(Group, Rule)>,
    /// Buttons in no group. As groups of one they need no activity state:
    /// every rule but `Switched` comes to "down if any input holds it".
    loose_buttons: ButtonBits,
    /// Per group, which inputs are active on it, as a mask.
    active: Vec<u32>,
    selected: usize,
    /// Which inputs were present at the last step, as a mask.
    present: u32,
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
    /// Checks everything a profile can say about a mux.
    pub(crate) fn validate(config: &MuxConfig, inputs: usize) -> Result<(), GraphError> {
        let thresholds_ok = config.engage.is_finite()
            && config.release.is_finite()
            && config.release >= 0.0
            && config.release <= config.engage;
        if !thresholds_ok {
            return Err(GraphError::BadThreshold);
        }
        if let Some(switch) = config.switch {
            let is_button = matches!(switch.button, Control::Button(_)) && switch.button.is_valid();
            if !is_button || switch.input.is_some_and(|input| input >= inputs) {
                return Err(GraphError::BadControl);
            }
        }
        Ok(())
    }

    pub(crate) fn new(config: MuxConfig, inputs: usize) -> Self {
        let mut buttons: ButtonBits = 0;
        let mut axes: AxisBits = 0;
        for (group, _) in &config.rules {
            buttons |= group.buttons;
            axes |= group.axes;
        }
        let mut rules = config.rules.clone();
        for axis in 0..AXES {
            if axes & (1 << axis) == 0 {
                let group = Group {
                    buttons: 0,
                    axes: 1 << axis,
                };
                rules.push((group, config.ungrouped_axes));
            }
        }
        Self {
            active: vec![0; rules.len()],
            rules,
            loose_buttons: !buttons,
            config,
            inputs,
            selected: 0,
            present: u32::MAX,
            switch_down: false,
        }
    }

    /// Moves the selection to the next input that is present.
    pub(crate) fn switch(&mut self) {
        for step in 1..=self.inputs {
            let candidate = (self.selected + step) % self.inputs;
            if self.present & (1 << candidate) != 0 {
                self.selected = candidate;
                return;
            }
        }
    }

    /// Selects an input by index, as a client asks. Does nothing if it is
    /// out of range, so a repeated or stale request is harmless.
    pub(crate) fn select(&mut self, input: usize) {
        if input < self.inputs {
            self.selected = input;
        }
    }

    fn uses_selection(&self) -> bool {
        self.config.ungrouped_buttons == Rule::Switched
            || self.rules.iter().any(|(_, rule)| *rule == Rule::Switched)
    }

    /// The inputs feedback goes to, as a mask.
    pub(crate) fn feedback_inputs(&self) -> u32 {
        let all = (1 << self.inputs) - 1;
        match self.config.feedback {
            MuxFeedback::None => 0,
            MuxFeedback::Inputs(mask) => mask & all,
            MuxFeedback::Active if self.uses_selection() => 1 << self.selected,
            MuxFeedback::Active => all,
        }
    }

    /// Computes the output from the inputs. `present` says which inputs
    /// are connected; an absent one is expected to read as at rest.
    pub(crate) fn step(&mut self, inputs: &mut [PadState], present: u32, out: &mut PadState) {
        self.present = present;
        // The switch button raises the switch on its press, once, and is
        // then removed so nothing downstream sees it.
        if let Some(Switch {
            input,
            button: Control::Button(button),
        }) = self.config.switch
        {
            let bit = (1 as ButtonBits) << button;
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
        // The selection never rests on an input that is gone.
        if present & (1 << self.selected) == 0 {
            self.switch();
        }

        // Buttons in no group, all at once.
        *out = PadState::NEUTRAL;
        let loose = if self.config.ungrouped_buttons == Rule::Switched {
            inputs[self.selected].buttons()
        } else {
            inputs.iter().fold(0, |held, state| held | state.buttons())
        };
        *out.buttons_mut() = loose & self.loose_buttons;

        for (index, (group, rule)) in self.rules.iter().enumerate() {
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
                    let owner = if *active == 0 {
                        0
                    } else {
                        (31 - active.leading_zeros()) as usize
                    };
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
                            inputs.iter().map(|state| state.axes()[axis]).fold(
                                0.0,
                                |best: f32, value| {
                                    if value.abs() > best.abs() {
                                        value
                                    } else {
                                        best
                                    }
                                },
                            )
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
