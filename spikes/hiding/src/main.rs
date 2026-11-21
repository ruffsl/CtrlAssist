//! Probe for the hiding, permissions and identification spike.
//!
//! Each subcommand is one observation or one fixture. Fixtures (`fake-pad`,
//! `fake-hid`, `grab`) hold until killed, so a test can kill them without
//! cleanup and see what the kernel leaves behind.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, Device, EventType, InputEvent, InputId,
    KeyCode, UinputAbsSetup,
};

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Report what the calling user can open: uinput, uhid, and every pad's nodes.
    Access,
    /// List pads with every identifier the kernel and udev expose for them.
    Pads,
    /// Create a uinput gamepad and hold it; with --pulse, press a button repeatedly.
    FakePad {
        #[arg(long, default_value = "Spike Fake Pad")]
        name: String,
        #[arg(long, default_value_t = 0x1209)]
        vendor: u16,
        #[arg(long, default_value_t = 0x5eed)]
        product: u16,
        /// Milliseconds between button edges; 0 holds silent.
        #[arg(long, default_value_t = 0)]
        pulse: u64,
    },
    /// Create a uhid gamepad (generic HID descriptor) and hold it.
    FakeHid {
        #[arg(long, default_value = "Spike Fake HID Pad")]
        name: String,
        #[arg(long, default_value_t = 0)]
        pulse: u64,
    },
    /// Take an exclusive grab on an event node and hold it.
    Grab { node: PathBuf },
    /// Count the events one reader sees on an event, js or hidraw node in a window.
    Listen {
        node: PathBuf,
        #[arg(long, default_value_t = 1000)]
        millis: u64,
        /// Print the running count every second, and say why reading
        /// stopped if it does before the window ends.
        #[arg(long)]
        progress: bool,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Access => access(),
        Command::Pads => pads(),
        Command::FakePad {
            name,
            vendor,
            product,
            pulse,
        } => fake_pad(&name, vendor, product, pulse),
        Command::FakeHid { name, pulse } => fake_hid(&name, pulse),
        Command::Grab { node } => grab(&node),
        Command::Listen { node, millis, progress } => listen(&node, millis, progress),
    }
}

// --- access -----------------------------------------------------------------

fn try_open(path: &Path, write: bool) -> String {
    let result = OpenOptions::new()
        .read(true)
        .write(write)
        .custom_flags(libc::O_NONBLOCK)
        .open(path);
    match result {
        Ok(_) => "yes".into(),
        Err(e) => format!("no ({})", e.kind()),
    }
}

fn access_row(label: &str, path: &Path) {
    let meta = match std::fs::metadata(path) {
        Ok(m) => format!("{:o} {}:{}", m.mode() & 0o7777, m.uid(), m.gid()),
        Err(e) => format!("missing ({})", e.kind()),
    };
    println!(
        "{label:<10} {:<24} mode={meta:<16} read={:<24} write={}",
        path.display(),
        try_open(path, false),
        try_open(path, true),
    );
}

fn access() -> Result<()> {
    // SAFETY: getuid and getgid have no preconditions and cannot fail.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    println!("caller uid={uid} gid={gid}");
    access_row("uinput", Path::new("/dev/uinput"));
    access_row("uhid", Path::new("/dev/uhid"));
    for pad in find_pads()? {
        println!("pad \"{}\"", pad.name);
        for (kind, node) in &pad.nodes {
            access_row(kind, node);
        }
    }
    Ok(())
}

// --- pads -------------------------------------------------------------------

