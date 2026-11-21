//! Times the input path: two fake pads through the input thread's mux to
//! a virtual pad, read back as a game would.
//!
//! It first watches the input thread sit idle, then drives one stick axis
//! on the primary and one button on the assist at a fixed rate, and pairs
//! each stick frame sent with the frame that comes out of the virtual
//! pad. One line of results is printed.

#![expect(clippy::print_stdout, reason = "this binary's output is its result")]

use std::fs::File;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Parser;
use ctrlassist_core::{
    Graph, GraphSpec, Input, Layout, Mode, MuxConfig, NodeId, NodeSpec, SourceId, SourceSpec,
    gamepad,
};
use ctrlassist_device::{Command, Identity, InputThread, Pad, Setup, SetupId, VirtualPad};
use ctrlassist_test::FakePad;
use rustix::event::{PollFd, PollFlags, poll};

const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const ABS_X: u16 = 0x00;
const BTN_TR: u16 = 0x137;

#[derive(Parser)]
struct Cli {
    /// Events per second sent on each fake pad.
    #[arg(long, default_value_t = 1000)]
    rate: u64,
    /// Seconds to drive the pads.
    #[arg(long, default_value_t = 10)]
    seconds: u64,
    /// Seconds to watch the idle input thread first.
    #[arg(long, default_value_t = 5)]
    idle_seconds: u64,
    /// Busy threads to run beside the measurement.
    #[arg(long, default_value_t = 0)]
    load: usize,
}

/// How many times the thread named `input` has been scheduled out.
fn input_thread_switches() -> Result<u64> {
    for task in std::fs::read_dir("/proc/self/task")? {
        let task = task?.path();
        if std::fs::read_to_string(task.join("comm"))?.trim() != "input" {
            continue;
        }
        let status = std::fs::read_to_string(task.join("status"))?;
        return Ok(status
            .lines()
            .filter(|line| line.contains("ctxt_switches"))
            .filter_map(|line| line.split_whitespace().nth(1)?.parse::<u64>().ok())
            .sum());
    }
    bail!("no thread named input");
}

fn percentile(sorted: &[u64], fraction: f64) -> u64 {
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[index]
}

fn read(fake: &FakePad) -> Result<Pad> {
    let file = File::open(fake.event_node()).context("opening a fake pad")?;
    Pad::from_fd(file.into()).context("reading a fake pad")
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut primary =
        FakePad::new("Bench Primary", 0x1209, 0x10f0).context("creating a fake pad")?;
    let mut assist = FakePad::new("Bench Assist", 0x1209, 0x10f1).context("creating a fake pad")?;
    let sink = VirtualPad::create(Identity::Generic).context("creating the virtual pad")?;
    let _ = std::process::Command::new("udevadm").arg("settle").status();
    let mut game =
        Pad::from_fd(File::open(sink.event_node())?.into()).context("reading the virtual pad")?;

    let source = || SourceSpec {
        deadzone: 0.1,
        layout: Layout::gamepad(),
    };
    let graph = Graph::build(GraphSpec {
        sources: vec![source(), source()],
        nodes: vec![NodeSpec::Mux {
            inputs: vec![Input::Source(SourceId(0)), Input::Source(SourceId(1))],
            config: MuxConfig::new(Mode::Priority, &Layout::gamepad()),
        }],
        sinks: vec![Input::Node(NodeId(0), 0)],
    })
    .context("building the graph")?;
    let setup = Setup::new(
        graph,
        vec![Some(read(&primary)?), Some(read(&assist)?)],
        vec![sink],
    );
    let (input, _reports) = InputThread::spawn()?;
    input.send(Command::Start(SetupId(0), Box::new(setup)))?;

    // The reader stamps every frame in which the driven stick moved.
    let (received_tx, received_rx) = mpsc::channel::<Instant>();
    thread::spawn(move || {
        let mut last = 0.0;
        loop {
            if poll(&mut [PollFd::new(&game, PollFlags::IN)], None).is_err() {
                return;
            }
            let now = Instant::now();
            let read = game.read(|state| {
                let value = state.get(gamepad::LEFT_X);
                if value != last {
                    last = value;
                    let _ = received_tx.send(now);
                }
            });
            if read.is_err() {
                return;
            }
        }
    });

    // Idle: nothing is sent; count the input thread's wakeups.
    let before = input_thread_switches()?;
    thread::sleep(Duration::from_secs(cli.idle_seconds));
    let idle_wakeups = (input_thread_switches()? - before) as f64 / cli.idle_seconds.max(1) as f64;

    for _ in 0..cli.load {
        thread::spawn(|| {
            loop {
                std::hint::spin_loop();
            }
        });
    }

    // Driven: stick frames on the primary, button edges on the assist.
    // The stick values stay clear of the deadzone and of each other, so
    // every frame changes what the game reads.
    let values = [8000, 16000, 24000, 32000, -8000, -16000, -24000, -32000];
    let period = Duration::from_nanos(1_000_000_000 / cli.rate.max(1));
    let total = cli.rate * cli.seconds;
    let mut sent = Vec::with_capacity(total as usize);
    let mut next = Instant::now();
    for index in 0..total {
        sent.push(Instant::now());
        primary.frame(&[(EV_ABS, ABS_X, values[(index % 8) as usize])])?;
        assist.frame(&[(EV_KEY, BTN_TR, (index % 2) as i32)])?;
        next += period;
        thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    thread::sleep(Duration::from_millis(500));

    let received: Vec<Instant> = received_rx.try_iter().collect();
    if received.len() != sent.len() {
        bail!(
            "sent {} frames and read back {}",
            sent.len(),
            received.len()
        );
    }
    let mut latencies: Vec<u64> = sent
        .iter()
        .zip(&received)
        .map(|(sent, received)| received.saturating_duration_since(*sent).as_micros() as u64)
        .collect();
    latencies.sort_unstable();
    println!(
        "realtime={} load={} rate={} frames={} p50={}us p99={}us p99.9={}us max={}us input_thread_idle_wakeups_per_s={idle_wakeups:.1}",
        input.is_realtime(),
        cli.load,
        cli.rate,
        sent.len(),
        percentile(&latencies, 0.50),
        percentile(&latencies, 0.99),
        percentile(&latencies, 0.999),
        latencies[latencies.len() - 1],
    );
    Ok(())
}
