//! Watching pads connect and disconnect, for the control side.
//!
//! udev's monitor can't move between threads, so it gets one of its own,
//! which sleeps until udev or the daemon has something for it. Whether a
//! pad can rumble is asked here too, as it connects: that opens its event
//! node, and file I/O stays off the runtime's workers.

use std::fs::File;
use std::io;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use ctrlassist_device::{Change, Found, Monitor, Pad};
use rustix::event::{EventfdFlags, PollFd, PollFlags, eventfd, poll};
use tokio::sync::mpsc::UnboundedSender;
use tracing::debug;

/// What the watch saw.
#[derive(Debug)]
pub(crate) enum Seen {
    /// A pad connected, or changed; and whether it can rumble, when that
    /// could be asked.
    Arrived(Found, Option<bool>),
    /// The pad on this event node has gone.
    Left(PathBuf),
    /// Every change udev had sent when a [flush](Flusher::flush) was
    /// asked for this pad, for this request of its, has been passed on.
    Flushed { pad: String, generation: u64 },
}

/// Whether the pad on `node` can rumble; `None` when it can't be opened
/// to ask.
fn can_rumble(node: &Path) -> Option<bool> {
    let file = File::open(node).ok()?;
    Some(Pad::from_fd(OwnedFd::from(file)).ok()?.can_rumble())
}

fn arrived(found: Found) -> Seen {
    let rumble = can_rumble(&found.node);
    Seen::Arrived(found, rumble)
}

/// The flushes asked of the watch, and what wakes it to them.
#[derive(Debug)]
struct Flushes {
    asked: Mutex<Vec<(String, u64)>>,
    wake: OwnedFd,
}

/// Asks the watch to pass on every change udev has sent so far, and then
/// [say so](Seen::Flushed), in order with them.
#[derive(Clone, Debug)]
pub(crate) struct Flusher {
    flushes: Arc<Flushes>,
}

impl Flusher {
    /// Asks for a flush, for `pad`'s request `generation`.
    pub(crate) fn flush(&self, pad: &str, generation: u64) {
        if let Ok(mut asked) = self.flushes.asked.lock() {
            asked.push((pad.to_string(), generation));
        }
        if let Err(error) = rustix::io::write(&self.flushes.wake, &1u64.to_ne_bytes()) {
            // Full, after more asks than there can be: readable already.
            debug!(%error, "couldn't wake the hotplug watch to flush");
        }
    }
}

/// The running watch.
#[must_use = "dropping the watch stops it, and pads connecting are no longer noticed"]
#[derive(Debug)]
pub(crate) struct Watch {
    wake: OwnedFd,
    flushes: Arc<Flushes>,
    thread: Option<JoinHandle<()>>,
}

impl Watch {
    /// Starts watching, and returns the pads connected now. Every change
    /// after those goes to `seen`.
    pub(crate) fn start(seen: UnboundedSender<Seen>) -> io::Result<(Self, Vec<Seen>)> {
        let wake = eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)?;
        let stop = wake.try_clone()?;
        let flushes = Arc::new(Flushes {
            asked: Mutex::new(Vec::new()),
            wake: eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)?,
        });
        let flushing = Arc::clone(&flushes);
        let (started, start) = sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("hotplug".into())
            .spawn(move || {
                // The monitor first, so nothing is missed between the
                // list and the first change.
                let begun = Monitor::new().and_then(|monitor| {
                    let now = ctrlassist_device::connected()?;
                    Ok((monitor, now.into_iter().map(arrived).collect::<Vec<_>>()))
                });
                let mut monitor = match begun {
                    Ok((monitor, now)) => {
                        let _ = started.send(Ok(now));
                        monitor
                    }
                    Err(error) => {
                        let _ = started.send(Err(error));
                        return;
                    }
                };
                loop {
                    let mut fds = [
                        PollFd::new(&monitor, PollFlags::IN),
                        PollFd::new(&stop, PollFlags::IN),
                        PollFd::new(&flushing.wake, PollFlags::IN),
                    ];
                    if poll(&mut fds, None).is_err() {
                        continue;
                    }
                    let [changed, stopped, flush] = fds.map(|fd| !fd.revents().is_empty());
                    if stopped {
                        return;
                    }
                    let asked = if flush {
                        let mut count = [0u8; 8];
                        let _ = rustix::io::read(&flushing.wake, &mut count);
                        flushing
                            .asked
                            .lock()
                            .map(|mut asked| std::mem::take(&mut *asked))
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                    // Every change waiting is read, whatever woke it: one
                    // a flush was asked after is passed on before it.
                    if changed || flush {
                        monitor.read(|change| {
                            let _ = seen.send(match change {
                                Change::Arrived(found) => arrived(found),
                                Change::Left(node) => Seen::Left(node),
                                // A change this release doesn't know says
                                // nothing it can act on.
                                _ => return,
                            });
                        });
                    }
                    for (pad, generation) in asked {
                        let _ = seen.send(Seen::Flushed { pad, generation });
                    }
                }
            })?;
        let now = start
            .recv()
            .map_err(|_| io::Error::other("the hotplug thread ended before it began"))??;
        Ok((
            Self {
                wake,
                flushes,
                thread: Some(thread),
            },
            now,
        ))
    }

    /// What asks this watch for a flush.
    pub(crate) fn flusher(&self) -> Flusher {
        Flusher {
            flushes: Arc::clone(&self.flushes),
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        let _ = rustix::io::write(&self.wake, &1u64.to_ne_bytes());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
