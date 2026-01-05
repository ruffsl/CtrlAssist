use crate::demux_modes::DemuxModeType;
use crate::mux_modes::ModeType;
use crate::{DemuxRumbleTarget, HideType, RumbleTarget, SpoofTarget};
use gilrs::{GamepadId, Gilrs};
use serde::{Deserialize, Serialize};
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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum OperationMode {
    #[default]
    Mux,
    Demux,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OperationStatus {
    Stopped,
    Running,
}

pub struct TrayState {
    /// Available controllers
    pub controllers: Vec<ControllerInfo>,

    /// Current operation mode (Mux or Demux)
    pub operation_mode: OperationMode,

    /// Operation running status
    pub status: OperationStatus,

    /// Operation thread handle (if running)
    pub operation_handle: Option<thread::JoinHandle<()>>,

    /// Shutdown signal for operation thread
    pub shutdown_signal: Option<Arc<AtomicBool>>,

    /// Path(s) to virtual device(s) for FF thread unblocking
    pub virtual_device_paths: Vec<PathBuf>,

    // Mux-specific state
    pub mux: MuxState,

    // Demux-specific state
    pub demux: DemuxState,
}

pub struct MuxState {
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
    /// Shared runtime settings for live updates
    pub runtime_settings: Option<Arc<crate::mux_runtime::RuntimeSettings>>,
}

pub struct DemuxState {
    /// Currently selected primary controller ID
    pub selected_primary: Option<GamepadId>,
    /// Number of virtual sinks
    pub sinks: usize,
    /// Current demux mode
    pub mode: DemuxModeType,
    /// Current hide strategy
    pub hide: HideType,
    /// Current spoof target
    pub spoof: SpoofTarget,
    /// Current rumble target
    pub rumble: DemuxRumbleTarget,
    /// Shared runtime settings for live updates
    pub runtime_settings: Option<Arc<crate::demux_runtime::DemuxRuntimeSettings>>,
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
        let mux_primary = config
            .mux
            .primary_name
            .as_ref()
            .and_then(|name| controllers.iter().find(|c| &c.name == name))
            .map(|c| c.id)
            .or_else(|| controllers.first().map(|c| c.id));

        let mux_assist = config
            .mux
            .assist_name
            .as_ref()
            .and_then(|name| controllers.iter().find(|c| &c.name == name))
            .map(|c| c.id)
            .or_else(|| controllers.get(1).map(|c| c.id));

        let demux_primary = config
            .demux
            .primary_name
            .as_ref()
            .and_then(|name| controllers.iter().find(|c| &c.name == name))
            .map(|c| c.id)
            .or_else(|| controllers.first().map(|c| c.id));

        Self {
            controllers,
            operation_mode: config.operation_mode,
            status: OperationStatus::Stopped,
            operation_handle: None,
            shutdown_signal: None,
            virtual_device_paths: Vec::new(),
            mux: MuxState {
                selected_primary: mux_primary,
                selected_assist: mux_assist,
                mode: config.mux.mode,
                hide: config.mux.hide,
                spoof: config.mux.spoof,
                rumble: config.mux.rumble,
                runtime_settings: None,
            },
            demux: DemuxState {
                selected_primary: demux_primary,
                sinks: config.demux.sinks,
                mode: config.demux.mode,
                hide: config.demux.hide,
                spoof: config.demux.spoof,
                rumble: config.demux.rumble,
                runtime_settings: None,
            },
        }
    }

    pub fn to_config(&self) -> TrayConfig {
        TrayConfig {
            operation_mode: self.operation_mode,
            mux: super::config::MuxConfig {
                primary_name: self
                    .mux
                    .selected_primary
                    .and_then(|id| self.controllers.iter().find(|c| c.id == id))
                    .map(|c| c.name.clone()),
                assist_name: self
                    .mux
                    .selected_assist
                    .and_then(|id| self.controllers.iter().find(|c| c.id == id))
                    .map(|c| c.name.clone()),
                mode: self.mux.mode.clone(),
                hide: self.mux.hide.clone(),
                spoof: self.mux.spoof.clone(),
                rumble: self.mux.rumble.clone(),
            },
            demux: super::config::DemuxConfig {
                primary_name: self
                    .demux
                    .selected_primary
                    .and_then(|id| self.controllers.iter().find(|c| c.id == id))
                    .map(|c| c.name.clone()),
                sinks: self.demux.sinks,
                mode: self.demux.mode.clone(),
                hide: self.demux.hide.clone(),
                spoof: self.demux.spoof.clone(),
                rumble: self.demux.rumble.clone(),
            },
        }
    }

    pub fn is_valid_for_mux_start(&self) -> bool {
        self.operation_mode == OperationMode::Mux
            && self.mux.selected_primary.is_some()
            && self.mux.selected_assist.is_some()
            && self.mux.selected_primary != self.mux.selected_assist
            && self.status == OperationStatus::Stopped
    }

    pub fn is_valid_for_demux_start(&self) -> bool {
        self.operation_mode == OperationMode::Demux
            && self.demux.selected_primary.is_some()
            && self.demux.sinks > 0
            && self.status == OperationStatus::Stopped
    }

    pub fn get_mux_primary_name(&self) -> String {
        self.mux
            .selected_primary
            .and_then(|id| self.controllers.iter().find(|c| c.id == id))
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "None".to_string())
    }

    pub fn get_mux_assist_name(&self) -> String {
        self.mux
            .selected_assist
            .and_then(|id| self.controllers.iter().find(|c| c.id == id))
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "None".to_string())
    }

    pub fn get_demux_primary_name(&self) -> String {
        self.demux
            .selected_primary
            .and_then(|id| self.controllers.iter().find(|c| c.id == id))
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "None".to_string())
    }
}
