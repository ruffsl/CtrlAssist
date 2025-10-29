use clap::Parser;
use gilrs::{Gilrs, Event, Button, Axis};
use evdev::{InputEvent, KeyCode, AbsoluteAxisCode, AbsInfo, UinputAbsSetup};

/// CtrlAssist: Merge two gamepads into one virtual device with assist mode
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Deadman button (e.g., LeftTrigger, etc.)
    #[arg(long, default_value = "LeftTrigger")]
    deadman: String,
    /// Primary controller index
    #[arg(long, default_value_t = 0)]
    primary: usize,
    /// Assist controller index
    #[arg(long, default_value_t = 1)]
    assist: usize,
}

fn main() {
    let args = Args::parse();
    let mut gilrs = Gilrs::new().expect("Failed to initialize gilrs");

    // List available gamepads and collect their IDs
    println!("Available gamepads:");
    let gamepads: Vec<_> = gilrs.gamepads().collect();
    for (idx, (_id, gamepad)) in gamepads.iter().enumerate() {
        println!("  [{}] {}", idx, gamepad.name());
    }

    // Get GamepadId from index
    let primary_id = gamepads.get(args.primary).map(|(id, _)| *id)
        .expect("Primary controller index out of range");
    let assist_id = gamepads.get(args.assist).map(|(id, _)| *id)
        .expect("Assist controller index out of range");

    let deadman_button = match args.deadman.as_str() {
        "LeftTrigger" => Button::LeftTrigger,
        _ => Button::LeftTrigger,
    };


    // Setup axes for virtual device
    let abs_setup = AbsInfo::new(0, 0, 255, 0, 0, 0);
    let abs_x = UinputAbsSetup::new(AbsoluteAxisCode::ABS_X, abs_setup);
    let abs_y = UinputAbsSetup::new(AbsoluteAxisCode::ABS_Y, abs_setup);
    let abs_rx = UinputAbsSetup::new(AbsoluteAxisCode::ABS_RX, abs_setup);
    let abs_ry = UinputAbsSetup::new(AbsoluteAxisCode::ABS_RY, abs_setup);
    let abs_z = UinputAbsSetup::new(AbsoluteAxisCode::ABS_Z, abs_setup);
    let abs_rz = UinputAbsSetup::new(AbsoluteAxisCode::ABS_RZ, abs_setup);

    // Create a virtual gamepad device using evdev/uinput
    let builder = evdev::uinput::VirtualDevice::builder().unwrap();
    let mut uinput_dev = builder
        .name("CtrlAssist Virtual Gamepad")
        .with_keys(&evdev::AttributeSet::from_iter([
            KeyCode::BTN_SOUTH, KeyCode::BTN_EAST, KeyCode::BTN_WEST, KeyCode::BTN_NORTH,
            KeyCode::BTN_TL, KeyCode::BTN_TR, KeyCode::BTN_THUMBL, KeyCode::BTN_THUMBR,
            KeyCode::BTN_SELECT, KeyCode::BTN_START, KeyCode::BTN_MODE,
            KeyCode::KEY_UP, KeyCode::KEY_DOWN, KeyCode::KEY_LEFT, KeyCode::KEY_RIGHT
        ]))
        .unwrap()
        .with_absolute_axis(&abs_x)
        .unwrap()
        .with_absolute_axis(&abs_y)
        .unwrap()
        .with_absolute_axis(&abs_rx)
        .unwrap()
        .with_absolute_axis(&abs_ry)
        .unwrap()
        .with_absolute_axis(&abs_z)
        .unwrap()
        .with_absolute_axis(&abs_rz)
        .unwrap()
        .build()
        .unwrap();

    println!("Starting assist mode: primary={}, assist={}, deadman={:?}", args.primary, args.assist, deadman_button);

    // Main event loop
    loop {
        while let Some(Event { id: _id, event, .. }) = gilrs.next_event() {
            // Read deadman button state from assist controller
            let assist_deadman = !gilrs.gamepad(assist_id).is_pressed(deadman_button);

            // If assist deadman is held, assist controller takes priority
            let active_id = if assist_deadman { assist_id } else { primary_id };

            // Forward input from active controller to virtual device
            // Map gilrs event to evdev InputEvent and send to uinput_dev
            match event {
                gilrs::EventType::ButtonPressed(button, _) => {
                    if let Some(key) = gilrs_button_to_evdev_key(button) {
                        let input_event = InputEvent::new(
                            evdev::EventType::KEY.0,
                            key.0,
                            1
                        );
                        let _ = uinput_dev.emit(&[input_event]);
                    }
                }
                gilrs::EventType::ButtonReleased(button, _) => {
                    if let Some(key) = gilrs_button_to_evdev_key(button) {
                        let input_event = InputEvent::new(
                            evdev::EventType::KEY.0,
                            key.0,
                            0
                        );
                        let _ = uinput_dev.emit(&[input_event]);
                    }
                }
                gilrs::EventType::AxisChanged(axis, value, _) => {
                    if let Some(abs_axis) = gilrs_axis_to_evdev_axis(axis) {
                        let input_event = InputEvent::new(
                            evdev::EventType::ABSOLUTE.0,
                            abs_axis.0,
                            value as i32
                        );
                        let _ = uinput_dev.emit(&[input_event]);
                    }
                }
                _ => {}
            }
            println!("Active controller: {:?}, Event: {:?}", active_id, event);
        }
    }

// Helper: Map gilrs Button to evdev Key
fn gilrs_button_to_evdev_key(button: Button) -> Option<KeyCode> {
    match button {
        Button::South => Some(KeyCode::BTN_SOUTH),
        Button::East => Some(KeyCode::BTN_EAST),
        Button::West => Some(KeyCode::BTN_WEST),
        Button::North => Some(KeyCode::BTN_NORTH),
        Button::LeftTrigger => Some(KeyCode::BTN_TL),
        Button::RightTrigger => Some(KeyCode::BTN_TR),
        Button::LeftThumb => Some(KeyCode::BTN_THUMBL),
        Button::RightThumb => Some(KeyCode::BTN_THUMBR),
        Button::Select => Some(KeyCode::BTN_SELECT),
        Button::Start => Some(KeyCode::BTN_START),
        Button::Mode => Some(KeyCode::BTN_MODE),
        Button::DPadUp => Some(KeyCode::KEY_UP),
        Button::DPadDown => Some(KeyCode::KEY_DOWN),
        Button::DPadLeft => Some(KeyCode::KEY_LEFT),
        Button::DPadRight => Some(KeyCode::KEY_RIGHT),
        _ => None,
    }
}

fn gilrs_axis_to_evdev_axis(axis: Axis) -> Option<AbsoluteAxisCode> {
    match axis {
        Axis::LeftStickX => Some(AbsoluteAxisCode::ABS_X),
        Axis::LeftStickY => Some(AbsoluteAxisCode::ABS_Y),
        Axis::RightStickX => Some(AbsoluteAxisCode::ABS_RX),
        Axis::RightStickY => Some(AbsoluteAxisCode::ABS_RY),
        Axis::LeftZ => Some(AbsoluteAxisCode::ABS_Z),
        Axis::RightZ => Some(AbsoluteAxisCode::ABS_RZ),
        _ => None,
    }
}
}
