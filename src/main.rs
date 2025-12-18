use clap::{Parser, Subcommand, ValueEnum};
use evdev::uinput::VirtualDevice;
use evdev::{Device, EventType, FFEffect, InputEvent};
use ff_helpers::process_ff_event;
use gilrs::{GamepadId, Gilrs};
use log::{error, info};
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

mod evdev_helpers;
mod ff_helpers;
mod mux_modes;
mod udev_helpers;

const NEXT_EVENT_TIMEOUT: Duration = Duration::from_millis(1000);
const RETRY_INTERVAL: Duration = Duration::from_millis(50);
const VIRTUAL_DEV_TIMEOUT: Duration = Duration::from_secs(2);

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
    let count = gilrs
        .gamepads()
        .map(|(id, gamepad)| {
            let msg = format!("({}) {}", id, gamepad.name());
            info!("{}", msg);
            println!("{}", msg);
        })
        .count();
    if count == 0 {
        println!("  No controllers found.");
    }
    Ok(())
}

fn run_mux(args: MuxArgs) -> Result<(), Box<dyn Error>> {
    if args.primary == args.assist {
        return Err("Primary and Assist controllers must be separate devices.".into());
    }

    let gilrs = Gilrs::new().map_err(|e| format!("Failed to init Gilrs: {e}"))?;

    let find_id = |target_idx: usize| -> Result<GamepadId, Box<dyn Error>> {
        gilrs
            .gamepads()
            .find(|(id, _)| usize::from(*id) == target_idx)
            .map(|(id, _)| id)
            .ok_or_else(|| format!("Controller ID {} not found", target_idx).into())
    };

    let primary_id = find_id(args.primary)?;
    let assist_id = find_id(args.assist)?;
    let primary_gp = gilrs.gamepad(primary_id);
    let assist_gp = gilrs.gamepad(assist_id);
    let primary_name = primary_gp.name().to_string();
    let assist_name = assist_gp.name().to_string();
    let primary_path = udev_helpers::resolve_event_path(primary_id)
        .ok_or("Could not find filesystem path for primary device")?;
    let assist_path = udev_helpers::resolve_event_path(assist_id)
        .ok_or("Could not find filesystem path for assist device")?;

    let primary_msg = format!(
        "Primary: ({}) {} @ {}",
        primary_id,
        primary_name,
        primary_path.display()
    );
    info!("{}", primary_msg);
    println!("{}", primary_msg);
    let assist_msg = format!(
        "Assist:  ({}) {} @ {}",
        assist_id,
        assist_name,
        assist_path.display()
    );
    info!("{}", assist_msg);
    println!("{}", assist_msg);

    let mut restore_paths = HashSet::new();
    if args.hide {
        info!("Hiding controllers (requires root)...");
        udev_helpers::restrict_gamepad_devices(&primary_gp, &mut restore_paths)?;
        udev_helpers::restrict_gamepad_devices(&assist_gp, &mut restore_paths)?;
        if restore_paths.is_empty() {
            return Err("Devices could not be hidden. Check permissions.".into());
        }
    }

    let virtual_info = match args.spoof {
        SpoofTarget::Primary => evdev_helpers::VirtualGamepadInfo::from(&gilrs.gamepad(primary_id)),
        SpoofTarget::Assist => evdev_helpers::VirtualGamepadInfo::from(&gilrs.gamepad(assist_id)),
        SpoofTarget::None => evdev_helpers::VirtualGamepadInfo {
            name: "CtrlAssist Virtual Gamepad".to_string(),
            vendor_id: None,
            product_id: None,
        },
    };

    let mut vf_dev = evdev_helpers::create_virtual_gamepad(&virtual_info)?;
    let vi_dev = wait_for_virtual_device(&mut vf_dev)?;

    let virtual_msg = format!("Virtual: ({}) {}", "#", virtual_info.name);
    info!("{}", virtual_msg);
    println!("{}", virtual_msg);

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();
    let restore_paths_vec: Vec<String> = restore_paths.into_iter().collect();

    ctrlc::set_handler(move || {
        println!("\nShutting down...");
        r.store(false, Ordering::SeqCst);
        if !restore_paths_vec.is_empty() {
            println!("Restoring controllers...");
            for path in &restore_paths_vec {
                if let Err(e) = udev_helpers::restore_device(path) {
                    error!("Failed to restore {}: {}", path, e);
                } else {
                    info!("Restored: {}", path);
                }
            }
        }
        std::process::exit(0);
    })?;

    let mux_msg = "Mux Active. Press Ctrl+C to exit.";
    info!("{}", mux_msg);
    println!("{}", mux_msg);

    let mode_type = args.mode.clone();
    let input_thread = thread::spawn(move || {
        run_input_loop(vi_dev, mode_type, primary_id, assist_id);
    });

    let phys_paths = match args.rumble {
        RumbleTarget::Primary => vec![(primary_id, primary_path.clone(), primary_name.clone())],
        RumbleTarget::Assist => vec![(assist_id, assist_path.clone(), assist_name.clone())],
        RumbleTarget::Both => vec![
            (primary_id, primary_path.clone(), primary_name.clone()),
            (assist_id, assist_path.clone(), assist_name.clone()),
        ],
        RumbleTarget::None => vec![],
    };
    let ff_thread = thread::spawn(move || {
        run_ff_loop(vf_dev, phys_paths, running);
    });

    let mut errors = Vec::new();
    if input_thread.join().is_err() {
        error!("Input thread panicked");
        errors.push("Input thread panicked");
    }
    if ff_thread.join().is_err() {
        error!("Force feedback thread panicked");
        errors.push("Force feedback thread panicked");
    }
    if !errors.is_empty() {
        return Err(errors.join("; ").into());
    }
    Ok(())
}

