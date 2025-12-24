use crate::gilrs_helper;
use crate::mux_modes::ModeType;
use crate::udev_helpers::ScopedDeviceHider;
use crate::{evdev_helpers, ff_helpers, run_ff_loop, run_input_loop, HideType, RumbleTarget, SpoofTarget};
use gilrs::{GamepadId, Gilrs};
use ksni::{Category, Icon, MenuItem, Status, ToolTip, Tray, menu};
use log::{error, info};
use notify_rust::Notification;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use super::config::TrayConfig;
use super::state::{MuxStatus, TrayState};

pub struct CtrlAssistTray {
    state: Arc<Mutex<TrayState>>,
    gilrs: Arc<Mutex<Gilrs>>,
}

impl CtrlAssistTray {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        let gilrs = Gilrs::new().map_err(|e| format!("Failed to init Gilrs: {}", e))?;
        let config = TrayConfig::load();
        let state = TrayState::new(&gilrs, config);

        Ok(Self {
            state: Arc::new(Mutex::new(state)),
            gilrs: Arc::new(Mutex::new(gilrs)),
        })
    }

    fn send_notification(summary: &str, body: &str) {
        if let Err(e) = Notification::new()
            .summary(summary)
            .body(body)
            .appname("CtrlAssist")
            .show()
        {
            error!("Failed to send notification: {}", e);
        }
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
        
        info!("Starting mux: primary={:?}, assist={:?}", primary_id, assist_id);

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

        // Clone settings for thread
        let mode = state.mode.clone();
        let hide = state.hide.clone();
        let spoof = state.spoof.clone();
        let rumble = state.rumble.clone();
        let gilrs_arc = Arc::clone(&self.gilrs);

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_clone = Arc::clone(&shutdown);
        state.shutdown_signal = Some(shutdown);

        // Spawn mux thread
        let handle = thread::spawn(move || {
            if let Err(e) = run_mux_thread(
                gilrs_arc,
                primary_id,
                assist_id,
                mode,
                hide,
                spoof,
                rumble,
                shutdown_clone,
            ) {
                error!("Mux thread error: {}", e);
                Self::send_notification("CtrlAssist - Error", &format!("Mux failed: {}", e));
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
        Self::send_notification("CtrlAssist", "Stopping mux...");

        // Signal shutdown
        if let Some(shutdown) = &state.shutdown_signal {
            shutdown.store(true, Ordering::SeqCst);
        }

        // Wait for thread to finish
        if let Some(handle) = state.mux_handle.take() {
            let _ = handle.join();
        }

        state.status = MuxStatus::Stopped;
        state.shutdown_signal = None;

        info!("Mux stopped");
        Self::send_notification("CtrlAssist", "Mux stopped");
    }
}

impl Tray for CtrlAssistTray {
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
        let state = self.state.lock();
        let is_running = state.status == MuxStatus::Running;

        vec![
            // Controller Selection
            menu::SubMenu {
                label: format!("Primary: {}", state.get_primary_name()),
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
                enabled: !is_running && state.is_valid_for_start(),
                activate: Box::new(|this: &mut Self| {
                    this.start_mux();
                }),
                ..Default::default()
            }
            .into(),
            menu::StandardItem {
                label: "Stop Mux".into(),
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
    let is_selected = matches!((&state.mode, &mode), 
        (ModeType::Priority, ModeType::Priority) |
        (ModeType::Average, ModeType::Average) |
        (ModeType::Toggle, ModeType::Toggle));
    
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
    let is_selected = matches!((&state.hide, &hide),
        (HideType::None, HideType::None) |
        (HideType::Steam, HideType::Steam) |
        (HideType::System, HideType::System));
    
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
    let is_selected = matches!((&state.spoof, &spoof),
        (SpoofTarget::None, SpoofTarget::None) |
        (SpoofTarget::Primary, SpoofTarget::Primary) |
        (SpoofTarget::Assist, SpoofTarget::Assist));
    
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
    let is_selected = matches!((&state.rumble, &rumble),
        (RumbleTarget::Both, RumbleTarget::Both) |
        (RumbleTarget::Primary, RumbleTarget::Primary) |
        (RumbleTarget::Assist, RumbleTarget::Assist) |
        (RumbleTarget::None, RumbleTarget::None));
    
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
    gilrs_arc: Arc<Mutex<Gilrs>>,
    primary_id: GamepadId,
    assist_id: GamepadId,
    mode: ModeType,
    hide: HideType,
    spoof: SpoofTarget,
    rumble: RumbleTarget,
    shutdown: Arc<AtomicBool>,
) -> Result<(), Box<dyn Error>> {
    let mut gilrs = gilrs_arc.lock();
    let mut resources = gilrs_helper::discover_gamepad_resources(&*gilrs);

    // Setup hiding
    let mut hider = ScopedDeviceHider::new(hide);
    if let Some(primary_res) = resources.get(&primary_id) {
        hider.hide_gamepad_devices(primary_res)?;
    }
    if let Some(assist_res) = resources.get(&assist_id) {
        hider.hide_gamepad_devices(assist_res)?;
    }

    // Setup virtual device
    let virtual_info = match spoof {
        SpoofTarget::Primary => evdev_helpers::VirtualGamepadInfo::from(&gilrs.gamepad(primary_id)),
        SpoofTarget::Assist => evdev_helpers::VirtualGamepadInfo::from(&gilrs.gamepad(assist_id)),
        SpoofTarget::None => evdev_helpers::VirtualGamepadInfo {
            name: "CtrlAssist Virtual Gamepad".into(),
            vendor_id: None,
            product_id: None,
        },
    };

    let mut v_uinput = evdev_helpers::create_virtual_gamepad(&virtual_info)?;
    let v_resource = gilrs_helper::wait_for_virtual_device(&mut v_uinput)?;
    let v_dev = v_resource.device;

    // Setup FF targets
    let mut ff_targets = Vec::new();
    let rumble_ids = match rumble {
        RumbleTarget::Primary => vec![primary_id],
        RumbleTarget::Assist => vec![assist_id],
        RumbleTarget::Both => vec![primary_id, assist_id],
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
    
    drop(gilrs); // Release lock before spawning threads
    
    let gilrs_arc_input = Arc::clone(&gilrs_arc);
    let input_handle = thread::spawn(move || {
        let mut gilrs = gilrs_arc_input.lock();
        run_input_loop(*gilrs, v_dev, mode, primary_id, assist_id, shutdown_input);
    });
    
    let ff_handle = thread::spawn(move || {
        run_ff_loop(&mut v_uinput, ff_targets, shutdown_ff);
    });

    input_handle.join().ok();
    ff_handle.join().ok();

    Ok(())
}
