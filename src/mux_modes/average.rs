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
        todo!("Average mode not yet implemented");
    }
}