struct Pad {
    name: String,
    props: Vec<(String, String)>,
    /// (kind, devnode) for the event node and its js and hidraw siblings.
    nodes: Vec<(&'static str, PathBuf)>,
}

fn sysattr(dev: &udev::Device, name: &str) -> String {
    dev.attribute_value(name)
        .map(|v| v.to_string_lossy().trim().to_string())
        .unwrap_or_default()
}

fn subsystem_is(dev: &udev::Device, wanted: &str) -> bool {
    dev.subsystem().and_then(|s| s.to_str()) == Some(wanted)
}

fn find_pads() -> Result<Vec<Pad>> {
    let mut enumerator = udev::Enumerator::new()?;
    enumerator.match_subsystem("input")?;
    enumerator.match_property("ID_INPUT_JOYSTICK", "1")?;
    let mut pads = Vec::new();
    for dev in enumerator.scan_devices()? {
        let Some(node) = dev.devnode() else { continue };
        if !node.to_string_lossy().contains("/event") {
            continue;
        }
        // The input device itself (inputN) is the event node's parent.
        let Some(input) = dev.parent() else { continue };
        // The HID device, when there is one, owns the hidraw siblings. A
        // uinput pad has none.
        let hid = std::iter::successors(Some(input.clone()), udev::Device::parent)
            .find(|d| subsystem_is(d, "hid"));

        let mut props = vec![
            ("name".to_string(), sysattr(&input, "name")),
            ("uniq".to_string(), sysattr(&input, "uniq")),
            ("phys".to_string(), sysattr(&input, "phys")),
            (
                "id".to_string(),
                format!(
                    "bus={} vendor={} product={} version={}",
                    sysattr(&input, "id/bustype"),
                    sysattr(&input, "id/vendor"),
                    sysattr(&input, "id/product"),
                    sysattr(&input, "id/version"),
                ),
            ),
            ("syspath".to_string(), input.syspath().display().to_string()),
        ];
        for key in [
            "ID_SERIAL",
            "ID_SERIAL_SHORT",
            "ID_PATH",
            "ID_BUS",
            "ID_VENDOR_ID",
            "ID_MODEL_ID",
            "ID_INPUT_JOYSTICK",
            "ID_INPUT_ACCELEROMETER",
            "TAGS",
            "CURRENT_TAGS",
        ] {
            if let Some(value) = dev.property_value(key) {
                props.push((key.to_string(), value.to_string_lossy().into_owned()));
            }
        }
        if let Some(hid) = &hid {
            props.push(("hid".to_string(), hid.sysname().to_string_lossy().into_owned()));
            for key in ["HID_ID", "HID_UNIQ", "HID_PHYS"] {
                if let Some(value) = hid.property_value(key) {
                    props.push((key.to_string(), value.to_string_lossy().into_owned()));
                }
            }
        }

        let mut nodes = vec![("event", node.to_path_buf())];
        let mut children = udev::Enumerator::new()?;
        children.match_parent(hid.as_ref().unwrap_or(&input))?;
        for child in children.scan_devices()? {
            let Some(child_node) = child.devnode() else {
                continue;
            };
            let text = child_node.to_string_lossy();
            // Only this input device's js node, but every hidraw under the HID parent.
            if text.contains("/js") && child.parent().is_some_and(|p| p.syspath() == input.syspath())
            {
                nodes.push(("js", child_node.to_path_buf()));
            } else if text.contains("hidraw") {
                nodes.push(("hidraw", child_node.to_path_buf()));
            }
        }

        pads.push(Pad {
            name: sysattr(&input, "name"),
            props,
            nodes,
        });
    }
    Ok(pads)
}

fn pads() -> Result<()> {
    for pad in find_pads()? {
        println!("pad \"{}\"", pad.name);
        for (key, value) in &pad.props {
            println!("  {key:<24} {value}");
        }
        for (kind, node) in &pad.nodes {
            println!("  node {kind:<19} {}", node.display());
        }
    }
    Ok(())
}

// --- fixtures ---------------------------------------------------------------

fn hold_or_pulse(pulse: u64, mut edge: impl FnMut(bool) -> Result<()>) -> Result<()> {
    let mut pressed = false;
    loop {
        if pulse == 0 {
            sleep(Duration::from_secs(3600));
        } else {
            pressed = !pressed;
            edge(pressed)?;
            sleep(Duration::from_millis(pulse));
        }
    }
}

fn fake_pad(name: &str, vendor: u16, product: u16, pulse: u64) -> Result<()> {
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
    let stick = AbsInfo::new(0, -32768, 32767, 16, 128, 0);
    let mut builder = VirtualDevice::builder()?
        .name(name)
        .input_id(InputId::new(BusType::BUS_USB, vendor, product, 1))
        .with_keys(&keys)?;
    for axis in [
        AbsoluteAxisCode::ABS_X,
        AbsoluteAxisCode::ABS_Y,
        AbsoluteAxisCode::ABS_RX,
        AbsoluteAxisCode::ABS_RY,
    ] {
        builder = builder.with_absolute_axis(&UinputAbsSetup::new(axis, stick))?;
    }
    let mut pad = builder.build().context("creating the uinput pad")?;
    for node in pad.enumerate_dev_nodes_blocking()? {
        println!("node {}", node?.display());
    }
    hold_or_pulse(pulse, |pressed| {
        pad.emit(&[InputEvent::new(
            EventType::KEY.0,
            KeyCode::BTN_SOUTH.0,
            i32::from(pressed),
        )])?;
        Ok(())
    })
}

/// A generic HID game pad: eight buttons, then X and Y as signed bytes.
const HID_GAMEPAD_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x05, // Usage (Game Pad)
    0xA1, 0x01, // Collection (Application)
    0xA1, 0x00, //   Collection (Physical)
    0x05, 0x09, //     Usage Page (Button)
    0x19, 0x01, //     Usage Minimum (1)
    0x29, 0x08, //     Usage Maximum (8)
    0x15, 0x00, //     Logical Minimum (0)
    0x25, 0x01, //     Logical Maximum (1)
    0x95, 0x08, //     Report Count (8)
    0x75, 0x01, //     Report Size (1)
    0x81, 0x02, //     Input (Data, Variable, Absolute)
    0x05, 0x01, //     Usage Page (Generic Desktop)
    0x09, 0x30, //     Usage (X)
    0x09, 0x31, //     Usage (Y)
    0x15, 0x81, //     Logical Minimum (-127)
    0x25, 0x7F, //     Logical Maximum (127)
    0x75, 0x08, //     Report Size (8)
    0x95, 0x02, //     Report Count (2)
    0x81, 0x02, //     Input (Data, Variable, Absolute)
    0xC0, //   End Collection
    0xC0, // End Collection
];

