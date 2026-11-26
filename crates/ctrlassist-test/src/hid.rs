//! A fake pad at the HID level.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};

/// A generic HID game pad: eight buttons, then X and Y as signed bytes.
const DESCRIPTOR: &[u8] = &[
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

// Event types from linux/uhid.h.
const UHID_CREATE2: u32 = 11;
const UHID_INPUT2: u32 = 12;
/// The bus type for USB, from linux/input.h.
const BUS_USB: u16 = 3;

/// A gamepad made through uhid.
///
/// The kernel gives it a HID device with the generic driver bound, an
/// event node and a hidraw node, so it can stand in for a pad whose driver
/// is re-bound or whose hidraw node is read. Dropping it removes it.
pub struct FakeHidPad {
    uhid: File,
}

impl std::fmt::Debug for FakeHidPad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeHidPad").finish_non_exhaustive()
    }
}

/// Appends `text` as a fixed-width, zero-terminated field.
fn field(event: &mut Vec<u8>, text: &str, width: usize) {
    let mut bytes = text.as_bytes().to_vec();
    bytes.truncate(width - 1);
    bytes.resize(width, 0);
    event.extend_from_slice(&bytes);
}

impl FakeHidPad {
    /// Creates the pad. `uniq` is the address it reports, which is how a
    /// real pad is told from another of the same model.
    ///
    /// # Errors
    ///
    /// Fails if uhid can't be opened or the kernel refuses the pad.
    pub fn new(name: &str, uniq: &str, vendor: u16, product: u16) -> io::Result<Self> {
        let mut uhid = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/uhid")?;
        // UHID_CREATE2, packed: type u32, name[128], phys[64], uniq[64],
        // rd_size u16, bus u16, vendor u32, product u32, version u32,
        // country u32, rd_data[4096].
        let mut event = Vec::with_capacity(4 + 4372);
        event.extend_from_slice(&UHID_CREATE2.to_ne_bytes());
        field(&mut event, name, 128);
        // Marked as the tests', by where it says it is plugged.
        field(
            &mut event,
            &format!("{}hid", ctrlassist_device::TEST_MARKER),
            64,
        );
        field(&mut event, uniq, 64);
        let size = u16::try_from(DESCRIPTOR.len())
            .expect("the descriptor is a small constant, so its length fits in u16");
        event.extend_from_slice(&size.to_ne_bytes());
        event.extend_from_slice(&BUS_USB.to_ne_bytes());
        event.extend_from_slice(&u32::from(vendor).to_ne_bytes());
        event.extend_from_slice(&u32::from(product).to_ne_bytes());
        event.extend_from_slice(&1u32.to_ne_bytes());
        event.extend_from_slice(&0u32.to_ne_bytes());
        let mut descriptor = DESCRIPTOR.to_vec();
        descriptor.resize(4096, 0);
        event.extend_from_slice(&descriptor);
        uhid.write_all(&event)?;
        Ok(Self { uhid })
    }

    /// Sends one input report: the eight buttons as bits, then X and Y.
    ///
    /// # Errors
    ///
    /// Fails if the kernel refuses the write.
    pub fn report(&mut self, buttons: u8, x: i8, y: i8) -> io::Result<()> {
        // UHID_INPUT2: type u32, size u16, then the report.
        let mut event = UHID_INPUT2.to_ne_bytes().to_vec();
        event.extend_from_slice(&3u16.to_ne_bytes());
        event.extend_from_slice(&[buttons, x.to_ne_bytes()[0], y.to_ne_bytes()[0]]);
        self.uhid.write_all(&event)
    }
}

