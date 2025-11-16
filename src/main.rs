use clap::{Parser, Subcommand};
use evdev::{AbsInfo, AbsoluteAxisCode, InputEvent, KeyCode, UinputAbsSetup};
use gilrs::{Axis, Button, GamepadId, Gilrs};

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
        Commands::Start { primary, assist } => {
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

            if let Err(e) = start_assist(primary_id, assist_id) {
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
) -> Result<(), Box<dyn std::error::Error>> {
    if primary_id == assist_id {
        return Err("The primary and assist controllers must be different devices.".into());
    }

    // Setup axes for virtual device
    let abs_setup = AbsInfo::new(0, 0, 255, 0, 0, 0);
    let abs_x = UinputAbsSetup::new(AbsoluteAxisCode::ABS_X, abs_setup);
    let abs_y = UinputAbsSetup::new(AbsoluteAxisCode::ABS_Y, abs_setup);
    let abs_z = UinputAbsSetup::new(AbsoluteAxisCode::ABS_Z, abs_setup);
    let abs_rx = UinputAbsSetup::new(AbsoluteAxisCode::ABS_RX, abs_setup);
    let abs_ry = UinputAbsSetup::new(AbsoluteAxisCode::ABS_RY, abs_setup);
    let abs_rz = UinputAbsSetup::new(AbsoluteAxisCode::ABS_RZ, abs_setup);
    let abs_hx = UinputAbsSetup::new(AbsoluteAxisCode::ABS_HAT0X, abs_setup);
    let abs_hy = UinputAbsSetup::new(AbsoluteAxisCode::ABS_HAT0Y, abs_setup);

    // Create a virtual gamepad device using evdev/uinput
    let virtual_name = "CtrlAssist Virtual Gamepad";
    let builder = evdev::uinput::VirtualDevice::builder().unwrap();
    let mut uinput_dev = builder
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
        ]))
        .unwrap()
        .with_absolute_axis(&abs_x)
        .unwrap()
        .with_absolute_axis(&abs_y)
        .unwrap()
        .with_absolute_axis(&abs_z)
        .unwrap()
        .with_absolute_axis(&abs_rx)
        .unwrap()
        .with_absolute_axis(&abs_ry)
        .unwrap()
        .with_absolute_axis(&abs_rz)
        .unwrap()
        .with_absolute_axis(&abs_hx)
        .unwrap()
        .with_absolute_axis(&abs_hy)
        .unwrap()
        .build()
        .unwrap();

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

    // Set up Ctrl+C handler
    ctrlc::set_handler(|| {
        println!("\nShutdown signal received. Exiting.");
        std::process::exit(0);
    })?;

    let deadman_button = Button::LeftThumb;
    let mut active_id = primary_id;

    loop {
        while let Some(event) = gilrs.next_event() {
            // Ignore events from virtual device
            if event.id == virtual_id {
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
                        let _ = uinput_dev.emit(&[input_event]);
                    }
                }
                gilrs::EventType::ButtonReleased(button, _) => {
                    if let Some(key) = gilrs_button_to_evdev_key(button) {
                        let input_event = InputEvent::new(evdev::EventType::KEY.0, key.0, 0);
                        let _ = uinput_dev.emit(&[input_event]);
                    }
                }
                gilrs::EventType::ButtonChanged(button, value, _) => {
                    if let Some(abs_axis) = gilrs_button_to_evdev_axis(button) {

                        let scaled_value;
                        if button == Button::DPadUp || button == Button::DPadLeft {
                            scaled_value = ((-value + 1.0) * 127.5).round() as i32;
                        } else if button == Button::DPadDown || button == Button::DPadRight {
                            scaled_value = ((value + 1.0) * 127.5).round() as i32;
                        } else {
                            scaled_value = ((value) * 255.0).round() as i32;
                        }
                        let input_event =
                            InputEvent::new(evdev::EventType::ABSOLUTE.0, abs_axis.0, scaled_value);
                        let _ = uinput_dev.emit(&[input_event]);
                    }
                }
                gilrs::EventType::AxisChanged(axis, value, _) => {
                    if let Some(abs_axis) = gilrs_axis_to_evdev_axis(axis) {
                        let scaled_value;
                        if axis == Axis::LeftStickY || axis == Axis::RightStickY {
                            scaled_value = ((-value + 1.0) * 127.5).round() as i32;
                        } else {
                            scaled_value = ((value + 1.0) * 127.5).round() as i32;
                        }
                        let input_event =
                            InputEvent::new(evdev::EventType::ABSOLUTE.0, abs_axis.0, scaled_value);
                        let _ = uinput_dev.emit(&[input_event]);
                    }
                }
                _ => {}
            }
            let syn_event = InputEvent::new(evdev::EventType::SYNCHRONIZATION.0, 0, 0);
            let _ = uinput_dev.emit(&[syn_event]);
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
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
