use crate::mux_modes::ModeType;
use crate::demux_modes::DemuxModeType;
use crate::{HideType, RumbleTarget, DemuxRumbleTarget, SpoofTarget};
use crate::tray::state::OperationMode;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TrayConfig {
    #[serde(default)]
    pub operation_mode: OperationMode,
    #[serde(default)]
    pub mux: MuxConfig,
    #[serde(default)]
    pub demux: DemuxConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MuxConfig {
    /// Last selected primary controller (by name for best-effort matching)
    pub primary_name: Option<String>,
    /// Last selected assist controller (by name)
    pub assist_name: Option<String>,
    /// Last used mux mode
    #[serde(default)]
    pub mode: ModeType,
    /// Last used hide strategy
    #[serde(default)]
    pub hide: HideType,
    /// Last used spoof target
    #[serde(default)]
    pub spoof: SpoofTarget,
    /// Last used rumble target
    #[serde(default)]
    pub rumble: RumbleTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DemuxConfig {
    /// Last selected primary controller (by name)
    pub primary_name: Option<String>,
    /// Last used sinks count
    pub sinks: usize,
    /// Last used demux mode
    #[serde(default)]
    pub mode: DemuxModeType,
    /// Last used hide strategy
    #[serde(default)]
    pub hide: HideType,
    /// Last used spoof target
    #[serde(default)]
    pub spoof: SpoofTarget,
    /// Last used rumble target
    #[serde(default)]
    pub rumble: DemuxRumbleTarget,
}

impl TrayConfig {
    /// Get the config file path ($XDG_CONFIG_HOME/ctrlassist/config.toml)
    pub fn config_path() -> Result<PathBuf, Box<dyn Error>> {
        let config_dir = dirs::config_dir()
            .ok_or("Could not determine config directory")?
            .join("ctrlassist");

        fs::create_dir_all(&config_dir)?;
        Ok(config_dir.join("config.toml"))
    }

    /// Load config from disk, or return default if not found
    pub fn load() -> Self {
        match Self::config_path() {
            Ok(path) => {
                if path.exists() {
                    match fs::read_to_string(&path) {
                        Ok(content) => match toml::from_str(&content) {
                            Ok(config) => {
                                info!("Loaded config from {}", path.display());
                                return config;
                            }
                            Err(e) => {
                                warn!("Failed to parse config file: {}", e);
                            }
                        },
                        Err(e) => {
                            warn!("Failed to read config file: {}", e);
                        }
                    }
                }
            }
            Err(e) => {
                warn!("Failed to get config path: {}", e);
            }
        }

        info!("Using default configuration");
        Self::default()
    }

    /// Save config to disk
    pub fn save(&self) -> Result<(), Box<dyn Error>> {
        let path = Self::config_path()?;
        let content = toml::to_string_pretty(self)?;
        fs::write(&path, content)?;
        info!("Saved config to {}", path.display());
        Ok(())
    }
}
