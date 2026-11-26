//! A stand-in Steam Controller (2026) made through uhid, for the kernel's
//! hid-steam to bind, and the observers the virtual machine check drives.
//! Every tool writes JSON lines to a log, each with `t`, CLOCK_MONOTONIC
//! in seconds, so that the logs' times compare with each other and with
//! the kernel's.
//!
//!   steam-spike fake <own|own47|real> <log> <fifo>
//!   steam-spike raw <hidraw node> <log> <fifo>
//!   steam-spike events <event node> <log>
//!   steam-spike uevents <log>
//!   steam-spike now
//!
//! `fake` and `raw` take commands, one a line, from a FIFO:
//!
//!   fake: send <hex>            an input report as it is
//!         state <id> <buttons> <lx> <ly> <count> <every ms>
//!                               `count` input reports of `id` (42, 45
//!                               or 47) with these 32 bits of buttons and
//!                               the left stick so, one every `every` ms
//!         quit
//!   raw:  write <hex>           write(2) to the node: an output report
//!         setfeature <hex>      HIDIOCSFEATURE
//!         getfeature <hex>      HIDIOCGFEATURE, the buffer as given
//!         quit
//!
//! It is a spike's: it panics on what it doesn't expect.

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;

#[repr(C)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

unsafe extern "C" {
    fn clock_gettime(clock: i32, ts: *mut [i64; 2]) -> i32;
    fn poll(fds: *mut PollFd, n: u64, timeout: i32) -> i32;
    fn ioctl(fd: i32, request: u64, ...) -> i32;
    fn socket(domain: i32, kind: i32, protocol: i32) -> i32;
    fn bind(fd: i32, addr: *const c_void, len: u32) -> i32;
    fn recv(fd: i32, buf: *mut c_void, len: usize, flags: i32) -> isize;
}

const O_NONBLOCK: i32 = 0o4000;
const POLLIN: i16 = 1;
const CLOCK_MONOTONIC: i32 = 1;

fn now() -> f64 {
    let mut ts = [0i64; 2];
    // SAFETY: ts is a valid timespec for the call to fill.
    unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) };
    ts[0] as f64 + ts[1] as f64 / 1e9
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).expect("hex"))
        .collect()
}

fn quote(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A JSON-lines log, flushed line by line.
struct Log(File);

impl Log {
    fn open(path: &str) -> Self {
        Self(File::create(path).expect("the log can be created"))
    }

    /// One line: `t`, then `fields`, each already JSON.
    fn line(&mut self, t: f64, fields: &[(&str, String)]) {
        let mut out = format!("{{\"t\":{t:.6}");
        for (name, value) in fields {
            out.push_str(&format!(",{}:{value}", quote(name)));
        }
        out.push_str("}\n");
        self.0.write_all(out.as_bytes()).expect("the log can be written");
    }

    fn now(&mut self, fields: &[(&str, String)]) {
        self.line(now(), fields);
    }
}

/// A FIFO of commands, opened for writing too so it never reaches its end.
struct Commands {
    fifo: File,
    pending: Vec<u8>,
}

impl Commands {
    fn open(path: &str) -> Self {
        let fifo = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(O_NONBLOCK)
            .open(path)
            .expect("the FIFO can be opened");
        Self {
            fifo,
            pending: Vec::new(),
        }
    }

    fn lines(&mut self) -> Vec<String> {
        let mut buf = [0u8; 4096];
        loop {
            match self.fifo.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => self.pending.extend_from_slice(&buf[..n]),
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => panic!("the FIFO: {error}"),
            }
        }
        let mut lines = Vec::new();
        while let Some(end) = self.pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line).trim().to_string();
            if !line.is_empty() {
                lines.push(line);
            }
        }
        lines
    }
}

fn wait(fds: &mut [PollFd], timeout_ms: i32) {
    // SAFETY: fds is a valid array of pollfd of its length.
    unsafe { poll(fds.as_mut_ptr(), fds.len() as u64, timeout_ms) };
}

// --- The stand-in ---------------------------------------------------------

const UHID_DESTROY: u32 = 1;
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
const UHID_DATA: usize = 4096;
const UHID_EVENT: usize = 4 + 4372;
const BUS_USB: u16 = 3;

