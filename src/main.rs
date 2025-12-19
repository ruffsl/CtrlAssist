use clap::{Parser, Subcommand, ValueEnum};
use evdev::uinput::VirtualDevice;
use evdev::{Device, EventType, FFEffect, InputEvent};
use gilrs::{GamepadId, Gilrs};
use gilrs_helper::GamepadResource;
use log::{error, info, warn};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

mod evdev_helpers;
mod ff_helpers;
mod gilrs_helper;
mod mux_modes;
mod udev_helpers;

const NEXT_EVENT_TIMEOUT: Duration = Duration::from_millis(1000);

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
    Mux(MuxArgs),
}
#[derive(clap::Args, Debug)]
struct MuxArgs {
    /// Primary controller ID (see 'list' command).
    #[arg(short, long, default_value_t = 0)]
    primary: usize,

    /// Assist controller ID (see 'list' command).
    #[arg(short, long, default_value_t = 1)]
    assist: usize,

    /// Hide primary and assist controllers.
    #[arg(long, default_value_t = false)]
    hide: bool,

    /// Spoof target for virtual device.
    #[arg(long, value_enum, default_value_t = SpoofTarget::default())]
    spoof: SpoofTarget,

    /// Mode type for combining controllers.
    #[arg(long, value_enum, default_value_t = mux_modes::ModeType::default())]
    mode: mux_modes::ModeType,

    /// Rumble target for virtual device.
    #[arg(long, value_enum, default_value_t = RumbleTarget::default())]
    rumble: RumbleTarget,
}
#[derive(ValueEnum, Clone, Debug, Default)]
pub enum SpoofTarget {
    #[default]
    Primary,
    Assist,
    None,
}
#[derive(ValueEnum, Clone, Debug, Default)]
pub enum RumbleTarget {
    Primary,
    Assist,
    #[default]
    Both,
    None,
}

fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();
    let cli = Cli::parse();
    match cli.command {
        Commands::List => list_gamepads(),
        Commands::Mux(args) => run_mux(args),
    }
}

fn list_gamepads() -> Result<(), Box<dyn Error>> {
    let gilrs = Gilrs::new().map_err(|e| format!("Failed to init Gilrs: {e}"))?;
    let mut found = false;
    for (id, gamepad) in gilrs.gamepads() {
        println!("({}) {}", id, gamepad.name());
        found = true;
    }
    if !found {
        println!("  No controllers found.");
    }
    Ok(())
}