/// A report descriptor that declares the reports the kernel's PlayStation
/// driver exchanges with a DualSense over USB, by number and size, as
/// vendor-defined data: one input report, one output report and three
/// feature reports. It is nothing of Sony's; a real pad's descriptor
/// says what each byte is, which only a program reading the raw node
/// itself goes by.
const SYNTHETIC_DUALSENSE: [u8; 55] = [
    0x06, 0x00, 0xff, // Usage page: vendor-defined.
    0x09, 0x01, // Usage 1.
    0xa1, 0x01, // Collection: application.
    0x15, 0x00, // Logical minimum 0.
    0x26, 0xff, 0x00, // Logical maximum 255.
    0x75, 0x08, // Report size: 8 bits.
    0x85, 0x01, 0x95, 0x3f, 0x09, 0x02, 0x81, 0x02, // Report 1: 63 bytes in.
    0x85, 0x02, 0x95, 0x3f, 0x09, 0x03, 0x91, 0x02, // Report 2: 63 bytes out.
    0x85, 0x05, 0x95, 0x28, 0x09, 0x04, 0xb1, 0x02, // Feature 5: 40 bytes.
    0x85, 0x09, 0x95, 0x13, 0x09, 0x05, 0xb1, 0x02, // Feature 9: 19 bytes.
    0x85, 0x20, 0x95, 0x3f, 0x09, 0x06, 0xb1, 0x02, // Feature 32: 63 bytes.
    0xc0, // End of the collection.
];

/// What a virtual DualSense can be made from where no real one has been
/// copied: a descriptor of this crate's own, and a
/// firmware report that says only which way the pad takes rumble, the
/// newer way if `newer`.
#[must_use]
pub fn synthetic_dualsense(newer: bool) -> ctrlassist_device::DualSenseData {
    let mut firmware = vec![0u8; 64];
    firmware[0] = 0x20;
    if newer {
        // The update version, 2.21, at which the driver changes how it
        // marks rumble in an output report.
        firmware[44..46].copy_from_slice(&0x0215u16.to_le_bytes());
    }
    ctrlassist_device::DualSenseData {
        descriptor: SYNTHETIC_DUALSENSE.to_vec(),
        firmware,
    }
}

/// A virtual DualSense that stands in for a connected real one: made
/// from [`synthetic_dualsense`], with an address that isn't one of the
/// daemon's own, so a daemon that takes the tests' pads takes it for a
/// pad to play with and to copy. It is
/// served on a thread of its own, as a real pad answers whoever reads
/// its raw node. Dropping it disconnects it.
pub struct FakeDualSense {
    outputs: std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    states: Option<std::sync::mpsc::Sender<ctrlassist_core::PadState>>,
    serving: Option<std::thread::JoinHandle<()>>,
    node: std::path::PathBuf,
}

impl std::fmt::Debug for FakeDualSense {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeDualSense")
            .field("node", &self.node)
            .finish_non_exhaustive()
    }
}

