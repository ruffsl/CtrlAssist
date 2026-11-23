//! Watching pads connect and disconnect, for the control side.
//!
//! udev's monitor can't move between threads, so it gets one of its own,
//! which sleeps until udev or the daemon has something for it, and reads
//! nothing but udev: a pad is said to have arrived as soon as it is seen.
//! Whether it can rumble is learned after, by opening its event node,
//! which can wait up to 30 seconds on whatever program makes the pad; so
//! a thread of its own opens the pads, one after another, and what it
//! learns follows the arrival. Nothing waits for that thread: it ends
//! once the watch has, after the opening under way, if any. The pads
//! there as the watch starts are opened as it starts, by its caller,
//! the monitor's thread reading udev meanwhile, so that the daemon's
//! first state says what they can do.

use std::fs::File;
use std::io;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Sender, channel, sync_channel};
use std::thread::JoinHandle;

use ctrlassist_device::{Anchor, Change, Found, Monitor, Pad};
use rustix::event::{EventfdFlags, PollFd, PollFlags, eventfd, poll};
use tokio::sync::mpsc::UnboundedSender;
use tracing::warn;

/// What the watch saw.
#[derive(Debug)]
pub(crate) enum Seen {
    /// A pad connected, or changed.
    Arrived(Found),
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
    /// Whether the pad on the event node `node`, at `syspath` in sysfs,
    /// can rumble, learned by opening it once it arrived.
    Rumble {
        node: PathBuf,
        syspath: PathBuf,
        rumble: bool,
    },
    /// udev's events overran the watch, and some were lost: every pad
    /// connected now, and every device a pad can be anchored to.
    Overran {
        connected: Vec<Found>,
        anchors: Vec<Anchor>,
    },
}

/// Whether the pad on `node` can rumble; `None` when it can't be opened
/// to ask.
fn can_rumble(node: &Path) -> Option<bool> {
    let file = File::open(node).ok()?;
    Some(Pad::from_fd(OwnedFd::from(file)).ok()?.can_rumble())
}

/// Starts the thread that opens each pad sent to it, to learn whether it
/// can rumble, and says so to `seen`. It ends once every sender has gone
/// and what was sent is done.
fn opener(seen: UnboundedSender<Seen>) -> io::Result<Sender<(PathBuf, PathBuf)>> {
    let (open, opening) = channel::<(PathBuf, PathBuf)>();
    std::thread::Builder::new()
        .name("rumble".into())
        .spawn(move || {
            for (node, syspath) in opening {
                let Some(rumble) = can_rumble(&node) else {
                    continue;
                };
                let learned = Seen::Rumble {
                    node,
                    syspath,
                    rumble,
                };
                if seen.send(learned).is_err() {
                    return;
                }
            }
        })?;
    Ok(open)
}

/// What the opener is sent of a pad: its node, and its node's place.
fn to_open(found: &Found) -> (PathBuf, PathBuf) {
    (found.node.clone(), found.syspath.clone())
}

/// The pads connected now and the devices they can be anchored to, after
/// an overrun; `None`, with why logged, if udev can't be asked.
fn looked_again() -> Option<(Vec<Found>, Vec<Anchor>)> {
    let looked = ctrlassist_device::connected()
        .and_then(|connected| Ok((connected, ctrlassist_device::anchors()?)));
    match looked {
        Ok(looked) => Some(looked),
        Err(error) => {
            warn!(%error, "udev's events overran the hotplug watch, and what is connected couldn't be looked at again");
            None
        }
    }
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
    /// its anchor, and then whether each can rumble. Every change after
    /// those goes to `seen`, whether each pad that arrives can rumble
    /// among them.
    pub(crate) fn start(seen: UnboundedSender<Seen>) -> io::Result<(Self, Vec<Seen>)> {
        let wake = eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)?;
        let stop = wake.try_clone()?;
        let open = opener(seen.clone())?;
        // Each pad once it is said to have arrived, so that what is
        // learned of it comes after.
        let learn = move |pads: Vec<(PathBuf, PathBuf)>| {
            for pad in pads {
                let _ = open.send(pad);
            }
        };
        let (started, start) = sync_channel(1);
        let thread = std::thread::Builder::new()
            .name("hotplug".into())
            .spawn(move || {
                // The monitor first, so nothing is missed between the
                // list and the first change.
                let begun = Monitor::new()
                    .and_then(|monitor| Ok((monitor, ctrlassist_device::connected()?)));
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
                    monitor.read(|change| match change {
                        Change::Arrived(found) => {
                            let opened = vec![to_open(&found)];
                            let _ = seen.send(Seen::Arrived(found));
                            learn(opened);
                        }
                        Change::Left(node) => {
                            let _ = seen.send(Seen::Left(node));
                        }
                        Change::Unplugged(device) => {
                            let _ = seen.send(Seen::Unplugged(device));
                        }
                        Change::Rebinding { anchor, present } => {
                            let _ = seen.send(Seen::Rebinding { anchor, present });
                        }
                        Change::Overran => {
                            if let Some((connected, anchors)) = looked_again() {
                                let opened = connected.iter().map(to_open).collect();
                                let _ = seen.send(Seen::Overran { connected, anchors });
                                learn(opened);
                            }
                        }
                        // A change this release doesn't know says nothing
                        // it can act on.
                        _ => {}
                    });
                }
            })?;
        let now: Vec<Found> = start
            .recv()
            .map_err(|_| io::Error::other("the hotplug thread ended before it began"))??;
        let learned: Vec<Seen> = now
            .iter()
            .filter_map(|found| {
                let (node, syspath) = to_open(found);
                let rumble = can_rumble(&node)?;
                Some(Seen::Rumble {
                    node,
                    syspath,
                    rumble,
                })
            })
            .collect();
        let now = now.into_iter().map(Seen::Arrived).chain(learned).collect();
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
