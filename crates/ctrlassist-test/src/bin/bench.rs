//! Times the input path: two fake pads through the input thread's mux to
//! a virtual pad, read back as a game would. The input thread is this
//! process's own, or with `--daemon` the daemon's, run as a child with a
//! priority profile over the same two pads, so the two can be compared.
//!
//! It first watches the input thread sit idle, then drives one stick axis
//! on the primary and one button on the assist at a fixed rate, and pairs
//! each stick frame sent with the frame that comes out of the virtual
//! pad. One line of results is printed.
//!
//! With `--motion` the two fake pads are DualSenses whose raw nodes the
//! input thread reads as well, each report carrying a new reading of the
//! motion sensors, and the virtual pad is a DualSense that writes every
//! one: what a pad's motion costs the path. It needs the fake pads' raw
//! nodes, which a seat's user is given where a PlayStation pad's is
//! (Steam's udev rules give it), and has no daemon to hide them.

#![expect(clippy::print_stdout, reason = "this binary's output is its result")]

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::Parser;
use ctrlassist_core::{
    Graph, GraphSpec, Input, Layout, Mode, Motion, MuxConfig, NodeId, NodeSpec, PadState, SourceId,
    SourceSpec, gamepad,
};
use ctrlassist_device::{Command, Identity, InputThread, Pad, Setup, SetupId, VirtualPad};
use ctrlassist_test::{FakePad, raw_node_of, synthetic_dualsense};
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
    #[arg(long, value_name = "COMMAND", conflicts_with = "motion")]
    daemon: Option<PathBuf>,
    /// Two fake DualSenses read for motion as well, into a virtual
    /// DualSense, each report a new reading of the sensors.
    #[arg(long)]
    motion: bool,
}

/// What the input thread has spent on a CPU, in the kernel's clock
/// ticks: the 14th and 15th fields of its `stat`.
fn input_thread_ticks(pid: u32) -> Result<u64> {
    let stat = std::fs::read_to_string(input_thread(pid)?.join("stat"))?;
    let mut fields = stat
        .rsplit_once(')')
        .map(|(_, rest)| rest.split_whitespace().skip(11))
        .context("the thread's stat has no name")?;
    let mut ticks = || fields.next()?.parse::<u64>().ok();
    Ok(ticks().context("no user time")? + ticks().context("no system time")?)
}

/// A fake DualSense held by a thread of its own, which answers whatever
/// reads its raw node and, once told how fast and how many, writes that
/// many reports, each a new reading of its sensors, and says when it
/// wrote each.
struct Driven {
    node: PathBuf,
    go: Sender<(u64, u64)>,
    sent: Receiver<Vec<Instant>>,
}

impl Driven {
    /// `state` is what the pad holds at its `index`th report.
    fn new(last: u8, state: fn(u64) -> PadState) -> Result<Self> {
        let data = synthetic_dualsense(true);
        let mut pad = VirtualPad::dualsense(&data, [0x02, 0xca, 0x7e, 0x00, 0xbe, last])
            .context("creating a fake DualSense")?;
        let node = pad.event_node().to_path_buf();
        let (go, told) = mpsc::channel::<(u64, u64)>();
        let (done, sent) = mpsc::channel();
        thread::spawn(move || {
            // Answers the pad's readers until `until`, or for a
            // millisecond.
            let serve = |pad: &mut VirtualPad, until: Option<Instant>| {
                let left = until.map_or(Duration::from_millis(1), |until| {
                    until.saturating_duration_since(Instant::now())
                });
                let wait = rustix::event::Timespec {
                    tv_sec: left.as_secs() as i64,
                    tv_nsec: i64::from(left.subsec_nanos()),
                };
                let ready = poll(&mut [PollFd::new(&*pad, PollFlags::IN)], Some(&wait));
                ready.is_ok() && pad.serve(|_| Ok(())).is_ok()
            };
            let (rate, total) = loop {
                match told.try_recv() {
                    Ok(told) => break told,
                    Err(mpsc::TryRecvError::Disconnected) => return,
                    Err(mpsc::TryRecvError::Empty) => {}
                }
                if !serve(&mut pad, None) {
                    return;
                }
            };
            let period = Duration::from_nanos(1_000_000_000 / rate.max(1));
            let mut stamps = Vec::with_capacity(total as usize);
            let mut next = Instant::now();
            for index in 0..total {
                while Instant::now() < next {
                    if !serve(&mut pad, Some(next)) {
                        return;
                    }
                }
                stamps.push(Instant::now());
                if pad.write(&state(index)).is_err() {
                    return;
                }
                next += period;
            }
            let _ = done.send(stamps);
            // Held, and answered, until the measurement is over.
            while !matches!(told.try_recv(), Err(mpsc::TryRecvError::Disconnected)) {
                if !serve(&mut pad, None) {
                    return;
                }
            }
        });
        Ok(Self { node, go, sent })
    }

