//! Spike: one real DualSense's motion sensors and touchpad carried to a
//! virtual DualSense, beside its buttons, sticks and triggers. See the
//! README for what it answers and how to run it.

mod report;
mod sys;
mod virtual_pad;

use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::path::{Path, PathBuf};

use anyhow::{Context, anyhow, bail};
use clap::{Parser, ValueEnum};
use ctrlassist_core::Level;
use ctrlassist_device::{DualSenseData, Pad, PadError, own_address};
use ctrlassist_hider::{Client, SOCKET_PATH};
use rustix::event::{PollFd, PollFlags, poll};

use report::{CALIBRATION, CALIBRATION_SIZE, Calibration, Clock, Motion, Reader, Touch};
use virtual_pad::VirtualDualSense;

const SONY: u16 = 0x054c;
/// The DualSense, and the DualSense Edge, whose report is the same.
const DUALSENSES: [u16; 2] = [0x0ce6, 0x0df2];

#[derive(Parser, Debug)]
#[command(about = "Carries one DualSense's motion and touchpad to a virtual DualSense")]
struct Args {
    /// The pad's event node. The first DualSense connected if not given.
    #[arg(long)]
    pad: Option<PathBuf>,
    /// How the pad is kept from games: by the installed hiding service,
    /// which also hands over its raw node, or not at all (a grab of its
    /// event node only, and its raw node opened by path, so games and
    /// Steam see two DualSenses).
    #[arg(long, value_enum, default_value_t = Hide::Service)]
    hide: Hide,
    /// Where the virtual pad's motion comes from: the pad, or nowhere
    /// (lying flat and still, as the daemon's virtual pad has it now).
    #[arg(long, value_enum, default_value_t = Source::Pad)]
    motion: Source,
    /// How the pad's sensor counts become the virtual pad's.
    #[arg(long, value_enum, default_value_t = Calibrate::Sdl)]
    calibration: Calibrate,
    /// What the virtual pad's sensor clock follows.
    #[arg(long, value_enum, default_value_t = Tick::Pad)]
    clock: Tick,
    /// Where the virtual pad's touchpad, contacts and click, comes from.
    #[arg(long, value_enum, default_value_t = Source::Pad)]
    touch: Source,
    /// Writes every report's motion and touch to this file, as CSV.
    #[arg(long)]
    record: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
enum Hide {
    Service,
    None,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
enum Source {
    /// The pad itself.
    Pad,
    /// Nothing: at rest.
    None,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
enum Calibrate {
    /// The virtual pad gives the neutral calibration; the pad's counts
    /// are turned into turning and gravity by the pad's calibration as
    /// SDL reads it (gyroscope bias subtracted), and back into counts by
    /// the neutral one.
    Sdl,
    /// As `sdl`, with the pad's calibration read as the kernel's driver
    /// reads it (no gyroscope bias subtracted).
    Kernel,
    /// The virtual pad gives the pad's own calibration report, and the
    /// pad's counts pass unchanged: whoever reads the virtual pad
    /// calibrates them as they would the pad.
    Copy,
    /// The virtual pad gives the neutral calibration, and the pad's counts
    /// pass unchanged: the pad's own calibration is never applied.
    Raw,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, ValueEnum)]
enum Tick {
    /// The virtual pad's own clock, started at zero and advanced by the
    /// time between the pad's own sensor readings.
    Pad,
    /// The time each report reached the spike, on the monotonic clock, as
    /// the daemon's virtual pad stamps its reports now.
    Arrival,
}

fn main() -> anyhow::Result<()> {
    // Before any thread starts, so every thread has them blocked.
    let signals = sys::ending_signals().context("blocking SIGINT and SIGTERM")?;
    tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let args = Args::parse();
    tracing::info!(?args, "starting");

    let node = match &args.pad {
        Some(node) => node.clone(),
        None => first_dualsense()?,
    };
    tracing::info!(node = %node.display(), "the pad");
    let event = open(&node, false)?;

    // The pad, its raw node to read, and its raw node to write to.
    let (client, pad_fd, raw, raw_out) = match args.hide {
        Hide::Service => {
            let client = Client::connect(Path::new(SOCKET_PATH))
                .context("connecting to the hiding service")?;
            match client.version() {
                Ok(version) => tracing::info!(version, "the hiding service"),
                Err(error) => tracing::warn!(%error, "the hiding service's release"),
            }
            let hidden = client.hide(event.as_fd()).context("hiding the pad")?;
            drop(event);
            tracing::info!(dislodged = hidden.dislodged, "hidden");
            let raw = client.raw_of(hidden.token).context("the pad's raw node")?;
            let raw_out = client.raw_of(hidden.token).ok();
            (Some(client), hidden.event, raw, raw_out)
        }
        Hide::None => {
            let hidraw = hidraw_of(&node)?;
            tracing::warn!(
                hidraw = %hidraw.display(),
                "not hidden: games read the pad as well as the virtual one"
            );
            let raw = open(&hidraw, false)?;
            let raw_out = open(&hidraw, true).ok();
            (None, event, raw, raw_out)
        }
    };
    let mut pad = Pad::from_fd(pad_fd).context("reading the pad")?;
    if client.is_none() {
        pad.grab().context("grabbing the pad")?;
    }
    let (bus, vendor, product) = sys::info(raw.as_fd()).context("the raw node's numbers")?;
    if vendor != SONY || !DUALSENSES.contains(&product) {
        bail!("the raw node is not a DualSense's: {vendor:04x}:{product:04x}");
    }
    tracing::info!(bus, "the pad's raw node");
    rustix::fs::fcntl_setfl(&raw, rustix::fs::OFlags::NONBLOCK)?;
    let raw = File::from(raw);

    let pad_calibration = sys::feature(raw.as_fd(), CALIBRATION, CALIBRATION_SIZE)
        .context("the pad's calibration report")?;
    let neutral = report::neutral_calibration();
    tracing::info!(
        report = %hex(&pad_calibration),
        sdl = ?Calibration::read(&pad_calibration, Reader::Sdl),
        kernel = ?Calibration::read(&pad_calibration, Reader::Kernel),
        "the pad's calibration"
    );
    let (given, from) = match args.calibration {
        Calibrate::Copy => {
            let mut copied = [0u8; CALIBRATION_SIZE];
            for (place, byte) in copied.iter_mut().zip(&pad_calibration) {
                *place = *byte;
            }
            (copied, None)
        }
        Calibrate::Raw => (neutral, None),
        Calibrate::Sdl => (
            neutral,
            Some(Calibration::read(&pad_calibration, Reader::Sdl)),
        ),
        Calibrate::Kernel => (
            neutral,
            Some(Calibration::read(&pad_calibration, Reader::Kernel)),
        ),
    };
    let to = Calibration::read(&neutral, Reader::Sdl);

    // The pad's own descriptor and firmware where it is on USB, as the
    // daemon copies them; the shipped ones otherwise.
    let data = DualSenseData::read(raw.as_fd()).unwrap_or_else(|error| {
        tracing::info!(%error, "the shipped descriptor and firmware");
        DualSenseData::shipped()
    });
    let address = own_address(0x00ff_0000 | (std::process::id() & 0xffff));
    let mut virtual_pad =
        VirtualDualSense::create(&data, given, address).context("making the virtual pad")?;
    tracing::info!("the virtual pad is up");

    // Rumble, lights and trigger effects from games, passed on whole.
    if let Some(raw_out) = raw_out
        && let Err(error) = pad.set_raw(raw_out)
    {
        tracing::warn!(%error, "the pad won't be sent what games send");
    }

    let mut record = match &args.record {
        Some(path) => {
            let mut file = BufWriter::new(File::create(path).context("the record file")?);
            writeln!(file, "# calibration given: {}", hex(&given))?;
            writeln!(file, "# the pad's: {}", hex(&pad_calibration))?;
            writeln!(
                file,
                "arrival_us,pad_tick,virtual_tick,pad_gx,pad_gy,pad_gz,pad_ax,pad_ay,pad_az,\
                 out_gx,out_gy,out_gz,out_ax,out_ay,out_az,touch0,x0,y0,touch1,x1,y1,click"
            )?;
            Some(file)
        }
        None => None,
    };

    let mut clock = Clock::default();
    let mut sequence = 0u8;
    let mut level = Level::default();
    let mut stats = Stats::default();
    let mut buffer = [0u8; 128];
    let outcome = loop {
        let mut fds = [
            PollFd::new(&signals, PollFlags::IN),
            PollFd::new(&virtual_pad, PollFlags::IN),
            PollFd::new(&pad, PollFlags::IN),
            PollFd::new(&raw, PollFlags::IN),
        ];
        if let Err(error) = poll(&mut fds, None) {
            if error == rustix::io::Errno::INTR {
                continue;
            }
            break Err(error.into());
        }
        if !fds[0].revents().is_empty() {
            tracing::info!("asked to stop");
            break Ok(());
        }
        // Games first: a feature report asked waits on the answer.
        let served = virtual_pad.serve(|report| {
            if let Some(asked) = rumble_of(report)
                && asked != level
            {
                level = asked;
                if let Err(error) = pad.rumble(level) {
                    tracing::warn!(%error, "rumble");
                }
            }
            if let Err(error) = pad.pass(report) {
                tracing::warn!(%error, "passing a game's report");
            }
        });
        if let Err(error) = served {
            break Err(anyhow!(error).context("serving the virtual pad"));
        }
        // The pad's event node before its raw one: the kernel's driver
        // reports a frame to the event node before the raw node is given
        // the report it came from.
        match pad.read(|_| {}) {
            Ok(()) => {}
            Err(PadError::Gone) => break Err(anyhow!("the pad has gone")),
            Err(error) => break Err(anyhow!(error).context("reading the pad")),
        }
        let gone = loop {
            let length = match (&raw).read(&mut buffer) {
                Ok(0) => break true,
                Ok(length) => length,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break false,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    tracing::warn!(%error, "reading the raw node");
                    break true;
                }
            };
            let Some(usb) = report::as_usb(&buffer[..length]) else {
                continue;
            };
            let arrival = monotonic_us();
            let read = Motion::of(&usb);
            let mut motion = match args.motion {
                Source::Pad => match &from {
                    Some(from) => from.convert(read, &to),
                    None => read,
                },
                Source::None => Motion::STILL,
            };
            motion.tick = match args.clock {
                Tick::Pad => clock.advance(read.tick),
                Tick::Arrival => (arrival.wrapping_mul(3)) as u32,
            };
            let touch = match args.touch {
                Source::Pad => Touch::of(&usb),
                Source::None => Touch::NONE,
            };
            sequence = sequence.wrapping_add(1);
            let mut out = report::input_report(pad.state(), sequence);
            motion.onto(&mut out);
            touch.onto(&mut out);
            if let Err(error) = virtual_pad.write(&out) {
                tracing::warn!(%error, "writing the virtual pad");
            }
            stats.take(read.tick, arrival);
            if let Some(file) = &mut record {
                let [(t0, x0, y0), (t1, x1, y1)] = touch.contacts();
                let written = writeln!(
                    file,
                    "{arrival},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{x0},{y0},{},{x1},{y1},{}",
                    read.tick,
                    motion.tick,
                    read.gyro[0],
                    read.gyro[1],
                    read.gyro[2],
                    read.accel[0],
                    read.accel[1],
                    read.accel[2],
                    motion.gyro[0],
                    motion.gyro[1],
                    motion.gyro[2],
                    motion.accel[0],
                    motion.accel[1],
                    motion.accel[2],
                    u8::from(t0),
                    u8::from(t1),
                    u8::from(touch.click),
                );
                if let Err(error) = written {
                    tracing::warn!(%error, "the record file; recording stops");
                    record = None;
                }
            }
        };
        if gone {
            break Err(anyhow!("the pad's raw node has gone"));
        }
    };

