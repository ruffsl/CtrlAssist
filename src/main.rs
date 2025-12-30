use clap::{Parser, Subcommand, ValueEnum};
use gilrs::Gilrs;
use log::info;
use serde::{Deserialize, Serialize};
use std::error::Error;

mod evdev_helpers;
mod ff_helpers;
mod gilrs_helper;
mod mux_manager;
mod mux_modes;
mod mux_runtime;
mod tui;
mod tray;
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
    Mux(MuxArgs),

    /// Launch system tray app for graphical control.
    Tray,

    /// Launch terminal UI for interactive control.
    Tui,
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
    #[arg(long, value_enum, default_value_t = mux_modes::ModeType::default())]
    mode: mux_modes::ModeType,

    /// Rumble target for virtual device.
    #[arg(long, value_enum, default_value_t = RumbleTarget::default())]
    rumble: RumbleTarget,
}

#[derive(ValueEnum, Clone, Debug, Default, Serialize, Deserialize)]
pub enum HideType {
    #[default]
    None,
    Steam,
    System,
}

#[derive(ValueEnum, Clone, Debug, Default, Serialize, Deserialize)]
pub enum SpoofTarget {
    Primary,
    Assist,
    #[default]
    None,
}

#[derive(ValueEnum, Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub enum RumbleTarget {
    Primary,
    Assist,
    #[default]
    Both,
    None,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    env_logger::init();
    let cli = Cli::parse();
    match cli.command {
        Commands::List => list_gamepads(),
        Commands::Mux(args) => run_mux(args),
        Commands::Tray => tray::run_tray().await,
        Commands::Tui => tui::run_tui().await,
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
    let resources = gilrs_helper::discover_gamepad_resources(&gilrs);

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

    // Start mux using the shared helper
    let config = mux_manager::MuxConfig {
        primary_id: p_id,
        assist_id: a_id,
        mode: args.mode,
        hide: args.hide,
        spoof: args.spoof,
        rumble: args.rumble,
    };

    use std::sync::mpsc;
    let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();

    // Spawn mux in a thread, so we can join it in main
    let mux_thread = std::thread::spawn(move || {
        let mux_handle = mux_manager::start_mux(gilrs, config).expect("Failed to start mux");
        // Wait for shutdown signal (blocks efficiently)
        let _ = shutdown_rx.recv();
        mux_handle.0.shutdown();
    });

    // Setup Ctrl+C handler to send shutdown signal
    ctrlc::set_handler(move || {
        println!("\nShutting down...");
        // Ignore error if already sent
        let _ = shutdown_tx.send(());
    })?;

    info!("Mux Active. Press Ctrl+C to exit.");
    println!("Mux Active. Press Ctrl+C to exit.");

    // Wait for mux thread to finish
    let _ = mux_thread.join();
    Ok(())
}
