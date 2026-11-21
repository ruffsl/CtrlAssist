//! Times a target between fake pads and its virtual pad.
//!
//! Two fake pads are made through uinput. The target (our loop or the
//! legacy binary) is started on them as a child process. The bench first
//! watches the target sit idle, then drives one stick axis on the primary
//! and one button on the assist at a fixed rate, and pairs each stick frame
//! it sent with the frame that comes out of the target's virtual pad.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, Device, EventType, InputEvent, InputId,
    KeyCode, UinputAbsSetup,
};

use crate::Target;

const PRIMARY_NAME: &str = "Loop Fake Primary";
const ASSIST_NAME: &str = "Loop Fake Assist";

/// A pad gilrs accepts as a gamepad: the standard buttons and two sticks.
fn fake_pad(name: &str, product: u16) -> Result<(VirtualDevice, PathBuf)> {
    let keys = AttributeSet::from_iter([
        KeyCode::BTN_SOUTH,
        KeyCode::BTN_EAST,
        KeyCode::BTN_NORTH,
        KeyCode::BTN_WEST,
        KeyCode::BTN_TL,
        KeyCode::BTN_TR,
        KeyCode::BTN_SELECT,
        KeyCode::BTN_START,
        KeyCode::BTN_MODE,
        KeyCode::BTN_THUMBL,
        KeyCode::BTN_THUMBR,
    ]);
    let stick = AbsInfo::new(0, -32768, 32767, 0, 0, 0);
    let mut builder = VirtualDevice::builder()?
        .name(name)
        .input_id(InputId::new(BusType::BUS_USB, 0x1209, product, 1))
        .with_keys(&keys)?;
    for axis in [
        AbsoluteAxisCode::ABS_X,
        AbsoluteAxisCode::ABS_Y,
        AbsoluteAxisCode::ABS_RX,
        AbsoluteAxisCode::ABS_RY,
    ] {
        builder = builder.with_absolute_axis(&UinputAbsSetup::new(axis, stick))?;
    }
    let mut pad = builder.build()?;
    let node = pad
        .enumerate_dev_nodes_blocking()?
        .filter_map(Result::ok)
        .find(|node| node.to_string_lossy().contains("/event"))
        .context("the fake pad has no event node")?;
    Ok((pad, node))
}

/// Starts the target on the two fake pads and returns it with the event
/// node of its virtual pad, read from the line it prints.
fn start_target(
    target: Target,
    legacy_bin: Option<&Path>,
    primary: &Path,
    assist: &Path,
    rt: bool,
    exe: Option<&Path>,
) -> Result<(Child, PathBuf)> {
    let mut command = match target {
        Target::Ours => {
            let mut command = match exe {
                Some(exe) => Command::new(exe),
                None => Command::new(std::env::current_exe()?),
            };
            command.arg("mux").arg("--primary").arg(primary).arg("--assist").arg(assist);
            if rt {
                command.arg("--rt");
            }
            command
        }
        Target::Legacy => {
            let bin = legacy_bin.context("--legacy-bin is required for the legacy target")?;
            // The legacy CLI takes gilrs's pad numbers, which `list` prints.
            let listing = Command::new(bin).arg("list").output()?;
            let listing = String::from_utf8_lossy(&listing.stdout).into_owned();
            let id_of = |name: &str| -> Result<String> {
                listing
                    .lines()
                    .find(|line| line.contains(name))
                    .and_then(|line| line.split(['(', ')']).nth(1))
                    .map(str::to_string)
                    .with_context(|| format!("legacy `list` did not show {name}:\n{listing}"))
            };
            let mut command = Command::new(bin);
            command
                .arg("mux")
                .arg("--primary")
                .arg(id_of(PRIMARY_NAME)?)
                .arg("--assist")
                .arg(id_of(ASSIST_NAME)?);
            command
        }
    };
    let mut child = command.stdout(Stdio::piped()).spawn()?;
    let stdout = BufReader::new(child.stdout.take().context("no stdout from the target")?);
    for line in stdout.lines() {
        let line = line?;
        if let Some(node) = line.strip_prefix("Virtual:").and_then(|rest| rest.rsplit("@ ").next()) {
            return Ok((child, PathBuf::from(node.trim())));
        }
    }
    bail!("the target exited without naming its virtual pad");
}

