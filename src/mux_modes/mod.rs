pub mod average;
pub mod priority;
pub mod toggle;

use evdev::InputEvent;
use gilrs::{Event, GamepadId};

// Enum for all muxing modes
#[derive(clap::ValueEnum, Clone, Debug, Default)]
pub enum ModeType {
    Average,
    #[default]
    Priority,
    Toggle,
}

/// The trait all muxing modes must implement
pub trait MuxMode {
    fn handle_event(
        &mut self,
        event: &Event,
        primary_id: GamepadId,
        assist_id: GamepadId,
        gilrs: &gilrs::Gilrs,
    ) -> Option<Vec<InputEvent>>;
}

/// Factory function to create the correct mode handler
pub fn make_mode_handler(mode: ModeType) -> Box<dyn MuxMode> {
    match mode {
        ModeType::Average => Box::new(average::AverageMode::new()),
        ModeType::Priority => Box::new(priority::PriorityMode::new()),
        ModeType::Toggle => Box::new(toggle::ToggleMode::new()),
    }
}
