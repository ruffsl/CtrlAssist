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
use std::thread::JoinHandle;

use ctrlassist_device::{Change, Found, Monitor, Pad};
use rustix::event::{EventfdFlags, PollFd, PollFlags, eventfd, poll};
use tokio::sync::mpsc::UnboundedSender;

/// What the watch saw.
#[derive(Debug)]
pub(crate) enum Seen {
    /// A pad connected, or changed; and whether it can rumble, when that
    /// could be asked.
    Arrived(Found, Option<bool>),
    /// The pad on this event node has gone.
    Left(PathBuf),
    /// The device at this place in sysfs, which pads can be anchored to,
    /// has gone.
    Unplugged(PathBuf),
    /// The hiding service has asked a re-bind of the device at `anchor`,
    /// and of the event nodes under it only those at `present` are still
    /// there: see [`Change::Rebinding`](ctrlassist_device::Change).
    Rebinding {
        anchor: PathBuf,
        present: Vec<PathBuf>,
    },
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

/// The running watch.
#[must_use = "dropping the watch stops it, and pads connecting are no longer noticed"]
#[derive(Debug)]
pub(crate) struct Watch {
    wake: OwnedFd,
    thread: Option<JoinHandle<()>>,
}

impl Watch {
    /// Starts watching, and returns the pads connected now, each with
    /// its anchor. Every change after those goes to `seen`.
    pub(crate) fn start(seen: UnboundedSender<Seen>) -> io::Result<(Self, Vec<Seen>)> {
        let wake = eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)?;
        let stop = wake.try_clone()?;
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
                    ];
                    if poll(&mut fds, None).is_err() {
                        continue;
                    }
                    if !fds[1].revents().is_empty() {
                        return;
                    }
                    monitor.read(|change| {
                        let _ = seen.send(match change {
                            Change::Arrived(found) => arrived(found),
                            Change::Left(node) => Seen::Left(node),
                            Change::Unplugged(device) => Seen::Unplugged(device),
                            Change::Rebinding { anchor, present } => {
                                Seen::Rebinding { anchor, present }
                            }
                            // A change this release doesn't know says
                            // nothing it can act on.
                            _ => return,
                        });
                    });
                }
            })?;
        let now = start
            .recv()
            .map_err(|_| io::Error::other("the hotplug thread ended before it began"))??;
        Ok((
            Self {
                wake,
                thread: Some(thread),
            },
            now,
        ))
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
