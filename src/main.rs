use clap::{Parser, Subcommand};
use evdev::InputEvent;
use gilrs::{Axis, Button, GamepadId, Gilrs};
use std::collections::HashSet;
use std::error::Error;
use std::time::Duration;

mod evdev_helpers;
mod log_setup;
mod udev_helpers;

/// Multiplex multiple controllers into virtual gamepad.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// List all detected controllers and respective IDs.
    List,

    /// Multiplex connected controllers into virtual gamepad.
    Mux {
        /// The ID of the primary controller (see 'list' command).
        #[arg(short, long, default_value_t = 0)]
        primary: usize,

        /// The ID of the assist controller (see 'list' command).
        #[arg(short, long, default_value_t = 1)]
        assist: usize,

        /// Optionally hide primary and assist controllers
        #[arg(long, default_value_t = false)]
        hide: bool,

        /// Spoof type for virtual device.
        #[arg(long, value_enum, default_value_t = SpoofType::Primary)]
        spoof: SpoofType,
    },
}

#[derive(clap::ValueEnum, Clone, Debug, Default)]
pub enum SpoofType {
    #[default]
    Primary,
    Assist,
    None,
}

fn main() -> Result<(), Box<dyn Error>> {
    log_setup::init_logger().expect("Failed to set logger");

    let cli = Cli::parse();

    match &cli.command {
        Commands::List => {
            list_gamepads()?;
        }
        Commands::Mux {
            primary,
            assist,
            hide,
            spoof,
        } => {
            mux_gamepads(*primary, *assist, *hide, spoof.clone())?;
        }
    }
    Ok(())
}

/// List all detected controllers.
fn list_gamepads() -> Result<(), gilrs::Error> {
    let gilrs = Gilrs::new()?;

    println!("Detected controllers:");
    let mut count = 0;
    for (id, gamepad) in gilrs.gamepads() {
        println!("  ID: {} - Name: {}", id, gamepad.name());
        count += 1;
    }

    if count == 0 {
        println!("  No controllers found.");
    }

    Ok(())
}