fn wait_for_virtual_device(v_dev: &mut VirtualDevice) -> Result<Device, Box<dyn Error>> {
    let v_path = v_dev
        .enumerate_dev_nodes_blocking()?
        .filter_map(Result::ok)
        .find(|pb| pb.to_string_lossy().contains("event"))
        .ok_or("Could not find virtual device path")?;

    let start = Instant::now();
    while start.elapsed() < VIRTUAL_DEV_TIMEOUT {
        match Device::open(&v_path) {
            Ok(dev) => return Ok(dev),
            Err(_) => thread::sleep(RETRY_INTERVAL),
        }
    }
    Err("Timed out waiting for virtual device creation".into())
}

fn run_input_loop(mut v_dev: Device, mode: mux_modes::ModeType, p_id: GamepadId, a_id: GamepadId) {
    let mut gilrs = match Gilrs::new() {
        Ok(g) => g,
        Err(e) => {
            error!("Input Thread Gilrs init failed: {}", e);
            return;
        }
    };

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

struct PhysicalFFDev {
    dev: Device,
    effect_map: HashMap<i16, FFEffect>,
}

fn run_ff_loop(
    mut v_dev: VirtualDevice,
    phys_paths: Vec<(GamepadId, std::path::PathBuf, String)>,
    running: Arc<AtomicBool>,
) {
    let mut phys_devs: Vec<PhysicalFFDev> = Vec::new();
    for (id, path, name) in &phys_paths {
        match Device::open(path) {
            Ok(dev) => {
                if dev.supported_ff().is_some() {
                    phys_devs.push(PhysicalFFDev {
                        dev,
                        effect_map: HashMap::new(),
                    });
                } else {
                    log::warn!(
                        "Controller ({}) '{}' at '{}' does not support force feedback (FF)",
                        id,
                        name,
                        path.display()
                    );
                }
            }
            Err(e) => {
                log::warn!(
                    "Failed to open controller ({}) '{}' at '{}': {}",
                    id,
                    name,
                    path.display(),
                    e
                );
            }
        }
    }

    info!("FF Thread started.");

    while running.load(Ordering::Relaxed) {
        let events: Vec<_> = match v_dev.fetch_events() {
            Ok(iter) => iter.collect(),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => vec![],
            Err(e) => {
                error!("Error fetching FF events: {}", e);
                vec![]
            }
        };

        for event in events {
            process_ff_event(event, &mut v_dev, &mut phys_devs);
        }
    }
}
