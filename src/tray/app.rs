use crate::demux_manager::{self, DemuxConfig, DemuxHandle};
use crate::demux_modes::DemuxModeType;
use crate::mux_manager::{self, MuxConfig, MuxHandle};
use crate::mux_modes::MuxModeType;
use crate::{DemuxRumbleTarget, HideType, RumbleTarget, SpoofTarget};
use gilrs::GamepadId;
use ksni::{Category, MenuItem, Status, ToolTip, Tray, menu};
use log::{error, info};
use notify_rust::Notification;
use parking_lot::Mutex;
use std::error::Error;
use std::sync::{Arc, mpsc};
use std::thread;

use super::config::TrayConfig;
use super::state::{OperationMode, OperationStatus, TrayState};

/// Helper macro for simple standard menu items
macro_rules! simple_item {
    (
        label: $label:expr,
        icon: $icon:expr,
        enabled: $enabled:expr,
        action: |$tray:ident| $action_expr:expr
    ) => {
        menu::StandardItem {
            label: $label.into(),
            icon_name: $icon.into(),
            enabled: $enabled,
            activate: Box::new(|$tray: &mut CtrlAssistTray| $action_expr),
            ..Default::default()
        }
        .into()
    };
}

/// Macro for Controller Selection Submenus
macro_rules! controller_menu {
    (
        label: $label_prefix:literal,
        current_id: $current_id:expr,
        name: $name:expr,
        controllers: $controllers:expr,
        enabled: $enabled:expr,
        assign: |$state:ident, $id:ident| $block:block
    ) => {{
        let display_name = truncate_name(&$name);
        let id_str = $current_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "#".to_string());

        menu::SubMenu {
            label: format!("{}: ({}) {}", $label_prefix, id_str, display_name),
            icon_name: "input-gaming".into(),
            enabled: $enabled,
            submenu: if $controllers.is_empty() {
                vec![]
            } else {
                vec![
                    menu::RadioGroup {
                        selected: $current_id
                            .and_then(|id| $controllers.iter().position(|c| c.id == id))
                            .unwrap_or(0),
                        select: Box::new(|this: &mut CtrlAssistTray, index| {
                            let mut $state = this.state.lock();
                            if let Some(c) = $state.controllers.get(index) {
                                let $id = c.id;
                                $block
                            }
                        }),
                        options: $controllers
                            .iter()
                            .map(|c| menu::RadioItem {
                                label: format!("({}) {}", c.id, c.name),
                                enabled: $enabled,
                                ..Default::default()
                            })
                            .collect(),
                    }
                    .into(),
                ]
            },
            ..Default::default()
        }
        .into()
    }};
}

/// Macro for Enum Selection Submenus (Modes, Hide, Spoof, Rumble)
macro_rules! enum_menu {
    (
        label: $label_fmt:literal,
        icon: $icon:expr,
        current: $current:expr,
        type: $enum_type:ty,
        variants: [ $($variant:ident),+ ],
        enabled: $enabled:expr,
        access: { $($access:tt)+ },
        on_change: |$tray:ident, $state:ident, $new_val:ident| $change_block:block
    ) => {{
        let variants = vec![ $( <$enum_type>::$variant ),+ ];
        let selected_idx = variants.iter().position(|v| v == &$current).unwrap_or(0);

        menu::SubMenu {
            label: format!($label_fmt, $current),
            icon_name: $icon.into(),
            enabled: $enabled,
            submenu: vec![menu::RadioGroup {
                selected: selected_idx,
                select: Box::new({
                    let variants = variants.clone();
                    move |$tray: &mut CtrlAssistTray, index| {
                        if let Some($new_val) = variants.get(index).cloned() {
                            let mut state = $tray.state.lock();
                            if state.$($access)+ != $new_val {
                                state.$($access)+ = $new_val.clone();
                                let $state = &mut state;
                                $change_block
                            }
                        }
                    }
                }),
                options: variants.iter().map(|v| {
                    menu::RadioItem {
                        label: format!("{:?}", v),
                        enabled: $enabled,
                        ..Default::default()
                    }
                }).collect(),
            }.into()],
            ..Default::default()
        }.into()
    }};
}

