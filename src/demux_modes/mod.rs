pub mod helpers;
pub mod multicast;
pub mod unicast;

use evdev::InputEvent;
use gilrs::{Event, GamepadId};
use serde::{Deserialize, Serialize};

/// Enum for all demuxing modes
#[derive(clap::ValueEnum, Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub enum DemuxModeType {
    Multicast,
    #[default]
    Unicast,
}

/// Output from a demux mode handling an event
pub struct DemuxOutput {
    /// Events routed to specific virtual devices
    pub events: Vec<(usize, Vec<InputEvent>)>,
    /// Optional request to update which virtuals are considered "active"
    pub set_active_virtuals: Option<Vec<usize>>,
}

impl DemuxOutput {
    /// Create output with only events
    pub fn events(events: Vec<(usize, Vec<InputEvent>)>) -> Self {
        Self {
            events,
            set_active_virtuals: None,
        }
    }
}

/// The trait all demuxing modes must implement
pub trait DemuxMode {
    /// Handle an input event and return routed events
    fn handle_event(
        &mut self,
        event: &Event,
        primary_id: GamepadId,
        virtuals: usize,
        gilrs: &gilrs::Gilrs,
    ) -> Option<DemuxOutput>;

    /// Define which virtuals should be active by default for this mode
    fn initial_active_virtuals(&self, virtuals: usize) -> Vec<usize>;
}

/// Factory function to create the correct demux mode
pub fn create_demux_mode(mode: DemuxModeType) -> Box<dyn DemuxMode> {
    match mode {
        DemuxModeType::Multicast => Box::new(multicast::MulticastMode),
        DemuxModeType::Unicast => Box::new(unicast::UnicastMode::default()),
    }
}
