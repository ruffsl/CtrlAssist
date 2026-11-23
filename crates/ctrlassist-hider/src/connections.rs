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

use std::collections::BTreeMap;

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

#[cfg(test)]
mod tests {
    use super::*;

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
