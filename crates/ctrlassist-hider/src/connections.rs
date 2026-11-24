//! How many connections the service keeps, and whose.
//!
//! Anyone may connect, and each connection costs a descriptor and a
//! thread, so the service caps them. A cap per user alone can be walked
//! around: a user runs processes as many user IDs, the subordinate ones
//! a user namespace maps, which most distributions give every user. So
//! the users with no session on a seat share one small cap between them,
//! and a user logged in at a seat, whose daemon is the service's client,
//! has room of their own that no other user's connections take. Nobody
//! can log in at a seat as a subordinate user ID.
//!
//! A connection the service won't serve is closed on a thread of its own.
//! A client may send descriptors as soon as it has connected, before it
//! is accepted, and closing its connection releases them on the thread
//! that closes it, which one whose release blocks (a TCP socket made to
//! linger) holds there. So the main thread, which accepts, closes none.

use std::collections::BTreeMap;
use std::io;
use std::os::fd::OwnedFd;
use std::sync::mpsc::{self, SyncSender, TrySendError};

use tracing::error;

/// The most connections a user with a session on a seat may hold at
/// once. Their daemon holds one for each pad it has hidden, and one more
/// for each request under way beside them: one for each pad it is taking,
/// one for a DualSense's copy, one for its first look. That is two for
/// each pad and two more at most: 34 for sixteen pads, with a few over.
const SEATED_PER_USER: usize = 40;

/// The most connections a user with no session on a seat may hold at
/// once. Such a user is never in front of the machine, so is refused
/// every request; their daemon, which can start before their session
/// does, looks for the service again when it next needs it.
const UNSEATED_PER_USER: usize = 8;

/// The most connections every user with no session on a seat may hold
/// at once, between them.
const UNSEATED: usize = 32;

/// The user an unknown peer is counted as.
const NOBODY: u32 = u32::MAX;

/// Whose a connection is, as it was counted when accepted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Holder {
    user: u32,
    seated: bool,
}

/// The connections open, by user.
#[derive(Debug, Default)]
pub(crate) struct Connections {
    by_user: BTreeMap<u32, usize>,
    unseated: usize,
}

impl Connections {
    /// Counts a new connection of `user` (`None` when unknown), who has
    /// a session on a seat if `seated`. Returns who holds it, to release
    /// it by, or `None` if it is over a cap and is to be closed at once.
    pub(crate) fn admit(&mut self, user: Option<u32>, seated: bool) -> Option<Holder> {
        let user = user.unwrap_or(NOBODY);
        let seated = seated && user != NOBODY;
        let count = self.by_user.get(&user).copied().unwrap_or(0);
        let most = if seated {
            SEATED_PER_USER
        } else {
            UNSEATED_PER_USER
        };
        if count >= most || (!seated && self.unseated >= UNSEATED) {
            return None;
        }
        *self.by_user.entry(user).or_default() += 1;
        if !seated {
            self.unseated += 1;
        }
        Some(Holder { user, seated })
    }

    /// Releases a connection [`admit`](Self::admit) counted.
    pub(crate) fn release(&mut self, holder: Holder) {
        if let Some(count) = self.by_user.get_mut(&holder.user) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                self.by_user.remove(&holder.user);
            }
        }
        if !holder.seated {
            self.unseated = self.unseated.saturating_sub(1);
        }
    }

    /// How many connections are open.
    pub(crate) fn open(&self) -> usize {
        self.by_user.values().sum()
    }
}

/// How many connections may wait for the closer. A close that blocks
/// holds these behind it, and the service accepts nothing more until
/// there is room, leaving clients in the backlog.
pub(crate) const CLOSING: usize = 64;

/// The thread that closes the connections the service won't serve.
#[derive(Debug)]
pub(crate) struct Closer {
    queue: SyncSender<OwnedFd>,
    /// Connections the queue had no room for, kept until it has.
    kept: Vec<OwnedFd>,
}

impl Closer {
    /// Starts the thread, with room for `room` connections waiting, and
    /// `closed` called after each close.
    pub(crate) fn start(room: usize, closed: impl Fn() + Send + 'static) -> io::Result<Self> {
        let (queue, closing) = mpsc::sync_channel::<OwnedFd>(room);
        std::thread::Builder::new()
            .name("closer".into())
            .spawn(move || {
                for connection in closing {
                    drop(connection);
                    closed();
                }
            })?;
        Ok(Self {
            queue,
            kept: Vec::new(),
        })
    }

