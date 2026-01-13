pub mod manager;
pub mod modes;
pub mod runtime;

/// Rumble target for mux
#[derive(
    clap::ValueEnum, Clone, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq,
)]
pub enum MuxRumbleTarget {
    Active,
    Assist,
    #[default]
    Both,
    None,
    Primary,
}
