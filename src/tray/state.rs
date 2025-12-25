use crate::mux_modes::ModeType;
use crate::{HideType, RumbleTarget, SpoofTarget};
use gilrs::{GamepadId, Gilrs};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;

use super::config::TrayConfig;

#[derive(Debug, Clone)]
pub struct ControllerInfo {
    pub id: GamepadId,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MuxStatus {
    Stopped,
    Running,
}

pub struct TrayState {
    /// Available controllers
    pub controllers: Vec<ControllerInfo>,
    /// Currently selected primary controller ID
    pub selected_primary: Option<GamepadId>,
    /// Currently selected assist controller ID
    pub selected_assist: Option<GamepadId>,
    /// Current mux mode
    pub mode: ModeType,
    /// Current hide strategy
    pub hide: HideType,
    /// Current spoof target
    pub spoof: SpoofTarget,
    /// Current rumble target
    pub rumble: RumbleTarget,
    /// Mux running status
    pub status: MuxStatus,
    /// Mux thread handle (if running)
    pub mux_handle: Option<thread::JoinHandle<()>>,
    /// Shutdown signal for mux thread
    pub shutdown_signal: Option<Arc<AtomicBool>>,
    /// Path to virtual device for FF thread unblocking
    pub virtual_device_path: Option<PathBuf>,
}

impl TrayState {
    pub fn new(gilrs: &Gilrs, config: TrayConfig) -> Self {
        let controllers: Vec<ControllerInfo> = gilrs
            .gamepads()
            .map(|(id, gamepad)| ControllerInfo {
                id,
                name: gamepad.name().to_string(),
            })
            .collect();

        // Try to match saved controller names to current controllers (best-effort)
        let selected_primary = config
            .primary_name
            .as_ref()
            .and_then(|name| controllers.iter().find(|c| &c.name == name))
            .map(|c| c.id)
            .or_else(|| controllers.first().map(|c| c.id));

        let selected_assist = config
            .assist_name
            .as_ref()
            .and_then(|name| controllers.iter().find(|c| &c.name == name))
            .map(|c| c.id)
            .or_else(|| controllers.get(1).map(|c| c.id));

        Self {
            controllers,
            selected_primary,
            selected_assist,
            mode: config.mode,
            hide: config.hide,
            spoof: config.spoof,
            rumble: config.rumble,
            status: MuxStatus::Stopped,
            mux_handle: None,
            shutdown_signal: None,
            virtual_device_path: None,
        }
    }

    pub fn to_config(&self) -> TrayConfig {
        TrayConfig {
            primary_name: self
                .selected_primary
                .and_then(|id| self.controllers.iter().find(|c| c.id == id))
                .map(|c| c.name.clone()),
            assist_name: self
                .selected_assist
                .and_then(|id| self.controllers.iter().find(|c| c.id == id))
                .map(|c| c.name.clone()),
            mode: self.mode.clone(),
            hide: self.hide.clone(),
            spoof: self.spoof.clone(),
            rumble: self.rumble.clone(),
        }
    }

    pub fn is_valid_for_start(&self) -> bool {
        self.selected_primary.is_some()
            && self.selected_assist.is_some()
            && self.selected_primary != self.selected_assist
            && self.status == MuxStatus::Stopped
    }

    pub fn get_primary_name(&self) -> String {
        self.selected_primary
            .and_then(|id| self.controllers.iter().find(|c| c.id == id))
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "None".to_string())
    }

    pub fn get_assist_name(&self) -> String {
        self.selected_assist
            .and_then(|id| self.controllers.iter().find(|c| c.id == id))
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "None".to_string())
    }
}
