use super::MuxMode;
use evdev::InputEvent;
use gilrs::{Event, GamepadId};

pub struct AverageMode;

impl MuxMode for AverageMode {
    fn handle_event(
        &mut self,
        event: &Event,
        primary_id: GamepadId,
        assist_id: GamepadId,
        gilrs: &gilrs::Gilrs,
    ) -> Option<Vec<InputEvent>> {
        // TODO: Move the current event handling logic here
        None
    }
}
