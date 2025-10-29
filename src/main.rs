use clap::Parser;
use gilrs::{Gilrs, Event, Button};
// use evdev::{Device, InputEvent}; // Uncomment when implementing virtual device logic

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

    // TODO: Create virtual device using evdev/uinput
    // let uinput_dev = ...

    println!("Starting assist mode: primary={}, assist={}, deadman={:?}", args.primary, args.assist, deadman_button);

    // Main event loop
    loop {
        while let Some(Event { id: _id, event, .. }) = gilrs.next_event() {
            // Read deadman button state from assist controller
            let assist_deadman = !gilrs.gamepad(assist_id).is_pressed(deadman_button);

            // If assist deadman is held, assist controller takes priority
            let active_id = if assist_deadman { assist_id } else { primary_id };

            // Forward input from active controller to virtual device
            // TODO: Map gilrs event to evdev InputEvent and send to uinput_dev
            println!("Active controller: {:?}, Event: {:?}", active_id, event);
        }
        // TODO: Optimize for minimal latency (consider event-driven or async)
    }
}
