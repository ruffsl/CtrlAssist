use crate::demux::DemuxRumbleTarget;
use crate::mux::MuxRumbleTarget;
use clap::{Parser, Subcommand, ValueEnum};
use log::info;
use serde::{Deserialize, Serialize};
use signal_hook::consts::signal::{SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use std::error::Error;

mod core;
mod demux;
mod mux;
mod mux2;
mod tray;
mod utils;

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

    /// Multiplex controllers using new graph-based architecture (experimental).
    Mux2(Mux2Args),

    /// Demultiplex one controller to multiple virtual gamepads.
    Demux(DemuxArgs),

    /// Launch system tray app for graphical control.
    Tray,
}

#[derive(clap::Args, Debug)]
struct MuxArgs {
    /// Primary controller ID (see 'list' command).
    #[arg(long, default_value_t = 0)]
    primary: usize,

    /// Assist controller ID (see 'list' command).
    #[arg(long, default_value_t = 1)]
    assist: usize,

    /// Hide primary and assist controllers.
    #[arg(long, value_enum, default_value_t = HideType::default())]
    hide: HideType,

    /// Spoof target for virtual device.
    #[arg(long, value_enum, default_value_t = SpoofTarget::default())]
    spoof: SpoofTarget,

    /// Mode type for combining controllers.
    #[arg(long, value_enum, default_value_t = crate::mux::modes::MuxModeType::default())]
    mode: crate::mux::modes::MuxModeType,

    /// Rumble target for virtual device.
    #[arg(long, value_enum, default_value_t = MuxRumbleTarget::default())]
    rumble: MuxRumbleTarget,
}

#[derive(clap::Args, Debug)]
struct Mux2Args {
    /// Primary controller ID (see 'list' command).
    #[arg(long, default_value_t = 0)]
    primary: usize,

    /// Assist controller ID (see 'list' command).
    #[arg(long, default_value_t = 1)]
    assist: usize,

    /// Hide primary and assist controllers.
    #[arg(long, value_enum, default_value_t = HideType::default())]
    hide: HideType,

    /// Spoof target for virtual device.
    #[arg(long, value_enum, default_value_t = SpoofTarget::default())]
    spoof: SpoofTarget,

    /// Mode type for combining controllers.
    #[arg(long, value_enum, default_value_t = crate::core::nodes::mux::MuxModeType::default())]
    mode: crate::core::nodes::mux::MuxModeType,
}

#[derive(clap::Args, Debug)]
struct DemuxArgs {
    /// Primary controller ID (see 'list' command).
    #[arg(long, default_value_t = 0)]
    primary: usize,

    /// Number of virtual devices to create.
    #[arg(long, default_value_t = 2)]
    virtuals: usize,

    /// Hide primary controller.
    #[arg(long, value_enum, default_value_t = HideType::default())]
    hide: HideType,

    /// Spoof target for virtual devices.
    #[arg(long, value_enum, default_value_t = SpoofTarget::default())]
    spoof: SpoofTarget,

    /// Mode type for routing controller.
    #[arg(long, value_enum, default_value_t = crate::demux::modes::DemuxModeType::default())]
    mode: crate::demux::modes::DemuxModeType,

    /// Rumble target for force feedback.
    #[arg(long, value_enum, default_value_t = DemuxRumbleTarget::default())]
    rumble: DemuxRumbleTarget,
}

#[derive(ValueEnum, Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub enum HideType {
    #[default]
    None,
    Steam,
    System,
}

#[derive(ValueEnum, Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub enum SpoofTarget {
    Assist,
    #[default]
    None,
    Primary,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();
    let cli = Cli::parse();
    match cli.command {
        Commands::List => list_gamepads(),
        Commands::Mux(args) => run_mux(args),
        Commands::Mux2(args) => run_mux2(args),
        Commands::Demux(args) => run_demux(args),
        Commands::Tray => tray::run_tray().await,
    }
}

