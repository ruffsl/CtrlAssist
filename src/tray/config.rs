use crate::mux_modes::ModeType;
use crate::{HideType, RumbleTarget, SpoofTarget};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrayConfig {
    /// Last selected primary controller (by name for best-effort matching)
    pub primary_name: Option<String>,
    /// Last selected assist controller (by name)
    pub assist_name: Option<String>,
    /// Last used mux mode
    pub mode: ModeType,
    /// Last used hide strategy
    pub hide: HideType,
    /// Last used spoof target
    pub spoof: SpoofTarget,
    /// Last used rumble target
    pub rumble: RumbleTarget,
}

impl Default for TrayConfig {
    fn default() -> Self {
        Self {
            primary_name: None,
            assist_name: None,
            mode: ModeType::default(),
            hide: HideType::default(),
            spoof: SpoofTarget::default(),
            rumble: RumbleTarget::default(),
        }
    }
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