/// Writes a `UHID_CREATE2` event by hand.
///
/// The `uhid-virt` crate wraps this, but its FFI layer runs bindgen, which
/// needs libclang at build time; the event is simple enough to lay out here.
/// Layout (packed, from `linux/uhid.h`): type u32, then name[128], phys[64],
/// uniq[64], rd_size u16, bus u16, vendor u32, product u32, version u32,
/// country u32, rd_data[4096].
fn uhid_create2(name: &str, uniq: &str, descriptor: &[u8]) -> Vec<u8> {
    const UHID_CREATE2: u32 = 11;
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
    let size = u16::try_from(descriptor.len()).expect("descriptor fits in u16 because it is a small constant");
    event.extend_from_slice(&size.to_ne_bytes());
    event.extend_from_slice(&BUS_USB.to_ne_bytes());
    event.extend_from_slice(&0x1209u32.to_ne_bytes());
    event.extend_from_slice(&0x5eeeu32.to_ne_bytes());
    event.extend_from_slice(&1u32.to_ne_bytes());
    event.extend_from_slice(&0u32.to_ne_bytes());
    let mut data = descriptor.to_vec();
    data.resize(4096, 0);
    event.extend_from_slice(&data);
    event
}

/// A `UHID_INPUT2` event: type u32, size u16, then the report.
fn uhid_input2(report: &[u8]) -> Vec<u8> {
    const UHID_INPUT2: u32 = 12;
    let mut event = Vec::new();
    event.extend_from_slice(&UHID_INPUT2.to_ne_bytes());
    let size = u16::try_from(report.len()).expect("report fits in u16 because it is three bytes");
    event.extend_from_slice(&size.to_ne_bytes());
    event.extend_from_slice(report);
    event
}