fn run_mux(args: MuxArgs) -> Result<(), Box<dyn Error>> {
    if args.primary == args.assist {
        return Err("Primary and Assist controllers must be separate devices.".into());
    }

    let gilrs = Gilrs::new().map_err(|e| format!("Failed to init Gilrs: {e}"))?;
    let mut resources = gilrs_helper::discover_gamepad_resources(&gilrs);

    // Identify primary and assist resources
    let p_id = resources
        .keys()
        .find(|&&id| usize::from(id) == args.primary)
        .copied()
        .ok_or(format!("Primary ID {} not found", args.primary))?;
    let a_id = resources
        .keys()
        .find(|&&id| usize::from(id) == args.assist)
        .copied()
        .ok_or(format!("Assist ID {} not found", args.assist))?;

    let primary_msg = format!(
        "Primary: ({}) {} @ {}",
        p_id,
        resources[&p_id].name,
        resources[&p_id].path.display()
    );
    info!("{}", primary_msg);
    println!("{}", primary_msg);
    let assist_msg = format!(
        "Assist:  ({}) {} @ {}",
        a_id,
        resources[&a_id].name,
        resources[&a_id].path.display()
    );
    info!("{}", assist_msg);
    println!("{}", assist_msg);

    // Handle hiding via udev
    let mut restore_paths = HashSet::new();
    if args.hide {
        info!("Hiding controllers (requires root)...");
        udev_helpers::restrict_gamepad_devices(&gilrs.gamepad(p_id), &mut restore_paths)?;
        udev_helpers::restrict_gamepad_devices(&gilrs.gamepad(a_id), &mut restore_paths)?;
        if restore_paths.is_empty() {
            return Err("Devices could not be hidden. Check permissions.".into());
        }
    }

    // Setup Virtual Device
    let virtual_info = match args.spoof {
        SpoofTarget::Primary => evdev_helpers::VirtualGamepadInfo::from(&gilrs.gamepad(p_id)),
        SpoofTarget::Assist => evdev_helpers::VirtualGamepadInfo::from(&gilrs.gamepad(a_id)),
        SpoofTarget::None => evdev_helpers::VirtualGamepadInfo {
            name: "CtrlAssist Virtual Gamepad".into(),
            vendor_id: None,
            product_id: None,
        },
    };

    let mut v_uinput = evdev_helpers::create_virtual_gamepad(&virtual_info)?;
    let v_resource = gilrs_helper::wait_for_virtual_device(&mut v_uinput)?;

    let virtual_msg = format!(
        "Virtual: ({}) {} @ {}",
        "#",
        v_resource.name,
        v_resource.path.display()
    );
    info!("{}", virtual_msg);
    println!("{}", virtual_msg);

    // Setup Shutdown Signal
    let running = Arc::new(AtomicBool::new(true));
    let r_signal = running.clone();
    let restore_vec: Vec<String> = restore_paths.into_iter().collect();
    ctrlc::set_handler(move || {
        println!("\nShutting down...");
        r_signal.store(false, Ordering::SeqCst);
        for path in &restore_vec {
            let _ = udev_helpers::restore_device(path);
        }
        std::process::exit(0);
    })?;

    // Prepare FF targets by moving Device ownership
    let mut ff_targets = Vec::new();
    let rumble_ids = match args.rumble {
        RumbleTarget::Primary => vec![p_id],
        RumbleTarget::Assist => vec![a_id],
        RumbleTarget::Both => vec![p_id, a_id],
        RumbleTarget::None => vec![],
    };

    for id in rumble_ids {
        if let Some(res) = resources.remove(&id) {
            ff_targets.push(res);
        }
    }

    // Spawn Threads
    let mode_type = args.mode;
    thread::spawn(move || run_input_loop(gilrs, v_resource.device, mode_type, p_id, a_id));
    thread::spawn(move || run_ff_loop(v_uinput, ff_targets, running));

    let mux_msg = "Mux Active. Press Ctrl+C to exit.";
    info!("{}", mux_msg);
    println!("{}", mux_msg);
    thread::park(); // Keep main thread alive
    Ok(())
}

fn run_input_loop(
    mut gilrs: Gilrs,
    mut v_dev: Device,
    mode: mux_modes::ModeType,
    p_id: GamepadId,
    a_id: GamepadId,
) {
    let mut mux_mode = mux_modes::create_mux_mode(mode);

    loop {
        while let Some(event) = gilrs.next_event_blocking(Some(NEXT_EVENT_TIMEOUT)) {
            if event.id != p_id && event.id != a_id {
                continue;
            }
            if let Some(mut out_events) = mux_mode.handle_event(&event, p_id, a_id, &gilrs)
                && !out_events.is_empty()
            {
                out_events.push(InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0));
                if let Err(e) = v_dev.send_events(&out_events) {
                    error!("Failed to write input events: {}", e);
                }
            }
        }
    }
}

pub struct PhysicalFFDev {
    pub dev: Device,
    pub effect_map: HashMap<i16, FFEffect>,
}

fn run_ff_loop(
    mut v_uinput: VirtualDevice,
    targets: Vec<GamepadResource>,
    running: Arc<AtomicBool>,
) {
    let mut phys_devs: Vec<PhysicalFFDev> = targets
        .into_iter()
        .filter_map(|res| {
            if res.device.supported_ff().is_some() {
                Some(PhysicalFFDev {
                    dev: res.device,
                    effect_map: HashMap::new(),
                })
            } else {
                warn!("Device {} does not support FF", res.name);
                None
            }
        })
        .collect();

    info!("FF Thread started.");

    while running.load(Ordering::Relaxed) {
        let events: Vec<_> = match v_uinput.fetch_events() {
            Ok(iter) => iter.collect(),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => vec![],
            Err(e) => {
                error!("Error fetching FF events: {}", e);
                vec![]
            }
        };

        for event in events {
            ff_helpers::process_ff_event(event, &mut v_uinput, &mut phys_devs);
        }
    }
}
