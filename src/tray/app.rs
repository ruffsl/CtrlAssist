use crate::mux_manager::{self, MuxConfig, MuxHandle};
use crate::mux_modes::ModeType;
use crate::{HideType, RumbleTarget, SpoofTarget};
use gilrs::Gilrs;
use ksni::{Category, MenuItem, Status, ToolTip, Tray, menu};
use log::{error, info};
use notify_rust::Notification;
use parking_lot::Mutex;
use std::error::Error;
use std::sync::Arc;
use std::thread;

use super::config::TrayConfig;
use super::state::{MuxStatus, TrayState};

pub struct CtrlAssistTray {
    state: Arc<Mutex<TrayState>>,
    // Store shutdown sender for signaling
    shutdown_tx: Option<std::sync::mpsc::Sender<()>>,
}

impl CtrlAssistTray {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        let gilrs = Gilrs::new().map_err(|e| format!("Failed to init Gilrs: {}", e))?;
        let config = TrayConfig::load();
        let state = TrayState::new(&gilrs, config);

        Ok(Self {
            state: Arc::new(Mutex::new(state)),
            shutdown_tx: None,
        })
    }

    fn send_notification(summary: &str, body: &str) {
        let summary = summary.to_string();
        let body = body.to_string();
        tokio::task::spawn_blocking(move || {
            if let Err(e) = Notification::new()
                .summary(&summary)
                .body(&body)
                .appname("CtrlAssist")
                .show()
            {
                error!("Failed to send notification: {}", e);
            }
        });
    }

    fn start_mux(&mut self) {
        let mut state = self.state.lock();

        if !state.is_valid_for_start() {
            Self::send_notification(
                "CtrlAssist - Cannot Start",
                "Please select two different controllers first",
            );
            return;
        }

        let primary_id = state.selected_primary.unwrap();
        let assist_id = state.selected_assist.unwrap();

        info!(
            "Starting mux: primary={:?}, assist={:?}",
            primary_id, assist_id
        );

        // Create notification with settings
        let notification_body = format!(
            "Primary: {}\nAssist: {}\nMode: {:?}\nHide: {:?}\nSpoof: {:?}\nRumble: {:?}",
            state.get_primary_name(),
            state.get_assist_name(),
            state.mode,
            state.hide,
            state.spoof,
            state.rumble
        );
        Self::send_notification("CtrlAssist - Starting", &notification_body);

        // Prepare config for mux
        let config = MuxConfig {
            primary_id,
            assist_id,
            mode: state.mode.clone(),
            hide: state.hide.clone(),
            spoof: state.spoof.clone(),
            rumble: state.rumble.clone(),
        };

        // Use a channel for shutdown signaling
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel::<()>();
        self.shutdown_tx = Some(shutdown_tx);

        let state_arc = Arc::clone(&self.state);
        let handle = thread::spawn(move || {
            match start_mux_with_state(config, state_arc) {
                Ok(mux_handle) => {
                    // Wait for shutdown signal (blocks efficiently)
                    let _ = shutdown_rx.recv();
                    // Properly shutdown mux (unblocks FF thread)
                    mux_handle.shutdown();
                }
                Err(e) => {
                    error!("Mux thread error: {}", e);
                    Self::send_notification("CtrlAssist - Error", &format!("Mux failed: {}", e));
                }
            }
        });

        state.mux_handle = Some(handle);
        state.status = MuxStatus::Running;

        // Save config
        if let Err(e) = state.to_config().save() {
            error!("Failed to save config: {}", e);
        }
    }

    fn stop_mux(&mut self) {
        let mut state = self.state.lock();

        if state.status == MuxStatus::Stopped {
            return;
        }

        info!("Stopping mux");

        // Signal shutdown via channel
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        state.virtual_device_path = None;

        // Wait for thread to finish
        if let Some(handle) = state.mux_handle.take() {
            drop(state); // Release lock before joining
            let _ = handle.join();
            state = self.state.lock();
        }

        state.status = MuxStatus::Stopped;
        state.shutdown_signal = None;

        info!("Mux stopped");
        Self::send_notification("CtrlAssist", "Mux stopped");
    }

    fn refresh_controllers(&self) {
        let mut state = self.state.lock();
        if let Ok(gilrs) = Gilrs::new() {
            let controllers: Vec<_> = gilrs
                .gamepads()
                .map(|(id, gamepad)| super::state::ControllerInfo {
                    id,
                    name: gamepad.name().to_string(),
                })
                .collect();
            state.controllers = controllers;

            // Try to keep selected controllers if still present
            if let Some(primary_id) = state.selected_primary {
                if !state.controllers.iter().any(|c| c.id == primary_id) {
                    state.selected_primary = state.controllers.first().map(|c| c.id);
                }
            } else {
                state.selected_primary = state.controllers.first().map(|c| c.id);
            }

            if let Some(assist_id) = state.selected_assist {
                if !state.controllers.iter().any(|c| c.id == assist_id) {
                    state.selected_assist = state.controllers.get(1).map(|c| c.id);
                }
            } else {
                state.selected_assist = state.controllers.get(1).map(|c| c.id);
            }
        }
    }
}

