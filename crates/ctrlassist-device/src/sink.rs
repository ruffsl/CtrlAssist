//! A virtual pad: what a game reads in place of the pads behind it. Made
//! through uinput, or through uhid for an identity games read raw.

use std::ffi::CString;
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use ctrlassist_core::{Control, Effect, FeedbackError, Level, PadState, gamepad};

use crate::dualsense::{self, DualSenseData, Emulator};
use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, EventSummary, EventType, FFEffectCode,
    FFEffectKind, InputEvent, InputId, KeyCode, UInputCode, UinputAbsSetup,
};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};

/// How the daemon's virtual pads are told from every other pad, by any
/// process: the start of their `phys` string, which uinput lets its
/// creator set and nothing else on the machine uses.
pub const OWN_MARKER: &str = "ctrlassist/virtual/";

/// How the daemon's virtual pads made through uhid are told from every
/// other pad: the start of the address each reports, which is marked as
/// locally administered, so no pad's maker gave it out. The kernel's
/// PlayStation driver carries a pad's address to its input devices and
/// nothing else its creator sets.
pub const OWN_ADDRESS: &str = "02:ca:55:";

/// How the fake pads the tests make are told from every other pad: the
/// start of their `phys` string. A daemon takes no such pad for one to
/// play with unless it is asked to, so a test run beside a daemon in
/// use leaves that daemon's pads alone.
pub const TEST_MARKER: &str = "ctrlassist/test/";

/// The same for a fake pad of a driver that carries no `phys` to its
/// input devices: the start of the address it reports.
pub const TEST_ADDRESS: &str = "02:ca:7e:";

/// The address of the daemon's virtual pad numbered `number`: the
/// [marker](OWN_ADDRESS), then the number's low three bytes.
#[must_use]
pub fn own_address(number: u32) -> [u8; 6] {
    let [_, a, b, c] = number.to_be_bytes();
    [0x02, 0xca, 0x55, a, b, c]
}

/// The most effects a game may hold on one virtual pad.
const EFFECTS: u32 = ctrlassist_core::EFFECTS as u32;

/// What a virtual pad presents itself as.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Identity {
    /// A pad that claims to be nothing else. SDL and Steam adopt it with
    /// a mapping generated from its controls.
    Generic,
    /// An Xbox Series pad, which SDL and Steam recognize by its numbers.
    Xbox,
    /// A DualSense, which SDL and Steam read through its raw HID node:
    /// made through uhid from a connected pad's copied data.
    DualSense,
}

impl Identity {
    /// The name a game sees.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Generic => "CtrlAssist Virtual Pad",
            Self::Xbox => "Microsoft Xbox Series S|X Controller",
            Self::DualSense => dualsense::NAME,
        }
    }

    /// The bus, vendor, product and version it reports. The generic
    /// pad's are the sample numbers 0.4.0's virtual pad carried, which
    /// SDL and Steam are known to adopt.
    fn id(self) -> InputId {
        match self {
            Self::Generic => InputId::new(BusType::BUS_USB, 0x1234, 0x5678, 0x0111),
            Self::Xbox => InputId::new(BusType::BUS_USB, 0x045e, 0x0b12, 0x0507),
            Self::DualSense => InputId::new(
                BusType::BUS_USB,
                dualsense::VENDOR,
                dualsense::PRODUCT,
                0x8111,
            ),
        }
    }

    /// The vendor and product numbers a game sees.
    #[must_use]
    pub fn model(self) -> (u16, u16) {
        let id = self.id();
        (id.vendor(), id.product())
    }

    /// The highest value of a trigger, as that pad's driver reports it.
    fn trigger_max(self) -> i32 {
        match self {
            Self::Generic | Self::DualSense => 255,
            Self::Xbox => 1023,
        }
    }
}

