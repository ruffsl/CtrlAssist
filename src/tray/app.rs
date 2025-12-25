use crate::gilrs_helper;
use crate::mux_modes::ModeType;
use crate::udev_helpers::ScopedDeviceHider;
use crate::{HideType, RumbleTarget, SpoofTarget, evdev_helpers, run_ff_loop, run_input_loop};
use gilrs::Gilrs;
use ksni::{Category, MenuItem, Status, ToolTip, Tray, menu};
use log::{error, info};
use notify_rust::Notification;
use parking_lot::Mutex;
use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use super::config::{MuxSettings, TrayConfig};
use super::state::{MuxStatus, TrayState};

pub struct CtrlAssistTray {
    state: Arc<Mutex<TrayState>>,
}

impl CtrlAssistTray {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        let gilrs = Gilrs::new().map_err(|e| format!("Failed to init Gilrs: {}", e))?;
        let config = TrayConfig::load();
        let state = TrayState::new(&gilrs, config);

        Ok(Self {
            state: Arc::new(Mutex::new(state)),
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

        // Group settings for thread
        let mux_settings = MuxSettings {
            primary_id,
            assist_id,
            mode: state.mode.clone(),
            hide: state.hide.clone(),
            spoof: state.spoof.clone(),
            rumble: state.rumble.clone(),
        };

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);
        state.shutdown_signal = Some(shutdown);

        // Setup channel to receive virtual device path from mux thread
        let (vdev_path_tx, vdev_path_rx) = std::sync::mpsc::channel();

        // Spawn mux thread
        let handle = thread::spawn(move || {
            if let Err(e) = run_mux_thread(mux_settings, shutdown_clone, vdev_path_tx) {
                error!("Mux thread error: {}", e);
                Self::send_notification("CtrlAssist - Error", &format!("Mux failed: {}", e));
            }
        });

        // Wait for mux thread to send virtual device path
        if let Ok(path) = vdev_path_rx.recv() {
            state.virtual_device_path = Some(path);
        } else {
            state.virtual_device_path = None;
        }

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

        // Signal shutdown
        if let Some(shutdown) = &state.shutdown_signal {
            shutdown.store(true, Ordering::SeqCst);
        }

        // Unblock FF thread: send a no-op force feedback event and SYN_REPORT
        if let Some(path) = &state.virtual_device_path
            && let Ok(mut v_dev) = evdev::Device::open(path)
        {
            use evdev::{EventType, InputEvent};
            let _ = v_dev.send_events(&[
                InputEvent::new(EventType::FORCEFEEDBACK.0, 0, 0),
                InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
            ]);
        }
        state.virtual_device_path = None;

        // Wait for thread to finish
        if let Some(handle) = state.mux_handle.take() {
            let _ = handle.join();
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

// Mux thread function
fn run_mux_thread(
    settings: MuxSettings,
    shutdown: Arc<AtomicBool>,
    vdev_path_tx: std::sync::mpsc::Sender<std::path::PathBuf>,
) -> Result<(), Box<dyn Error>> {
    let gilrs = Gilrs::new().map_err(|e| format!("Failed to init Gilrs: {e}"))?;
    let mut resources = gilrs_helper::discover_gamepad_resources(&gilrs);

    // Setup hiding
    let mut hider = ScopedDeviceHider::new(settings.hide.clone());
    if let Some(primary_res) = resources.get(&settings.primary_id) {
        hider.hide_gamepad_devices(primary_res)?;
    }
    if let Some(assist_res) = resources.get(&settings.assist_id) {
        hider.hide_gamepad_devices(assist_res)?;
    }

    // Setup virtual device
    let virtual_info = match settings.spoof {
        SpoofTarget::Primary => {
            evdev_helpers::VirtualGamepadInfo::from(&gilrs.gamepad(settings.primary_id))
        }
        SpoofTarget::Assist => {
            evdev_helpers::VirtualGamepadInfo::from(&gilrs.gamepad(settings.assist_id))
        }
        SpoofTarget::None => evdev_helpers::VirtualGamepadInfo {
            name: "CtrlAssist Virtual Gamepad".into(),
            vendor_id: None,
            product_id: None,
        },
    };

    let mut v_uinput = evdev_helpers::create_virtual_gamepad(&virtual_info)?;
    let v_resource = gilrs_helper::wait_for_virtual_device(&mut v_uinput)?;
    let v_dev = v_resource.device;

    // Send virtual device path to tray state
    let _ = vdev_path_tx.send(v_resource.path.clone());

    // Setup FF targets
    let mut ff_targets = Vec::new();
    let rumble_ids = match settings.rumble {
        RumbleTarget::Primary => vec![settings.primary_id],
        RumbleTarget::Assist => vec![settings.assist_id],
        RumbleTarget::Both => vec![settings.primary_id, settings.assist_id],
        RumbleTarget::None => vec![],
    };

    for id in rumble_ids {
        if let Some(res) = resources.remove(&id) {
            ff_targets.push(res);
        }
    }

    // Spawn threads
    let shutdown_input = Arc::clone(&shutdown);
    let shutdown_ff = Arc::clone(&shutdown);

    let input_handle = thread::spawn(move || {
        run_input_loop(
            gilrs,
            v_dev,
            settings.mode,
            settings.primary_id,
            settings.assist_id,
            shutdown_input,
        );
    });

    let ff_handle = thread::spawn(move || {
        run_ff_loop(&mut v_uinput, ff_targets, shutdown_ff);
    });

    input_handle.join().ok();
    ff_handle.join().ok();

    Ok(())
}