/// Written here from what hid-steam reads: Valve's vendor page, the input
/// reports 0x42 (53 bytes after the number), 0x45 (45), 0x43 (14) and
/// 0x79 (1), the output reports 0x80 (9) and 0x81 (7), and the feature
/// reports 1 and 2 (63), each byte a value 0 to 255. With `with_47`, an
/// input report 0x47 of 45 too.
fn own_descriptor(with_47: bool) -> Vec<u8> {
    let report = |id: u8, count: u8, main: u8| [0x85, id, 0x95, count, 0x09, id, main, 0x02];
    let mut descriptor = vec![
        0x06, 0x00, 0xff, // Usage page: vendor 0xff00.
        0x09, 0x01, // Usage 1.
        0xa1, 0x01, // Collection: application.
        0x15, 0x00, 0x26, 0xff, 0x00, // Logical 0 to 255.
        0x75, 0x08, // 8 bits each.
    ];
    for (id, count) in [(0x42, 53), (0x45, 45), (0x43, 14), (0x79, 1)] {
        descriptor.extend(report(id, count, 0x81));
    }
    if with_47 {
        descriptor.extend(report(0x47, 45, 0x81));
    }
    for (id, count) in [(0x80, 9), (0x81, 7)] {
        descriptor.extend(report(id, count, 0x91));
    }
    for id in [0x01, 0x02] {
        descriptor.extend(report(id, 63, 0xb1));
    }
    descriptor.push(0xc0);
    descriptor
}

/// The descriptor a retail controller on a cable gave, as published in
/// Nonary/libvirtualgamepad (captures/sc26-report-descriptor-1.hex, MIT):
/// a mouse (0x40), a keyboard (0x41), and Valve's collection.
const REAL_DESCRIPTOR: &str = "05010902a10185400901a100050919012902150025017501950281027506950181010501093009311581257f750895028106950109388106050c0a380295018106c0c005010906a1018541050719e029e71500250175019508810281011900296515002565750895068100c00600ff0901a1018542150026ff0075089535094281028544150026ff0075089505094481028579150026ff0075089501097981028543150026ff007508950e09438102857b150026ff007508950c097b81028545150026ff007508952d094581028580150026ff0075089509098091028581150026ff0075089507098191028582150026ff0075089503098291028583150026ff0075089509098391028584150026ff0075089508098491028585150026ff0075089503098591028586150026ff0075089503098691028587150026ff007508953f098791028589150026ff007508953f098991028588150026ff007508953f098891028501953f0901b1028502953f0901b102c0";

/// The controller's serial, as the stand-in answers command 0xae for the
/// unit (1) and for the board (0); and the HID device's own unique
/// string, which a real one's USB serial number would be.
const UNIT_SERIAL: &str = "SPIKEUNIT0001";
const BOARD_SERIAL: &str = "SPIKEBOARD001";
const HID_UNIQ: &str = "SPIKEUSB0001";

/// What the stand-in answers a GET of a feature report with, from the
/// command last SET on it; None for an error.
fn feature_reply(rnum: u8, last: Option<&[u8]>) -> Option<Vec<u8>> {
    let last = last?;
    let command = *last.get(1)?;
    let mut reply = vec![0u8; 64];
    reply[0] = rnum;
    reply[1] = command;
    match (rnum, command) {
        (1, 0xae) => {
            let serial = if last.get(3) == Some(&1) { UNIT_SERIAL } else { BOARD_SERIAL };
            reply[2] = 0x15;
            reply[3] = *last.get(3)?;
            reply[4..4 + serial.len()].copy_from_slice(serial.as_bytes());
        }
        (1, 0x83) => {
            // A retail controller's attributes, from the same capture:
            // its product, revision, bootloader and firmware build times,
            // board, and an interval of 4000 µs.
            let attributes = unhex("010213000002000000000a2ef9d26804e3854d6a094a0000000ba00f0000");
            reply[2] = attributes.len() as u8;
            reply[3..3 + attributes.len()].copy_from_slice(&attributes);
        }
        (2, 0xb4) => {
            // Wireless state: connected.
            reply[2] = 1;
            reply[3] = 2;
        }
        _ => {
            // Anything else: the command echoed, as an acknowledgment.
            let n = last.len().min(64);
            reply[..n].copy_from_slice(&last[..n]);
            reply[0] = rnum;
        }
    }
    Some(reply)
}

fn field(event: &mut Vec<u8>, text: &str, width: usize) {
    let mut bytes = text.as_bytes().to_vec();
    bytes.resize(width, 0);
    event.extend_from_slice(&bytes);
}

fn word(event: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes(event[at..at + 4].try_into().expect("four bytes"))
}