fn list_gamepads() -> Result<(), Box<dyn Error>> {
    let gilrs =
        crate::utils::gilrs::new_gilrs().map_err(|e| format!("Failed to init Gilrs: {e}"))?;
    let resources = crate::utils::gilrs::discover_gamepad_resources(&gilrs);
    let mut found = false;
    for (id, gamepad) in gilrs.gamepads() {
        println!(
            "({}) {} @ {}",
            id,
            gamepad.name(),
            resources[&id].path.display()
        );
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

    let gilrs =
        crate::utils::gilrs::new_gilrs().map_err(|e| format!("Failed to init Gilrs: {e}"))?;
    let resources = crate::utils::gilrs::discover_gamepad_resources(&gilrs);

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

    // Start mux
    let config = crate::mux::manager::MuxConfig {
        primary_id: p_id,
        assist_id: a_id,
        mode: args.mode,
        hide: args.hide,
        spoof: args.spoof,
        rumble: args.rumble,
    };

    use std::sync::mpsc;
    let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();

    let mux_thread = std::thread::spawn(move || {
        let mux_handle =
            crate::mux::manager::start_mux(gilrs, config).expect("Failed to start mux");
        let _ = shutdown_rx.recv();
        mux_handle.0.shutdown();
    });

    // Handle both SIGINT and SIGTERM
    let mut signals = Signals::new([SIGINT, SIGTERM])?;
    std::thread::spawn(move || {
        if let Some(_sig) = signals.forever().next() {
            println!("\nShutting down...");
            let _ = shutdown_tx.send(());
        }
    });

    info!("Mux Active. Press Ctrl+C to exit.");
    println!("Mux Active. Press Ctrl+C to exit.");

    let _ = mux_thread.join();
    Ok(())
}

fn run_mux2(args: Mux2Args) -> Result<(), Box<dyn Error>> {
    if args.primary == args.assist {
        return Err("Primary and Assist controllers must be separate devices.".into());
    }

    let gilrs =
        crate::utils::gilrs::new_gilrs().map_err(|e| format!("Failed to init Gilrs: {e}"))?;
    let resources = crate::utils::gilrs::discover_gamepad_resources(&gilrs);

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

    // Get spoof info if needed
    let spoof_info = match args.spoof {
        SpoofTarget::Primary => Some(crate::utils::evdev::VirtualGamepadInfo::from(
            &gilrs.gamepad(p_id),
        )),
        SpoofTarget::Assist => Some(crate::utils::evdev::VirtualGamepadInfo::from(
            &gilrs.gamepad(a_id),
        )),
        SpoofTarget::None => None,
    };

    // Need to drop gilrs before starting mux2 (which creates its own)
    drop(gilrs);

    // Start mux2
    let mux2_handle =
        crate::mux2::manager::start_mux2(p_id, a_id, args.mode, args.hide, spoof_info)?;

    // Handle both SIGINT and SIGTERM
    let shutdown = mux2_handle.shutdown.clone();
    let mut signals = Signals::new([SIGINT, SIGTERM])?;
    std::thread::spawn(move || {
        if let Some(_sig) = signals.forever().next() {
            println!("\nShutting down...");
            shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    });

    info!("Mux2 Active (graph-based). Press Ctrl+C to exit.");
    println!("Mux2 Active (graph-based). Press Ctrl+C to exit.");

    let _ = mux2_handle.input_thread.join();
    Ok(())
}

fn run_demux(args: DemuxArgs) -> Result<(), Box<dyn Error>> {
    let gilrs =
        crate::utils::gilrs::new_gilrs().map_err(|e| format!("Failed to init Gilrs: {e}"))?;
    let resources = crate::utils::gilrs::discover_gamepad_resources(&gilrs);

    // Identify primary resource
    let p_id = resources
        .keys()
        .find(|&&id| usize::from(id) == args.primary)
        .copied()
        .ok_or(format!("Primary ID {} not found", args.primary))?;

    let primary_msg = format!(
        "Primary: ({}) {} @ {}",
        p_id,
        resources[&p_id].name,
        resources[&p_id].path.display()
    );
    info!("{}", primary_msg);
    println!("{}", primary_msg);

    // Start demux
    let config = crate::demux::manager::DemuxConfig {
        primary_id: p_id,
        virtuals: args.virtuals,
        mode: args.mode,
        hide: args.hide,
        spoof: args.spoof,
        rumble: args.rumble,
    };

    use std::sync::mpsc;
    let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();

    let demux_thread = std::thread::spawn(move || {
        let demux_handle =
            crate::demux::manager::start_demux(gilrs, config).expect("Failed to start demux");
        let _ = shutdown_rx.recv();
        demux_handle.0.shutdown();
    });

    // Handle both SIGINT and SIGTERM
    let mut signals = Signals::new([SIGINT, SIGTERM])?;
    std::thread::spawn(move || {
        if let Some(_sig) = signals.forever().next() {
            println!("\nShutting down...");
            let _ = shutdown_tx.send(());
        }
    });

    info!("Demux Active. Press Ctrl+C to exit.");
    println!("Demux Active. Press Ctrl+C to exit.");

    let _ = demux_thread.join();
    Ok(())
}