/// What a game asks of a virtual pad.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Request {
    /// Store or replace a rumble effect under the game's number for it.
    Upload {
        /// The game's number for the effect.
        id: i16,
        /// The effect.
        effect: Effect,
    },
    /// Remove an effect.
    Erase {
        /// The game's number for the effect.
        id: i16,
    },
    /// Play an effect this many times, or stop it with zero.
    Play {
        /// The game's number for the effect.
        id: i16,
        /// How many times.
        count: u32,
    },
    /// Scale every effect on this pad; `0xffff` is full strength.
    Gain(u16),
    /// Run the motors at this level until told otherwise: what a pad
    /// that is sent levels, not effects, asks.
    Level(Level),
    /// A feature report was asked for or set, on a virtual pad that has
    /// them: its number, whether it was being set, and whether the pad
    /// had an answer. Nothing is to be done about it; it is here for
    /// whoever is working out what a game makes of the pad.
    Feature {
        /// The report's number.
        number: u8,
        /// Whether the game was setting it, not asking for it.
        set: bool,
        /// Whether the pad answered.
        answered: bool,
    },
}

const BUTTONS: [(KeyCode, Control); 11] = [
    (KeyCode::BTN_SOUTH, gamepad::SOUTH),
    (KeyCode::BTN_EAST, gamepad::EAST),
    (KeyCode::BTN_NORTH, gamepad::NORTH),
    (KeyCode::BTN_WEST, gamepad::WEST),
    (KeyCode::BTN_TL, gamepad::LEFT_BUMPER),
    (KeyCode::BTN_TR, gamepad::RIGHT_BUMPER),
    (KeyCode::BTN_SELECT, gamepad::SELECT),
    (KeyCode::BTN_START, gamepad::START),
    (KeyCode::BTN_MODE, gamepad::MODE),
    (KeyCode::BTN_THUMBL, gamepad::LEFT_THUMB),
    (KeyCode::BTN_THUMBR, gamepad::RIGHT_THUMB),
];

const STICKS: [(AbsoluteAxisCode, Control); 4] = [
    (AbsoluteAxisCode::ABS_X, gamepad::LEFT_X),
    (AbsoluteAxisCode::ABS_Y, gamepad::LEFT_Y),
    (AbsoluteAxisCode::ABS_RX, gamepad::RIGHT_X),
    (AbsoluteAxisCode::ABS_RY, gamepad::RIGHT_Y),
];

const TRIGGERS: [(AbsoluteAxisCode, Control); 2] = [
    (AbsoluteAxisCode::ABS_Z, gamepad::LEFT_TRIGGER),
    (AbsoluteAxisCode::ABS_RZ, gamepad::RIGHT_TRIGGER),
];

/// The directional pad as a hat: its axis, then the buttons that push it
/// negative and positive.
const HATS: [(AbsoluteAxisCode, Control, Control); 2] = [
    (
        AbsoluteAxisCode::ABS_HAT0X,
        gamepad::DPAD_LEFT,
        gamepad::DPAD_RIGHT,
    ),
    (
        AbsoluteAxisCode::ABS_HAT0Y,
        gamepad::DPAD_UP,
        gamepad::DPAD_DOWN,
    ),
];

const STICK_MIN: i32 = -32768;
const STICK_MAX: i32 = 32767;

/// A stick's value on the wire. Zero stays exactly zero.
fn stick(value: f32) -> i32 {
    let scale = if value < 0.0 {
        -(STICK_MIN as f32)
    } else {
        STICK_MAX as f32
    };
    ((value * scale).round() as i32).clamp(STICK_MIN, STICK_MAX)
}

fn trigger(value: f32, max: i32) -> i32 {
    ((value * max as f32).round() as i32).clamp(0, max)
}

pub(crate) fn hat(state: &PadState, negative: Control, positive: Control) -> i32 {
    i32::from(state.get(positive) > 0.0) - i32::from(state.get(negative) > 0.0)
}

