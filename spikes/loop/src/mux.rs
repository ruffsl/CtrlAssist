//! The loop under test. Everything the data path does happens on the
//! calling thread, which sleeps in `epoll_wait` with no timeout: it wakes
//! only for a pad's events or the virtual pad's feedback requests.

use std::collections::HashMap;
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::path::Path;

use anyhow::{Context, Result, bail};
use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, Device, EventSummary, EventType, FFEffect,
    FFEffectCode, InputEvent, UInputCode, UinputAbsSetup,
};

const PRIMARY: u64 = 0;
const ASSIST: u64 = 1;
const SINK: u64 = 2;

fn open_pad(path: &Path) -> Result<Device> {
    // A node udev is still setting up can refuse the first open.
    let mut pad = Device::open(path);
    for _ in 0..200 {
        if pad.is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        pad = Device::open(path);
    }
    let pad = pad.with_context(|| format!("opening {}", path.display()))?;
    pad.set_nonblocking(true)?;
    Ok(pad)
}

/// A virtual pad with the primary's keys and axes, and rumble if the
/// primary can rumble. A sink's capabilities are fixed here, at creation.
fn build_sink(primary: &Device) -> Result<VirtualDevice> {
    let mut builder = VirtualDevice::builder()?.name("Loop Spike Virtual Pad");
    if let Some(keys) = primary.supported_keys() {
        builder = builder.with_keys(keys)?;
    }
    if let Some(axes) = primary.supported_absolute_axes() {
        let state = primary.get_abs_state()?;
        for axis in axes.iter() {
            let info = state[usize::from(axis.0)];
            let info = AbsInfo::new(
                info.value,
                info.minimum,
                info.maximum,
                info.fuzz,
                info.flat,
                info.resolution,
            );
            builder = builder.with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode(axis.0), info))?;
        }
    }
    if primary
        .supported_ff()
        .is_some_and(|ff| ff.contains(FFEffectCode::FF_RUMBLE))
    {
        builder = builder
            .with_ff(&AttributeSet::from_iter([FFEffectCode::FF_RUMBLE]))?
            .with_ff_effects_max(16);
    }
    Ok(builder.build()?)
}

fn epoll_add(epoll: RawFd, fd: RawFd, token: u64) -> Result<()> {
    let mut event = libc::epoll_event {
        events: libc::EPOLLIN as u32,
        u64: token,
    };
    // SAFETY: `event` is a valid epoll_event for the duration of the call,
    // and both descriptors are open.
    if unsafe { libc::epoll_ctl(epoll, libc::EPOLL_CTL_ADD, fd, &raw mut event) } < 0 {
        bail!("epoll_ctl: {}", io::Error::last_os_error());
    }
    Ok(())
}

/// Forwards whole report frames from a pad to the sink. The evdev crate's
/// synced reader turns a dropped-events marker into the events that bring
/// its view back in line, so a resync needs nothing more here.
fn forward(pad: &mut Device, sink: &mut VirtualDevice, frame: &mut Vec<InputEvent>) -> Result<()> {
    loop {
        match pad.fetch_events() {
            Ok(events) => {
                for event in events {
                    if event.event_type() == EventType::SYNCHRONIZATION {
                        if !frame.is_empty() {
                            sink.emit(frame)?;
                            frame.clear();
                        }
                    } else {
                        frame.push(event);
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
            Err(e) => return Err(e).context("reading a pad"),
        }
    }
}

/// Answers the sink's feedback requests on the same thread. An upload
/// blocks the game until it is answered, so it is handled before the loop
/// sleeps again.
fn feedback(
    sink: &mut VirtualDevice,
    primary: &mut Device,
    effects: &mut HashMap<i16, FFEffect>,
) -> Result<()> {
    let events: Vec<InputEvent> = match sink.fetch_events() {
        Ok(events) => events.collect(),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
        Err(e) => return Err(e).context("reading the virtual pad"),
    };
    for event in events {
        match event.destructure() {
            EventSummary::UInput(request, UInputCode::UI_FF_UPLOAD, ..) => {
                let mut upload = sink.process_ff_upload(request)?;
                match primary.upload_ff_effect(upload.effect()) {
                    Ok(effect) => {
                        effects.insert(upload.effect_id(), effect);
                    }
                    Err(e) => upload.set_retval(-e.raw_os_error().unwrap_or(libc::EIO)),
                }
            }
            EventSummary::UInput(request, UInputCode::UI_FF_ERASE, ..) => {
                let erase = sink.process_ff_erase(request)?;
                if let Ok(id) = i16::try_from(erase.effect_id()) {
                    effects.remove(&id);
                }
            }
            EventSummary::ForceFeedback(_, code, value) => {
                if let Some(effect) = i16::try_from(code.0).ok().and_then(|id| effects.get_mut(&id)) {
                    if value > 0 { effect.play(value)? } else { effect.stop()? }
                }
            }
            _ => {}
        }
    }
    Ok(())
}

pub(crate) fn run(primary: &Path, assist: &Path, rt: bool) -> Result<()> {
    let mut primary = open_pad(primary)?;
    let mut assist = open_pad(assist)?;
    let mut sink = build_sink(&primary)?;
    let sink_fd = sink.as_raw_fd();
    // SAFETY: `sink_fd` is the sink's open descriptor; F_SETFL with
    // O_NONBLOCK only changes its file status flags.
    unsafe { libc::fcntl(sink_fd, libc::F_SETFL, libc::O_NONBLOCK) };
    // Before the line the bench waits for: it stops reading stdout after it.
    if rt && !crate::make_realtime(10) {
        bail!("the kernel refused SCHED_FIFO");
    }
    for node in sink.enumerate_dev_nodes_blocking()? {
        let node = node?;
        if node.to_string_lossy().contains("/event") {
            println!("Virtual: @ {}", node.display());
        }
    }

    // SAFETY: epoll_create1 takes only flags and returns a new descriptor.
    let epoll = unsafe { libc::epoll_create1(libc::EPOLL_CLOEXEC) };
    if epoll < 0 {
        bail!("epoll_create1: {}", io::Error::last_os_error());
    }
    epoll_add(epoll, primary.as_raw_fd(), PRIMARY)?;
    epoll_add(epoll, assist.as_raw_fd(), ASSIST)?;
    epoll_add(epoll, sink_fd, SINK)?;

    let mut effects = HashMap::new();
    let mut frame = Vec::with_capacity(16);
    let mut ready = [libc::epoll_event { events: 0, u64: 0 }; 8];
    loop {
        // SAFETY: `ready` is a valid, exclusively borrowed buffer of eight
        // events, the capacity passed. A timeout of -1 sleeps until an event.
        let count = unsafe { libc::epoll_wait(epoll, ready.as_mut_ptr(), 8, -1) };
        if count < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            bail!("epoll_wait: {error}");
        }
        for event in &ready[..usize::try_from(count).unwrap_or(0)] {
            match event.u64 {
                PRIMARY => forward(&mut primary, &mut sink, &mut frame)?,
                ASSIST => forward(&mut assist, &mut sink, &mut frame)?,
                _ => feedback(&mut sink, &mut primary, &mut effects)?,
            }
        }
    }
}