/// Opens a node udev may still be creating: the kernel announces a new
/// device before its node and permissions are in place.
fn open_when_ready(node: &Path) -> Result<Device> {
    let mut last = None;
    for _ in 0..200 {
        match Device::open(node) {
            Ok(device) => return Ok(device),
            Err(e) => last = Some(e),
        }
        thread::sleep(Duration::from_millis(10));
    }
    Err(last.expect("the loop ran at least once because its bound is a nonzero constant").into())
}

/// What a process has used so far.
struct Usage {
    /// User plus system CPU ticks.
    ticks: u64,
    /// Context switches summed over every thread.
    switches: u64,
    /// Context switches of the thread named `input`, if there is one.
    input_switches: Option<u64>,
    /// Resident memory in kilobytes.
    resident_kb: u64,
}

fn usage(pid: u32) -> Result<Usage> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let fields: Vec<&str> = stat.rsplit(") ").next().unwrap_or("").split(' ').collect();
    let ticks = fields[11].parse::<u64>()? + fields[12].parse::<u64>()?;
    let field = |status: &str, name: &str| -> u64 {
        status
            .lines()
            .filter(|line| line.starts_with(name))
            .filter_map(|line| line.split_whitespace().nth(1)?.parse::<u64>().ok())
            .sum()
    };
    let mut switches = 0;
    let mut input_switches = None;
    for task in std::fs::read_dir(format!("/proc/{pid}/task"))? {
        let status = std::fs::read_to_string(task?.path().join("status"))?;
        let count = field(&status, "voluntary_ctxt_switches") + field(&status, "nonvoluntary_ctxt_switches");
        switches += count;
        if status.lines().next().is_some_and(|line| line.ends_with("\tinput")) {
            input_switches = Some(count);
        }
    }
    let status = std::fs::read_to_string(format!("/proc/{pid}/status"))?;
    Ok(Usage {
        ticks,
        switches,
        input_switches,
        resident_kb: field(&status, "VmRSS"),
    })
}

fn percentile(sorted: &[u64], fraction: f64) -> u64 {
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[index]
}

