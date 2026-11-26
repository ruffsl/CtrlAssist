//! A fake pad with rumble motors.

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::JoinHandle;

use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, EventSummary, FFEffectCode, FFEffectKind,
    InputId, KeyCode, UInputCode, UinputAbsSetup,
};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fs::{OFlags, fcntl_getfl, fcntl_setfl};

/// How hard the two motors were set: strong, then weak.
pub type Motors = (u16, u16);

/// Whether the pad's maker answers what it is asked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Answering {
    Yes,
    /// It has stopped answering, as a program that hangs does.
    No,
    /// The pad is being dropped.
    Ended,
}

/// A gamepad made through uinput that accepts rumble, as a real pad's
/// driver does, and remembers what its motors were told.
///
/// The kernel holds whoever asks a uinput pad to rumble until the pad's
/// maker answers, so a thread of the pad's own answers for it, until it
/// is told to [stop answering](Self::stop_answering).
pub struct RumblePad {
    event_node: PathBuf,
    motors: Arc<Mutex<Vec<Motors>>>,
    answering: Arc<(Mutex<Answering>, Condvar)>,
    /// The pad's uinput descriptor, to look at what waits unread.
    asked: OwnedFd,
    serving: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for RumblePad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RumblePad")
            .field("event_node", &self.event_node)
            .finish_non_exhaustive()
    }
}

/// Answers every request waiting on `device`, noting what the motors
/// were told.
fn serve(
    device: &mut VirtualDevice,
    effects: &mut HashMap<u16, Motors>,
    motors: &Mutex<Vec<Motors>>,
) -> io::Result<()> {
    let events: Vec<_> = match device.fetch_events() {
        Ok(events) => events.collect(),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
        Err(error) => return Err(error),
    };
    for event in events {
        match event.destructure() {
            EventSummary::UInput(request, UInputCode::UI_FF_UPLOAD, ..) => {
                // Dropping the upload answers it.
                let upload = device.process_ff_upload(request)?;
                if let (
                    Ok(id),
                    FFEffectKind::Rumble {
                        strong_magnitude,
                        weak_magnitude,
                    },
                ) = (u16::try_from(upload.effect_id()), upload.effect().kind)
                {
                    effects.insert(id, (strong_magnitude, weak_magnitude));
                }
            }
            EventSummary::UInput(request, UInputCode::UI_FF_ERASE, ..) => {
                let erase = device.process_ff_erase(request)?;
                if let Ok(id) = u16::try_from(erase.effect_id()) {
                    effects.remove(&id);
                }
            }
            EventSummary::ForceFeedback(_, code, value) if code != FFEffectCode::FF_GAIN => {
                let level = if value > 0 {
                    effects.get(&code.0).copied().unwrap_or_default()
                } else {
                    (0, 0)
                };
                motors
                    .lock()
                    .expect("nothing panics while the motor log is held")
                    .push(level);
            }
            _ => {}
        }
    }
    Ok(())
}

impl RumblePad {
    /// Creates the pad and waits for its event node.
    ///
    /// # Errors
    ///
    /// Fails if uinput can't be opened or the kernel refuses the pad.
    pub fn new(name: &str, vendor: u16, product: u16) -> io::Result<Self> {
        let keys = AttributeSet::from_iter([KeyCode::BTN_SOUTH, KeyCode::BTN_EAST]);
        let stick = AbsInfo::new(0, -32768, 32767, 0, 0, 0);
        let mut builder = VirtualDevice::builder()?
            .with_phys(&crate::marker())?
            .name(name)
            .input_id(InputId::new(BusType::BUS_USB, vendor, product, 1))
            .with_keys(&keys)?
            .with_ff(&AttributeSet::from_iter([FFEffectCode::FF_RUMBLE]))?
            .with_ff_effects_max(4);
        for axis in [AbsoluteAxisCode::ABS_X, AbsoluteAxisCode::ABS_Y] {
            builder = builder.with_absolute_axis(&UinputAbsSetup::new(axis, stick))?;
        }
        let mut device = builder.build()?;
        let event_node = device
            .enumerate_dev_nodes_blocking()?
            .filter_map(Result::ok)
            .find(|node| node.to_string_lossy().contains("/event"))
            .ok_or_else(|| io::Error::other("the fake pad has no event node"))?;
        let flags = fcntl_getfl(&device)?;
        fcntl_setfl(&device, flags | OFlags::NONBLOCK)?;
        let asked = device.as_fd().try_clone_to_owned()?;
        // As for `FakePad`: wait for udev to finish with the node.
        let _ = std::process::Command::new("udevadm").arg("settle").status();

        let motors = Arc::new(Mutex::new(Vec::new()));
        let answering = Arc::new((Mutex::new(Answering::Yes), Condvar::new()));
        let serving = std::thread::Builder::new()
            .name("rumble-pad".into())
            .spawn({
                let motors = Arc::clone(&motors);
                let answering = Arc::clone(&answering);
                move || {
                    let mut effects = HashMap::new();
                    // Test support: a short poll, so the thread notices
                    // the pad being dropped.
                    let wait = Timespec {
                        tv_sec: 0,
                        tv_nsec: 10_000_000,
                    };
                    loop {
                        let _ = poll(&mut [PollFd::new(&device, PollFlags::IN)], Some(&wait));
                        // Held while serving, so that once told to stop
                        // answering, nothing more is answered.
                        let (mode, changed) = &*answering;
                        let mut mode = mode.lock().unwrap_or_else(PoisonError::into_inner);
                        while *mode == Answering::No {
                            mode = changed.wait(mode).unwrap_or_else(PoisonError::into_inner);
                        }
                        if *mode == Answering::Ended
                            || serve(&mut device, &mut effects, &motors).is_err()
                        {
                            return;
                        }
                    }
                }
            })?;
        Ok(Self {
            event_node,
            motors,
            answering,
            asked,
            serving: Some(serving),
        })
    }

    /// The pad's event node.
    #[must_use]
    pub fn event_node(&self) -> &Path {
        &self.event_node
    }

    /// Every setting its motors were given so far, in order; `(0, 0)` is
    /// a stop.
    ///
    /// # Panics
    ///
    /// Panics if the serving thread panicked while noting one.
    #[must_use]
    pub fn motors(&self) -> Vec<Motors> {
        self.motors
            .lock()
            .expect("nothing panics while the motor log is held")
            .clone()
    }

    fn answer(&self, now: Answering) {
        let (mode, changed) = &*self.answering;
        *mode.lock().unwrap_or_else(PoisonError::into_inner) = now;
        changed.notify_all();
    }

    /// From now on leaves everything it is asked unanswered, as a program
    /// that has hung does: whoever updates or erases one of its effects
    /// waits, up to the kernel's 30 seconds. Nothing is answered once
    /// this returns.
    pub fn stop_answering(&self) {
        self.answer(Answering::No);
    }

    /// Answers again, what waits and what comes.
    pub fn answer_again(&self) {
        self.answer(Answering::Yes);
    }

    /// Whether something waits unread: while it isn't answering, a
    /// request.
    #[must_use]
    pub fn is_asked(&self) -> bool {
        let mut fds = [PollFd::new(&self.asked, PollFlags::IN)];
        let now = Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        poll(&mut fds, Some(&now)).is_ok_and(|ready| ready > 0)
    }
}

impl Drop for RumblePad {
    fn drop(&mut self) {
        self.answer(Answering::Ended);
        if let Some(serving) = self.serving.take() {
            let _ = serving.join();
        }
        // The pad goes with the serving thread's descriptor and this
        // one: the kernel then fails whatever still waits on it.
    }
}
