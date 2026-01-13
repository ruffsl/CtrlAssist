pub mod manager;
pub mod modes;
pub mod runtime;

#[derive(
    clap::ValueEnum, Clone, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq,
)]
pub enum MuxRumbleTarget {
    #[default]
    Active,
    Assist,
    Both,
    None,
    Primary,
}
