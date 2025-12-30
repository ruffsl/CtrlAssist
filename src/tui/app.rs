// src/tui/app.rs

use crate::mux_manager::{self, MuxConfig, MuxHandle};
use crate::mux_modes::ModeType;
use crate::tray::config::TrayConfig;
use crate::{HideType, RumbleTarget, SpoofTarget};
use gilrs::{GamepadId, Gilrs};
use log::{error, info};
use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FocusedSection {
    PrimaryController,
    AssistController,
    Mode,
    Hide,
    Spoof,
    Rumble,
    StartStop,
}

impl FocusedSection {
    pub fn next(&self) -> Self {
        match self {
            Self::PrimaryController => Self::AssistController,
            Self::AssistController => Self::Mode,
            Self::Mode => Self::Hide,
            Self::Hide => Self::Spoof,
            Self::Spoof => Self::Rumble,
            Self::Rumble => Self::StartStop,
            Self::StartStop => Self::PrimaryController,
        }
    }

    pub fn prev(&self) -> Self {
        match self {
            Self::PrimaryController => Self::StartStop,
            Self::AssistController => Self::PrimaryController,
            Self::Mode => Self::AssistController,
            Self::Hide => Self::Mode,
            Self::Spoof => Self::Hide,
            Self::Rumble => Self::Spoof,
            Self::StartStop => Self::Rumble,
        }
    }
}

pub struct TuiApp {
    // Controller state
    pub controllers: Vec<ControllerInfo>,
    pub selected_primary: Option<GamepadId>,
    pub selected_assist: Option<GamepadId>,
    
    // Mux settings
    pub mode: ModeType,
    pub hide: HideType,
    pub spoof: SpoofTarget,
    pub rumble: RumbleTarget,
    
    // UI state
    pub focused_section: FocusedSection,
    pub status: MuxStatus,
    pub status_message: String,
    
    // Mux state
    mux_handle: Option<thread::JoinHandle<()>>,
    shutdown_signal: Option<Arc<AtomicBool>>,
    runtime_settings: Option<Arc<crate::mux_runtime::RuntimeSettings>>,
    
    // Config
    config: TrayConfig,
    
    // Exit flag
    pub should_quit: bool,
}

impl TuiApp {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        let gilrs = Gilrs::new().map_err(|e| format!("Failed to init Gilrs: {}", e))?;
        let config = TrayConfig::load();
        
        let controllers: Vec<ControllerInfo> = gilrs
            .gamepads()
            .map(|(id, gamepad)| ControllerInfo {
                id,
                name: gamepad.name().to_string(),
            })
            .collect();

        // Try to match saved controller names
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