    /// The pad as the input thread reads it: its event node, and its raw
    /// node for its motion.
    fn read(&self) -> Result<Pad> {
        let mut pad =
            Pad::from_fd(File::open(&self.node)?.into()).context("reading a fake DualSense")?;
        let raw = raw_node_of(&self.node).context("a fake DualSense has no raw node")?;
        let raw = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&raw)
            .with_context(|| format!("opening {}, a fake DualSense's raw node, which a seat's user is given where a PlayStation pad's is", raw.display()))?;
        pad.set_raw(raw.into())
            .context("reading a fake DualSense's raw node")?;
        Ok(pad)
    }
}

/// The driven stick's places, clear of the deadzone and of each other in
/// the eight bits a DualSense reports a stick in.
const PLACES: [f32; 8] = [0.25, 0.5, 0.75, 1.0, -0.25, -0.5, -0.75, -1.0];

/// A new reading of the sensors, 4 ms after the last, that differs from
/// the one before it.
fn reading(index: u64) -> Motion {
    let turn = (index % 7) as f32;
    Motion::new(
        [turn, 10.0 - turn, -5.0],
        [0.0, 1.0, 0.0],
        index as u32 + 1,
        4000,
    )
}

fn primary_state(index: u64) -> PadState {
    PadState::NEUTRAL
        .with(gamepad::LEFT_X, PLACES[(index % 8) as usize])
        .with_motion(reading(index))
}

fn assist_state(index: u64) -> PadState {
    PadState::NEUTRAL
        .with(gamepad::RIGHT_BUMPER, (index % 2) as f32)
        .with_motion(reading(index))
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
                "mode = \"priority\"\nprimary = \"model:{:04x}:{:04x}\"\nassist = \"model:{:04x}:{:04x}\"\n",
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
    // The pads driven: two through uinput, or two fake DualSenses.
    let (mut plain, moving) = if cli.motion {
        let moving = (
            Driven::new(1, primary_state)?,
            Driven::new(2, assist_state)?,
        );
        (None, Some(moving))
    } else {
        let primary =
            FakePad::new("Bench Primary", 0x1209, 0x10f0).context("creating a fake pad")?;
        let assist = FakePad::new("Bench Assist", 0x1209, 0x10f1).context("creating a fake pad")?;
        (Some((primary, assist)), None)
    };
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
        let sink = if cli.motion {
            let data = synthetic_dualsense(true);
            VirtualPad::dualsense(&data, ctrlassist_device::own_address(0x00be_0000))
        } else {
            VirtualPad::create(Identity::Generic)
        }
        .context("creating the virtual pad")?;
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
        let pads = match (&plain, &moving) {
            (Some((primary, assist)), _) => vec![Some(read(primary)?), Some(read(assist)?)],
            (_, Some((primary, assist))) => vec![Some(primary.read()?), Some(assist.read()?)],
            (None, None) => bail!("no pads were made"),
        };
        let setup = Setup::new(graph, pads, vec![sink]);
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
    let ticks = input_thread_ticks(pid)?;
    let started = Instant::now();
    let sent = if let Some((primary, assist)) = &moving {
        for pad in [primary, assist] {
            pad.go
                .send((cli.rate, total))
                .context("a fake DualSense has gone")?;
        }
        let sent = primary.sent.recv().context("a fake DualSense has gone")?;
        assist.sent.recv().context("a fake DualSense has gone")?;
        sent
    } else {
        let mut sent = Vec::with_capacity(total as usize);
        let mut next = Instant::now();
        for index in 0..total {
            sent.push(Instant::now());
            if let Some((primary, assist)) = &mut plain {
                primary.frame(&[(EV_ABS, ABS_X, values[(index % 8) as usize])])?;
                assist.frame(&[(EV_KEY, BTN_TR, (index % 2) as i32)])?;
            }
            next += period;
            thread::sleep(next.saturating_duration_since(Instant::now()));
        }
        sent
    };
    // How much of one CPU the input thread took while driven.
    let spent =
        (input_thread_ticks(pid)? - ticks) as f64 / rustix::param::clock_ticks_per_second() as f64;
    let cpu = 100.0 * spent / started.elapsed().as_secs_f64();
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
        "path={} motion={} realtime={} load={} rate={} frames={} p50={}us p99={}us p99.9={}us max={}us input_thread_cpu={cpu:.1}% input_thread_idle_wakeups_per_s={idle_wakeups:.1}",
        if daemon.is_some() { "daemon" } else { "own" },
        cli.motion,
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
    drop(moving);
    Ok(())
}
