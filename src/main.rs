use clap::{Parser, Subcommand};
use evdev::InputEvent; // EventType}; // Added EventType
// use gilrs::ff::{BaseEffect, BaseEffectType, EffectBuilder, Replay}; // Added FF imports
use gilrs::{GamepadId, Gilrs};
use std::collections::HashSet;
use std::error::Error;
use std::sync::{Arc, Mutex}; // Added for thread safety
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
        /// Primary controller ID (see 'list' command).
        #[arg(short, long, default_value_t = 0)]
        primary: usize,

        /// Assist controller ID (see 'list' command).
        #[arg(short, long, default_value_t = 1)]
        assist: usize,

        /// Hide primary and assist controllers.
        #[arg(long, default_value_t = false)]
        hide: bool,

        /// Spoof type for virtual device.
        #[arg(long, value_enum, default_value_t = SpoofType::default())]
        spoof: SpoofType,

        /// Mode type for combining controllers.
        #[arg(long, value_enum, default_value_t = mux_modes::ModeType::default())]
        mode: mux_modes::ModeType,

        /// Enable rumble/force feedback for primary controller.
        #[arg(long, default_value_t = true)]
        rumble_primary: bool,

        /// Enable rumble/force feedback for assist controller.
        #[arg(long, default_value_t = true)]
        rumble_assist: bool,
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
            rumble_primary,
            rumble_assist,
        } => {
            mux_gamepads(
                *primary,
                *assist,
                *hide,
                spoof.clone(),
                mode.clone(),
                *rumble_primary,
                *rumble_assist,
            )?;
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
    _rumble_primary: bool,
    _rumble_assist: bool,
) -> Result<(), Box<dyn Error>> {
    // --- 1. Setup and Validation ---
    if primary_usize == assist_usize {
        return Err("Primary and Assist controllers must be separate devices.".into());
    }

    // Find connected controllers.
    let mut gilrs = Gilrs::new()?;
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
    }
    let primary_id =
        primary_opt.ok_or_else(|| format!("Primary controller ID {} not found.", primary_usize))?;
    let assist_id =
        assist_opt.ok_or_else(|| format!("Assist controller ID {} not found.", assist_usize))?;

    let primary_gamepad = gilrs.gamepad(primary_id);
    let assist_gamepad = gilrs.gamepad(assist_id);

    println!("Connected controllers:");
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

    // --- 2. Hide Controllers (Optional) ---
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

    // --- 3. Create Virtual Gamepad ---
    use evdev_helpers::VirtualGamepadInfo;
    let virtual_info = match spoof {
        SpoofType::Primary => VirtualGamepadInfo::from(&primary_gamepad),
        SpoofType::Assist => VirtualGamepadInfo::from(&assist_gamepad),
        SpoofType::None => VirtualGamepadInfo {
            name: "CtrlAssist Virtual Gamepad".to_string(),
            vendor_id: None,
            product_id: None,
        },
    };


    // Create the virtual device
    let mut virtual_dev = evdev_helpers::create_virtual_gamepad(&virtual_info)?;

    // Wait for the virtual device to appear in /dev/input and get its event node path
    let start = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(2);
    let mut virtual_id_opt = None;
    let mut virtual_event_path = None;
    while start.elapsed() < timeout {
        gilrs = Gilrs::new()?;
        if let Some((id, _)) = gilrs
            .gamepads()
            .find(|(id, g)| g.name() == virtual_info.name && *id != primary_id && *id != assist_id)
        {
            virtual_id_opt = Some(id);
        }
        // Use evdev's enumerate_dev_nodes_blocking to get the event node
        if virtual_event_path.is_none() {
            if let Ok(mut nodes) = virtual_dev.enumerate_dev_nodes_blocking() {
                while let Some(Ok(path)) = nodes.next() {
                    if path.file_name().map(|n| n.to_string_lossy().starts_with("event")).unwrap_or(false) {
                        virtual_event_path = Some(path);
                        break;
                    }
                }
            }
        }
        if virtual_id_opt.is_some() && virtual_event_path.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let virtual_id = virtual_id_opt.ok_or("Could not find virtual device ID")?;
    let virtual_gamepad = gilrs.gamepad(virtual_id);
    let virtual_event_path = virtual_event_path.ok_or("Could not find virtual event node path")?;
    println!(
        "  Virtual: ID: {} - Name: {} - Event: {}",
        virtual_id,
        virtual_gamepad.name(),
        virtual_event_path.display()
    );

    // --- 4. Prepare Shared State ---

    // --- 5. Setup Graceful Shutdown ---
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

    // --- 6. Main Threads ---
    use std::thread;
    println!("\nAssist mode active. Press Ctrl+C to exit.");

    // INPUT THREAD: Proxies Real -> Virtual
    let input_event_path = virtual_event_path.clone();
    let input_thread = thread::spawn(move || {
        use mux_modes::create_mux_mode;
        let mut mux_mode = create_mux_mode(mode);
        let timeout = Some(Duration::from_millis(1000));
        // Open the event node for writing input events
        let mut v_dev = match evdev::Device::open(&input_event_path) {
            Ok(dev) => dev,
            Err(e) => {
                log::error!("Failed to open virtual event node for input: {}", e);
                return;
            }
        };
        loop {
            while let Some(event) = gilrs.next_event_blocking(timeout) {
                // Only process events from primary or assist
                if event.id != primary_id && event.id != assist_id {
                    continue;
                }
                if let Some(mut events) =
                    mux_mode.handle_event(&event, primary_id, assist_id, &gilrs)
                    && !events.is_empty()
                {
                    events.push(InputEvent::new(evdev::EventType::SYNCHRONIZATION.0, 0, 0));
                    if let Err(e) = v_dev.send_events(&events) {
                        log::error!("Emit failed: {}", e);
                    }
                }
            }
        }
    });

    // FF THREAD: Proxies Virtual -> Real
    // Only the FF thread owns the VirtualDevice
    // Let's assume the device event path is known for primary and assist
    let primary_path = "/dev/input/event256"; // Replace X with actual event number later
    let assist_path = "/dev/input/event29"; // Replace Y with actual event number later
    let physical_dev_paths = vec![primary_path, assist_path];

    // Bookkeeping struct for each physical device
    use std::collections::HashMap;
    struct PhysicalFFDev {
        dev: evdev::Device,
        effect_map: HashMap<i16, i16>, // virtual_effect_id -> physical_effect_id
    }

    // Collect only devices that support FF, and move them into the thread
    let mut ff_devs: Vec<PhysicalFFDev> = physical_dev_paths
        .iter()
        .filter_map(|path| evdev::Device::open(path).ok())
        .filter(|dev| dev.supported_ff().is_some())
        .map(|dev| PhysicalFFDev { dev, effect_map: HashMap::new() })
        .collect();

    let ff_thread = thread::spawn(move || {
        use evdev::{
            EventSummary, FFStatusCode, InputEvent, UInputCode,
        };
        use std::collections::BTreeSet;
        let mut ids: BTreeSet<u16> = (0..16).collect();

        const STOPPED: i32 = FFStatusCode::FF_STATUS_STOPPED.0 as i32;
        const PLAYING: i32 = FFStatusCode::FF_STATUS_PLAYING.0 as i32;
        let mut v_dev = virtual_dev;
        loop {
            let events: Vec<InputEvent> = match v_dev.fetch_events() {
                Ok(evts) => evts.collect(),
                Err(e) => {
                    log::error!("Failed to fetch events from virtual device: {}", e);
                    continue;
                }
            };
            for event in events {
                println!("FF Event: {:?}", event);
                match event.destructure() {
                    EventSummary::UInput(event, UInputCode::UI_FF_UPLOAD, ..) => {
                        let mut event = match v_dev.process_ff_upload(event) {
                            Ok(ev) => ev,
                            Err(e) => {
                                log::error!("Failed to process FF upload: {}", e);
                                continue;
                            }
                        };
                        let id = ids.iter().next().copied();
                        match id {
                            Some(id) => {
                                ids.remove(&id);
                                event.set_effect_id(id as i16);
                                event.set_retval(0);
                            }
                            None => {
                                event.set_retval(-1);
                            }
                        }
                        let virt_id = event.effect_id();
                        println!("    upload effect {:?}", event.effect());
                        for phys_dev in &mut ff_devs {
                            let effect = event.effect();
                            match phys_dev.dev.upload_ff_effect(effect) {
                                Ok(real_id) => {
                                    let phys_id = real_id.id() as i16;
                                    phys_dev.effect_map.insert(virt_id, phys_id);
                                    println!(
                                        "    mapped virtual effect ID {} to physical effect ID {}",
                                        virt_id, phys_id
                                    );
                                }
                                Err(e) => {
                                    log::error!("Failed to upload FF effect: {}", e);
                                }
                            }
                        }
                    }
                    EventSummary::UInput(event, UInputCode::UI_FF_ERASE, ..) => {
                        let event = match v_dev.process_ff_erase(event) {
                            Ok(ev) => ev,
                            Err(e) => {
                                log::error!("Failed to process FF erase: {}", e);
                                continue;
                            }
                        };
                        let virt_id = event.effect_id();
                        ids.insert(virt_id as u16);
                        println!("    erase effect ID = {}", virt_id);
                        for phys_dev in &mut ff_devs {
                            phys_dev.effect_map.remove(&(virt_id as i16));
                        }
                    }
                    EventSummary::ForceFeedback(.., effect_id, STOPPED) => {
                        println!("    stopped effect ID = {}", effect_id.0);
                        for phys_dev in &mut ff_devs {
                            let virt_id = effect_id.0 as i16;
                            if let Some(&phys_id) = phys_dev.effect_map.get(&virt_id) {
                                let play_event = evdev::InputEvent::new(
                                    evdev::EventType::FORCEFEEDBACK.0,
                                    phys_id as u16,
                                    0, // 1 = play, 0 = stop
                                );
                                let _ = phys_dev.dev.send_events(&[play_event]);
                                println!(
                                    "    Stopping virtual effect ID {} to physical effect ID {}",
                                    virt_id, phys_id
                                );
                            }
                        }
                    }
                    EventSummary::ForceFeedback(.., effect_id, PLAYING) => {
                        println!("    playing effect ID = {}", effect_id.0);
                        for phys_dev in &mut ff_devs {
                            let virt_id = effect_id.0 as i16;
                            if let Some(&phys_id) = phys_dev.effect_map.get(&virt_id) {
                                let play_event = evdev::InputEvent::new(
                                    evdev::EventType::FORCEFEEDBACK.0,
                                    phys_id as u16,
                                    1, // 1 = play, 0 = stop
                                );
                                let _ = phys_dev.dev.send_events(&[play_event]);
                                println!(
                                    "    Playing virtual effect ID {} to physical effect ID {}",
                                    virt_id, phys_id
                                );
                            }
                        }
                    }
                    _ => {
                        println!("  event = {:?}", event);
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    });

    let _ = input_thread.join();
    let _ = ff_thread.join();
    Ok(())
}
