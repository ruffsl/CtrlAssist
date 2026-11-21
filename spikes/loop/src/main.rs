//! The input loop spike: one thread blocked in epoll over two pads and the
//! virtual pad's feedback requests, and a bench that times it, or the legacy
//! 0.4.0 binary, between fake pads and the virtual pad.

mod bench;
mod mux;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Clone, Copy, ValueEnum, Debug, PartialEq)]
enum Target {
    Ours,
    Legacy,
}

#[derive(Subcommand)]
enum Command {
    /// Forward two pads to one virtual pad from a single epoll-blocked thread.
    Mux {
        #[arg(long)]
        primary: PathBuf,
        #[arg(long)]
        assist: PathBuf,
        /// Run the loop under SCHED_FIFO.
        #[arg(long)]
        rt: bool,
    },
    /// Time a target between fake pads and its virtual pad.
    Bench {
        #[arg(long, value_enum)]
        target: Target,
        /// Path to the legacy `ctrlassist` binary, for `--target legacy`.
        #[arg(long)]
        legacy_bin: Option<PathBuf>,
        /// Events per second emitted on each fake pad.
        #[arg(long, default_value_t = 1000)]
        rate: u64,
        #[arg(long, default_value_t = 10)]
        seconds: u64,
        /// Seconds to watch the idle target before any events.
        #[arg(long, default_value_t = 5)]
        idle_seconds: u64,
        /// Busy threads to run beside the measurement.
        #[arg(long, default_value_t = 0)]
        load: usize,
        /// Run our loop under SCHED_FIFO (ignored for legacy).
        #[arg(long)]
        rt: bool,
        /// Another binary to run as the target, taking the same `mux`
        /// arguments and printing the same line as ours.
        #[arg(long)]
        exe: Option<PathBuf>,
        /// Commands to run beside the target once it is up, such as
        /// clients of it. Each is split on spaces.
        #[arg(long)]
        with: Vec<String>,
    },
    /// Spin on this many threads until killed.
    Burn { threads: usize },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Mux { primary, assist, rt } => mux::run(&primary, &assist, rt),
        Command::Bench {
            target,
            legacy_bin,
            rate,
            seconds,
            idle_seconds,
            load,
            rt,
            exe,
            with,
        } => bench::run(
            target,
            legacy_bin.as_deref(),
            rate,
            seconds,
            idle_seconds,
            load,
            rt,
            exe.as_deref(),
            &with,
        ),
        Command::Burn { threads } => {
            for _ in 1..threads {
                std::thread::spawn(|| {
                    loop {
                        std::hint::spin_loop();
                    }
                });
            }
            loop {
                std::hint::spin_loop();
            }
        }
    }
}

/// Puts the calling thread under SCHED_FIFO at a low real-time priority.
/// Returns whether the kernel allowed it.
pub(crate) fn make_realtime(priority: i32) -> bool {
    let param = libc::sched_param {
        sched_priority: priority,
    };
    // SAFETY: `param` is a valid sched_param for the duration of the call;
    // pid 0 means the calling thread.
    unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &raw const param) == 0 }
}
