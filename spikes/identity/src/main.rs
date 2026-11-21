//! Fixtures for the virtual pad identity spike.
//!
//! `uinput-pad` makes an evdev-level pad carrying any name and IDs.
//! `uhid-dualsense` makes a HID-level pad by replaying a real DualSense's
//! report descriptor and feature reports, so the kernel's PlayStation driver
//! binds to it as it would to the real thing. Both log the feedback they are
//! sent.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, EventSummary, EventType, FFEffectCode,
    InputEvent, InputId, KeyCode, UInputCode, UinputAbsSetup,
};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

fn parse_hex(text: &str) -> Result<u16, std::num::ParseIntError> {
    u16::from_str_radix(text.trim_start_matches("0x"), 16)
}

#[derive(Subcommand)]
enum Command {
    /// Print what a real DualSense's hidraw node gives an emulator to replay.
    Capture { hidraw: PathBuf },
    /// Create a uinput gamepad with the given identity and log its feedback.
    UinputPad {
        #[arg(long)]
        name: String,
        /// Hex vendor ID.
        #[arg(long, value_parser = parse_hex)]
        vendor: u16,
        /// Hex product ID.
        #[arg(long, value_parser = parse_hex)]
        product: u16,
        /// Hex version (bcdDevice); part of the ID SDL builds.
        #[arg(long, value_parser = parse_hex, default_value = "8111")]
        version: u16,
    },
    /// Create a uhid DualSense from a real one's descriptor and log its feedback.
    UhidDualsense {
        /// hidraw node of a connected DualSense on USB to copy from.
        hidraw: PathBuf,
        /// Press and release the cross button on this period, in milliseconds.
        #[arg(long, default_value_t = 0)]
        press: u64,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Capture { hidraw } => {
            let captured = Captured::from(&hidraw)?;
            println!("descriptor: {} bytes", captured.descriptor.len());
            for (id, report) in &captured.features {
                println!("feature {id:#04x}: {} bytes: {}", report.len(), hex(report));
            }
            println!("input: {} bytes: {}", captured.input.len(), hex(&captured.input));
            Ok(())
        }
        Command::UinputPad {
            name,
            vendor,
            product,
            version,
        } => uinput_pad(&name, vendor, product, version),
        Command::UhidDualsense { hidraw, press } => uhid_dualsense(&hidraw, press),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

/// Waits until `fd` is readable or the deadline passes.
fn readable_before(fd: RawFd, deadline: Instant) -> Result<bool> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let millis = i32::try_from(remaining.as_millis()).unwrap_or(i32::MAX);
    // SAFETY: `pollfd` is a valid, exclusively borrowed array of one element
    // for the duration of the call.
    let ready = unsafe { libc::poll(&raw mut pollfd, 1, millis) };
    if ready < 0 {
        bail!("poll: {}", std::io::Error::last_os_error());
    }
    Ok(ready > 0)
}

// --- uinput -----------------------------------------------------------------

fn uinput_pad(name: &str, vendor: u16, product: u16, version: u16) -> Result<()> {
    // The layout the kernel's PlayStation and Xbox drivers both present:
    // two sticks, two analog triggers, a hat, and the standard buttons.
    let keys = AttributeSet::from_iter([
        KeyCode::BTN_SOUTH,
        KeyCode::BTN_EAST,
        KeyCode::BTN_NORTH,
        KeyCode::BTN_WEST,
        KeyCode::BTN_TL,
        KeyCode::BTN_TR,
        KeyCode::BTN_TL2,
        KeyCode::BTN_TR2,
        KeyCode::BTN_SELECT,
        KeyCode::BTN_START,
        KeyCode::BTN_MODE,
        KeyCode::BTN_THUMBL,
        KeyCode::BTN_THUMBR,
    ]);
    let stick = AbsInfo::new(128, 0, 255, 0, 0, 0);
    let hat = AbsInfo::new(0, -1, 1, 0, 0, 0);
    let mut builder = VirtualDevice::builder()?
        .name(name)
        .input_id(InputId::new(BusType::BUS_USB, vendor, product, version))
        .with_keys(&keys)?
        .with_ff(&AttributeSet::from_iter([FFEffectCode::FF_RUMBLE]))?
        .with_ff_effects_max(16);
    for (axis, info) in [
        (AbsoluteAxisCode::ABS_X, stick),
        (AbsoluteAxisCode::ABS_Y, stick),
        (AbsoluteAxisCode::ABS_Z, stick),
        (AbsoluteAxisCode::ABS_RX, stick),
        (AbsoluteAxisCode::ABS_RY, stick),
        (AbsoluteAxisCode::ABS_RZ, stick),
        (AbsoluteAxisCode::ABS_HAT0X, hat),
        (AbsoluteAxisCode::ABS_HAT0Y, hat),
    ] {
        builder = builder.with_absolute_axis(&UinputAbsSetup::new(axis, info))?;
    }
    let mut pad = builder.build().context("creating the uinput pad")?;
    for node in pad.enumerate_dev_nodes_blocking()? {
        println!("node {}", node?.display());
    }
    loop {
        let events: Vec<InputEvent> = pad.fetch_events()?.collect();
        for event in events {
            match event.destructure() {
                EventSummary::UInput(request, UInputCode::UI_FF_UPLOAD, ..) => {
                    let upload = pad.process_ff_upload(request)?;
                    println!("feedback: upload effect {} {:?}", upload.effect_id(), upload.effect().kind);
                }
                EventSummary::UInput(request, UInputCode::UI_FF_ERASE, ..) => {
                    let erase = pad.process_ff_erase(request)?;
                    println!("feedback: erase effect {}", erase.effect_id());
                }
                EventSummary::ForceFeedback(_, effect, value) => {
                    println!("feedback: effect {} play={value}", effect.0);
                }
                _ if event.event_type() == EventType::SYNCHRONIZATION => {}
                _ => println!("feedback: other {event:?}"),
            }
        }
    }
}

// --- capture from a real DualSense -----------------------------------------

/// The feature reports the kernel's PlayStation driver reads while binding:
/// calibration, pairing info (which holds the address) and firmware version.
const FEATURE_REPORTS: [(u8, usize); 3] = [(0x05, 41), (0x09, 20), (0x20, 64)];

struct Captured {
    descriptor: Vec<u8>,
    features: Vec<(u8, Vec<u8>)>,
    input: Vec<u8>,
}

impl Captured {
    fn from(hidraw: &Path) -> Result<Self> {
        let name = hidraw.file_name().context("hidraw path has no file name")?;
        let descriptor = std::fs::read(
            Path::new("/sys/class/hidraw").join(name).join("device/report_descriptor"),
        )
        .context("reading the report descriptor from sysfs")?;

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(hidraw)
            .with_context(|| format!("opening {}", hidraw.display()))?;
        let mut features = Vec::new();
        for (id, size) in FEATURE_REPORTS {
            let mut buffer = vec![0u8; size];
            buffer[0] = id;
            // HIDIOCGFEATURE(len): _IOC(READ | WRITE, 'H', 0x07, len).
            let request = (3u64 << 30) | ((size as u64) << 16) | (u64::from(b'H') << 8) | 0x07;
            // SAFETY: `buffer` is a live allocation of exactly `size` bytes,
            // the length encoded in the request, and the kernel writes at
            // most that many.
            let got = unsafe { libc::ioctl(file.as_raw_fd(), request as _, buffer.as_mut_ptr()) };
            if got < 0 {
                bail!("HIDIOCGFEATURE {id:#04x}: {}", std::io::Error::last_os_error());
            }
            features.push((id, buffer));
        }
        let mut input = vec![0u8; 128];
        let read = file.read(&mut input).context("reading one input report")?;
        input.truncate(read);
        Ok(Self {
            descriptor,
            features,
            input,
        })
    }
}

// --- uhid -------------------------------------------------------------------

// Event types from linux/uhid.h.
const UHID_START: u32 = 2;
const UHID_STOP: u32 = 3;
const UHID_OPEN: u32 = 4;
const UHID_CLOSE: u32 = 5;
const UHID_OUTPUT: u32 = 6;
const UHID_GET_REPORT: u32 = 9;
const UHID_GET_REPORT_REPLY: u32 = 10;
const UHID_CREATE2: u32 = 11;
const UHID_INPUT2: u32 = 12;
const UHID_SET_REPORT: u32 = 13;
const UHID_SET_REPORT_REPLY: u32 = 14;

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_ne_bytes([bytes[at], bytes[at + 1]])
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// `UHID_CREATE2`, packed: type u32, name[128], phys[64], uniq[64],
/// rd_size u16, bus u16, vendor u32, product u32, version u32, country u32,
/// rd_data[4096].
fn create2(name: &str, uniq: &str, vendor: u32, product: u32, version: u32, descriptor: &[u8]) -> Vec<u8> {
    const BUS_USB: u16 = 3;
    let mut event = Vec::with_capacity(4 + 4372);
    event.extend_from_slice(&UHID_CREATE2.to_ne_bytes());
    let mut field = |text: &str, width: usize| {
        let mut bytes = text.as_bytes().to_vec();
        bytes.truncate(width - 1);
        bytes.resize(width, 0);
        event.extend_from_slice(&bytes);
    };
    field(name, 128);
    field("", 64);
    field(uniq, 64);
    let size = u16::try_from(descriptor.len()).expect("a HID report descriptor is at most 4096 bytes");
    event.extend_from_slice(&size.to_ne_bytes());
    event.extend_from_slice(&BUS_USB.to_ne_bytes());
    event.extend_from_slice(&vendor.to_ne_bytes());
    event.extend_from_slice(&product.to_ne_bytes());
    event.extend_from_slice(&version.to_ne_bytes());
    event.extend_from_slice(&0u32.to_ne_bytes());
    let mut data = descriptor.to_vec();
    data.resize(4096, 0);
    event.extend_from_slice(&data);
    event
}

fn input2(report: &[u8]) -> Vec<u8> {
    let mut event = UHID_INPUT2.to_ne_bytes().to_vec();
    let size = u16::try_from(report.len()).expect("an input report is 64 bytes");
    event.extend_from_slice(&size.to_ne_bytes());
    event.extend_from_slice(report);
    event
}

fn uhid_dualsense(real: &Path, press: u64) -> Result<()> {
    let mut captured = Captured::from(real)?;

    // The driver refuses a second pad with an address it already knows, so
    // the copy gets its own. Pairing info holds it at bytes 1..7, low byte first.
    let address = [0x02, 0xc7, 0x41, 0x55, 0x53, 0x54];
    for (id, report) in &mut captured.features {
        if *id == 0x09 {
            report[1..7].copy_from_slice(&address);
        }
    }
    let uniq = address.iter().rev().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":");

