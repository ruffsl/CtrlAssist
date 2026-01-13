pub mod manager;
pub mod modes;
pub mod runtime;

/// Rumble target for demux
#[derive(
    clap::ValueEnum, Clone, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq,
)]
pub enum DemuxRumbleTarget {
    #[default]
    Active,
    None,
}
