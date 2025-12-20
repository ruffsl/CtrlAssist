pub mod average;
pub mod helpers;
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

/// Factory function to create the correct mux mode
pub fn create_mux_mode(mode: ModeType) -> Box<dyn MuxMode> {
    match mode {
        ModeType::Average => Box::new(average::AverageMode),
        ModeType::Priority => Box::new(priority::PriorityMode),
        ModeType::Toggle => Box::new(toggle::ToggleMode::default()),
    }
}
