use clap::{Parser, Subcommand};
use evdev::{AbsInfo, AbsoluteAxisCode, InputEvent, KeyCode, UinputAbsSetup};
use gilrs::{Axis, Button, GamepadId, Gilrs};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use udev::Enumerator;

/// A CLI tool to merge two gamepads into one virtual controller.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Lists all connected gamepads and their IDs.
    List,

    /// Starts the controller assist mode.
    Start {
        /// The ID of the primary controller (see 'list' command).
        #[arg(short, long, default_value_t = 0)]
        primary: usize,

        /// The ID of the assist controller (see 'list' command).
        #[arg(short, long, default_value_t = 1)]
        assist: usize,

        /// Optionally restrict device permissions for selected controllers
        #[arg(long, default_value_t = false)]
        hide: bool,
    },
}

fn main() {
    // Parse the command-line arguments
    let cli = Cli::parse();

    // Match on the subcommand
    match &cli.command {
        Commands::List => {
            if let Err(e) = list_gamepads() {
                eprintln!("Error listing gamepads: {}", e);
            }
        }
        Commands::Start {
            primary,
            assist,
            hide,
        } => {
            let gilrs = Gilrs::new().expect("Failed to initialize Gilrs");
            let gamepad_ids: Vec<GamepadId> = gilrs.gamepads().map(|(id, _)| id).collect();
            let primary_id = *gamepad_ids
                .get(*primary)
                .expect("Invalid primary controller ID");
            let assist_id = *gamepad_ids
                .get(*assist)
                .expect("Invalid assist controller ID");

            println!("\nControllers found and verified:");
            let primary_gamepad = gilrs.gamepad(primary_id);
            let assist_gamepad = gilrs.gamepad(assist_id);
            println!("Primary:");
            println!("  ID: {} - Name: {}", primary_id, primary_gamepad.name());
            println!("Assist:");
            println!("  ID: {} - Name: {}", assist_id, assist_gamepad.name());

            if let Err(e) = start_assist(primary_id, assist_id, *hide) {
                eprintln!("Error in assist mode: {}", e);
            }
        }
    }
}

/// Lists all connected gamepads.
fn list_gamepads() -> Result<(), gilrs::Error> {
    let gilrs = Gilrs::new()?;

    println!("Connected Gamepads:");
    if gilrs.gamepads().count() == 0 {
        println!("  No gamepads found.");
    }

    for (id, gamepad) in gilrs.gamepads() {
        println!("  ID: {} - Name: {}", id, gamepad.name());
    }

    Ok(())
}