fn half(event: &[u8], at: usize) -> u16 {
    u16::from_ne_bytes(event[at..at + 2].try_into().expect("two bytes"))
}

/// An input report of the 2026 layout: the sequence, 32 bits of buttons,
/// the triggers, the left stick at (lx, ly), the right stick, both
/// trackpads' places (which the kernel reports only with their touch
/// bits among the buttons), the sensors' clock at 4000 µs a report, and
/// a reading of the accelerometer and gyroscope.
fn state_report(id: u8, sequence: u32, buttons: u32, lx: i16, ly: i16) -> Vec<u8> {
    let size = if id == 0x42 { 54 } else { 46 };
    let mut report = vec![0u8; size];
    report[0] = id;
    report[1] = sequence as u8;
    report[2..6].copy_from_slice(&buttons.to_le_bytes());
    let mut put = |at: usize, value: i16| report[at..at + 2].copy_from_slice(&value.to_le_bytes());
    put(6, 1000); // Left trigger.
    put(8, 2000); // Right trigger.
    put(10, lx);
    put(12, ly);
    put(14, 3000); // Right stick.
    put(16, -3000);
    put(18, 1234); // Left trackpad.
    put(20, -2345);
    put(24, 3456); // Right trackpad.
    put(26, -4567);
    put(34, 100); // Accelerometer.
    put(36, -200);
    put(38, 16384);
    put(40, 16); // Gyroscope.
    put(42, -32);
    put(44, 48);
    report[30..34].copy_from_slice(&(sequence.wrapping_mul(4000)).to_le_bytes());
    report
}

