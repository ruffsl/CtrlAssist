pub mod average;
pub mod helpers;
pub mod priority;
pub mod toggle;

use evdev::InputEvent;
use gilrs::{Event, GamepadId};
use serde::{Deserialize, Serialize};

// Enum for all muxing modes
#[derive(clap::ValueEnum, Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub enum MuxModeType {
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
pub fn create_mux_mode(mode: MuxModeType) -> Box<dyn MuxMode> {
    match mode {
        MuxModeType::Average => Box::new(average::AverageMode),
        MuxModeType::Priority => Box::new(priority::PriorityMode),
        MuxModeType::Toggle => Box::new(toggle::ToggleMode::default()),
    }
}