/// Stub function for starting the main assist logic.
fn start_assist(
    primary_id: GamepadId,
    assist_id: GamepadId,
    hide: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    if primary_id == assist_id {
        return Err("The primary and assist controllers must be different devices.".into());
    }

    let virtual_name = "CtrlAssist Virtual Gamepad";
    let mut virtual_dev = create_virtual_gamepad(virtual_name)?;

    // sleep to allow the virtual device to be recognized by the system
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Gilrs needs to be mutable here for the event loop later.
    let mut gilrs = Gilrs::new()?;

    // Get the GamepadId for the virtual device by matching the name string used during creation
    let virtual_id = gilrs
        .gamepads()
        .find_map(|(id, gamepad)| {
            if gamepad.name() == virtual_name {
                Some(id)
            } else {
                None
            }
        })
        .expect("Virtual device not found in gilrs");
    println!("Virtual:");
    println!("  ID: {} - Name: {}", virtual_id, virtual_name);

    // Optionally restrict device permissions for only the selected primary and assist controllers
    let mut restricted_paths_per_gamepad = Vec::new();
    if hide {
        let gilrs = Gilrs::new()?;
        let gamepads = [primary_id, assist_id];
        for gp_id in gamepads.iter() {
            let gamepad = gilrs.gamepad(*gp_id);
            println!(
                "Restricting for gamepad: ID={} Name={} Vendor={:?} Product={:?}",
                gp_id,
                gamepad.name(),
                gamepad.vendor_id(),
                gamepad.product_id()
            );
            let mut restricted_paths = std::collections::HashSet::new();
            restrict_gamepad_devices(
                gamepad.vendor_id(),
                gamepad.product_id(),
                &mut restricted_paths,
            );
            restricted_paths_per_gamepad.push(restricted_paths);
        }
    }
    let restore_paths_per_gamepad: Vec<Vec<_>> = restricted_paths_per_gamepad
        .iter()
        .map(|set| set.iter().cloned().collect())
        .collect();
    ctrlc::set_handler(move || {
        println!("\nShutdown signal received.");
        if hide {
            println!("Restoring device permissions...");
            for (i, paths) in restore_paths_per_gamepad.iter().enumerate() {
                println!("Gamepad {}:", i);
                for path in paths {
                    let _ = restore_device(path);
                    println!("  Restored permissions for device: {}", path);
                }
            }
        }
        std::process::exit(0);
    })?;

    let deadman_button = Button::LeftThumb;
    let mut active_id = primary_id;
    let timeout = Some(std::time::Duration::from_millis(1000));

    loop {
        while let Some(event) = gilrs.next_event_blocking(timeout) {
            // Ignore events from virtual device
            if event.id != primary_id && event.id != assist_id {
                continue;
            }

            // Deadman button toggles active controller on press
            match event.event {
                gilrs::EventType::ButtonPressed(button, _)
                    if button == deadman_button && event.id == assist_id =>
                {
                    // Toggle active controller
                    if active_id == primary_id {
                        active_id = assist_id;
                        println!("Toggled active controller to assist (ID: {})", assist_id);
                    } else {
                        active_id = primary_id;
                        println!("Toggled active controller to primary (ID: {})", primary_id);
                    }
                    continue;
                }
                gilrs::EventType::ButtonReleased(button, _)
                    if button == deadman_button && event.id == assist_id =>
                {
                    // No-op on release
                    continue;
                }
                _ => {}
            }

            // Only relay events from the currently active controller
            if event.id != active_id {
                continue;
            }

            println!("Event: {:?}", event);

            match event.event {
                gilrs::EventType::ButtonPressed(button, _) => {
                    if let Some(key) = gilrs_button_to_evdev_key(button) {
                        let input_event = InputEvent::new(evdev::EventType::KEY.0, key.0, 1);
                        let _ = virtual_dev.emit(&[input_event]);
                    }
                }
                gilrs::EventType::ButtonReleased(button, _) => {
                    if let Some(key) = gilrs_button_to_evdev_key(button) {
                        let input_event = InputEvent::new(evdev::EventType::KEY.0, key.0, 0);
                        let _ = virtual_dev.emit(&[input_event]);
                    }
                }
                gilrs::EventType::ButtonChanged(button, value, _) => {
                    if let Some(abs_axis) = gilrs_button_to_evdev_axis(button) {
                        let scaled_value = match button {
                            Button::DPadUp | Button::DPadLeft => {
                                ((-value + 1.0) * 127.5).round() as i32
                            }
                            Button::DPadDown | Button::DPadRight => {
                                ((value + 1.0) * 127.5).round() as i32
                            }
                            _ => (value * 255.0).round() as i32,
                        };
                        let input_event =
                            InputEvent::new(evdev::EventType::ABSOLUTE.0, abs_axis.0, scaled_value);
                        let _ = virtual_dev.emit(&[input_event]);
                    }
                }
                gilrs::EventType::AxisChanged(axis, value, _) => {
                    if let Some(abs_axis) = gilrs_axis_to_evdev_axis(axis) {
                        let scaled_value = match axis {
                            Axis::LeftStickY | Axis::RightStickY => {
                                ((-value + 1.0) * 127.5).round() as i32
                            }
                            _ => ((value + 1.0) * 127.5).round() as i32,
                        };
                        let input_event =
                            InputEvent::new(evdev::EventType::ABSOLUTE.0, abs_axis.0, scaled_value);
                        let _ = virtual_dev.emit(&[input_event]);
                    }
                }
                _ => {}
            }
            let syn_event = InputEvent::new(evdev::EventType::SYNCHRONIZATION.0, 0, 0);
            let _ = virtual_dev.emit(&[syn_event]);
        }
    }
}

