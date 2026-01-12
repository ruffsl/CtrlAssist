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

/// Output from a mux mode handling an event
pub struct MuxOutput {
    /// Events to forward to the virtual device
    pub events: Vec<InputEvent>,
    /// Optional request to update which controllers are considered "active" for FF
    pub set_active_controllers: Option<Vec<GamepadId>>,
}

impl MuxOutput {
    /// Create output with only events
    pub fn events(events: Vec<InputEvent>) -> Self {
        Self {
            events,
            set_active_controllers: None,
        }
    }
    
    /// Create output with events and active controller update
    pub fn with_active(events: Vec<InputEvent>, active: Vec<GamepadId>) -> Self {
        Self {
            events,
            set_active_controllers: Some(active),
        }
    }
}

/// The trait all muxing modes must implement
pub trait MuxMode {
    fn handle_event(
        &mut self,
        event: &Event,
        primary_id: GamepadId,
        assist_id: GamepadId,
        gilrs: &gilrs::Gilrs,
    ) -> Option<MuxOutput>;
    
    /// Return the initial set of active controllers for this mode.
    /// Used when the mode is first initialized or changes.
    fn initial_active_controllers(&self, primary_id: GamepadId, assist_id: GamepadId) -> Vec<GamepadId> {
        vec![primary_id, assist_id]
    }
}

/// Factory function to create the correct mux mode
pub fn create_mux_mode(mode: MuxModeType) -> Box<dyn MuxMode> {
    match mode {
        MuxModeType::Average => Box::new(average::AverageMode),
        MuxModeType::Priority => Box::new(priority::PriorityMode),
        MuxModeType::Toggle => Box::new(toggle::ToggleMode::default()),
    }
}
