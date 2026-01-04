pub mod unicast;
pub mod multicast;
pub mod helpers;

use evdev::InputEvent;
use gilrs::{Event, GamepadId};
use serde::{Deserialize, Serialize};

/// Enum for all demuxing modes
#[derive(clap::ValueEnum, Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub enum DemuxModeType {
    #[default]
    Unicast,
    Multicast,
}

/// The trait all demuxing modes must implement
/// Takes events from one primary controller and routes to multiple virtual devices
pub trait DemuxMode {
    fn handle_event(
        &mut self,
        event: &Event,
        primary_id: GamepadId,
        virtual_count: usize,
        gilrs: &gilrs::Gilrs,
    ) -> Option<Vec<(usize, Vec<InputEvent>)>>; // Returns (virtual_index, events) pairs
}

/// Factory function to create the correct demux mode
pub fn create_demux_mode(mode: DemuxModeType) -> Box<dyn DemuxMode> {
    match mode {
        DemuxModeType::Unicast => Box::new(unicast::UnicastMode::default()),
        DemuxModeType::Multicast => Box::new(multicast::MulticastMode),
    }
}