    /// Has `connection` closed on the closer's thread, never this one:
    /// kept, if there is no room for it yet.
    pub(crate) fn close(&mut self, connection: OwnedFd) {
        match self.queue.try_send(connection) {
            Ok(()) => {}
            Err(TrySendError::Full(connection)) => self.kept.push(connection),
            Err(TrySendError::Disconnected(connection)) => {
                // The thread ends only by a panic. Left open, as closing
                // it here could block whoever accepts.
                error!("the closer has stopped; a connection it was to close is left open");
                std::mem::forget(connection);
            }
        }
    }

    /// Whether a connection is kept for want of room. Until it is handed
    /// over, nothing more should be accepted.
    pub(crate) fn full(&self) -> bool {
        !self.kept.is_empty()
    }

    /// Hands over what was kept, as far as there is room now.
    pub(crate) fn retry(&mut self) {
        for connection in std::mem::take(&mut self.kept) {
            self.close(connection);
        }
    }
}

impl Drop for Closer {
    /// Only as the service ends: what is kept is left for the kernel to
    /// close as the process exits, when a socket never lingers.
    fn drop(&mut self) {
        for connection in self.kept.drain(..) {
            std::mem::forget(connection);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};
    use std::os::fd::AsFd;
    use std::time::Duration;

    use rustix::event::{PollFd, PollFlags, poll};
    use rustix::io::Errno;
    use rustix::net::{AddressFamily, RecvFlags, SocketFlags, SocketType, socketpair};

    use super::*;

    /// A TCP socket whose close lingers: its peer, returned with it, never
    /// reads, so once its window is full neither its data nor the end
    /// after it can go. Dropping the peer resets it, which ends the wait.
    fn lingering() -> (OwnedFd, TcpStream) {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let mut stuck = TcpStream::connect(server.local_addr().unwrap()).unwrap();
        let (peer, _) = server.accept().unwrap();
        rustix::net::sockopt::set_socket_linger(&stuck, Some(Duration::from_secs(3600))).unwrap();
        stuck.set_nonblocking(true).unwrap();
        let chunk = [0u8; 65536];
        let full = loop {
            if let Err(error) = stuck.write(&chunk) {
                break error;
            }
        };
        assert_eq!(full.kind(), io::ErrorKind::WouldBlock);
        (OwnedFd::from(stuck), peer)
    }

    /// A connection, as the service's end and the client's, that carries
    /// `fds` as a client can send them before it is accepted.
    fn connection(fds: &[OwnedFd]) -> (OwnedFd, OwnedFd) {
        let (service, client) = socketpair(
            AddressFamily::UNIX,
            SocketType::SEQPACKET,
            SocketFlags::CLOEXEC,
            None,
        )
        .unwrap();
        let fds: Vec<_> = fds.iter().map(AsFd::as_fd).collect();
        ctrlassist_hider::send(client.as_fd(), &[6, 0, 0, 0, 0], &fds).unwrap();
        (service, client)
    }

    /// Whether the service's end of the client's connection is still open.
    fn open(client: &OwnedFd) -> bool {
        rustix::net::recv(client, &mut [0u8; 8], RecvFlags::DONTWAIT) == Err(Errno::AGAIN)
    }

    /// Where the closer's thread sleeps, as the kernel names it: `0` while
    /// it runs. No other test in this binary starts a closer.
    fn closer_sleeps_in() -> String {
        let mut found = Vec::new();
        for task in std::fs::read_dir("/proc/self/task").unwrap() {
            let task = task.unwrap().path();
            if std::fs::read_to_string(task.join("comm")).unwrap().trim() == "closer" {
                found.push(task);
            }
        }
        let [task] = found.as_slice() else {
            panic!("one closer's thread, not {}", found.len());
        };
        std::fs::read_to_string(task.join("wchan")).unwrap()
    }

    #[test]
    fn a_close_that_blocks_holds_up_the_closer_and_nothing_else() {
        let (closes, closed) = mpsc::channel();
        let mut closer = Closer::start(2, move || closes.send(()).unwrap()).unwrap();
        let (stuck, peer) = lingering();
        let (service, blocked) = connection(&[stuck]);
        closer.close(service);
        // The closer has begun closing it: its end is gone.
        poll(&mut [PollFd::new(&blocked, PollFlags::IN)], None).unwrap();
        // And is held in the close, waiting for the lingering socket's data
        // to go: either it is found asleep there, or the close has ended
        // and it has said so.
        loop {
            assert_eq!(
                closed.try_recv(),
                Err(mpsc::TryRecvError::Empty),
                "the close didn't block"
            );
            if closer_sleeps_in() == "wait_woken" {
                break;
            }
            std::thread::yield_now();
        }
        // Each handed over at once all the same: two have room behind it,
        // and the next is kept, with nothing more to be accepted.
        let behind: Vec<_> = (0..3).map(|_| connection(&[])).collect();
        let mut clients = Vec::new();
        for (count, (service, client)) in behind.into_iter().enumerate() {
            assert!(!closer.full(), "{count} handed over");
            closer.close(service);
            clients.push(client);
        }
        assert!(closer.full());
        assert!(clients.iter().all(open));
        assert_eq!(closed.try_recv(), Err(mpsc::TryRecvError::Empty));
        // The close ends: the rest are closed, what was kept once there is
        // room for it.
        drop(peer);
        for _ in 0..4 {
            closed.recv().unwrap();
            closer.retry();
        }
        assert!(!closer.full());
        assert!(!clients.iter().any(open));
    }

    #[test]
    fn a_user_at_a_seat_has_room_for_a_daemon_with_sixteen_pads() {
        let mut connections = Connections::default();
        // A connection for each pad hidden, one for each being taken, and
        // two more.
        let daemon = 2 * 16 + 2;
        let held: Vec<Holder> = (0..daemon)
            .map(|_| connections.admit(Some(1000), true).expect("room"))
            .collect();
        for _ in daemon..SEATED_PER_USER {
            assert!(connections.admit(Some(1000), true).is_some());
        }
        assert_eq!(connections.admit(Some(1000), true), None);
        assert!(connections.admit(Some(1001), true).is_some());
        connections.release(held[0]);
        assert!(connections.admit(Some(1000), true).is_some());
        assert_eq!(connections.open(), SEATED_PER_USER + 1);
    }

    #[test]
    fn one_user_with_no_seat_holds_at_most_eight() {
        let mut connections = Connections::default();
        for _ in 0..UNSEATED_PER_USER {
            assert!(connections.admit(Some(1001), false).is_some());
        }
        assert_eq!(connections.admit(Some(1001), false), None);
        assert!(connections.admit(Some(1002), false).is_some());
    }

    #[test]
    fn users_with_no_seat_share_one_cap_that_leaves_room_for_those_with_one() {
        let mut connections = Connections::default();
        // One user's subordinate IDs, as many connections each as one may
        // hold, until the shared cap is reached.
        let mut held = Vec::new();
        for user in 100_000..100_100 {
            for _ in 0..UNSEATED_PER_USER {
                if let Some(holder) = connections.admit(Some(user), false) {
                    held.push(holder);
                }
            }
        }
        assert_eq!(held.len(), UNSEATED);
        assert_eq!(connections.admit(Some(200_000), false), None);
        assert_eq!(connections.admit(None, false), None);
        // A user at a seat still has all their room.
        for _ in 0..SEATED_PER_USER {
            assert!(connections.admit(Some(1000), true).is_some());
        }
        // One of the shared closes: one more fits, and no more.
        connections.release(held[3]);
        assert!(connections.admit(Some(200_000), false).is_some());
        assert_eq!(connections.admit(Some(200_001), false), None);
        assert_eq!(connections.open(), UNSEATED + SEATED_PER_USER);
    }

    #[test]
    fn an_unknown_peer_counts_as_one_user_with_no_seat() {
        let mut connections = Connections::default();
        let holder = connections.admit(None, true).expect("room");
        assert!(!holder.seated);
        for _ in 1..UNSEATED_PER_USER {
            assert!(connections.admit(None, true).is_some());
        }
        assert_eq!(connections.admit(None, true), None);
    }

    #[test]
    fn a_release_goes_to_the_count_it_was_taken_from() {
        let mut connections = Connections::default();
        // The same user, counted once with no seat and once with one, as
        // when they log in between two connections.
        let before = connections.admit(Some(1000), false).expect("room");
        let after = connections.admit(Some(1000), true).expect("room");
        assert_eq!(connections.unseated, 1);
        connections.release(after);
        assert_eq!(connections.unseated, 1);
        connections.release(before);
        assert_eq!(connections.unseated, 0);
        assert_eq!(connections.open(), 0);
        assert!(connections.by_user.is_empty());
    }
}