fn fake(variant: &str, log_path: &str, fifo_path: &str) {
    let mut log = Log::open(log_path);
    let descriptor = match variant {
        "own" => own_descriptor(false),
        "own47" => own_descriptor(true),
        "real" => unhex(REAL_DESCRIPTOR),
        other => panic!("no variant {other}"),
    };
    let mut commands = Commands::open(fifo_path);
    let mut uhid = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(O_NONBLOCK)
        .open("/dev/uhid")
        .expect("uhid can be opened");

    let mut event = Vec::with_capacity(UHID_EVENT);
    event.extend_from_slice(&UHID_CREATE2.to_ne_bytes());
    field(&mut event, "Spike Steam Controller", 128);
    field(&mut event, "usb-spike-1/input0", 64);
    field(&mut event, HID_UNIQ, 64);
    event.extend_from_slice(&(descriptor.len() as u16).to_ne_bytes());
    event.extend_from_slice(&BUS_USB.to_ne_bytes());
    event.extend_from_slice(&0x28deu32.to_ne_bytes());
    event.extend_from_slice(&0x1302u32.to_ne_bytes());
    event.extend_from_slice(&0x0307u32.to_ne_bytes());
    event.extend_from_slice(&0u32.to_ne_bytes());
    let mut padded = descriptor.clone();
    padded.resize(UHID_DATA, 0);
    event.extend_from_slice(&padded);
    let created = now();
    uhid.write_all(&event).expect("uhid takes the device");
    log.line(
        created,
        &[
            ("ev", quote("create")),
            ("variant", quote(variant)),
            ("descriptor", quote(&hex(&descriptor))),
        ],
    );

    // Input reports due, by when.
    let mut due: Vec<(f64, Vec<u8>)> = Vec::new();
    // The last command SET on each feature report, by number.
    let mut last: [Option<Vec<u8>>; 256] = std::array::from_fn(|_| None);
    let mut buf = vec![0u8; UHID_EVENT];
    loop {
        let timeout = due
            .iter()
            .map(|(when, _)| ((when - now()) * 1000.0).ceil().max(0.0) as i32)
            .min()
            .unwrap_or(1000);
        let mut fds = [
            PollFd { fd: uhid.as_raw_fd(), events: POLLIN, revents: 0 },
            PollFd { fd: commands.fifo.as_raw_fd(), events: POLLIN, revents: 0 },
        ];
        wait(&mut fds, timeout);

        loop {
            match uhid.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => panic!("uhid: {error}"),
            }
            let t = now();
            match word(&buf, 0) {
                UHID_START => {
                    let flags = u64::from_ne_bytes(buf[4..12].try_into().expect("eight bytes"));
                    log.line(t, &[("ev", quote("start")), ("flags", flags.to_string())]);
                }
                UHID_STOP => log.line(t, &[("ev", quote("stop"))]),
                UHID_OPEN => log.line(t, &[("ev", quote("open"))]),
                UHID_CLOSE => log.line(t, &[("ev", quote("close"))]),
                UHID_OUTPUT => {
                    let size = usize::from(half(&buf, 4 + UHID_DATA)).min(UHID_DATA);
                    let rtype = buf[4 + UHID_DATA + 2];
                    log.line(
                        t,
                        &[
                            ("ev", quote("output")),
                            ("rtype", rtype.to_string()),
                            ("data", quote(&hex(&buf[4..4 + size]))),
                        ],
                    );
                }
                UHID_GET_REPORT => {
                    let id = word(&buf, 4);
                    let (rnum, rtype) = (buf[8], buf[9]);
                    let reply = feature_reply(rnum, last[usize::from(rnum)].as_deref());
                    let mut answer = UHID_GET_REPORT_REPLY.to_ne_bytes().to_vec();
                    answer.extend_from_slice(&id.to_ne_bytes());
                    match &reply {
                        Some(reply) => {
                            answer.extend_from_slice(&0u16.to_ne_bytes());
                            answer.extend_from_slice(&(reply.len() as u16).to_ne_bytes());
                            answer.extend_from_slice(reply);
                        }
                        None => {
                            answer.extend_from_slice(&5u16.to_ne_bytes());
                            answer.extend_from_slice(&0u16.to_ne_bytes());
                        }
                    }
                    uhid.write_all(&answer).expect("uhid takes the reply");
                    log.line(
                        t,
                        &[
                            ("ev", quote("get_report")),
                            ("rnum", rnum.to_string()),
                            ("rtype", rtype.to_string()),
                            ("reply", reply.map_or("null".into(), |reply| quote(&hex(&reply)))),
                        ],
                    );
                }
                UHID_SET_REPORT => {
                    let id = word(&buf, 4);
                    let (rnum, rtype) = (buf[8], buf[9]);
                    let size = usize::from(half(&buf, 10)).min(UHID_DATA);
                    let data = buf[12..12 + size].to_vec();
                    let mut answer = UHID_SET_REPORT_REPLY.to_ne_bytes().to_vec();
                    answer.extend_from_slice(&id.to_ne_bytes());
                    answer.extend_from_slice(&0u16.to_ne_bytes());
                    uhid.write_all(&answer).expect("uhid takes the reply");
                    log.line(
                        t,
                        &[
                            ("ev", quote("set_report")),
                            ("rnum", rnum.to_string()),
                            ("rtype", rtype.to_string()),
                            ("data", quote(&hex(&data))),
                        ],
                    );
                    last[usize::from(rnum)] = Some(data);
                }
                other => log.line(t, &[("ev", quote("uhid")), ("type", other.to_string())]),
            }
        }

        for line in commands.lines() {
            log.now(&[("ev", quote("command")), ("line", quote(&line))]);
            let words: Vec<&str> = line.split_whitespace().collect();
            match words.as_slice() {
                ["send", data] => due.push((now(), unhex(data))),
                ["state", id, buttons, lx, ly, count, every] => {
                    let id = u8::from_str_radix(id, 16).expect("an id in hex");
                    let buttons = u32::from_str_radix(buttons, 16).expect("buttons in hex");
                    let (lx, ly): (i16, i16) = (lx.parse().expect("lx"), ly.parse().expect("ly"));
                    let count: u32 = count.parse().expect("a count");
                    let every: f64 = every.parse().expect("an interval");
                    let start = now();
                    for n in 0..count {
                        due.push((start + f64::from(n) * every / 1000.0, state_report(id, n, buttons, lx, ly)));
                    }
                }
                ["quit"] => {
                    let destroy = UHID_DESTROY.to_ne_bytes();
                    uhid.write_all(&destroy).expect("uhid takes the destroy");
                    log.now(&[("ev", quote("destroyed"))]);
                    return;
                }
                _ => log.now(&[("ev", quote("unknown command"))]),
            }
        }

        due.sort_by(|a, b| a.0.total_cmp(&b.0));
        let t = now();
        while due.first().is_some_and(|(when, _)| *when <= t) {
            let (_, report) = due.remove(0);
            let mut event = UHID_INPUT2.to_ne_bytes().to_vec();
            event.extend_from_slice(&(report.len() as u16).to_ne_bytes());
            event.extend_from_slice(&report);
            let sent = now();
            let result = uhid.write(&event);
            log.line(
                sent,
                &[
                    ("ev", quote("sent")),
                    ("data", quote(&hex(&report))),
                    ("ok", result.is_ok().to_string()),
                ],
            );
        }
    }
}