pub struct CtrlAssistTray {
    state: Arc<Mutex<TrayState>>,
    /// Store shutdown sender for signaling
    shutdown_tx: Option<mpsc::Sender<()>>,
}

impl CtrlAssistTray {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        let gilrs =
            crate::gilrs_helper::new_gilrs().map_err(|e| format!("Failed to init Gilrs: {}", e))?;
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
        // Spawning on the tokio runtime to keep UI responsive
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
        let mode = state.operation_mode;
        let valid = match mode {
            OperationMode::Mux => state.is_valid_for_mux_start(),
            OperationMode::Demux => state.is_valid_for_demux_start(),
        };

        if !valid {
            let msg = match mode {
                OperationMode::Mux => "Please select two different controllers first",
                OperationMode::Demux => "Please select a primary controller first",
            };
            Self::send_notification("CtrlAssist - Cannot Start", msg);
            return;
        }

        match mode {
            OperationMode::Mux => {
                drop(state);
                self.start_mux();
            }
            OperationMode::Demux => {
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

        let config = MuxConfig {
            primary_id,
            assist_id,
            mode: state.mux.mode.clone(),
            hide: state.mux.hide.clone(),
            spoof: state.mux.spoof.clone(),
            rumble: state.mux.rumble.clone(),
        };

        // Use a channel for shutdown signaling
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();
        self.shutdown_tx = Some(shutdown_tx);
        let state_arc = Arc::clone(&self.state);

        let handle = thread::spawn(move || match start_mux_with_state(config, state_arc) {
            Ok(mux_handle) => {
                // Wait for shutdown signal (blocks efficiently)
                let _ = shutdown_rx.recv();
                mux_handle.shutdown();
            }
            Err(e) => {
                error!("Mux thread error: {}", e);
                Self::send_notification("CtrlAssist - Error", &format!("Mux failed: {}", e));
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

        let config = DemuxConfig {
            primary_id,
            sinks: state.demux.sinks,
            mode: state.demux.mode.clone(),
            hide: state.demux.hide.clone(),
            spoof: state.demux.spoof.clone(),
            rumble: state.demux.rumble.clone(),
        };

        // Use a channel for shutdown signaling
        let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();
        self.shutdown_tx = Some(shutdown_tx);
        let state_arc = Arc::clone(&self.state);

        let handle = thread::spawn(move || match start_demux_with_state(config, state_arc) {
            Ok(demux_handle) => {
                // Wait for shutdown signal (blocks efficiently)
                let _ = shutdown_rx.recv();
                demux_handle.shutdown();
            }
            Err(e) => {
                error!("Demux thread error: {}", e);
                Self::send_notification("CtrlAssist - Error", &format!("Demux failed: {}", e));
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

        if let Some(handle) = state.operation_handle.take() {
            // Drop lock to avoid deadlock during join if thread tries to lock state while stopping
            drop(state);
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
        if let Ok(gilrs) = crate::gilrs_helper::new_gilrs() {
            let controllers: Vec<_> = gilrs
                .gamepads()
                .map(|(id, gamepad)| super::state::ControllerInfo {
                    id,
                    name: gamepad.name().to_string(),
                })
                .collect();
            state.controllers = controllers;

            let validate = |sel: Option<GamepadId>, ctrls: &[super::state::ControllerInfo]| {
                sel.filter(|id| ctrls.iter().any(|c| c.id == *id))
            };

            state.mux.selected_primary = validate(state.mux.selected_primary, &state.controllers)
                .or_else(|| state.controllers.first().map(|c| c.id));
            state.mux.selected_assist = validate(state.mux.selected_assist, &state.controllers)
                .or_else(|| state.controllers.get(1).map(|c| c.id));
            state.demux.selected_primary =
                validate(state.demux.selected_primary, &state.controllers)
                    .or_else(|| state.controllers.first().map(|c| c.id));
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
            OperationStatus::Running => format!("CtrlAssist [{:?} Running]", state.operation_mode),
            OperationStatus::Stopped => format!("CtrlAssist [{:?}]", state.operation_mode),
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
        let description = if state.status == OperationStatus::Running {
            match state.operation_mode {
                OperationMode::Mux => format!(
                    "Mux: {} + {}",
                    state.get_mux_primary_name(),
                    state.get_mux_assist_name()
                ),
                OperationMode::Demux => format!(
                    "Demux: {} to {} sinks",
                    state.get_demux_primary_name(),
                    state.demux.sinks
                ),
            }
        } else {
            "Not running".into()
        };

        ToolTip {
            title: "CtrlAssist".into(),
            description,
            icon_name: "input-gaming".into(),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        self.refresh_controllers();
        let state = self.state.lock();
        let is_running = state.status == OperationStatus::Running;
        let op_mode = state.operation_mode;

        let mut items = vec![
            enum_menu!(
                label: "Operation: {:?}",
                icon: "swap-panels",
                current: op_mode,
                type: OperationMode,
                variants: [Mux, Demux],
                enabled: !is_running,
                access: { operation_mode },
                on_change: |_tray, _state, _val| {}
            ),
            MenuItem::Separator,
        ];

        match op_mode {
            OperationMode::Mux => {
                items.extend(vec![
                    simple_item!(
                        label: "Refresh Controllers",
                        icon: "view-refresh",
                        enabled: !is_running,
                        action: |t| t.refresh_controllers()
                    ),
                    controller_menu!(
                        label: "Primary",
                        current_id: state.mux.selected_primary,
                        name: state.get_mux_primary_name(),
                        controllers: state.controllers,
                        enabled: !is_running,
                        assign: |s, id| { s.mux.selected_primary = Some(id); }
                    ),
                    controller_menu!(
                        label: "Assist",
                        current_id: state.mux.selected_assist,
                        name: state.get_mux_assist_name(),
                        controllers: state.controllers,
                        enabled: !is_running,
                        assign: |s, id| { s.mux.selected_assist = Some(id); }
                    ),
                    MenuItem::Separator,
                    enum_menu!(
                        label: "Mode: {:?}",
                        icon: "media-playlist-shuffle",
                        current: state.mux.mode,
                        type: MuxModeType,
                        variants: [Priority, Average, Toggle],
                        enabled: true,
                        access: { mux.mode },
                        on_change: |_t, s, v| {
                            if let Some(r) = &s.mux.runtime_settings { r.update_mode(v.clone()); }
                        }
                    ),
                    enum_menu!(
                        label: "Hide: {:?}",
                        icon: "view-visible",
                        current: state.mux.hide,
                        type: HideType,
                        variants: [None, Steam, System],
                        enabled: !is_running,
                        access: { mux.hide },
                        on_change: |_t, _s, _v| {}
                    ),
                    enum_menu!(
                        label: "Spoof: {:?}",
                        icon: "edit-copy",
                        current: state.mux.spoof,
                        type: SpoofTarget,
                        variants: [None, Primary, Assist],
                        enabled: !is_running,
                        access: { mux.spoof },
                        on_change: |_t, _s, _v| {}
                    ),
                    enum_menu!(
                        label: "Rumble: {:?}",
                        icon: "notification-active",
                        current: state.mux.rumble,
                        type: RumbleTarget,
                        variants: [Both, Primary, Assist, None],
                        enabled: true,
                        access: { mux.rumble },
                        on_change: |_t, s, v| {
                            if let Some(r) = &s.mux.runtime_settings { r.update_rumble(v.clone()); }
                        }
                    ),
                ]);
            }
            OperationMode::Demux => {
                items.extend(vec![
                    simple_item!(
                        label: "Refresh Controllers",
                        icon: "view-refresh",
                        enabled: !is_running,
                        action: |t| t.refresh_controllers()
                    ),
                    controller_menu!(
                        label: "Primary",
                        current_id: state.demux.selected_primary,
                        name: state.get_demux_primary_name(),
                        controllers: state.controllers,
                        enabled: !is_running,
                        assign: |s, id| { s.demux.selected_primary = Some(id); }
                    ),
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
                                }),
                                ..Default::default()
                            }
                            .into(),
                        ],
                        ..Default::default()
                    }
                    .into(),
                    MenuItem::Separator,
                    enum_menu!(
                        label: "Mode: {:?}",
                        icon: "media-playlist-shuffle",
                        current: state.demux.mode,
                        type: DemuxModeType,
                        variants: [Unicast, Multicast],
                        enabled: true,
                        access: { demux.mode },
                        on_change: |_t, s, v| {
                            if let Some(r) = &s.demux.runtime_settings { r.update_mode(v.clone(), s.demux.sinks); }
                        }
                    ),
                    enum_menu!(
                        label: "Hide: {:?}",
                        icon: "view-visible",
                        current: state.demux.hide,
                        type: HideType,
                        variants: [None, Steam, System],
                        enabled: !is_running,
                        access: { demux.hide },
                        on_change: |_t, _s, _v| {}
                    ),
                    enum_menu!(
                        label: "Spoof: {:?}",
                        icon: "edit-copy",
                        current: state.demux.spoof,
                        type: SpoofTarget,
                        variants: [None, Primary],
                        enabled: !is_running,
                        access: { demux.spoof },
                        on_change: |_t, _s, _v| {}
                    ),
                    enum_menu!(
                        label: "Rumble: {:?}",
                        icon: "notification-active",
                        current: state.demux.rumble,
                        type: DemuxRumbleTarget,
                        variants: [Active, None],
                        enabled: true,
                        access: { demux.rumble },
                        on_change: |_t, s, v| {
                            if let Some(r) = &s.demux.runtime_settings { r.update_rumble(v.clone()); }
                        }
                    ),
                ]);
            }
        }

        items.extend(vec![
            MenuItem::Separator,
            simple_item!(label: "Start", icon: "media-playback-start", enabled: !is_running
                    && match state.operation_mode {
                        OperationMode::Mux => state.is_valid_for_mux_start(),
                        OperationMode::Demux => state.is_valid_for_demux_start(),
                    }, action: |t| t.start_operation()),
            simple_item!(label: "Stop", icon: "media-playback-stop", enabled: is_running, action: |t| t.stop_operation()),
            MenuItem::Separator,
            simple_item!(label: "Exit", icon: "application-exit", enabled: true, action: |t| { 
                t.stop_operation();
                // Graceful termination is preferred, but for tray apps exit(0) is often required
                // to break out of the native event loop immediately.
                std::process::exit(0);
            }),
        ]);
        items
    }
}

// Helper function to start mux and update state
fn start_mux_with_state(
    config: MuxConfig,
    state_arc: Arc<Mutex<TrayState>>,
) -> Result<MuxHandle, Box<dyn Error>> {
    let gilrs = crate::gilrs_helper::new_gilrs()?;
    let (h, r) = mux_manager::start_mux(gilrs, config)?;
    let mut s = state_arc.lock();
    s.virtual_device_paths = vec![h.virtual_device_path.clone()];
    s.mux.runtime_settings = Some(r);
    Ok(h)
}

// Helper function to start demux and update state
fn start_demux_with_state(
    config: DemuxConfig,
    state_arc: Arc<Mutex<TrayState>>,
) -> Result<DemuxHandle, Box<dyn Error>> {
    let gilrs = crate::gilrs_helper::new_gilrs()?;
    let (h, r) = demux_manager::start_demux(gilrs, config)?;
    let mut s = state_arc.lock();
    s.virtual_device_paths = h.virtual_device_paths.clone();
    s.demux.runtime_settings = Some(r);
    Ok(h)
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