/// Helper to create the virtual gamepad device
fn create_virtual_gamepad(
    virtual_name: &str,
) -> Result<evdev::uinput::VirtualDevice, Box<dyn std::error::Error>> {
    let max = 255;
    let abs_setup = AbsInfo::new((max / 2) as i32, 0, max, 0, 0, 0);
    let abs_z_setup = AbsInfo::new(0, 0, max, 0, 0, 0);
    let abs_x = UinputAbsSetup::new(AbsoluteAxisCode::ABS_X, abs_setup);
    let abs_y = UinputAbsSetup::new(AbsoluteAxisCode::ABS_Y, abs_setup);
    let abs_z = UinputAbsSetup::new(AbsoluteAxisCode::ABS_Z, abs_z_setup);
    let abs_rx = UinputAbsSetup::new(AbsoluteAxisCode::ABS_RX, abs_setup);
    let abs_ry = UinputAbsSetup::new(AbsoluteAxisCode::ABS_RY, abs_setup);
    let abs_rz = UinputAbsSetup::new(AbsoluteAxisCode::ABS_RZ, abs_z_setup);
    let abs_hx = UinputAbsSetup::new(AbsoluteAxisCode::ABS_HAT0X, abs_setup);
    let abs_hy = UinputAbsSetup::new(AbsoluteAxisCode::ABS_HAT0Y, abs_setup);

    let builder = evdev::uinput::VirtualDevice::builder()?;
    let dev = builder
        .name(virtual_name)
        .with_keys(&evdev::AttributeSet::from_iter([
            KeyCode::BTN_NORTH,
            KeyCode::BTN_SOUTH,
            KeyCode::BTN_EAST,
            KeyCode::BTN_WEST,
            KeyCode::BTN_TL,
            KeyCode::BTN_TR,
            KeyCode::BTN_THUMBL,
            KeyCode::BTN_THUMBR,
            KeyCode::BTN_SELECT,
            KeyCode::BTN_START,
            KeyCode::BTN_MODE,
            KeyCode::BTN_DPAD_UP,
            KeyCode::BTN_DPAD_DOWN,
            KeyCode::BTN_DPAD_LEFT,
            KeyCode::BTN_DPAD_RIGHT,
        ]))?
        .with_absolute_axis(&abs_x)?
        .with_absolute_axis(&abs_y)?
        .with_absolute_axis(&abs_z)?
        .with_absolute_axis(&abs_rx)?
        .with_absolute_axis(&abs_ry)?
        .with_absolute_axis(&abs_rz)?
        .with_absolute_axis(&abs_hx)?
        .with_absolute_axis(&abs_hy)?
        .build()?;
    Ok(dev)
}

// Helper: Map gilrs Button to evdev Key
fn gilrs_button_to_evdev_key(button: Button) -> Option<KeyCode> {
    match button {
        Button::North => Some(KeyCode::BTN_NORTH),
        Button::South => Some(KeyCode::BTN_SOUTH),
        Button::East => Some(KeyCode::BTN_EAST),
        Button::West => Some(KeyCode::BTN_WEST),
        Button::LeftTrigger => Some(KeyCode::BTN_TL),
        Button::RightTrigger => Some(KeyCode::BTN_TR),
        Button::LeftTrigger2 => Some(KeyCode::BTN_TL2),
        Button::RightTrigger2 => Some(KeyCode::BTN_TR2),
        Button::LeftThumb => Some(KeyCode::BTN_THUMBL),
        Button::RightThumb => Some(KeyCode::BTN_THUMBR),
        Button::Select => Some(KeyCode::BTN_SELECT),
        Button::Start => Some(KeyCode::BTN_START),
        Button::Mode => Some(KeyCode::BTN_MODE),
        Button::DPadUp => Some(KeyCode::BTN_DPAD_UP),
        Button::DPadDown => Some(KeyCode::BTN_DPAD_DOWN),
        Button::DPadLeft => Some(KeyCode::BTN_DPAD_LEFT),
        Button::DPadRight => Some(KeyCode::BTN_DPAD_RIGHT),
        _ => None,
    }
}

fn gilrs_button_to_evdev_axis(button: Button) -> Option<AbsoluteAxisCode> {
    match button {
        Button::LeftTrigger2 => Some(AbsoluteAxisCode::ABS_Z),
        Button::RightTrigger2 => Some(AbsoluteAxisCode::ABS_RZ),
        Button::DPadUp => Some(AbsoluteAxisCode::ABS_HAT0Y),
        Button::DPadDown => Some(AbsoluteAxisCode::ABS_HAT0Y),
        Button::DPadLeft => Some(AbsoluteAxisCode::ABS_HAT0X),
        Button::DPadRight => Some(AbsoluteAxisCode::ABS_HAT0X),
        _ => None,
    }
}