/// Multiplex connected controllers.
fn mux_gamepads(
    primary_usize: usize,
    assist_usize: usize,
    hide: bool,
    spoof: SpoofType,
) -> Result<(), Box<dyn Error>> {
    // --- 1. Setup and Validation ---
    if primary_usize == assist_usize {
        return Err("Primary and Assist controllers must be separate devices.".into());
    }

    // Find connected controllers.
    let gilrs = Gilrs::new()?;
    let mut primary_opt: Option<GamepadId> = None;
    let mut assist_opt: Option<GamepadId> = None;
    for (id, _gamepad) in gilrs.gamepads() {
        let id_usize: usize = id.into();
        if id_usize == primary_usize {
            primary_opt = Some(id);
        }
        if id_usize == assist_usize {
            assist_opt = Some(id);
        }
        if primary_opt.is_some() && assist_opt.is_some() {
            break;
        }
    }
    let primary_id =
        primary_opt.ok_or_else(|| format!("Primary controller ID {} not found.", primary_usize))?;
    let assist_id =
        assist_opt.ok_or_else(|| format!("Assist controller ID {} not found.", assist_usize))?;

    println!("Connected controllers:");
    let primary_gamepad = gilrs.gamepad(primary_id);
    let assist_gamepad = gilrs.gamepad(assist_id);
    println!(
        "  Primary: ID: {} - Name: {}",
        primary_id,
        primary_gamepad.name()
    );
    println!(
        "  Assist:  ID: {} - Name: {}",
        assist_id,
        assist_gamepad.name()
    );

    // Hide connected controllers.
    let mut restore_paths = HashSet::new();
    if hide {
        println!("\nHiding controllers... (requires root)");
        // We can re-use the gamepad objects from the *first* gilrs instance
        for gamepad in [&primary_gamepad, &assist_gamepad] {
            log::info!("Hiding: {}", gamepad.name());
            udev_helpers::restrict_gamepad_devices(gamepad, &mut restore_paths)?;
        }
        // If restore paths is empty, throw an error
        if restore_paths.is_empty() {
            return Err("Devices could not be hidden. Check permissions.".into());
        }
    }

    // Create virtual gamepad.
    use evdev_helpers::VirtualGamepadInfo;
    let virtual_info = match spoof {
        SpoofType::Primary => VirtualGamepadInfo::from(&primary_gamepad),
        SpoofType::Assist => VirtualGamepadInfo::from(&assist_gamepad),
        SpoofType::None => VirtualGamepadInfo {
            name: "CtrlAssist Virtual Gamepad",
            vendor_id: None,
            product_id: None,
        },
    };
    let mut virtual_dev = evdev_helpers::create_virtual_gamepad(&virtual_info)?;
    // Find virtual gamepad.
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(1);
    let virtual_id = loop {
        let gilrs = Gilrs::new()?;
        if let Some((id, _)) = gilrs
            .gamepads()
            .find(|(id, g)| g.name() == virtual_info.name && *id != primary_id && *id != assist_id)
        {
            break id;
        }
        if start.elapsed() >= timeout {
            return Err("Virtual gamepad not found.".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let mut gilrs = Gilrs::new()?;
    let virtual_gamepad = gilrs.gamepad(virtual_id);
    println!(
        "  Virtual: ID: {} - Name: {}",
        virtual_id,
        virtual_gamepad.name()
    );

    // --- 4. Setup Graceful Shutdown (Ctrl+C) ---

    // Convert HashSet to Vec for the 'move' closure
    let restore_paths_vec: Vec<String> = restore_paths.into_iter().collect();
    ctrlc::set_handler(move || {
        println!("\nShutting down.");
        if hide {
            println!("\nRestoring controllers...");
            for path in &restore_paths_vec {
                if let Err(e) = udev_helpers::restore_device(path) {
                    eprintln!("  Failed to restore {}: {}", path, e);
                } else {
                    log::info!("Restored: {}", path);
                }
            }
        }
        std::process::exit(0);
    })?;

    // --- 5. Main Event Loop ---

    println!("\nAssist mode active. Press Ctrl+C to exit.");
    let timeout = Some(Duration::from_millis(1000));
    fn deadzone(_axis: gilrs::Axis) -> f32 {
        0.1
    }

    loop {
        while let Some(event) = gilrs.next_event_blocking(timeout) {
            // Ignore events from devices other than primary and assist
            let other_id = match event.id {
                id if id == primary_id => assist_id,
                id if id == assist_id => primary_id,
                _ => continue,
            };

            // Always get up-to-date gamepad handles from active gilrs instance
            let other_gamepad = gilrs.gamepad(other_id);

            // --- Event Forwarding Logic ---
            let mut events = Vec::with_capacity(2);
            match event.event {
                // --- Digital Buttons ---
                gilrs::EventType::ButtonPressed(button, _)
                | gilrs::EventType::ButtonReleased(button, _) => {
                    if let Some(key) = evdev_helpers::gilrs_button_to_evdev_key(button) {
                        let value = if matches!(event.event, gilrs::EventType::ButtonPressed(..)) {
                            1
                        } else {
                            0
                        };
                        // Only relay if the other gamepad does not have the button pressed
                        let other_pressed = other_gamepad
                            .button_data(button)
                            .map_or(false, |d| d.value() != 0.0);
                        if other_pressed {
                            continue;
                        }
                        events.push(InputEvent::new(evdev::EventType::KEY.0, key.0, value));
                    }
                }

                // --- Analog Triggers / Pressure Buttons ---
                gilrs::EventType::ButtonChanged(button, value, _) => {
                    if let Some(abs_axis) = evdev_helpers::gilrs_button_to_evdev_axis(button) {
                        let mut value = value;
                        let mut button = button;

                        // Identify the Axis Pair (if this is a DPad button)
                        let axis_pair = match button {
                            Button::DPadUp | Button::DPadDown => {
                                Some([Button::DPadUp, Button::DPadDown])
                            }
                            Button::DPadLeft | Button::DPadRight => {
                                Some([Button::DPadLeft, Button::DPadRight])
                            }
                            _ => None,
                        };

                        // Apply Assist/Primary Logic
                        if let Some(pair) = axis_pair {
                            // Helper to check if the *other* gamepad is pressing a specific button
                            let is_other_pressing = |b| {
                                other_gamepad
                                    .button_data(b)
                                    .map_or(false, |d| d.value() > 0.0)
                            };

                            if other_id == assist_id {
                                // CASE A: We are Primary.
                                // If Assist is pressing *anything* on this axis, we are blocked.
                                if pair.iter().any(|&b| is_other_pressing(b)) {
                                    continue;
                                }
                            } else if other_id == primary_id {
                                // CASE B: We are Assist.
                                // If we are releasing (value == 0.0), we must check if Primary is holding a button.
                                if value == 0.0 {
                                    for &b in &pair {
                                        if is_other_pressing(b) {
                                            // Primary is holding this button; adopt its state immediately.
                                            // This effectively turns the "Release" event into a "Press" event
                                            // for the button the Primary is holding.
                                            button = b;
                                            value = 1.0;
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                        // Only relay if greater than other trigger value
                        let other_greater = match button {
                            Button::DPadUp
                            | Button::DPadDown
                            | Button::DPadLeft
                            | Button::DPadRight => false,
                            _ => other_gamepad
                                .button_data(button)
                                .map_or(false, |d| d.value() >= value),
                        };
                        if other_greater {
                            continue;
                        }
                        let scaled_value = match button {
                            // D-pad-as-axis (uncommon, but matches original logic)
                            Button::DPadUp | Button::DPadLeft => {
                                evdev_helpers::scale_stick(value, true)
                            }
                            Button::DPadDown | Button::DPadRight => {
                                evdev_helpers::scale_stick(value, false)
                            }
                            // Analog triggers (LT2/RT2)
                            _ => evdev_helpers::scale_trigger(value),
                        };
                        events.push(InputEvent::new(
                            evdev::EventType::ABSOLUTE.0,
                            abs_axis.0,
                            scaled_value,
                        ));
                    }
                }

                // --- Analog Sticks ---
                gilrs::EventType::AxisChanged(axis, value, _) => {
                    if let Some(abs_axis) = evdev_helpers::gilrs_axis_to_evdev_axis(axis) {
                        // Only relay if not conflicting with assist joysticks
                        let other_pushed = match axis {
                            Axis::LeftStickX | Axis::LeftStickY => {
                                other_gamepad
                                    .axis_data(Axis::LeftStickX)
                                    .map_or(false, |d| d.value().abs() >= deadzone(axis))
                                    || other_gamepad
                                        .axis_data(Axis::LeftStickY)
                                        .map_or(false, |d| d.value().abs() >= deadzone(axis))
                            }
                            Axis::RightStickX | Axis::RightStickY => {
                                other_gamepad
                                    .axis_data(Axis::RightStickX)
                                    .map_or(false, |d| d.value().abs() >= deadzone(axis))
                                    || other_gamepad
                                        .axis_data(Axis::RightStickY)
                                        .map_or(false, |d| d.value().abs() >= deadzone(axis))
                            }
                            _ => false,
                        };
                        if other_pushed && other_id == assist_id {
                            continue;
                        }
                        let scaled_value = match axis {
                            // Invert Y axes
                            Axis::LeftStickY | Axis::RightStickY => {
                                evdev_helpers::scale_stick(value, true)
                            }
                            // X axes
                            _ => evdev_helpers::scale_stick(value, false),
                        };
                        events.push(InputEvent::new(
                            evdev::EventType::ABSOLUTE.0,
                            abs_axis.0,
                            scaled_value,
                        ));
                    }
                }
                _ => {} // Ignore other events (Connected, Disconnected, etc.)
            }

            // If we have events to send, add a SYN_REPORT and emit
            if !events.is_empty() {
                // println!("Relaying Event: {:?}", event.event); // Uncomment for debugging
                events.push(InputEvent::new(evdev::EventType::SYNCHRONIZATION.0, 0, 0));
                virtual_dev.emit(&events)?;
            }
        }
    }
}
