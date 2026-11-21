//! The demux node: one pad in, several out.

use crate::control::{Control, PadState, gamepad};

/// How a demux spreads its input.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DemuxMode {
    /// The selected output follows the input; the others rest.
    Unicast,
    /// Every output follows the input.
    Multicast,
}

/// Which outputs a demux accepts feedback from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DemuxFeedback {
    /// None of them.
    None,
    /// The outputs the input currently reaches.
    Active,
}

/// A demux node's settings.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DemuxConfig {
    /// The mode.
    pub mode: DemuxMode,
    /// How many outputs it has.
    pub outputs: usize,
    /// The button that moves a unicast to its next output, besides a
    /// client asking. It is consumed.
    pub switch: Option<Control>,
    /// Where feedback comes from.
    pub feedback: DemuxFeedback,
}

impl DemuxConfig {
    /// A mode with the defaults: unicast switches on the Mode button.
    #[must_use]
    pub fn new(mode: DemuxMode, outputs: usize) -> Self {
        Self {
            mode,
            outputs,
            switch: (mode == DemuxMode::Unicast).then_some(gamepad::MODE),
            feedback: DemuxFeedback::Active,
        }
    }
}

pub(crate) struct Demux {
    config: DemuxConfig,
    selected: usize,
    switch_down: bool,
}

impl Demux {
    pub(crate) fn new(config: DemuxConfig) -> Self {
        Self {
            config,
            selected: 0,
            switch_down: false,
        }
    }

    pub(crate) fn switch(&mut self) {
        self.selected = (self.selected + 1) % self.config.outputs;
    }

    /// Whether feedback from this output is passed back to the input.
    pub(crate) fn feedback_from(&self, output: usize) -> bool {
        self.config.feedback == DemuxFeedback::Active
            && (self.config.mode == DemuxMode::Multicast || output == self.selected)
    }

    pub(crate) fn step(&mut self, mut input: PadState, outputs: &mut [PadState]) {
        if let Some(Control::Button(button)) = self.config.switch {
            let bit = (1 as crate::control::ButtonBits) << button;
            let down = input.buttons() & bit != 0;
            *input.buttons_mut() &= !bit;
            if down && !self.switch_down {
                self.switch();
            }
            self.switch_down = down;
        }
        for (index, output) in outputs.iter_mut().enumerate() {
            *output = if self.config.mode == DemuxMode::Multicast || index == self.selected {
                input
            } else {
                PadState::NEUTRAL
            };
        }
    }
}
