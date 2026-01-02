// pub mod helpers;
pub mod multicast;
pub mod unicast;

use evdev::InputEvent;
use gilrs::{Event, GamepadId};
use serde::{Deserialize, Serialize};

// Enum for all demuxing modes
#[derive(clap::ValueEnum, Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub enum ModeType {
    Multicast,
    #[default]
    Unicast,
}

/// The trait all demuxing modes must implement
pub trait DuxMode {
    fn handle_event(
        &mut self,
        event: &Event,
        primary_id: GamepadId,
        virtual_ids: &Vec<GamepadId>,
        gilrs: &gilrs::Gilrs,
    ) -> Option<Vec<InputEvent>>;
}

/// Factory function to create the correct dux mode
pub fn create_dux_mode(mode: ModeType) -> Box<dyn DuxMode> {
    match mode {
        ModeType::Multicast => Box::new(multicast::MulticastMode),
        ModeType::Unicast => Box::new(unicast::UnicastMode::default()),
    }
}