    let mut uhid: File = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open("/dev/uhid")
        .context("opening /dev/uhid")?;
    uhid.write_all(&create2(
        "Sony Interactive Entertainment DualSense Wireless Controller",
        &uniq,
        0x054c,
        0x0ce6,
        0x8111,
        &captured.descriptor,
    ))
    .context("creating the uhid pad")?;
    println!("created uhid DualSense, address {uniq}");

    // Neutral state: the captured report with the face buttons cleared.
    // Byte 8 holds the hat in its low nibble and square, cross, circle,
    // triangle in the high one.
    let mut report = captured.input.clone();
    report[8] = 0x08;
    let period = Duration::from_millis(if press == 0 { 250 } else { press });
    let mut pressed = false;
    let mut next = Instant::now();
    let mut buffer = vec![0u8; 4380];
    loop {
        if Instant::now() >= next {
            if press != 0 {
                pressed = !pressed;
                report[8] = if pressed { 0x28 } else { 0x08 };
            }
            uhid.write_all(&input2(&report))?;
            next += period;
        }
        if !readable_before(uhid.as_raw_fd(), next)? {
            continue;
        }
        let read = match uhid.read(&mut buffer) {
            Ok(read) => read,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
            Err(e) => return Err(e).context("reading from /dev/uhid"),
        };
        if read < 4 {
            continue;
        }
        match u32_at(&buffer, 0) {
            UHID_START => println!("kernel: start"),
            UHID_STOP => println!("kernel: stop"),
            UHID_OPEN => println!("kernel: open (something is reading the pad)"),
            UHID_CLOSE => println!("kernel: close"),
            UHID_OUTPUT => {
                let size = usize::from(u16_at(&buffer, 4 + 4096));
                println!("feedback: output report, {size} bytes: {}", hex(&buffer[4..4 + size.min(16)]));
            }
            UHID_GET_REPORT => {
                let request = u32_at(&buffer, 4);
                let number = buffer[8];
                let found = captured.features.iter().find(|(id, _)| *id == number);
                println!(
                    "kernel: get report {number:#04x} -> {}",
                    if found.is_some() { "answered" } else { "unknown, refused" }
                );
                let mut reply = UHID_GET_REPORT_REPLY.to_ne_bytes().to_vec();
                reply.extend_from_slice(&request.to_ne_bytes());
                match found {
                    Some((_, data)) => {
                        reply.extend_from_slice(&0u16.to_ne_bytes());
                        let size = u16::try_from(data.len()).expect("a feature report is at most 64 bytes");
                        reply.extend_from_slice(&size.to_ne_bytes());
                        reply.extend_from_slice(data);
                    }
                    None => {
                        let eio = u16::try_from(libc::EIO).expect("EIO is a small errno");
                        reply.extend_from_slice(&eio.to_ne_bytes());
                        reply.extend_from_slice(&0u16.to_ne_bytes());
                    }
                }
                uhid.write_all(&reply)?;
            }
            UHID_SET_REPORT => {
                let request = u32_at(&buffer, 4);
                let size = usize::from(u16_at(&buffer, 10));
                println!(
                    "feedback: set report {:#04x}, {size} bytes: {}",
                    buffer[8],
                    hex(&buffer[12..12 + size.min(16)])
                );
                let mut reply = UHID_SET_REPORT_REPLY.to_ne_bytes().to_vec();
                reply.extend_from_slice(&request.to_ne_bytes());
                reply.extend_from_slice(&0u16.to_ne_bytes());
                uhid.write_all(&reply)?;
            }
            other => println!("kernel: event type {other}"),
        }
    }
}