/// A virtual pad made through uinput.
struct Uinput {
    device: VirtualDevice,
    identity: Identity,
    event_node: PathBuf,
    held: PadState,
    /// Kept between calls so that writing and serving allocate nothing.
    scratch: Vec<InputEvent>,
}

impl Uinput {
    fn create(identity: Identity) -> io::Result<Self> {
        let keys = AttributeSet::from_iter(BUTTONS.iter().map(|(key, _)| *key));
        // The process and a count tell virtual pads apart in the marker.
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let phys = CString::new(format!("{OWN_MARKER}{}/{number}", std::process::id()))
            .map_err(io::Error::other)?;
        let mut builder = VirtualDevice::builder()?
            .name(identity.name())
            .input_id(identity.id())
            .with_phys(&phys)?
            .with_keys(&keys)?
            .with_ff(&AttributeSet::from_iter([FFEffectCode::FF_RUMBLE]))?
            .with_ff_effects_max(EFFECTS);
        let axis = |min, max| AbsInfo::new(0, min, max, 0, 0, 0);
        for (code, _) in STICKS {
            builder = builder
                .with_absolute_axis(&UinputAbsSetup::new(code, axis(STICK_MIN, STICK_MAX)))?;
        }
        for (code, _) in TRIGGERS {
            builder = builder
                .with_absolute_axis(&UinputAbsSetup::new(code, axis(0, identity.trigger_max())))?;
        }
        for (code, _, _) in HATS {
            builder = builder.with_absolute_axis(&UinputAbsSetup::new(code, axis(-1, 1)))?;
        }
        let mut device = builder.build()?;
        let event_node = device
            .enumerate_dev_nodes_blocking()?
            .filter_map(Result::ok)
            .find(|node| node.to_string_lossy().contains("/event"))
            .ok_or_else(|| io::Error::other("the virtual pad has no event node"))?;
        // A read with no request waiting must return, not block the
        // thread every pad is served on.
        let flags = fcntl_getfl(&device)?;
        fcntl_setfl(&device, flags | OFlags::NONBLOCK)?;
        Ok(Self {
            device,
            identity,
            event_node,
            held: PadState::NEUTRAL,
            scratch: Vec::with_capacity(64),
        })
    }

    fn write(&mut self, state: &PadState) -> io::Result<()> {
        self.scratch.clear();
        for (key, control) in BUTTONS {
            let (now, was) = (state.get(control), self.held.get(control));
            if now != was {
                self.scratch.push(InputEvent::new(
                    EventType::KEY.0,
                    key.0,
                    i32::from(now > 0.0),
                ));
            }
        }
        for (code, control) in STICKS {
            let (now, was) = (stick(state.get(control)), stick(self.held.get(control)));
            if now != was {
                self.scratch
                    .push(InputEvent::new(EventType::ABSOLUTE.0, code.0, now));
            }
        }
        let max = self.identity.trigger_max();
        for (code, control) in TRIGGERS {
            let (now, was) = (
                trigger(state.get(control), max),
                trigger(self.held.get(control), max),
            );
            if now != was {
                self.scratch
                    .push(InputEvent::new(EventType::ABSOLUTE.0, code.0, now));
            }
        }
        for (code, negative, positive) in HATS {
            let (now, was) = (
                hat(state, negative, positive),
                hat(&self.held, negative, positive),
            );
            if now != was {
                self.scratch
                    .push(InputEvent::new(EventType::ABSOLUTE.0, code.0, now));
            }
        }
        if !self.scratch.is_empty() {
            self.device.emit(&self.scratch)?;
        }
        self.held = *state;
        Ok(())
    }