pub(crate) fn run(
    target: Target,
    legacy_bin: Option<&Path>,
    rate: u64,
    seconds: u64,
    idle_seconds: u64,
    load: usize,
    rt: bool,
    exe: Option<&Path>,
    with: &[String],
) -> Result<()> {
    let (mut primary, primary_node) = fake_pad(PRIMARY_NAME, 0x10f0)?;
    let (mut assist, assist_node) = fake_pad(ASSIST_NAME, 0x10f1)?;
    // Let udev finish with the new nodes before gilrs or the loop opens them.
    thread::sleep(Duration::from_millis(500));

    let (mut child, sink_node) = start_target(target, legacy_bin, &primary_node, &assist_node, rt, exe)?;
    let pid = child.id();
    thread::sleep(Duration::from_millis(500));

    let mut companions = Vec::new();
    for command in with {
        let mut parts = command.split(' ');
        let program = parts.next().context("an empty --with command")?;
        companions.push(Command::new(program).args(parts).stderr(Stdio::null()).spawn()?);
    }
    if !companions.is_empty() {
        thread::sleep(Duration::from_secs(1));
    }

    // The reader stamps every frame that carries the driven axis.
    let (received_tx, received_rx) = mpsc::channel::<Instant>();
    let mut sink = match open_when_ready(&sink_node) {
        Ok(sink) => sink,
        Err(e) => {
            let state = child.try_wait()?;
            bail!("opening the virtual pad {}: {e}; target state: {state:?}", sink_node.display());
        }
    };
    thread::spawn(move || {
        crate::make_realtime(20);
        let mut has_axis = false;
        loop {
            let Ok(events) = sink.fetch_events() else { return };
            let now = Instant::now();
            for event in events {
                if event.event_type() == EventType::ABSOLUTE && event.code() == AbsoluteAxisCode::ABS_X.0 {
                    has_axis = true;
                } else if event.event_type() == EventType::SYNCHRONIZATION && has_axis {
                    has_axis = false;
                    if received_tx.send(now).is_err() {
                        return;
                    }
                }
            }
        }
    });

    // Idle: nothing is sent; count the target's wakeups.
    let before = usage(pid)?;
    thread::sleep(Duration::from_secs(idle_seconds));
    let after = usage(pid)?;
    let idle_wakeups = (after.switches - before.switches) as f64 / idle_seconds as f64;
    let input_idle_wakeups = match (before.input_switches, after.input_switches) {
        (Some(before), Some(after)) => format!("{:.1}", (after - before) as f64 / idle_seconds as f64),
        _ => "n/a".to_string(),
    };

    let mut burner = if load > 0 {
        Some(Command::new(std::env::current_exe()?).arg("burn").arg(load.to_string()).spawn()?)
    } else {
        None
    };
    if burner.is_some() {
        thread::sleep(Duration::from_millis(500));
    }

    // Driven: stick frames on the primary, button edges on the assist. The
    // stick values stay well clear of the center and of each other so no
    // filter in the target drops one.
    crate::make_realtime(20);
    let values = [8000, 16000, 24000, 32000, -8000, -16000, -24000, -32000];
    let period = Duration::from_nanos(1_000_000_000 / rate);
    let total = rate * seconds;
    let mut sent = Vec::with_capacity(total as usize);
    let ticks_before = usage(pid)?.ticks;
    let started = Instant::now();
    let mut next = started;
    for index in 0..total {
        let value = values[(index % 8) as usize];
        sent.push(Instant::now());
        primary.emit(&[InputEvent::new(EventType::ABSOLUTE.0, AbsoluteAxisCode::ABS_X.0, value)])?;
        assist.emit(&[InputEvent::new(EventType::KEY.0, KeyCode::BTN_TR.0, (index % 2) as i32)])?;
        next += period;
        thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    let elapsed = started.elapsed();
    thread::sleep(Duration::from_millis(500));
    let end = usage(pid)?;
    let ticks_after = end.ticks;
    for companion in &mut companions {
        companion.kill()?;
        companion.wait()?;
    }

    if let Some(burner) = &mut burner {
        burner.kill()?;
        burner.wait()?;
    }
    child.kill()?;
    child.wait()?;

    let received: Vec<Instant> = received_rx.try_iter().collect();
    let mut latencies: Vec<u64> = sent
        .iter()
        .zip(&received)
        .map(|(sent, received)| received.saturating_duration_since(*sent).as_micros() as u64)
        .collect();
    latencies.sort_unstable();
    // SAFETY: sysconf with a valid name has no preconditions.
    let ticks_per_second = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    let cpu = (ticks_after - ticks_before) as f64 / ticks_per_second / elapsed.as_secs_f64() * 100.0;

    if latencies.is_empty() {
        bail!("no frames came out of the target");
    }
    println!(
        "target={target:?} rt={rt} load={load} rate={rate} sent={} received={} \
         p50={}us p99={}us p99.9={}us max={}us cpu={cpu:.1}% idle_wakeups_per_s={idle_wakeups:.1} \
         input_thread_idle_wakeups_per_s={input_idle_wakeups} rss={}kB",
        sent.len(),
        received.len(),
        percentile(&latencies, 0.50),
        percentile(&latencies, 0.99),
        percentile(&latencies, 0.999),
        latencies[latencies.len() - 1],
        end.resident_kb,
    );
    Ok(())
}
