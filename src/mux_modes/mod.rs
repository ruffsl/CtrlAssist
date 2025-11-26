pub mod average;
pub mod priority;
pub mod toggle;

use evdev::InputEvent;
use gilrs::{Event, GamepadId};

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

/// Enum for all mode handlers
pub enum ModeHandler {
    Average(average::AverageMode),
    Priority(priority::PriorityMode),
    Toggle(toggle::ToggleMode),
}

impl MuxMode for ModeHandler {
    fn handle_event(
        &mut self,
        event: &Event,
        primary_id: GamepadId,
        assist_id: GamepadId,
        gilrs: &gilrs::Gilrs,
    ) -> Option<Vec<InputEvent>> {
        match self {
            ModeHandler::Average(m) => m.handle_event(event, primary_id, assist_id, gilrs),
            ModeHandler::Priority(m) => m.handle_event(event, primary_id, assist_id, gilrs),
            ModeHandler::Toggle(m) => m.handle_event(event, primary_id, assist_id, gilrs),
        }
    }
}

use crate::ModeType;
/// Factory function to create the correct mode handler
pub fn make_mode_handler(mode: ModeType) -> ModeHandler {
    match mode {
        ModeType::Average => ModeHandler::Average(average::AverageMode),
        ModeType::Priority => ModeHandler::Priority(priority::PriorityMode),
        ModeType::Toggle => ModeHandler::Toggle(toggle::ToggleMode),
    }
}