        Ok(Self {
            controllers,
            selected_primary,
            selected_assist,
            mode: config.mode,
            hide: config.hide,
            spoof: config.spoof,
            rumble: config.rumble,
            focused_section: FocusedSection::PrimaryController,
            status: MuxStatus::Stopped,
            status_message: "Ready".to_string(),
            mux_handle: None,
            shutdown_signal: None,
            runtime_settings: None,
            config,
            should_quit: false,
        })
    }

    pub fn refresh_controllers(&mut self) -> Result<(), Box<dyn Error>> {
        let gilrs = Gilrs::new().map_err(|e| format!("Failed to init Gilrs: {}", e))?;
        
        self.controllers = gilrs
            .gamepads()
            .map(|(id, gamepad)| ControllerInfo {
                id,
                name: gamepad.name().to_string(),
            })
            .collect();

        // Validate selections still exist
        if let Some(primary_id) = self.selected_primary {
            if !self.controllers.iter().any(|c| c.id == primary_id) {
                self.selected_primary = self.controllers.first().map(|c| c.id);
            }
        }

        if let Some(assist_id) = self.selected_assist {
            if !self.controllers.iter().any(|c| c.id == assist_id) {
                self.selected_assist = self.controllers.get(1).map(|c| c.id);
            }
        }

        Ok(())
    }

    pub fn start_mux(&mut self) {
        if !self.is_valid_for_start() {
            self.status_message = "Cannot start: select two different controllers".to_string();
            return;
        }

        let primary_id = self.selected_primary.unwrap();
        let assist_id = self.selected_assist.unwrap();

        info!(
            "Starting mux: primary={:?}, assist={:?}",
            primary_id, assist_id
        );

        let config = MuxConfig {
            primary_id,
            assist_id,
            mode: self.mode.clone(),
            hide: self.hide.clone(),
            spoof: self.spoof.clone(),
            rumble: self.rumble.clone(),
        };

        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel::<()>();
        self.shutdown_signal = None; // Will be set by thread

        let runtime_settings_arc = Arc::new(AtomicBool::new(false));
        let runtime_settings_clone = Arc::clone(&runtime_settings_arc);

        let handle = thread::spawn(move || {
            match start_mux_thread(config, runtime_settings_clone) {
                Ok(mux_handle) => {
                    let _ = shutdown_rx.recv();
                    mux_handle.shutdown();
                }
                Err(e) => {
                    error!("Mux thread error: {}", e);
                }
            }
        });

        self.mux_handle = Some(handle);
        self.status = MuxStatus::Running;
        self.status_message = format!(
            "Running: {} + {}",
            self.get_primary_name(),
            self.get_assist_name()
        );

        // Save config
        if let Err(e) = self.to_config().save() {
            error!("Failed to save config: {}", e);
        }
    }

    pub fn stop_mux(&mut self) {
        if self.status == MuxStatus::Stopped {
            return;
        }

        info!("Stopping mux");

        if let Some(shutdown) = &self.shutdown_signal {
            shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
        }

        if let Some(handle) = self.mux_handle.take() {
            let _ = handle.join();
        }

        self.status = MuxStatus::Stopped;
        self.shutdown_signal = None;
        self.runtime_settings = None;
        self.status_message = "Stopped".to_string();

        info!("Mux stopped");
    }

    pub fn cycle_primary(&mut self) {
        if self.status == MuxStatus::Running || self.controllers.is_empty() {
            return;
        }

        let current_idx = self
            .selected_primary
            .and_then(|id| self.controllers.iter().position(|c| c.id == id))
            .unwrap_or(0);

        let next_idx = (current_idx + 1) % self.controllers.len();
        self.selected_primary = Some(self.controllers[next_idx].id);
    }

    pub fn cycle_assist(&mut self) {
        if self.status == MuxStatus::Running || self.controllers.is_empty() {
            return;
        }

        let current_idx = self
            .selected_assist
            .and_then(|id| self.controllers.iter().position(|c| c.id == id))
            .unwrap_or(0);

        let next_idx = (current_idx + 1) % self.controllers.len();
        self.selected_assist = Some(self.controllers[next_idx].id);
    }

    pub fn cycle_mode(&mut self) {
        let new_mode = match self.mode {
            ModeType::Priority => ModeType::Average,
            ModeType::Average => ModeType::Toggle,
            ModeType::Toggle => ModeType::Priority,
        };

        let old_mode = self.mode.clone();
        self.mode = new_mode.clone();

        // Update live if running
        if self.status == MuxStatus::Running {
            if let Some(settings) = &self.runtime_settings {
                settings.update_mode(new_mode.clone());
                self.status_message = format!("Mode changed: {:?} → {:?}", old_mode, new_mode);
            }
        }

        // Save config
        if let Err(e) = self.to_config().save() {
            error!("Failed to save config: {}", e);
        }
    }

    pub fn cycle_hide(&mut self) {
        if self.status == MuxStatus::Running {
            return; // Can't change while running
        }

        self.hide = match self.hide {
            HideType::None => HideType::Steam,
            HideType::Steam => HideType::System,
            HideType::System => HideType::None,
        };
    }

    pub fn cycle_spoof(&mut self) {
        if self.status == MuxStatus::Running {
            return; // Can't change while running
        }

        self.spoof = match self.spoof {
            SpoofTarget::None => SpoofTarget::Primary,
            SpoofTarget::Primary => SpoofTarget::Assist,
            SpoofTarget::Assist => SpoofTarget::None,
        };
    }

    pub fn cycle_rumble(&mut self) {
        let new_rumble = match self.rumble {
            RumbleTarget::Both => RumbleTarget::Primary,
            RumbleTarget::Primary => RumbleTarget::Assist,
            RumbleTarget::Assist => RumbleTarget::None,
            RumbleTarget::None => RumbleTarget::Both,
        };

        let old_rumble = self.rumble.clone();
        self.rumble = new_rumble.clone();

        // Update live if running
        if self.status == MuxStatus::Running {
            if let Some(settings) = &self.runtime_settings {
                settings.update_rumble(new_rumble.clone());
                self.status_message = format!("Rumble changed: {:?} → {:?}", old_rumble, new_rumble);
            }
        }

        // Save config
        if let Err(e) = self.to_config().save() {
            error!("Failed to save config: {}", e);
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

    fn to_config(&self) -> TrayConfig {
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

    pub async fn run(
        &mut self,
        terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        while !self.should_quit {
            terminal.draw(|f| crate::tui::ui::draw(f, self))?;
            crate::tui::event::handle_events(self)?;
        }

        // Cleanup: stop mux if running
        if self.status == MuxStatus::Running {
            self.stop_mux();
        }

        Ok(())
    }
}

fn start_mux_thread(
    config: MuxConfig,
    shutdown_signal: Arc<AtomicBool>,
) -> Result<MuxHandle, Box<dyn Error>> {
    let gilrs = Gilrs::new().map_err(|e| format!("Failed to init Gilrs: {}", e))?;
    let (mux_handle, _runtime_settings) = mux_manager::start_mux(gilrs, config)?;
    Ok(mux_handle)
}