impl Tray for CtrlAssistTray {
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "ctrlassist".into()
    }

    fn category(&self) -> Category {
        Category::ApplicationStatus
    }

    fn title(&self) -> String {
        let state = self.state.lock();
        match state.status {
            MuxStatus::Running => "CtrlAssist [Running]".into(),
            MuxStatus::Stopped => "CtrlAssist [Stopped]".into(),
        }
    }

    fn icon_name(&self) -> String {
        let state = self.state.lock();
        match state.status {
            MuxStatus::Running => "input-gaming".into(),
            MuxStatus::Stopped => "input-gaming-symbolic".into(),
        }
    }

    fn status(&self) -> Status {
        let state = self.state.lock();
        match state.status {
            MuxStatus::Running => Status::Active,
            MuxStatus::Stopped => Status::Passive,
        }
    }

    fn tool_tip(&self) -> ToolTip {
        let state = self.state.lock();
        let description = match state.status {
            MuxStatus::Running => format!(
                "Muxing: {} + {}",
                state.get_primary_name(),
                state.get_assist_name()
            ),
            MuxStatus::Stopped => "Not running".to_string(),
        };

        ToolTip {
            icon_name: "input-gaming".into(),
            icon_pixmap: vec![],
            title: "CtrlAssist".into(),
            description,
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        self.refresh_controllers();
        let state = self.state.lock();
        let is_running = state.status == MuxStatus::Running;

        vec![
            // Refresh controllers
            menu::StandardItem {
                label: "Refresh Controllers".into(),
                icon_name: "view-refresh".into(),
                enabled: !is_running,
                activate: Box::new(|this: &mut Self| {
                    this.refresh_controllers();
                }),
                ..Default::default()
            }
            .into(),
            // Controller Selection
            menu::SubMenu {
                label: format!("Primary: {}", state.get_primary_name()),
                icon_name: "input-gaming".into(),
                enabled: !is_running,
                submenu: state
                    .controllers
                    .iter()
                    .map(|controller| {
                        let controller_id = controller.id;
                        let is_selected = state.selected_primary == Some(controller_id);
                        menu::CheckmarkItem {
                            label: controller.name.clone(),
                            checked: is_selected,
                            enabled: !is_running,
                            activate: Box::new(move |this: &mut Self| {
                                let mut state = this.state.lock();
                                state.selected_primary = Some(controller_id);
                            }),
                            ..Default::default()
                        }
                        .into()
                    })
                    .collect(),
                ..Default::default()
            }
            .into(),
            menu::SubMenu {
                label: format!("Assist: {}", state.get_assist_name()),
                icon_name: "input-gaming".into(),
                enabled: !is_running,
                submenu: state
                    .controllers
                    .iter()
                    .map(|controller| {
                        let controller_id = controller.id;
                        let is_selected = state.selected_assist == Some(controller_id);
                        menu::CheckmarkItem {
                            label: controller.name.clone(),
                            checked: is_selected,
                            enabled: !is_running,
                            activate: Box::new(move |this: &mut Self| {
                                let mut state = this.state.lock();
                                state.selected_assist = Some(controller_id);
                            }),
                            ..Default::default()
                        }
                        .into()
                    })
                    .collect(),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            // Mux Mode
            menu::SubMenu {
                label: format!("Mode: {:?}", state.mode),
                icon_name: "media-playlist-shuffle".into(),
                enabled: !is_running,
                submenu: vec![
                    create_mode_item(ModeType::Priority, &state, is_running),
                    create_mode_item(ModeType::Average, &state, is_running),
                    create_mode_item(ModeType::Toggle, &state, is_running),
                ],
                ..Default::default()
            }
            .into(),
            // Hide Strategy
            menu::SubMenu {
                label: format!("Hide: {:?}", state.hide),
                icon_name: "view-visible".into(),
                enabled: !is_running,
                submenu: vec![
                    create_hide_item(HideType::None, &state, is_running),
                    create_hide_item(HideType::Steam, &state, is_running),
                    create_hide_item(HideType::System, &state, is_running),
                ],
                ..Default::default()
            }
            .into(),
            // Spoof Target
            menu::SubMenu {
                label: format!("Spoof: {:?}", state.spoof),
                icon_name: "edit-copy".into(),
                enabled: !is_running,
                submenu: vec![
                    create_spoof_item(SpoofTarget::None, &state, is_running),
                    create_spoof_item(SpoofTarget::Primary, &state, is_running),
                    create_spoof_item(SpoofTarget::Assist, &state, is_running),
                ],
                ..Default::default()
            }
            .into(),
            // Rumble Target
            menu::SubMenu {
                label: format!("Rumble: {:?}", state.rumble),
                icon_name: "notification-active".into(),
                enabled: !is_running,
                submenu: vec![
                    create_rumble_item(RumbleTarget::Both, &state, is_running),
                    create_rumble_item(RumbleTarget::Primary, &state, is_running),
                    create_rumble_item(RumbleTarget::Assist, &state, is_running),
                    create_rumble_item(RumbleTarget::None, &state, is_running),
                ],
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            // Start/Stop
            menu::StandardItem {
                label: "Start Mux".into(),
                icon_name: "media-playback-start".into(),
                enabled: !is_running && state.is_valid_for_start(),
                activate: Box::new(|this: &mut Self| {
                    this.start_mux();
                }),
                ..Default::default()
            }
            .into(),
            menu::StandardItem {
                label: "Stop Mux".into(),
                icon_name: "media-playback-stop".into(),
                enabled: is_running,
                activate: Box::new(|this: &mut Self| {
                    this.stop_mux();
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            // Exit
            menu::StandardItem {
                label: "Exit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|this: &mut Self| {
                    this.stop_mux();
                    std::process::exit(0);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}

// Helper functions for menu items
fn create_mode_item(
    mode: ModeType,
    state: &parking_lot::lock_api::MutexGuard<parking_lot::RawMutex, TrayState>,
    is_running: bool,
) -> MenuItem<CtrlAssistTray> {
    let is_selected = matches!(
        (&state.mode, &mode),
        (ModeType::Priority, ModeType::Priority)
            | (ModeType::Average, ModeType::Average)
            | (ModeType::Toggle, ModeType::Toggle)
    );

    menu::CheckmarkItem {
        label: format!("{:?}", mode),
        checked: is_selected,
        enabled: !is_running,
        activate: Box::new(move |this: &mut CtrlAssistTray| {
            let mut state = this.state.lock();
            state.mode = mode.clone();
        }),
        ..Default::default()
    }
    .into()
}

fn create_hide_item(
    hide: HideType,
    state: &parking_lot::lock_api::MutexGuard<parking_lot::RawMutex, TrayState>,
    is_running: bool,
) -> MenuItem<CtrlAssistTray> {
    let is_selected = matches!(
        (&state.hide, &hide),
        (HideType::None, HideType::None)
            | (HideType::Steam, HideType::Steam)
            | (HideType::System, HideType::System)
    );

    menu::CheckmarkItem {
        label: format!("{:?}", hide),
        checked: is_selected,
        enabled: !is_running,
        activate: Box::new(move |this: &mut CtrlAssistTray| {
            let mut state = this.state.lock();
            state.hide = hide.clone();
        }),
        ..Default::default()
    }
    .into()
}

fn create_spoof_item(
    spoof: SpoofTarget,
    state: &parking_lot::lock_api::MutexGuard<parking_lot::RawMutex, TrayState>,
    is_running: bool,
) -> MenuItem<CtrlAssistTray> {
    let is_selected = matches!(
        (&state.spoof, &spoof),
        (SpoofTarget::None, SpoofTarget::None)
            | (SpoofTarget::Primary, SpoofTarget::Primary)
            | (SpoofTarget::Assist, SpoofTarget::Assist)
    );

    menu::CheckmarkItem {
        label: format!("{:?}", spoof),
        checked: is_selected,
        enabled: !is_running,
        activate: Box::new(move |this: &mut CtrlAssistTray| {
            let mut state = this.state.lock();
            state.spoof = spoof.clone();
        }),
        ..Default::default()
    }
    .into()
}

fn create_rumble_item(
    rumble: RumbleTarget,
    state: &parking_lot::lock_api::MutexGuard<parking_lot::RawMutex, TrayState>,
    is_running: bool,
) -> MenuItem<CtrlAssistTray> {
    let is_selected = matches!(
        (&state.rumble, &rumble),
        (RumbleTarget::Both, RumbleTarget::Both)
            | (RumbleTarget::Primary, RumbleTarget::Primary)
            | (RumbleTarget::Assist, RumbleTarget::Assist)
            | (RumbleTarget::None, RumbleTarget::None)
    );

    menu::CheckmarkItem {
        label: format!("{:?}", rumble),
        checked: is_selected,
        enabled: !is_running,
        activate: Box::new(move |this: &mut CtrlAssistTray| {
            let mut state = this.state.lock();
            state.rumble = rumble.clone();
        }),
        ..Default::default()
    }
    .into()
}

// Helper function to start mux and update state
fn start_mux_with_state(
    config: MuxConfig,
    state_arc: Arc<Mutex<TrayState>>,
) -> Result<MuxHandle, Box<dyn Error>> {
    let gilrs = Gilrs::new().map_err(|e| format!("Failed to init Gilrs: {}", e))?;
    let mux_handle = mux_manager::start_mux(gilrs, config)?;

    // Update state with handles
    {
        let mut state = state_arc.lock();
        state.virtual_device_path = Some(mux_handle.virtual_device_path.clone());
        state.shutdown_signal = Some(Arc::clone(&mux_handle.shutdown));
    }

    Ok(mux_handle)
}
