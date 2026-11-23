//! Re-binds in flight, a device at a time.
//!
//! A re-bind is in flight from the moment udev is asked for it until the
//! service has seen udev's answer, the change event carrying the
//! request's own UUID, or the device has gone. The next re-bind of the
//! same device waits for that first, whoever asked for either: the
//! showing again of a hide given up before udev took it, a hide soon
//! after a showing again whose client hung up, the sweep's. So a re-bind
//! is never judged on what an earlier one of the device did, and each
//! one's going and coming back is the pad's own.
//!
//! What waits is a device's turn: one per device, held from before the
//! request until the answer. Whoever waits for a turn watches a socket
//! whose other end the holder keeps, which hangs up when the turn ends,
//! so a wait can be watched beside a client's socket, and needs no time
//! limit.

use std::collections::BTreeMap;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use ctrlassist_hider::Refusal;
use rustix::net::{AddressFamily, SocketFlags, SocketType, socketpair};
use tracing::warn;

/// The turn each device with a re-bind in flight has, by its place in
/// sysfs: the end of a socket that hangs up when the turn ends.
static TURNS: Mutex<BTreeMap<PathBuf, Arc<OwnedFd>>> = Mutex::new(BTreeMap::new());

fn turns() -> MutexGuard<'static, BTreeMap<PathBuf, Arc<OwnedFd>>> {
    TURNS
        .lock()
        .expect("nothing panics while the re-binds in flight are held")
}

/// A device's turn to be re-bound, from before udev is asked until its
/// answer has come or the device has gone.
#[derive(Debug)]
#[must_use = "dropping it ends the device's turn, and lets its next re-bind go ahead"]
pub(crate) struct Turn {
    device: PathBuf,
    /// What those waiting for the turn watch.
    ended: Arc<OwnedFd>,
    /// The other end, closed last as the turn is dropped, which wakes
    /// them.
    _held: OwnedFd,
}

impl Drop for Turn {
    fn drop(&mut self) {
        let mut turns = turns();
        if turns
            .get(&self.device)
            .is_some_and(|ended| Arc::ptr_eq(ended, &self.ended))
        {
            turns.remove(&self.device);
        }
    }
}

/// Takes the turn of the device at `device`, once any re-bind of it in
/// flight has had its answer. Meanwhile `wait` is given what to wait on
/// for that, and may end the wait with a refusal, as a client's
/// `cancel` does.
pub(crate) fn take(
    device: &Path,
    mut wait: impl FnMut(BorrowedFd<'_>) -> Result<(), Refusal>,
) -> Result<Turn, Refusal> {
    loop {
        let ended = {
            let mut turns = turns();
            match turns.get(device) {
                Some(ended) => Arc::clone(ended),
                None => {
                    let (held, ended) = socketpair(
                        AddressFamily::UNIX,
                        SocketType::STREAM,
                        SocketFlags::CLOEXEC,
                        None,
                    )
                    .map_err(|error| {
                        warn!(%error, "couldn't make what a re-bind's turn is waited on by");
                        Refusal::Failed
                    })?;
                    let ended = Arc::new(ended);
                    turns.insert(device.to_path_buf(), Arc::clone(&ended));
                    return Ok(Turn {
                        device: device.to_path_buf(),
                        ended,
                        _held: held,
                    });
                }
            }
        };
        // Woken when that turn ends, or by whatever else `wait` watches;
        // then looked at again, since another waiter may have taken it.
        wait(ended.as_fd())?;
    }
}

#[cfg(test)]
mod tests {
    use rustix::event::{PollFd, PollFlags, poll};

    use super::*;

    fn wait_for(ended: BorrowedFd<'_>) -> Result<(), Refusal> {
        let mut fds = [PollFd::from_borrowed_fd(ended, PollFlags::IN)];
        poll(&mut fds, None).map_err(|_| Refusal::Failed)?;
        Ok(())
    }

    #[test]
    fn a_device_s_next_re_bind_waits_for_the_one_in_flight() {
        let device = Path::new("/sys/devices/ctrlassist-test/turns");
        let first = take(device, |_| panic!("nothing is in flight yet")).unwrap();
        // Another device has a turn of its own meanwhile.
        let other = take(Path::new("/sys/devices/ctrlassist-test/other"), |_| {
            panic!("another device's turn is its own")
        })
        .unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let second = take(device, wait_for).unwrap();
            sender.send(()).unwrap();
            drop(second);
        });
        // The second waits as long as the first holds the turn.
        assert!(receiver.try_recv().is_err());
        drop(first);
        receiver.recv().unwrap();
        waiter.join().unwrap();
        drop(other);
        let turns = turns();
        assert!(
            !turns.contains_key(device)
                && !turns.contains_key(Path::new("/sys/devices/ctrlassist-test/other"))
        );
    }

    #[test]
    fn a_wait_for_a_turn_can_be_given_up() {
        let device = Path::new("/sys/devices/ctrlassist-test/given-up");
        let first = take(device, |_| panic!("nothing is in flight yet")).unwrap();
        assert_eq!(
            take(device, |_| Err(Refusal::Cancelled)).unwrap_err(),
            Refusal::Cancelled
        );
        drop(first);
    }
}
