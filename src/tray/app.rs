use crate::demux_manager::{self, DemuxConfig, DemuxHandle};
use crate::demux_modes::DemuxModeType;
use crate::mux_manager::{self, MuxConfig, MuxHandle};
use crate::mux_modes::MuxModeType;
use crate::{DemuxRumbleTarget, HideType, RumbleTarget, SpoofTarget};
use gilrs::Gilrs;
use ksni::{Category, MenuItem, Status, ToolTip, Tray, menu};
use log::{error, info};
use notify_rust::Notification;
use parking_lot::Mutex;
use std::error::Error;
use std::sync::Arc;
use std::thread;

use super::config::TrayConfig;
use super::state::{OperationMode, OperationStatus, TrayState};

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

    fn start_operation(&mut self) {
        let state = self.state.lock();

        match state.operation_mode {
            OperationMode::Mux => {
                if !state.is_valid_for_mux_start() {
                    drop(state);
                    Self::send_notification(
                        "CtrlAssist - Cannot Start",
                        "Please select two different controllers first",
                    );
                    return;
                }
                drop(state);
                self.start_mux();
            }
            OperationMode::Demux => {
                if !state.is_valid_for_demux_start() {
                    drop(state);
                    Self::send_notification(
                        "CtrlAssist - Cannot Start",
                        "Please select a primary controller first",
                    );
                    return;
                }
                drop(state);
                self.start_demux();
            }
        }
    }

    fn start_mux(&mut self) {
        let mut state = self.state.lock();

        let primary_id = state.mux.selected_primary.unwrap();
        let assist_id = state.mux.selected_assist.unwrap();

        info!(
            "Starting mux: primary={:?}, assist={:?}",
            primary_id, assist_id
        );

        // Create notification with settings
        let notification_body = format!(
            "Primary: {}\nAssist: {}\nMode: {:?}\nHide: {:?}\nSpoof: {:?}\nRumble: {:?}",
            state.get_mux_primary_name(),
            state.get_mux_assist_name(),
            state.mux.mode,
            state.mux.hide,
            state.mux.spoof,
            state.mux.rumble
        );
        Self::send_notification("CtrlAssist - Starting Mux", &notification_body);

        // Prepare config for mux
        let config = MuxConfig {
            primary_id,
            assist_id,
            mode: state.mux.mode.clone(),
            hide: state.mux.hide.clone(),
            spoof: state.mux.spoof.clone(),
            rumble: state.mux.rumble.clone(),
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

        state.operation_handle = Some(handle);
        state.status = OperationStatus::Running;

        // Save config
        if let Err(e) = state.to_config().save() {
            error!("Failed to save config: {}", e);
        }
    }

    fn start_demux(&mut self) {
        let mut state = self.state.lock();

        let primary_id = state.demux.selected_primary.unwrap();

        info!("Starting demux: primary={:?}", primary_id);

        // Create notification with settings
        let notification_body = format!(
            "Primary: {}\nSinks: {}\nMode: {:?}\nHide: {:?}\nSpoof: {:?}\nRumble: {:?}",
            state.get_demux_primary_name(),
            state.demux.sinks,
            state.demux.mode,
            state.demux.hide,
            state.demux.spoof,
            state.demux.rumble
        );
        Self::send_notification("CtrlAssist - Starting Demux", &notification_body);

        // Prepare config for demux
        let config = DemuxConfig {
            primary_id,
            sinks: state.demux.sinks,
            mode: state.demux.mode.clone(),
            hide: state.demux.hide.clone(),
            spoof: state.demux.spoof.clone(),
            rumble: state.demux.rumble.clone(),
        };

        // Use a channel for shutdown signaling
        let (shutdown_tx, shutdown_rx) = std::sync::mpsc::channel::<()>();
        self.shutdown_tx = Some(shutdown_tx);

        let state_arc = Arc::clone(&self.state);
        let handle = thread::spawn(move || {
            match start_demux_with_state(config, state_arc) {
                Ok(demux_handle) => {
                    // Wait for shutdown signal (blocks efficiently)
                    let _ = shutdown_rx.recv();
                    // Properly shutdown demux (unblocks FF threads)
                    demux_handle.shutdown();
                }
                Err(e) => {
                    error!("Demux thread error: {}", e);
                    Self::send_notification("CtrlAssist - Error", &format!("Demux failed: {}", e));
                }
            }
        });

        state.operation_handle = Some(handle);
        state.status = OperationStatus::Running;

        // Save config
        if let Err(e) = state.to_config().save() {
            error!("Failed to save config: {}", e);
        }
    }

    fn stop_operation(&mut self) {
        let mut state = self.state.lock();

        if state.status == OperationStatus::Stopped {
            return;
        }

        info!("Stopping operation");

        // Signal shutdown via channel
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        state.virtual_device_paths.clear();

        // Wait for thread to finish
        if let Some(handle) = state.operation_handle.take() {
            drop(state); // Release lock before joining
            let _ = handle.join();
            state = self.state.lock();
        }

        state.status = OperationStatus::Stopped;
        state.shutdown_signal = None;

        // Clear runtime settings
        state.mux.runtime_settings = None;
        state.demux.runtime_settings = None;

        info!("Operation stopped");
        Self::send_notification("CtrlAssist", "Operation stopped");
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

            // Try to keep selected controllers if still present for mux
            if let Some(primary_id) = state.mux.selected_primary {
                if !state.controllers.iter().any(|c| c.id == primary_id) {
                    state.mux.selected_primary = state.controllers.first().map(|c| c.id);
                }
            } else {
                state.mux.selected_primary = state.controllers.first().map(|c| c.id);
            }

            if let Some(assist_id) = state.mux.selected_assist {
                if !state.controllers.iter().any(|c| c.id == assist_id) {
                    state.mux.selected_assist = state.controllers.get(1).map(|c| c.id);
                }
            } else {
                state.mux.selected_assist = state.controllers.get(1).map(|c| c.id);
            }

            // Try to keep selected controller if still present for demux
            if let Some(primary_id) = state.demux.selected_primary {
                if !state.controllers.iter().any(|c| c.id == primary_id) {
                    state.demux.selected_primary = state.controllers.first().map(|c| c.id);
                }
            } else {
                state.demux.selected_primary = state.controllers.first().map(|c| c.id);
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
        match (state.operation_mode, state.status) {
            (OperationMode::Mux, OperationStatus::Running) => "CtrlAssist [Mux Running]".into(),
            (OperationMode::Demux, OperationStatus::Running) => "CtrlAssist [Demux Running]".into(),
            (OperationMode::Mux, OperationStatus::Stopped) => "CtrlAssist [Mux]".into(),
            (OperationMode::Demux, OperationStatus::Stopped) => "CtrlAssist [Demux]".into(),
        }
    }

    fn icon_name(&self) -> String {
        let state = self.state.lock();
        match state.status {
            OperationStatus::Running => "input-gaming".into(),
            OperationStatus::Stopped => "input-gaming-symbolic".into(),
        }
    }

    fn status(&self) -> Status {
        let state = self.state.lock();
        match state.status {
            OperationStatus::Running => Status::Active,
            OperationStatus::Stopped => Status::Passive,
        }
    }

    fn tool_tip(&self) -> ToolTip {
        let state = self.state.lock();
        let description = match (state.operation_mode, state.status) {
            (OperationMode::Mux, OperationStatus::Running) => format!(
                "Muxing: {} + {}",
                state.get_mux_primary_name(),
                state.get_mux_assist_name()
            ),
            (OperationMode::Demux, OperationStatus::Running) => format!(
                "Demuxing: {} to {} sinks",
                state.get_demux_primary_name(),
                state.demux.sinks
            ),
            (_, OperationStatus::Stopped) => "Not running".to_string(),
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
        let is_running = state.status == OperationStatus::Running;

        let mut menu_items = vec![
            // Operation Mode Selection
            menu::SubMenu {
                label: format!("Operation: {:?}", state.operation_mode),
                icon_name: "swap-panels".into(),
                enabled: !is_running,
                submenu: vec![
                    create_operation_mode_item(OperationMode::Mux, &state, is_running),
                    create_operation_mode_item(OperationMode::Demux, &state, is_running),
                ],
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
        ];

        // Add mode-specific configuration
        match state.operation_mode {
            OperationMode::Mux => {
                menu_items.extend(create_mux_menu(&state, is_running));
            }
            OperationMode::Demux => {
                menu_items.extend(create_demux_menu(&state, is_running));
            }
        }

        menu_items.extend(vec![
            MenuItem::Separator,
            // Start/Stop
            menu::StandardItem {
                label: format!(
                    "Start {}",
                    match state.operation_mode {
                        OperationMode::Mux => "Mux",
                        OperationMode::Demux => "Demux",
                    }
                ),
                icon_name: "media-playback-start".into(),
                enabled: !is_running
                    && match state.operation_mode {
                        OperationMode::Mux => state.is_valid_for_mux_start(),
                        OperationMode::Demux => state.is_valid_for_demux_start(),
                    },
                activate: Box::new(|this: &mut Self| {
                    this.start_operation();
                }),
                ..Default::default()
            }
            .into(),
            menu::StandardItem {
                label: "Stop".into(),
                icon_name: "media-playback-stop".into(),
                enabled: is_running,
                activate: Box::new(|this: &mut Self| {
                    this.stop_operation();
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
                    this.stop_operation();
                    std::process::exit(0);
                }),
                ..Default::default()
            }
            .into(),
        ]);

        menu_items
    }
}

// Helper function to create operation mode selection item
fn create_operation_mode_item(
    mode: OperationMode,
    state: &parking_lot::lock_api::MutexGuard<parking_lot::RawMutex, TrayState>,
    is_running: bool,
) -> MenuItem<CtrlAssistTray> {
    let is_selected = state.operation_mode == mode;

    menu::CheckmarkItem {
        label: format!("{:?}", mode),
        checked: is_selected,
        enabled: !is_running,
        activate: Box::new(move |this: &mut CtrlAssistTray| {
            let mut state = this.state.lock();
            state.operation_mode = mode;

            // Save config
            if let Err(e) = state.to_config().save() {
                error!("Failed to save config: {}", e);
            }
        }),
        ..Default::default()
    }
    .into()
}

// Create mux-specific menu items
fn create_mux_menu(
    state: &parking_lot::lock_api::MutexGuard<parking_lot::RawMutex, TrayState>,
    is_running: bool,
) -> Vec<MenuItem<CtrlAssistTray>> {
    vec![
        // Refresh controllers (colocal to primary selection)
        menu::StandardItem {
            label: "Refresh Controllers".into(),
            icon_name: "view-refresh".into(),
            enabled: !is_running,
            activate: Box::new(|this: &mut CtrlAssistTray| {
                this.refresh_controllers();
            }),
            ..Default::default()
        }
        .into(),
        // Controller Selection
        menu::SubMenu {
            label: format!(
                "Primary: ({}) {}",
                state
                    .mux
                    .selected_primary
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "#".to_string()),
                truncate_name(&state.get_mux_primary_name())
            ),
            icon_name: "input-gaming".into(),
            enabled: !is_running,
            submenu: state
                .controllers
                .iter()
                .map(|controller| {
                    let controller_id = controller.id;
                    let is_selected = state.mux.selected_primary == Some(controller_id);
                    menu::CheckmarkItem {
                        label: format!("({}) {}", controller_id, controller.name),
                        checked: is_selected,
                        enabled: !is_running,
                        activate: Box::new(move |this: &mut CtrlAssistTray| {
                            let mut state = this.state.lock();
                            state.mux.selected_primary = Some(controller_id);
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
            label: format!(
                "Assist: ({}) {}",
                state
                    .mux
                    .selected_assist
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "#".to_string()),
                truncate_name(&state.get_mux_assist_name())
            ),
            icon_name: "input-gaming".into(),
            enabled: !is_running,
            submenu: state
                .controllers
                .iter()
                .map(|controller| {
                    let controller_id = controller.id;
                    let is_selected = state.mux.selected_assist == Some(controller_id);
                    menu::CheckmarkItem {
                        label: format!("({}) {}", controller_id, controller.name),
                        checked: is_selected,
                        enabled: !is_running,
                        activate: Box::new(move |this: &mut CtrlAssistTray| {
                            let mut state = this.state.lock();
                            state.mux.selected_assist = Some(controller_id);
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
            label: format!("Mode: {:?}", state.mux.mode),
            icon_name: "media-playlist-shuffle".into(),
            enabled: true, // Dynamically configurable while running
            submenu: vec![
                create_mux_mode_item(MuxModeType::Priority, state, true),
                create_mux_mode_item(MuxModeType::Average, state, true),
                create_mux_mode_item(MuxModeType::Toggle, state, true),
            ],
            ..Default::default()
        }
        .into(),
        // Hide Strategy
        menu::SubMenu {
            label: format!("Hide: {:?}", state.mux.hide),
            icon_name: "view-visible".into(),
            enabled: !is_running,
            submenu: vec![
                create_mux_hide_item(HideType::None, state, is_running),
                create_mux_hide_item(HideType::Steam, state, is_running),
                create_mux_hide_item(HideType::System, state, is_running),
            ],
            ..Default::default()
        }
        .into(),
        // Spoof Target
        menu::SubMenu {
            label: format!("Spoof: {:?}", state.mux.spoof),
            icon_name: "edit-copy".into(),
            enabled: !is_running,
            submenu: vec![
                create_mux_spoof_item(SpoofTarget::None, state, is_running),
                create_mux_spoof_item(SpoofTarget::Primary, state, is_running),
                create_mux_spoof_item(SpoofTarget::Assist, state, is_running),
            ],
            ..Default::default()
        }
        .into(),
        // Rumble Target
        menu::SubMenu {
            label: format!("Rumble: {:?}", state.mux.rumble),
            icon_name: "notification-active".into(),
            enabled: true, // Dynamically configurable while running
            submenu: vec![
                create_mux_rumble_item(RumbleTarget::Both, state, true),
                create_mux_rumble_item(RumbleTarget::Primary, state, true),
                create_mux_rumble_item(RumbleTarget::Assist, state, true),
                create_mux_rumble_item(RumbleTarget::None, state, true),
            ],
            ..Default::default()
        }
        .into(),
    ]
}

// Create demux-specific menu items
fn create_demux_menu(
    state: &parking_lot::lock_api::MutexGuard<parking_lot::RawMutex, TrayState>,
    is_running: bool,
) -> Vec<MenuItem<CtrlAssistTray>> {
    vec![
        // Refresh controllers (colocal to primary selection)
        menu::StandardItem {
            label: "Refresh Controllers".into(),
            icon_name: "view-refresh".into(),
            enabled: !is_running,
            activate: Box::new(|this: &mut CtrlAssistTray| {
                this.refresh_controllers();
            }),
            ..Default::default()
        }
        .into(),
        // Controller Selection
        menu::SubMenu {
            label: format!(
                "Primary: ({}) {}",
                state
                    .demux
                    .selected_primary
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| "#".to_string()),
                truncate_name(&state.get_demux_primary_name())
            ),
            icon_name: "input-gaming".into(),
            enabled: !is_running,
            submenu: state
                .controllers
                .iter()
                .map(|controller| {
                    let controller_id = controller.id;
                    let is_selected = state.demux.selected_primary == Some(controller_id);
                    menu::CheckmarkItem {
                        label: format!("({}) {}", controller_id, controller.name),
                        checked: is_selected,
                        enabled: !is_running,
                        activate: Box::new(move |this: &mut CtrlAssistTray| {
                            let mut state = this.state.lock();
                            state.demux.selected_primary = Some(controller_id);
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
        // Sinks Management
        menu::SubMenu {
            label: format!("Sinks: {}", state.demux.sinks),
            icon_name: "list-add".into(),
            enabled: !is_running,
            submenu: vec![
                menu::StandardItem {
                    label: "Increment (+1)".into(),
                    icon_name: "list-add".into(),
                    enabled: !is_running,
                    activate: Box::new(|this: &mut CtrlAssistTray| {
                        let mut state = this.state.lock();
                        state.demux.sinks += 1;
                        if let Err(e) = state.to_config().save() {
                            error!("Failed to save config: {}", e);
                        }
                    }),
                    ..Default::default()
                }
                .into(),
                menu::StandardItem {
                    label: "Decrement (-1)".into(),
                    icon_name: "list-remove".into(),
                    enabled: !is_running && state.demux.sinks > 1,
                    activate: Box::new(|this: &mut CtrlAssistTray| {
                        let mut state = this.state.lock();
                        if state.demux.sinks > 1 {
                            state.demux.sinks -= 1;
                        }
                        if let Err(e) = state.to_config().save() {
                            error!("Failed to save config: {}", e);
                        }
                    }),
                    ..Default::default()
                }
                .into(),
                menu::StandardItem {
                    label: "Reset (to 2)".into(),
                    icon_name: "view-refresh".into(),
                    enabled: !is_running,
                    activate: Box::new(|this: &mut CtrlAssistTray| {
                        let mut state = this.state.lock();
                        state.demux.sinks = 2;
                        if let Err(e) = state.to_config().save() {
                            error!("Failed to save config: {}", e);
                        }
                    }),
                    ..Default::default()
                }
                .into(),
            ],
            ..Default::default()
        }
        .into(),
        // Demux Mode
        menu::SubMenu {
            label: format!("Mode: {:?}", state.demux.mode),
            icon_name: "media-playlist-shuffle".into(),
            enabled: true, // Dynamically configurable while running
            submenu: vec![
                create_demux_mode_item(DemuxModeType::Unicast, state, true),
                create_demux_mode_item(DemuxModeType::Multicast, state, true),
            ],
            ..Default::default()
        }
        .into(),
        // Hide Strategy
        menu::SubMenu {
            label: format!("Hide: {:?}", state.demux.hide),
            icon_name: "view-visible".into(),
            enabled: !is_running,
            submenu: vec![
                create_demux_hide_item(HideType::None, state, is_running),
                create_demux_hide_item(HideType::Steam, state, is_running),
                create_demux_hide_item(HideType::System, state, is_running),
            ],
            ..Default::default()
        }
        .into(),
        // Spoof Target
        menu::SubMenu {
            label: format!("Spoof: {:?}", state.demux.spoof),
            icon_name: "edit-copy".into(),
            enabled: !is_running,
            submenu: vec![
                create_demux_spoof_item(SpoofTarget::None, state, is_running),
                create_demux_spoof_item(SpoofTarget::Primary, state, is_running),
            ],
            ..Default::default()
        }
        .into(),
        // Rumble Target
        menu::SubMenu {
            label: format!("Rumble: {:?}", state.demux.rumble),
            icon_name: "notification-active".into(),
            enabled: true, // Dynamically configurable while running
            submenu: vec![
                create_demux_rumble_item(DemuxRumbleTarget::Active, state, true),
                create_demux_rumble_item(DemuxRumbleTarget::None, state, true),
            ],
            ..Default::default()
        }
        .into(),
    ]
}

// Helper functions for mux menu items
fn create_mux_mode_item(
    mode: MuxModeType,
    state: &parking_lot::lock_api::MutexGuard<parking_lot::RawMutex, TrayState>,
    enabled: bool,
) -> MenuItem<CtrlAssistTray> {
    let is_selected = state.mux.mode == mode;

    menu::CheckmarkItem {
        label: format!("{:?}", mode),
        checked: is_selected,
        enabled,
        activate: Box::new(move |this: &mut CtrlAssistTray| {
            let mut state = this.state.lock();
            let old_mode = state.mux.mode.clone();
            state.mux.mode = mode.clone();

            if old_mode != mode {
                // If running, update live
                if state.status == OperationStatus::Running
                    && state.operation_mode == OperationMode::Mux
                    && let Some(runtime_settings) = &state.mux.runtime_settings
                {
                    runtime_settings.update_mode(mode.clone());
                    CtrlAssistTray::send_notification(
                        "CtrlAssist - Mode Changed",
                        &format!("Mux mode changed from {:?} to {:?}", old_mode, mode),
                    );
                }

                // Save config
                if let Err(e) = state.to_config().save() {
                    error!("Failed to save config: {}", e);
                }
            }
        }),
        ..Default::default()
    }
    .into()
}

fn create_mux_hide_item(
    hide: HideType,
    state: &parking_lot::lock_api::MutexGuard<parking_lot::RawMutex, TrayState>,
    is_running: bool,
) -> MenuItem<CtrlAssistTray> {
    let is_selected = matches!(
        (&state.mux.hide, &hide),
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
            state.mux.hide = hide.clone();
        }),
        ..Default::default()
    }
    .into()
}

fn create_mux_spoof_item(
    spoof: SpoofTarget,
    state: &parking_lot::lock_api::MutexGuard<parking_lot::RawMutex, TrayState>,
    is_running: bool,
) -> MenuItem<CtrlAssistTray> {
    let is_selected = matches!(
        (&state.mux.spoof, &spoof),
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
            state.mux.spoof = spoof.clone();
        }),
        ..Default::default()
    }
    .into()
}

fn create_mux_rumble_item(
    rumble: RumbleTarget,
    state: &parking_lot::lock_api::MutexGuard<parking_lot::RawMutex, TrayState>,
    enabled: bool,
) -> MenuItem<CtrlAssistTray> {
    let is_selected = matches!(
        (&state.mux.rumble, &rumble),
        (RumbleTarget::Both, RumbleTarget::Both)
            | (RumbleTarget::Primary, RumbleTarget::Primary)
            | (RumbleTarget::Assist, RumbleTarget::Assist)
            | (RumbleTarget::None, RumbleTarget::None)
    );

    menu::CheckmarkItem {
        label: format!("{:?}", rumble),
        checked: is_selected,
        enabled,
        activate: Box::new(move |this: &mut CtrlAssistTray| {
            let mut state = this.state.lock();
            let old_rumble = state.mux.rumble.clone();
            state.mux.rumble = rumble.clone();

            if old_rumble != rumble {
                // If running, update live
                if state.status == OperationStatus::Running
                    && state.operation_mode == OperationMode::Mux
                    && let Some(runtime_settings) = &state.mux.runtime_settings
                {
                    runtime_settings.update_rumble(rumble.clone());
                    CtrlAssistTray::send_notification(
                        "CtrlAssist - Rumble Changed",
                        &format!(
                            "Rumble target changed from {:?} to {:?}",
                            old_rumble, rumble
                        ),
                    );
                }

                // Save config
                if let Err(e) = state.to_config().save() {
                    error!("Failed to save config: {}", e);
                }
            }
        }),
        ..Default::default()
    }
    .into()
}

// Helper functions for demux menu items
fn create_demux_mode_item(
    mode: DemuxModeType,
    state: &parking_lot::lock_api::MutexGuard<parking_lot::RawMutex, TrayState>,
    enabled: bool,
) -> MenuItem<CtrlAssistTray> {
    let is_selected = state.demux.mode == mode;

    menu::CheckmarkItem {
        label: format!("{:?}", mode),
        checked: is_selected,
        enabled,
        activate: Box::new(move |this: &mut CtrlAssistTray| {
            let mut state = this.state.lock();
            let old_mode = state.demux.mode.clone();
            state.demux.mode = mode.clone();

            if old_mode != mode {
                // If running, update live
                if state.status == OperationStatus::Running
                    && state.operation_mode == OperationMode::Demux
                    && let Some(runtime_settings) = &state.demux.runtime_settings
                {
                    runtime_settings.update_mode(mode.clone());
                    CtrlAssistTray::send_notification(
                        "CtrlAssist - Mode Changed",
                        &format!("Demux mode changed from {:?} to {:?}", old_mode, mode),
                    );
                }

                // Save config
                if let Err(e) = state.to_config().save() {
                    error!("Failed to save config: {}", e);
                }
            }
        }),
        ..Default::default()
    }
    .into()
}

fn create_demux_hide_item(
    hide: HideType,
    state: &parking_lot::lock_api::MutexGuard<parking_lot::RawMutex, TrayState>,
    is_running: bool,
) -> MenuItem<CtrlAssistTray> {
    let is_selected = matches!(
        (&state.demux.hide, &hide),
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
            state.demux.hide = hide.clone();
        }),
        ..Default::default()
    }
    .into()
}

fn create_demux_spoof_item(
    spoof: SpoofTarget,
    state: &parking_lot::lock_api::MutexGuard<parking_lot::RawMutex, TrayState>,
    is_running: bool,
) -> MenuItem<CtrlAssistTray> {
    let is_selected = matches!(
        (&state.demux.spoof, &spoof),
        (SpoofTarget::None, SpoofTarget::None) | (SpoofTarget::Primary, SpoofTarget::Primary)
    );

    menu::CheckmarkItem {
        label: format!("{:?}", spoof),
        checked: is_selected,
        enabled: !is_running,
        activate: Box::new(move |this: &mut CtrlAssistTray| {
            let mut state = this.state.lock();
            state.demux.spoof = spoof.clone();
        }),
        ..Default::default()
    }
    .into()
}

fn create_demux_rumble_item(
    rumble: DemuxRumbleTarget,
    state: &parking_lot::lock_api::MutexGuard<parking_lot::RawMutex, TrayState>,
    enabled: bool,
) -> MenuItem<CtrlAssistTray> {
    let is_selected = matches!(
        (&state.demux.rumble, &rumble),
        (DemuxRumbleTarget::Active, DemuxRumbleTarget::Active)
            | (DemuxRumbleTarget::None, DemuxRumbleTarget::None)
    );

    menu::CheckmarkItem {
        label: format!("{:?}", rumble),
        checked: is_selected,
        enabled,
        activate: Box::new(move |this: &mut CtrlAssistTray| {
            let mut state = this.state.lock();
            let old_rumble = state.demux.rumble.clone();
            state.demux.rumble = rumble.clone();

            if old_rumble != rumble {
                // If running, update live
                if state.status == OperationStatus::Running
                    && state.operation_mode == OperationMode::Demux
                    && let Some(runtime_settings) = &state.demux.runtime_settings
                {
                    runtime_settings.update_rumble(rumble.clone());
                    CtrlAssistTray::send_notification(
                        "CtrlAssist - Rumble Changed",
                        &format!(
                            "Rumble target changed from {:?} to {:?}",
                            old_rumble, rumble
                        ),
                    );
                }

                // Save config
                if let Err(e) = state.to_config().save() {
                    error!("Failed to save config: {}", e);
                }
            }
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
    let (mux_handle, runtime_settings) = mux_manager::start_mux(gilrs, config)?;

    // Store handle reference in state
    {
        let mut state = state_arc.lock();
        state.virtual_device_paths = vec![mux_handle.virtual_device_path.clone()];
        state.shutdown_signal = Some(Arc::clone(&mux_handle.shutdown));
        state.mux.runtime_settings = Some(runtime_settings);
    }

    Ok(mux_handle)
}

// Helper function to start demux and update state
fn start_demux_with_state(
    config: DemuxConfig,
    state_arc: Arc<Mutex<TrayState>>,
) -> Result<DemuxHandle, Box<dyn Error>> {
    let gilrs = Gilrs::new().map_err(|e| format!("Failed to init Gilrs: {}", e))?;
    let (demux_handle, runtime_settings) = demux_manager::start_demux(gilrs, config)?;

    // Store handle reference in state
    {
        let mut state = state_arc.lock();
        state.virtual_device_paths = demux_handle.virtual_device_paths.clone();
        state.shutdown_signal = Some(Arc::clone(&demux_handle.shutdown));
        state.demux.runtime_settings = Some(runtime_settings);
    }

    Ok(demux_handle)
}

// Helper to truncate controller name for SubMenu label
fn truncate_name(name: &str) -> String {
    const MAX_LEN: usize = 17;
    const ELLIPSIS: &str = "...";
    if name.len() > MAX_LEN {
        let cutoff = MAX_LEN - ELLIPSIS.len();
        format!("{}{}", &name[..cutoff], ELLIPSIS)
    } else {
        name.to_string()
    }
}