    fn serve(
        &mut self,
        mut handle: impl FnMut(Request) -> Result<(), FeedbackError>,
    ) -> io::Result<()> {
        loop {
            self.scratch.clear();
            match self.device.fetch_events() {
                Ok(events) => self.scratch.extend(events),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(error),
            }
            if self.scratch.is_empty() {
                return Ok(());
            }
            for index in 0..self.scratch.len() {
                let Some(event) = self.scratch.get(index).copied() else {
                    break;
                };
                match event.destructure() {
                    EventSummary::UInput(request, UInputCode::UI_FF_UPLOAD, ..) => {
                        let mut upload = self.device.process_ff_upload(request)?;
                        let data = upload.effect();
                        let outcome = match data.kind {
                            FFEffectKind::Rumble {
                                strong_magnitude,
                                weak_magnitude,
                            } => handle(Request::Upload {
                                id: upload.effect_id(),
                                effect: Effect {
                                    strong: strong_magnitude,
                                    weak: weak_magnitude,
                                    length_ms: data.replay.length,
                                    delay_ms: data.replay.delay,
                                },
                            }),
                            // Only rumble is offered, so the kernel
                            // shouldn't pass anything else on.
                            _ => Err(FeedbackError::NoSuchEffect),
                        };
                        if let Err(refusal) = outcome {
                            upload.set_retval(-errno(refusal));
                        }
                        // Dropping the upload sends the answer.
                    }
                    EventSummary::UInput(request, UInputCode::UI_FF_ERASE, ..) => {
                        let mut erase = self.device.process_ff_erase(request)?;
                        let outcome = match i16::try_from(erase.effect_id()) {
                            Ok(id) => handle(Request::Erase { id }),
                            Err(_) => Err(FeedbackError::NoSuchEffect),
                        };
                        if let Err(refusal) = outcome {
                            erase.set_retval(-errno(refusal));
                        }
                    }
                    EventSummary::ForceFeedback(_, code, value)
                        if code == FFEffectCode::FF_GAIN =>
                    {
                        let gain =
                            u16::try_from(value.clamp(0, i32::from(u16::MAX))).unwrap_or(u16::MAX);
                        // Nothing waits on these two; a refusal has
                        // nowhere to go.
                        let _ = handle(Request::Gain(gain));
                    }
                    EventSummary::ForceFeedback(_, code, value) => {
                        if let Ok(id) = i16::try_from(code.0) {
                            let count = u32::try_from(value).unwrap_or(0);
                            let _ = handle(Request::Play { id, count });
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

/// The error number a game sees for a refusal.
fn errno(refusal: FeedbackError) -> i32 {
    let errno = match refusal {
        FeedbackError::Full => rustix::io::Errno::NOSPC,
        _ => rustix::io::Errno::INVAL,
    };
    errno.raw_os_error()
}

enum Kind {
    Uinput(Box<Uinput>),
    DualSense(Box<Emulator>),
}

/// A virtual pad.
///
/// It is written level-based, as pads are read: [`write`](Self::write)
/// takes the state the pad should now hold and sends the difference from
/// what it last held. It carries the gamepad's own controls: eleven
/// buttons, two sticks, two triggers and a hat.
#[must_use = "dropping a virtual pad removes it, which the game sees as a disconnect"]
pub struct VirtualPad {
    kind: Kind,
}

impl std::fmt::Debug for VirtualPad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VirtualPad")
            .field("identity", &self.identity())
            .finish_non_exhaustive()
    }
}

impl VirtualPad {
    /// Creates a virtual pad through uinput and waits for its event
    /// node.
    ///
    /// # Errors
    ///
    /// Fails if uinput can't be opened or the kernel refuses the pad,
    /// and for an identity that is made from a copied pad's data; see
    /// [`dualsense`](Self::dualsense).
    pub fn create(identity: Identity) -> io::Result<Self> {
        if identity == Identity::DualSense {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a DualSense is made from a connected one's copied data",
            ));
        }
        Ok(Self {
            kind: Kind::Uinput(Box::new(Uinput::create(identity)?)),
        })
    }

    /// Creates a virtual DualSense through uhid from a connected one's
    /// copied data, answers the kernel's driver as it binds, and waits
    /// for the pad's event node. `address` is the pad's own: the driver
    /// refuses a second pad with an address it already has.
    ///
    /// # Errors
    ///
    /// Fails if uhid can't be opened, the data isn't whole, or the
    /// kernel's PlayStation driver doesn't take the pad.
    pub fn dualsense(data: &DualSenseData, address: [u8; 6]) -> io::Result<Self> {
        Ok(Self {
            kind: Kind::DualSense(Box::new(Emulator::create(data, address)?)),
        })
    }

    /// What it presents itself as.
    #[must_use]
    pub fn identity(&self) -> Identity {
        match &self.kind {
            Kind::Uinput(pad) => pad.identity,
            Kind::DualSense(_) => Identity::DualSense,
        }
    }

    /// The pad's event node, which a game opens.
    #[must_use]
    pub fn event_node(&self) -> &Path {
        match &self.kind {
            Kind::Uinput(pad) => &pad.event_node,
            Kind::DualSense(pad) => pad.event_node(),
        }
    }

    /// Makes the pad hold `state`, sending what differs from what it
    /// holds as one report frame, or nothing if nothing differs.
    ///
    /// On a uinput pad the kernel ends a frame itself once it holds
    /// about as many changes as the pad has axes, so a write that
    /// changes nearly every control at once reaches the game as two
    /// frames. Both are correct states on the way to the one written.
    ///
    /// # Errors
    ///
    /// Fails if the kernel refuses the write; the pad's held state is
    /// then unchanged, so the next write sends the difference again.
    pub fn write(&mut self, state: &PadState) -> io::Result<()> {
        match &mut self.kind {
            Kind::Uinput(pad) => pad.write(state),
            Kind::DualSense(pad) => pad.write(state),
        }
    }

    /// Answers every request games have waiting, passing each to
    /// `handle`. Returns when none is left.
    ///
    /// A game's upload blocks in the kernel until it is answered, so this
    /// is called as soon as the pad is readable. What `handle` refuses is
    /// refused to the game; a request that isn't understood is refused
    /// too, never left unanswered.
    ///
    /// # Errors
    ///
    /// Fails if the kernel refuses a read or an answer.
    pub fn serve(
        &mut self,
        handle: impl FnMut(Request) -> Result<(), FeedbackError>,
    ) -> io::Result<()> {
        match &mut self.kind {
            Kind::Uinput(pad) => pad.serve(handle),
            Kind::DualSense(pad) => pad.serve(handle),
        }
    }
}

impl VirtualPad {
    /// The last output report a game sent a virtual DualSense, if one
    /// came since this was last asked: its lights and trigger effects,
    /// for the pads behind it. No other kind of virtual pad has any.
    pub fn take_output(&mut self) -> Option<&[u8]> {
        match &mut self.kind {
            Kind::Uinput(_) => None,
            Kind::DualSense(pad) => pad.take_output(),
        }
    }

    /// For a virtual pad that passes as one which reports all the time,
    /// changed or not: sends what it holds once more if it has been
    /// quiet for its period at `now_ms`, on the monotonic clock, and
    /// says when it would next be due. Nothing, and `None`, for any
    /// other.
    ///
    /// # Errors
    ///
    /// Fails if the kernel refuses the write.
    pub fn beat(&mut self, now_ms: u64) -> io::Result<Option<u64>> {
        match &mut self.kind {
            Kind::Uinput(_) => Ok(None),
            Kind::DualSense(pad) => pad.beat(now_ms).map(Some),
        }
    }

    /// The last output report a game sent a virtual DualSense, if any
    /// ever came.
    #[must_use]
    pub fn last_output(&self) -> Option<&[u8]> {
        match &self.kind {
            Kind::Uinput(_) => None,
            Kind::DualSense(pad) => pad.last_output(),
        }
    }

    /// What the games have set on a virtual DualSense through all their
    /// output reports, as one report for the pads behind it: each setting
    /// from the newest report that set it, since a DualSense takes only
    /// what a report marks. `fresh` for the pads a new report goes to,
    /// which then get what it asks once as well. None for any other
    /// kind of virtual pad, or before a game has sent one.
    pub(crate) fn settings(&mut self, fresh: bool) -> Option<[u8; dualsense::GAME_SIZE]> {
        match &mut self.kind {
            Kind::Uinput(_) => None,
            Kind::DualSense(pad) => pad.settings(fresh),
        }
    }
}

impl AsFd for VirtualPad {
    fn as_fd(&self) -> BorrowedFd<'_> {
        match &self.kind {
            Kind::Uinput(pad) => pad.device.as_fd(),
            Kind::DualSense(pad) => pad.as_fd(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A plain virtual pad writes the standard gamepad's controls and
    /// nothing past them: no back button, capacitive touch or touchpad
    /// click reaches a game through it, and it has no touchpad.
    #[test]
    fn nothing_past_the_standard_gamepad_is_written() {
        let written: Vec<Control> = BUTTONS
            .iter()
            .map(|(_, control)| *control)
            .chain(STICKS.iter().map(|(_, control)| *control))
            .chain(TRIGGERS.iter().map(|(_, control)| *control))
            .chain(
                HATS.iter()
                    .flat_map(|(_, negative, positive)| [*negative, *positive]),
            )
            .collect();
        for control in [
            gamepad::PADDLE_1,
            gamepad::PADDLE_2,
            gamepad::PADDLE_3,
            gamepad::PADDLE_4,
            gamepad::LEFT_STICK_TOUCH,
            gamepad::RIGHT_STICK_TOUCH,
            gamepad::LEFT_GRIP_TOUCH,
            gamepad::RIGHT_GRIP_TOUCH,
            gamepad::TOUCHPAD,
            gamepad::SECOND_TOUCHPAD,
        ] {
            assert!(!written.contains(&control), "{control:?}");
        }
    }

    #[test]
    fn a_stick_keeps_its_rest_and_its_ends() {
        assert_eq!(stick(0.0), 0);
        assert_eq!(stick(1.0), 32767);
        assert_eq!(stick(-1.0), -32768);
        assert_eq!(stick(0.5), 16384);
        assert_eq!(stick(f32::NAN), 0);
        assert_eq!(stick(9.0), 32767);
        assert_eq!(stick(-9.0), -32768);
    }

    #[test]
    fn a_trigger_fills_its_pad_s_range() {
        assert_eq!(trigger(0.0, 255), 0);
        assert_eq!(trigger(1.0, 255), 255);
        assert_eq!(trigger(1.0, 1023), 1023);
        assert_eq!(trigger(0.5, 255), 128);
        assert_eq!(trigger(-1.0, 255), 0);
        assert_eq!(trigger(f32::NAN, 255), 0);
    }

    #[test]
    fn opposite_directions_held_together_cancel_on_the_hat() {
        let both = PadState::NEUTRAL
            .with(gamepad::DPAD_LEFT, 1.0)
            .with(gamepad::DPAD_RIGHT, 1.0);
        assert_eq!(hat(&both, gamepad::DPAD_LEFT, gamepad::DPAD_RIGHT), 0);
        let left = PadState::NEUTRAL.with(gamepad::DPAD_LEFT, 1.0);
        assert_eq!(hat(&left, gamepad::DPAD_LEFT, gamepad::DPAD_RIGHT), -1);
        assert_eq!(
            hat(&PadState::NEUTRAL, gamepad::DPAD_UP, gamepad::DPAD_DOWN),
            0
        );
    }

    #[test]
    fn a_full_pad_is_out_of_space_and_any_other_refusal_is_invalid() {
        assert_eq!(errno(FeedbackError::Full), 28);
        assert_eq!(errno(FeedbackError::NoSuchEffect), 22);
        assert_eq!(errno(FeedbackError::NoSuchSink), 22);
    }
}