impl FakeDualSense {
    /// Creates the pad with the address `02:ca:7e:00:51:<last>`, which
    /// marks it as the tests'.
    ///
    /// # Errors
    ///
    /// Fails if uhid can't be opened or the kernel's PlayStation driver
    /// doesn't take the pad.
    pub fn new(last: u8) -> io::Result<Self> {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        let data = synthetic_dualsense(true);
        let address = [0x02, 0xca, 0x7e, 0x00, 0x51, last];
        let mut pad = ctrlassist_device::VirtualPad::dualsense(&data, address)?;
        let node = pad.event_node().to_path_buf();
        let (states, wanted) = std::sync::mpsc::channel();
        let outputs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let noted = std::sync::Arc::clone(&outputs);
        let serving = std::thread::Builder::new()
            .name("fake-dualsense".into())
            .spawn(move || {
                loop {
                    // Nothing says a state is waiting, so look often.
                    let mut fds = [PollFd::new(&pad, PollFlags::IN)];
                    let wait = Timespec {
                        tv_sec: 0,
                        tv_nsec: 5_000_000,
                    };
                    let _ = poll(&mut fds, Some(&wait));
                    if pad.serve(|_| Ok(())).is_err() {
                        return;
                    }
                    if let Some(report) = pad.take_output() {
                        noted
                            .lock()
                            .expect("nothing panics while the log is held")
                            .push(report.to_vec());
                    }
                    loop {
                        match wanted.try_recv() {
                            Ok(state) => {
                                let _ = pad.write(&state);
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => break,
                            // Dropped: the pad goes with this thread.
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                        }
                    }
                }
            })?;
        Ok(Self {
            outputs,
            states: Some(states),
            serving: Some(serving),
            node,
        })
    }

    /// The pad's event node.
    #[must_use]
    pub fn event_node(&self) -> &std::path::Path {
        &self.node
    }

    /// The output reports the pad was sent, in order, each as a real
    /// pad would have got it. Reports that arrive before the pad is
    /// looked at again are kept as the last of them.
    ///
    /// # Panics
    ///
    /// Panics if the serving thread panicked while noting one.
    #[must_use]
    pub fn outputs(&self) -> Vec<Vec<u8>> {
        self.outputs
            .lock()
            .expect("nothing panics while the log is held")
            .clone()
    }

    /// Presses or releases the south button.
    pub fn south(&self, pressed: bool) {
        let state = ctrlassist_core::PadState::NEUTRAL.with(
            ctrlassist_core::gamepad::SOUTH,
            f32::from(u8::from(pressed)),
        );
        if let Some(states) = &self.states {
            let _ = states.send(state);
        }
    }

    /// Reports `state`, its motion and touchpad with it, as the virtual
    /// DualSense it is made from writes them: a new count of readings is
    /// a reading, reported though nothing else changed.
    pub fn hold(&self, state: ctrlassist_core::PadState) {
        if let Some(states) = &self.states {
            let _ = states.send(state);
        }
    }
}

impl Drop for FakeDualSense {
    fn drop(&mut self) {
        drop(self.states.take());
        if let Some(serving) = self.serving.take() {
            let _ = serving.join();
        }
    }
}

/// A report descriptor that declares the reports the kernel's
/// PlayStation driver exchanges with a DualSense over Bluetooth, by
/// number and size, as vendor-defined data: one input and one output
/// report under the same number, and three feature reports. It is
/// nothing of Sony's.
const SYNTHETIC_BLUETOOTH_DUALSENSE: [u8; 55] = [
    0x06, 0x00, 0xff, // Usage page: vendor-defined.
    0x09, 0x01, // Usage 1.
    0xa1, 0x01, // Collection: application.
    0x15, 0x00, // Logical minimum 0.
    0x26, 0xff, 0x00, // Logical maximum 255.
    0x75, 0x08, // Report size: 8 bits.
    0x85, 0x31, 0x95, 0x4d, 0x09, 0x02, 0x81, 0x02, // Report 49: 77 bytes in.
    0x85, 0x31, 0x95, 0x4d, 0x09, 0x03, 0x91, 0x02, // Report 49: 77 bytes out.
    0x85, 0x05, 0x95, 0x28, 0x09, 0x04, 0xb1, 0x02, // Feature 5: 40 bytes.
    0x85, 0x09, 0x95, 0x13, 0x09, 0x05, 0xb1, 0x02, // Feature 9: 19 bytes.
    0x85, 0x20, 0x95, 0x3f, 0x09, 0x06, 0xb1, 0x02, // Feature 32: 63 bytes.
    0xc0, // End of the collection.
];

/// The bus type for Bluetooth, from linux/input.h.
const BUS_BLUETOOTH: u16 = 5;
const UHID_OUTPUT: u32 = 6;
const UHID_GET_REPORT: u32 = 9;
const UHID_GET_REPORT_REPLY: u32 = 10;
const UHID_SET_REPORT: u32 = 13;
const UHID_SET_REPORT_REPLY: u32 = 14;
/// The size of a uhid event's data, and of the largest event.
const UHID_DATA: usize = 4096;
const UHID_EVENT: usize = 4 + 4372;

/// The CRC-32 of `seed` followed by `data`: what a PlayStation pad and
/// the kernel's driver end a report with over Bluetooth, the seed
/// saying which way the report goes. Written apart from the daemon's
/// own, so that each checks the other.
#[must_use]
pub fn playstation_crc32(seed: u8, data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in std::iter::once(&seed).chain(data) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A feature report as a DualSense answers one over Bluetooth: its
/// number, zeroes but for `fill`, and a checksum in its last four bytes.
fn bluetooth_feature(number: u8, size: usize, fill: impl FnOnce(&mut [u8])) -> Vec<u8> {
    let mut report = vec![0u8; size];
    report[0] = number;
    fill(&mut report);
    let checksum = playstation_crc32(0xa3, &report[..size - 4]);
    report[size - 4..].copy_from_slice(&checksum.to_le_bytes());
    report
}

/// A stand-in for a DualSense connected over Bluetooth: a uhid device
/// on that bus with the DualSense's numbers, which the kernel's
/// PlayStation driver binds as one, answering the driver's feature
/// requests with checksums as a pad does there. It reports no input.
/// What it is sent is kept, as it arrived: the driver's own output
/// reports, and whatever is written to its raw node.
pub struct FakeBluetoothDualSense {
    outputs: std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    serving: Option<std::thread::JoinHandle<()>>,
    address: String,
}

impl std::fmt::Debug for FakeBluetoothDualSense {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeBluetoothDualSense")
            .field("address", &self.address)
            .finish_non_exhaustive()
    }
}

impl FakeBluetoothDualSense {
    /// Creates the pad with the address `02:ca:7e:00:b7:<last>`, which
    /// marks it as the tests'.
    ///
    /// # Errors
    ///
    /// Fails if uhid can't be opened or the kernel refuses the pad.
    pub fn new(last: u8) -> io::Result<Self> {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        use std::io::Read as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        use std::sync::atomic::Ordering;

        let address = [0x02, 0xca, 0x7e, 0x00, 0xb7, last];
        let text = address.map(|byte| format!("{byte:02x}")).join(":");
        let mut uhid = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
            .open("/dev/uhid")?;
        let mut event = Vec::with_capacity(UHID_EVENT);
        event.extend_from_slice(&UHID_CREATE2.to_ne_bytes());
        field(&mut event, "Fake Bluetooth DualSense", 128);
        field(
            &mut event,
            &format!("{}bluetooth", ctrlassist_device::TEST_MARKER),
            64,
        );
        field(&mut event, &text, 64);
        let size = u16::try_from(SYNTHETIC_BLUETOOTH_DUALSENSE.len())
            .expect("the descriptor is a small constant, so its length fits in u16");
        event.extend_from_slice(&size.to_ne_bytes());
        event.extend_from_slice(&BUS_BLUETOOTH.to_ne_bytes());
        event.extend_from_slice(&0x054cu32.to_ne_bytes());
        event.extend_from_slice(&0x0ce6u32.to_ne_bytes());
        event.extend_from_slice(&0x8100u32.to_ne_bytes());
        event.extend_from_slice(&0u32.to_ne_bytes());
        let mut descriptor = SYNTHETIC_BLUETOOTH_DUALSENSE.to_vec();
        descriptor.resize(UHID_DATA, 0);
        event.extend_from_slice(&descriptor);
        uhid.write_all(&event)?;

        // The address as the pad says it, last byte first; and the
        // update version at which the driver marks rumble the newer way.
        let pairing = bluetooth_feature(0x09, 20, |report| {
            for (place, byte) in report[1..7].iter_mut().zip(address.iter().rev()) {
                *place = *byte;
            }
        });
        let firmware = bluetooth_feature(0x20, 64, |report| {
            report[44..46].copy_from_slice(&0x0215u16.to_le_bytes());
        });
        let calibration = bluetooth_feature(0x05, 41, |_| {});

        let outputs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (noted, stopping) = (
            std::sync::Arc::clone(&outputs),
            std::sync::Arc::clone(&stop),
        );
        let serving = std::thread::Builder::new()
            .name("fake-bluetooth-dualsense".into())
            .spawn(move || {
                let mut event = vec![0u8; UHID_EVENT];
                let word = |event: &[u8], at: usize| {
                    u32::from_ne_bytes([event[at], event[at + 1], event[at + 2], event[at + 3]])
                };
                // Dropped with this thread, which lets the pad go.
                while !stopping.load(Ordering::Acquire) {
                    let mut fds = [PollFd::new(&uhid, PollFlags::IN)];
                    let wait = Timespec {
                        tv_sec: 0,
                        tv_nsec: 5_000_000,
                    };
                    let _ = poll(&mut fds, Some(&wait));
                    loop {
                        match uhid.read(&mut event) {
                            Ok(0) | Err(_) => break,
                            Ok(_) => {}
                        }
                        match word(&event, 0) {
                            // data[4096], then its size.
                            UHID_OUTPUT => {
                                let at = 4 + UHID_DATA;
                                let size =
                                    usize::from(u16::from_ne_bytes([event[at], event[at + 1]]));
                                noted
                                    .lock()
                                    .expect("nothing panics while the log is held")
                                    .push(event[4..4 + size.min(UHID_DATA)].to_vec());
                            }
                            // The request's number, then the report's.
                            UHID_GET_REPORT => {
                                let report = match event[8] {
                                    0x05 => Some(&calibration),
                                    0x09 => Some(&pairing),
                                    0x20 => Some(&firmware),
                                    _ => None,
                                };
                                // The request's number, an error, a size,
                                // the report.
                                let mut reply = UHID_GET_REPORT_REPLY.to_ne_bytes().to_vec();
                                reply.extend_from_slice(&event[4..8]);
                                match report {
                                    Some(report) => {
                                        reply.extend_from_slice(&0u16.to_ne_bytes());
                                        reply.extend_from_slice(
                                            &(report.len() as u16).to_ne_bytes(),
                                        );
                                        reply.extend_from_slice(report);
                                    }
                                    None => {
                                        reply.extend_from_slice(&5u16.to_ne_bytes());
                                        reply.extend_from_slice(&0u16.to_ne_bytes());
                                    }
                                }
                                let _ = uhid.write_all(&reply);
                            }
                            UHID_SET_REPORT => {
                                let mut reply = UHID_SET_REPORT_REPLY.to_ne_bytes().to_vec();
                                reply.extend_from_slice(&event[4..8]);
                                reply.extend_from_slice(&0u16.to_ne_bytes());
                                let _ = uhid.write_all(&reply);
                            }
                            _ => {}
                        }
                    }
                }
            })?;
        Ok(Self {
            outputs,
            stop,
            serving: Some(serving),
            address: text,
        })
    }

    /// The pad's event node, once the kernel's driver has made it;
    /// nothing if the driver hasn't taken the pad.
    #[must_use]
    pub fn event_node(&self) -> Option<std::path::PathBuf> {
        let id = format!("address:{}", self.address);
        for _ in 0..400 {
            let found = ctrlassist_device::connected()
                .ok()
                .into_iter()
                .flatten()
                .find(|found| found.identifiers.id().to_string() == id);
            if let Some(found) = found {
                return Some(found.node);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        None
    }

    /// The output reports the pad was sent, in order, each as it
    /// arrived: over Bluetooth, with its number, sequence and checksum.
    ///
    /// # Panics
    ///
    /// Panics if the serving thread panicked while noting one.
    #[must_use]
    pub fn outputs(&self) -> Vec<Vec<u8>> {
        self.outputs
            .lock()
            .expect("nothing panics while the log is held")
            .clone()
    }
}

impl Drop for FakeBluetoothDualSense {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(serving) = self.serving.take() {
            let _ = serving.join();
        }
    }
}
