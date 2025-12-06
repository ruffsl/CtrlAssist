use clap::{Parser, Subcommand};
use evdev::InputEvent;
use gilrs::{GamepadId, Gilrs};
use std::collections::HashSet;
use std::error::Error;
use std::time::Duration;

mod evdev_helpers;
mod log_setup;
mod mux_modes;
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

        /// Hide primary and assist controllers
        #[arg(long, default_value_t = false)]
        hide: bool,

        /// Spoof type for virtual device.
        #[arg(long, value_enum, default_value_t = SpoofType::Primary)]
        spoof: SpoofType,

        /// Mode type for combining controllers.
        #[arg(long, value_enum, default_value_t = mux_modes::ModeType::Priority)]
        mode: mux_modes::ModeType,
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
            mode,
        } => {
            mux_gamepads(*primary, *assist, *hide, spoof.clone(), mode.clone())?;
        }
    }
    Ok(())
}

/// List all detected controllers.
fn list_gamepads() -> Result<(), Box<gilrs::Error>> {
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
    mode: mux_modes::ModeType,
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

    // Select the mode handler using enum-based factory
    use mux_modes::create_mux_mode;
    let mut mux_mode = create_mux_mode(mode);

    loop {
        while let Some(event) = gilrs.next_event_blocking(timeout) {
            // Only process events from primary or assist
            if event.id != primary_id && event.id != assist_id {
                continue;
            }
            if let Some(events) = mux_mode.handle_event(&event, primary_id, assist_id, &gilrs)
                && !events.is_empty()
            {
                // Always add SYN_REPORT
                let mut events = events;
                events.push(InputEvent::new(evdev::EventType::SYNCHRONIZATION.0, 0, 0));
                virtual_dev.emit(&events)?;
            }
        }
    }
}