fn gilrs_axis_to_evdev_axis(axis: Axis) -> Option<AbsoluteAxisCode> {
    match axis {
        Axis::LeftStickX => Some(AbsoluteAxisCode::ABS_X),
        Axis::LeftStickY => Some(AbsoluteAxisCode::ABS_Y),
        Axis::LeftZ => Some(AbsoluteAxisCode::ABS_Z),
        Axis::RightStickX => Some(AbsoluteAxisCode::ABS_RX),
        Axis::RightStickY => Some(AbsoluteAxisCode::ABS_RY),
        Axis::RightZ => Some(AbsoluteAxisCode::ABS_RZ),
        _ => None,
    }
}

// Restrict access to gamepad devices matching vendor and product IDs
fn restrict_gamepad_devices(
    vendor_id: Option<u16>,
    product_id: Option<u16>,
    restricted_paths: &mut std::collections::HashSet<String>,
) {
    for subsystem in ["input", "hidraw"] {
        let mut enumerator = Enumerator::new().unwrap();
        enumerator.match_subsystem(subsystem).unwrap();
        for device in enumerator.scan_devices().unwrap() {
            if let Some(devnode) = device.devnode() {
                let path_str = devnode.to_string_lossy();
                let matches = match (vendor_id, product_id) {
                    (Some(vendor), Some(product)) => {
                        let sys_vendor = device.property_value("ID_VENDOR_ID");
                        let sys_product = device.property_value("ID_MODEL_ID");
                        let vendor_str = format!("{:04x}", vendor);
                        let product_str = format!("{:04x}", product);
                        let sys_vendor_match = sys_vendor
                            .and_then(|v| v.to_str().map(|s| s == vendor_str))
                            .unwrap_or(false);
                        let sys_product_match = sys_product
                            .and_then(|p| p.to_str().map(|s| s == product_str))
                            .unwrap_or(false);
                        sys_vendor_match && sys_product_match
                    }
                    _ => false,
                };
                if matches {
                    let related_paths = find_related_input_paths(&path_str);
                    for path in related_paths {
                        if restrict_device(&path).is_ok() && !restricted_paths.contains(&path) {
                            println!("  Restricted permissions for device: {}", path);
                            restricted_paths.insert(path);
                        }
                    }
                }
            }
        }
    }
}

/// Find all related device nodes (event, hidraw, symlinks) for a js device path
fn find_related_input_paths(js_path: &str) -> Vec<String> {
    let mut paths = vec![js_path.to_string()];
    let mut enumerator = Enumerator::new().unwrap();
    enumerator.match_subsystem("input").unwrap();
    // First, find the js device and its parent syspath
    let mut js_parent_syspath: Option<std::path::PathBuf> = None;
    for device in enumerator.scan_devices().unwrap() {
        if let Some(devnode) = device.devnode() {
            if devnode.to_str() == Some(js_path) {
                if let Some(parent) = device.parent() {
                    js_parent_syspath = Some(parent.syspath().to_path_buf());
                }
                break;
            }
        }
    }
    // Now, collect all event/hidraw siblings with the same parent syspath
    if let Some(parent_syspath) = js_parent_syspath {
        let mut enumerator = Enumerator::new().unwrap();
        enumerator.match_subsystem("input").unwrap();
        for device in enumerator.scan_devices().unwrap() {
            if let Some(devnode) = device.devnode() {
                let path_str = devnode.to_string_lossy();
                if path_str.contains("/dev/input/event") || path_str.contains("/dev/hidraw") {
                    if let Some(parent) = device.parent() {
                        let syspath = parent.syspath();
                        if syspath == parent_syspath {
                            paths.push(path_str.to_string());
                        }
                    }
                }
            }
        }
    }
    paths
}

fn restrict_device(path: &str) -> std::io::Result<()> {
    fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?; // Only root rw
    Ok(())
}

fn restore_device(path: &str) -> std::io::Result<()> {
    fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))?; // root+input group rw
    Ok(())
}