    // The game sees the virtual pad go; the pad is left at rest by its
    // writer, and only then shown again, by closing the connection that
    // hid it.
    drop(virtual_pad);
    if let Some(mut file) = record {
        file.flush()?;
    }
    drop(raw);
    pad.let_go().wait();
    drop(client);
    tracing::info!("done");
    outcome
}

/// What the reports' timing has been, said every thousand reports.
#[derive(Default)]
struct Stats {
    count: u32,
    first: Option<(u32, u64)>,
    last: Option<(u32, u64)>,
    longest_us: u64,
}

impl Stats {
    fn take(&mut self, tick: u32, arrival: u64) {
        if let Some((_, last)) = self.last {
            self.longest_us = self.longest_us.max(arrival.saturating_sub(last));
        }
        self.first.get_or_insert((tick, arrival));
        self.last = Some((tick, arrival));
        self.count += 1;
        if self.count == 1000 {
            if let (Some((tick0, arrival0)), Some((tick1, arrival1))) = (self.first, self.last) {
                let gaps = f64::from(self.count - 1);
                tracing::info!(
                    pad_ms = f64::from(tick1.wrapping_sub(tick0)) / 3000.0 / gaps,
                    arrival_ms = (arrival1 - arrival0) as f64 / 1000.0 / gaps,
                    longest_arrival_ms = self.longest_us as f64 / 1000.0,
                    "mean time between reports, by the pad's clock and as they came"
                );
            }
            *self = Self::default();
        }
    }
}

/// The rumble an output report leaves the pad at, as the device crate's
/// emulator reads it: marked in either of the firmware's two places, or
/// stopped by a report that marks it in neither.
fn rumble_of(report: &[u8]) -> Option<Level> {
    let [0x02, flags0, _, weak, strong, ..] = *report else {
        return None;
    };
    let flags2 = report.get(39).copied().unwrap_or(0);
    if flags0 & 0x01 == 0 && flags2 & 0x04 == 0 {
        return Some(Level::default());
    }
    Some(Level {
        strong: u16::from(strong) * 257,
        weak: u16::from(weak) * 257,
    })
}

fn monotonic_us() -> u64 {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    (now.tv_sec as u64)
        .saturating_mul(1_000_000)
        .saturating_add(now.tv_nsec as u64 / 1000)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn open(path: &Path, write: bool) -> anyhow::Result<OwnedFd> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(write)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    Ok(file.into())
}

/// The first DualSense connected that isn't a virtual one.
fn first_dualsense() -> anyhow::Result<PathBuf> {
    ctrlassist_device::connected()?
        .into_iter()
        .find(|found| {
            let ids = &found.identifiers;
            !found.is_own_output() && ids.vendor == SONY && DUALSENSES.contains(&ids.product)
        })
        .map(|found| found.node)
        .ok_or_else(|| anyhow!("no DualSense is connected"))
}

/// The raw node of the HID device a pad's event node is under.
fn hidraw_of(event: &Path) -> anyhow::Result<PathBuf> {
    let name = event
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("not an event node: {}", event.display()))?;
    let device = udev::Device::from_subsystem_sysname("input".into(), name.into())?;
    let hid = std::iter::successors(Some(device), udev::Device::parent)
        .find(|device| {
            device
                .subsystem()
                .is_some_and(|subsystem| subsystem == "hid")
        })
        .ok_or_else(|| anyhow!("the pad has no HID device"))?;
    let mut raw = udev::Enumerator::new()?;
    raw.match_subsystem("hidraw")?;
    raw.match_parent(&hid)?;
    raw.scan_devices()?
        .find_map(|device| device.devnode().map(Path::to_path_buf))
        .ok_or_else(|| anyhow!("the pad's HID device has no raw node"))
}
