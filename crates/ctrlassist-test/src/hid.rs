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
}

impl Drop for FakeDualSense {
    fn drop(&mut self) {
        drop(self.states.take());
        if let Some(serving) = self.serving.take() {
            let _ = serving.join();
        }
    }
}