fn fake_hid(name: &str, pulse: u64) -> Result<()> {
    let mut uhid = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/uhid")
        .context("opening /dev/uhid")?;
    uhid.write_all(&uhid_create2(name, "spike-fake-hid", HID_GAMEPAD_DESCRIPTOR))
        .context("creating the uhid pad")?;
    println!("created");
    hold_or_pulse(pulse, |pressed| {
        uhid.write_all(&uhid_input2(&[u8::from(pressed), 0, 0]))?;
        Ok(())
    })
}

fn grab(node: &Path) -> Result<()> {
    let mut device = Device::open(node).with_context(|| format!("opening {}", node.display()))?;
    device.grab().context("EVIOCGRAB")?;
    println!("grabbed {}", node.display());
    loop {
        sleep(Duration::from_secs(3600));
    }
}

// --- listen -----------------------------------------------------------------

/// Waits until `fd` is readable or the deadline passes.
fn readable_before(fd: RawFd, deadline: Instant) -> Result<bool> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Ok(false);
    }
    let mut pollfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let millis = i32::try_from(remaining.as_millis()).unwrap_or(i32::MAX).max(1);
    // SAFETY: `pollfd` is a valid, exclusively borrowed array of one element
    // for the duration of the call.
    let ready = unsafe { libc::poll(&raw mut pollfd, 1, millis) };
    if ready < 0 {
        bail!("poll: {}", std::io::Error::last_os_error());
    }
    Ok(ready > 0)
}

fn listen(node: &Path, millis: u64, progress: bool) -> Result<()> {
    let started = Instant::now();
    let deadline = started + Duration::from_millis(millis);
    let mut reported = 0;
    let text = node.to_string_lossy();
    let mut count = 0usize;

    if text.contains("/event") {
        let mut device =
            Device::open(node).with_context(|| format!("opening {}", node.display()))?;
        device.set_nonblocking(true)?;
        loop {
            // Wake at least once a second so progress is printed while idle.
            let tick = deadline.min(Instant::now() + Duration::from_secs(1));
            let ready = readable_before(device.as_raw_fd(), tick)?;
            let seconds = started.elapsed().as_secs();
            if progress && seconds > reported {
                reported = seconds;
                println!("t={seconds}s events={count}");
            }
            if !ready {
                if Instant::now() >= deadline {
                    break;
                }
                continue;
            }
            match device.fetch_events() {
                Ok(events) => {
                    count += events
                        .filter(|e| e.event_type() != EventType::SYNCHRONIZATION)
                        .count();
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) if progress => {
                    println!("stopped: {e}");
                    break;
                }
                Err(e) => return Err(e).context("reading events"),
            }
        }
    } else {
        // js nodes deliver 8-byte records, with the initial state flagged
        // 0x80 in the type byte; hidraw delivers one report per read.
        let is_js = text.contains("/js");
        let mut file = File::options()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(node)
            .with_context(|| format!("opening {}", node.display()))?;
        let mut buffer = [0u8; 4096];
        loop {
            // Wake at least once a second so progress is printed while idle.
            let tick = deadline.min(Instant::now() + Duration::from_secs(1));
            let ready = readable_before(file.as_raw_fd(), tick)?;
            let seconds = started.elapsed().as_secs();
            if progress && seconds > reported {
                reported = seconds;
                println!("t={seconds}s events={count}");
            }
            if !ready {
                if Instant::now() >= deadline {
                    break;
                }
                continue;
            }
            match file.read(&mut buffer) {
                Ok(0) => {
                    if progress {
                        println!("stopped: end of file");
                    }
                    break;
                }
                Ok(read) if is_js => {
                    count += buffer[..read]
                        .chunks_exact(8)
                        .filter(|record| record[6] & 0x80 == 0)
                        .count();
                }
                Ok(_) => count += 1,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) if progress => {
                    println!("stopped: {e}");
                    break;
                }
                Err(e) => return Err(e).context("reading"),
            }
        }
    }
    println!("events={count}");
    Ok(())
}