// --- The raw node --------------------------------------------------------

const fn ioc(dir: u64, kind: u8, nr: u8, size: usize) -> u64 {
    (dir << 30) | ((size as u64) << 16) | ((kind as u64) << 8) | nr as u64
}
const IOC_WRITE: u64 = 1;
const IOC_READ: u64 = 2;

fn raw_string(fd: i32, nr: u8) -> String {
    let mut buf = [0u8; 256];
    // SAFETY: buf is as long as the size the request names.
    let n = unsafe { ioctl(fd, ioc(IOC_READ, b'H', nr, buf.len()), buf.as_mut_ptr()) };
    if n < 0 {
        return format!("<error {}>", std::io::Error::last_os_error());
    }
    let bytes = &buf[..(n as usize).min(buf.len())];
    let end = bytes.iter().position(|byte| *byte == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

fn raw(node: &str, log_path: &str, fifo_path: &str) {
    let mut log = Log::open(log_path);
    let mut commands = Commands::open(fifo_path);
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(O_NONBLOCK)
        .open(node)
        .expect("the raw node can be opened");
    let opened = now();
    let fd = file.as_raw_fd();
    let mut info = [0u8; 8];
    // SAFETY: info is a hidraw_devinfo's size.
    unsafe { ioctl(fd, ioc(IOC_READ, b'H', 0x03, 8), info.as_mut_ptr()) };
    let mut size: i32 = 0;
    // SAFETY: size is an int.
    unsafe { ioctl(fd, ioc(IOC_READ, b'H', 0x01, 4), &mut size) };
    let mut descriptor = vec![0u8; 4 + 4096];
    descriptor[..4].copy_from_slice(&(size as u32).to_ne_bytes());
    // SAFETY: descriptor is a hidraw_report_descriptor's size.
    unsafe { ioctl(fd, ioc(IOC_READ, b'H', 0x02, 4 + 4096), descriptor.as_mut_ptr()) };
    log.line(
        opened,
        &[
            ("ev", quote("opened")),
            ("node", quote(node)),
            ("bus", word(&info, 0).to_string()),
            ("vendor", quote(&format!("{:04x}", half(&info, 4)))),
            ("product", quote(&format!("{:04x}", half(&info, 6)))),
            ("name", quote(&raw_string(fd, 0x04))),
            ("phys", quote(&raw_string(fd, 0x05))),
            ("uniq", quote(&raw_string(fd, 0x08))),
            ("descriptor", quote(&hex(&descriptor[4..4 + size.max(0) as usize]))),
        ],
    );
    let mut buf = [0u8; 4096];
    loop {
        let mut fds = [
            PollFd { fd, events: POLLIN, revents: 0 },
            PollFd { fd: commands.fifo.as_raw_fd(), events: POLLIN, revents: 0 },
        ];
        wait(&mut fds, 1000);
        loop {
            match file.read(&mut buf) {
                Ok(n) => log.now(&[("ev", quote("read")), ("data", quote(&hex(&buf[..n])))]),
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) => {
                    log.now(&[("ev", quote("read error")), ("error", quote(&error.to_string()))]);
                    return;
                }
            }
        }
        for line in commands.lines() {
            log.now(&[("ev", quote("command")), ("line", quote(&line))]);
            let words: Vec<&str> = line.split_whitespace().collect();
            match words.as_slice() {
                ["write", data] => {
                    let data = unhex(data);
                    let result = file.write(&data);
                    log.now(&[("ev", quote("wrote")), ("result", quote(&format!("{result:?}")))]);
                }
                [request @ ("setfeature" | "getfeature"), data] => {
                    let mut data = unhex(data);
                    let nr = if *request == "setfeature" { 0x06 } else { 0x07 };
                    // SAFETY: data is as long as the size the request names.
                    let result = unsafe {
                        ioctl(fd, ioc(IOC_READ | IOC_WRITE, b'H', nr, data.len()), data.as_mut_ptr())
                    };
                    let error = if result < 0 {
                        std::io::Error::last_os_error().to_string()
                    } else {
                        String::new()
                    };
                    log.now(&[
                        ("ev", quote(request)),
                        ("result", result.to_string()),
                        ("error", quote(&error)),
                        ("data", quote(&hex(&data))),
                    ]);
                }
                ["quit"] => {
                    log.now(&[("ev", quote("closing"))]);
                    return;
                }
                _ => log.now(&[("ev", quote("unknown command"))]),
            }
        }
    }
}

// --- Event nodes and uevents ---------------------------------------------

fn events(node: &str, log_path: &str) {
    let mut log = Log::open(log_path);
    let mut file = File::open(node).expect("the event node can be opened");
    let fd = file.as_raw_fd();
    let clock: i32 = CLOCK_MONOTONIC;
    // SAFETY: EVIOCSCLOCKID takes a pointer to an int.
    unsafe { ioctl(fd, ioc(IOC_WRITE, b'E', 0xa0, 4), &clock) };
    let mut name = [0u8; 256];
    // SAFETY: name is as long as the size EVIOCGNAME names.
    unsafe { ioctl(fd, ioc(IOC_READ, b'E', 0x06, name.len()), name.as_mut_ptr()) };
    let end = name.iter().position(|byte| *byte == 0).unwrap_or(name.len());
    log.now(&[
        ("ev", quote("opened")),
        ("node", quote(node)),
        ("name", quote(&String::from_utf8_lossy(&name[..end]))),
    ]);
    let mut buf = [0u8; 24 * 64];
    loop {
        let n = match file.read(&mut buf) {
            Ok(n) => n,
            Err(error) => {
                log.now(&[("ev", quote("read error")), ("error", quote(&error.to_string()))]);
                return;
            }
        };
        if n == 0 {
            log.now(&[("ev", quote("end"))]);
            return;
        }
        for event in buf[..n].chunks_exact(24) {
            let sec = i64::from_ne_bytes(event[0..8].try_into().expect("eight bytes"));
            let usec = i64::from_ne_bytes(event[8..16].try_into().expect("eight bytes"));
            let kind = half(event, 16);
            let code = half(event, 18);
            let value = i32::from_ne_bytes(event[20..24].try_into().expect("four bytes"));
            log.line(
                sec as f64 + usec as f64 / 1e6,
                &[("type", kind.to_string()), ("code", code.to_string()), ("value", value.to_string())],
            );
        }
    }
}

fn uevents(log_path: &str) {
    let mut log = Log::open(log_path);
    const AF_NETLINK: i32 = 16;
    const SOCK_DGRAM: i32 = 2;
    const NETLINK_KOBJECT_UEVENT: i32 = 15;
    // SAFETY: a plain socket call.
    let fd = unsafe { socket(AF_NETLINK, SOCK_DGRAM, NETLINK_KOBJECT_UEVENT) };
    assert!(fd >= 0, "netlink: {}", std::io::Error::last_os_error());
    // sockaddr_nl: family, padding, pid, groups: the kernel's group.
    let mut address = [0u8; 12];
    address[0..2].copy_from_slice(&(AF_NETLINK as u16).to_ne_bytes());
    address[8..12].copy_from_slice(&1u32.to_ne_bytes());
    // SAFETY: address is a sockaddr_nl of its length.
    let bound = unsafe { bind(fd, address.as_ptr().cast(), 12) };
    assert!(bound == 0, "bind: {}", std::io::Error::last_os_error());
    log.now(&[("ev", quote("listening"))]);
    let mut buf = vec![0u8; 16384];
    loop {
        // SAFETY: buf is valid for its length.
        let n = unsafe { recv(fd, buf.as_mut_ptr().cast(), buf.len(), 0) };
        let t = now();
        if n <= 0 {
            continue;
        }
        let parts: Vec<String> = buf[..n as usize]
            .split(|byte| *byte == 0)
            .filter(|part| !part.is_empty())
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect();
        let mut fields = vec![("header", quote(&parts[0]))];
        let mut properties = String::from("{");
        for (index, part) in parts[1..].iter().enumerate() {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            if index > 0 {
                properties.push(',');
            }
            properties.push_str(&format!("{}:{}", quote(key), quote(value)));
        }
        properties.push('}');
        fields.push(("properties", properties));
        log.line(t, &fields);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        [_, "fake", variant, log, fifo] => fake(variant, log, fifo),
        [_, "raw", node, log, fifo] => raw(node, log, fifo),
        [_, "events", node, log] => events(node, log),
        [_, "uevents", log] => uevents(log),
        [_, "now"] => {
            let mut out = std::io::stdout();
            let _ = writeln!(out, "{:.6}", now());
        }
        _ => {
            let mut err = std::io::stderr();
            let _ = writeln!(err, "usage: see the top of src/main.rs");
            std::process::exit(2);
        }
    }
}
