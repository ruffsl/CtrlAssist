//! Times the input path: two fake pads through the input thread's mux to
//! a virtual pad, read back as a game would. The input thread is this
//! process's own, or with `--daemon` the daemon's, run as a child with a
//! priority profile over the same two pads, so the two can be compared.
//!
//! It first watches the input thread sit idle, then drives one stick axis
//! on the primary and one button on the assist at a fixed rate, and pairs
//! each stick frame sent with the frame that comes out of the virtual
//! pad. One line of results is printed.

#![expect(clippy::print_stdout, reason = "this binary's output is its result")]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
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
    /// The built `ctrlassist` command: time the path through its daemon,
    /// run as a child of this process, in place of an input thread of
    /// this process's own.
    #[arg(long, value_name = "COMMAND")]
    daemon: Option<PathBuf>,
}

/// The thread named `input` of the process `pid`, as a directory under
/// `/proc`.
fn input_thread(pid: u32) -> Result<PathBuf> {
    for task in std::fs::read_dir(format!("/proc/{pid}/task"))? {
        let task = task?.path();
        if std::fs::read_to_string(task.join("comm"))?.trim() == "input" {
            return Ok(task);
        }
    }
    bail!("process {pid} has no thread named input");
}

/// How many times that thread has been scheduled out.
fn input_thread_switches(pid: u32) -> Result<u64> {
    let status = std::fs::read_to_string(input_thread(pid)?.join("status"))?;
    Ok(status
        .lines()
        .filter(|line| line.contains("ctxt_switches"))
        .filter_map(|line| line.split_whitespace().nth(1)?.parse::<u64>().ok())
        .sum())
}

/// Whether that thread runs under a real-time policy, as the kernel
/// reports it: the policy is the 41st field of the thread's `stat`, and
/// the fields after the name in brackets are counted from the third.
fn input_thread_is_realtime(pid: u32) -> Result<bool> {
    let stat = std::fs::read_to_string(input_thread(pid)?.join("stat"))?;
    let policy = stat
        .rsplit_once(')')
        .and_then(|(_, rest)| rest.split_whitespace().nth(38))
        .context("the thread's stat has no policy field")?;
    // 1 is first-in-first-out, 2 round-robin; both are real-time.
    Ok(matches!(policy, "1" | "2"))
}

/// The daemon as a child of this process, with directories of its own,
/// stopped and cleared away when dropped.
struct Daemon {
    command: PathBuf,
    child: Child,
    directory: PathBuf,
}

impl Daemon {
    /// Starts the daemon and waits until it answers.
    fn start(command: &Path) -> Result<Self> {
        let directory =
            std::env::temp_dir().join(format!("ctrlassist-bench-{}", std::process::id()));
        std::fs::create_dir_all(&directory)?;
        let child = std::process::Command::new(command)
            .arg("--socket")
            .arg(directory.join("zenoh.sock"))
            .args(["service", "daemon", "--state-dir"])
            .arg(directory.join("state"))
            .arg("--config-dir")
            .arg(directory.join("config"))
            // No hiding service: the bench's pads are its own to drive.
            .arg("--hider-socket")
            .arg(directory.join("no-hider.sock"))
            // The bench's pads are fake ones, which a daemon takes only
            // when asked to.
            .arg("--test-pads")
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("starting {}", command.display()))?;
        let daemon = Self {
            command: command.to_path_buf(),
            child,
            directory,
        };
        for _ in 0..200 {
            if daemon.ask(&["status"]).is_ok() {
                return Ok(daemon);
            }
            thread::sleep(Duration::from_millis(25));
        }
        bail!("the daemon never answered");
    }

    /// Runs the command against this daemon, and returns what it printed.
    fn ask(&self, arguments: &[&str]) -> Result<String> {
        let output = std::process::Command::new(&self.command)
            .arg("--socket")
            .arg(self.directory.join("zenoh.sock"))
            .args(arguments)
            .output()?;
        if !output.status.success() {
            bail!(
                "ctrlassist {}: {}",
                arguments.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Has the daemon run a priority profile over the two fake pads, and
    /// returns its virtual pad's event node.
    fn run(&self, primary: (u16, u16), assist: (u16, u16)) -> Result<PathBuf> {
        let profile = self.directory.join("bench.toml");
        std::fs::write(
            &profile,
            format!(
                "preset = \"priority\"\nprimary = \"model:{:04x}:{:04x}\"\nassist = \"model:{:04x}:{:04x}\"\n",
                primary.0, primary.1, assist.0, assist.1
            ),
        )?;
        let profile = profile
            .to_str()
            .context("the temporary directory isn't UTF-8")?;
        self.ask(&["profile", "put", profile])?;
        self.ask(&["start", "bench"])?;
        let setups = self.ask(&["setups", "--json"])?;
        if !setups.contains("\"phase\":\"running\"") {
            bail!("the daemon's setup isn't running: {setups}");
        }
        let node = setups
            .split_once("\"node\":\"")
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(node, _)| PathBuf::from(node))
            .context("the daemon's setup names no virtual pad")?;
        Ok(node)
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // Asked to stop as systemd asks, so it lets its pads go.
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .status();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
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
    // The path under test: this process's own input thread, or a
    // daemon's. Either is held until the measurement is over.
    let (own, daemon);
    let (sink_node, pid) = if let Some(command) = &cli.daemon {
        own = None;
        let started = Daemon::start(command)?;
        let node = started.run((0x1209, 0x10f0), (0x1209, 0x10f1))?;
        let pid = started.child.id();
        daemon = Some(started);
        (node, pid)
    } else {
        daemon = None;
        let sink = VirtualPad::create(Identity::Generic).context("creating the virtual pad")?;
        let node = sink.event_node().to_path_buf();
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
        let (input, reports) = InputThread::spawn()?;
        input.send(Command::Start(SetupId(0), Box::new(setup)))?;
        own = Some((input, reports));
        (node, std::process::id())
    };
    let _ = std::process::Command::new("udevadm").arg("settle").status();
    let mut game =
        Pad::from_fd(File::open(&sink_node)?.into()).context("reading the virtual pad")?;

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
    let before = input_thread_switches(pid)?;
    thread::sleep(Duration::from_secs(cli.idle_seconds));
    let idle_wakeups =
        (input_thread_switches(pid)? - before) as f64 / cli.idle_seconds.max(1) as f64;

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
        "path={} realtime={} load={} rate={} frames={} p50={}us p99={}us p99.9={}us max={}us input_thread_idle_wakeups_per_s={idle_wakeups:.1}",
        if daemon.is_some() { "daemon" } else { "own" },
        input_thread_is_realtime(pid)?,
        cli.load,
        cli.rate,
        sent.len(),
        percentile(&latencies, 0.50),
        percentile(&latencies, 0.99),
        percentile(&latencies, 0.999),
        latencies[latencies.len() - 1],
    );
    drop(own);
    drop(daemon);
    Ok(())
}
