//! A bounded queue from one thread to one other, that the sending thread
//! never waits on.
//!
//! The input thread sends its reports through this. The standard
//! library's bounded channel would do but for one thing: a send takes a
//! lock to wake a receiver that is waiting, and the receiver holds the
//! same lock as it starts or stops waiting, so the input thread, at
//! real-time priority, could wait on a thread of ordinary priority that
//! the scheduler has put aside. Here each place in the queue is used by
//! one side at a time, which the two counts settle, so its lock is
//! always free when taken: taking it is one atomic operation and never a
//! wait. The receiver sleeps by parking, and a send wakes it with an
//! unpark, which takes no lock either.
//!
//! The two counts go round when they pass the largest `usize`, after
//! some 4 billion items on a machine of 32 bits. A count is mapped to
//! its place by its low bits, the number of places being a power of two,
//! so a count's place is the same on both sides of the wrap; and how
//! full the queue is, the difference of the two counts taken round, is
//! right across it too. The room is still exactly the capacity asked:
//! the queue counts itself full at that number, whatever places beyond
//! it the power of two adds, which are then never all in use at once.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, TryLockError};
use std::thread::Thread;

struct Shared<T> {
    /// A power of two of them, at least `capacity`.
    places: Box<[Mutex<Option<T>>]>,
    /// How many may wait at once.
    capacity: usize,
    /// How many have been sent, written by the sender alone.
    sent: AtomicUsize,
    /// How many have been taken, written by the receiver alone.
    taken: AtomicUsize,
    sender_gone: AtomicBool,
    receiver_gone: AtomicBool,
}

impl<T> Shared<T> {
    fn place(&self, count: usize) -> &Mutex<Option<T>> {
        // The length is a power of two: this is the count's low bits.
        &self.places[count & (self.places.len() - 1)]
    }
}

/// The sending side, before it knows the thread it wakes.
pub(crate) struct Unbound<T>(Arc<Shared<T>>);

/// The sending side.
pub(crate) struct Sender<T> {
    shared: Arc<Shared<T>>,
    receiver: Thread,
}

/// The receiving side.
pub(crate) struct Receiver<T> {
    shared: Arc<Shared<T>>,
}

/// A queue with room for exactly `capacity`, at least one. The sending
/// side is bound to the receiving thread once that is started.
pub(crate) fn queue<T>(capacity: usize) -> (Unbound<T>, Receiver<T>) {
    queue_from(capacity, 0)
}

/// As [`queue`], with both counts starting at `count`: for trying their
/// wrap.
fn queue_from<T>(capacity: usize, count: usize) -> (Unbound<T>, Receiver<T>) {
    let capacity = capacity.max(1);
    let places = capacity
        .checked_next_power_of_two()
        .expect("a queue's room is some reports, far from half of the address space");
    let shared = Arc::new(Shared {
        places: (0..places).map(|_| Mutex::new(None)).collect(),
        capacity,
        sent: AtomicUsize::new(count),
        taken: AtomicUsize::new(count),
        sender_gone: AtomicBool::new(false),
        receiver_gone: AtomicBool::new(false),
    });
    (Unbound(Arc::clone(&shared)), Receiver { shared })
}

impl<T> Unbound<T> {
    /// The sending side, waking `receiver`, the thread that receives.
    pub(crate) fn to(self, receiver: Thread) -> Sender<T> {
        Sender {
            shared: self.0,
            receiver,
        }
    }
}

impl<T> Sender<T> {
    /// Sends `item` without waiting, or hands it back if the queue is
    /// full or the receiver has gone.
    pub(crate) fn send(&self, item: T) -> Result<(), T> {
        let shared = &*self.shared;
        let sent = shared.sent.load(Ordering::Relaxed);
        let taken = shared.taken.load(Ordering::Acquire);
        if sent.wrapping_sub(taken) >= shared.capacity
            || shared.receiver_gone.load(Ordering::Acquire)
        {
            return Err(item);
        }
        // The receiver is done with this place: it took the item there
        // before it counted it taken.
        match shared.place(sent).try_lock() {
            Ok(mut place) => *place = Some(item),
            Err(TryLockError::Poisoned(poisoned)) => *poisoned.into_inner() = Some(item),
            Err(TryLockError::WouldBlock) => return Err(item),
        }
        shared.sent.store(sent.wrapping_add(1), Ordering::Release);
        self.receiver.unpark();
        Ok(())
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        self.shared.sender_gone.store(true, Ordering::Release);
        self.receiver.unpark();
    }
}

impl<T> Receiver<T> {
    /// The next item, if one waits.
    pub(crate) fn try_recv(&self) -> Option<T> {
        let shared = &*self.shared;
        let taken = shared.taken.load(Ordering::Relaxed);
        if taken == shared.sent.load(Ordering::Acquire) {
            return None;
        }
        // The sender is done with this place: it put the item there
        // before it counted it sent.
        let item = match shared.place(taken).try_lock() {
            Ok(mut place) => place.take(),
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner().take(),
            // Never, as above; were it so, the item stays where it is.
            Err(TryLockError::WouldBlock) => return None,
        };
        shared.taken.store(taken.wrapping_add(1), Ordering::Release);
        item
    }

    /// The next item, waiting for one; none once the sender has gone and
    /// everything it sent has been taken. Called from one thread, the
    /// one the sender was bound to.
    pub(crate) fn recv(&self) -> Option<T> {
        loop {
            // Whether the sender had gone, looked at before the queue so
            // that what it sent before it went is still taken.
            let gone = self.shared.sender_gone.load(Ordering::Acquire);
            if let Some(item) = self.try_recv() {
                return Some(item);
            }
            if gone {
                return None;
            }
            // A send after the look above has unparked this already, and
            // the park returns at once.
            std::thread::park();
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        self.shared.receiver_gone.store(true, Ordering::Release);
        // What waits is dropped here, not by whichever side is last.
        while self.try_recv().is_some() {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_come_out_in_order_and_a_full_queue_hands_one_back() {
        let (unbound, receiver) = queue(3);
        let sender = unbound.to(std::thread::current());
        for item in 0..3 {
            sender.send(item).unwrap();
        }
        assert_eq!(sender.send(3), Err(3), "full");
        assert_eq!(receiver.try_recv(), Some(0));
        sender.send(3).unwrap();
        let rest: Vec<i32> = std::iter::from_fn(|| receiver.try_recv()).collect();
        assert_eq!(rest, [1, 2, 3]);
        assert_eq!(receiver.try_recv(), None);
        drop(sender);
        assert_eq!(receiver.recv(), None, "the sender has gone");
    }

    #[test]
    fn a_receiver_that_has_gone_refuses_what_is_sent_and_drops_what_waited() {
        let (unbound, receiver) = queue(4);
        let sender = unbound.to(std::thread::current());
        let waiting = Arc::new(());
        sender.send(Arc::clone(&waiting)).unwrap();
        drop(receiver);
        assert_eq!(Arc::strong_count(&waiting), 1, "dropped with the receiver");
        assert!(sender.send(Arc::clone(&waiting)).is_err());
    }

    /// Across the counts' wrap, with a room that isn't a power of two,
    /// the queue holds exactly its room and hands everything over once,
    /// in order.
    #[test]
    fn the_counts_going_round_lose_nothing_and_keep_the_room() {
        let (unbound, receiver) = queue_from(3, usize::MAX - 4);
        let sender = unbound.to(std::thread::current());
        let (mut next, mut expected) = (0, 0);
        for _ in 0..6 {
            while sender.send(next).is_ok() {
                next += 1;
            }
            assert_eq!(next - expected, 3, "full at exactly its room");
            for _ in 0..2 {
                assert_eq!(receiver.try_recv(), Some(expected));
                expected += 1;
            }
        }
        let rest: Vec<i32> = std::iter::from_fn(|| receiver.try_recv()).collect();
        assert_eq!(rest, (expected..next).collect::<Vec<_>>());
        assert!(
            receiver.shared.sent.load(Ordering::Relaxed) < 16,
            "the counts went round"
        );
    }

    /// Sent from one thread and received on another, round the queue
    /// many times, everything arrives once and in order.
    #[test]
    fn what_one_thread_sends_another_receives_whole_and_in_order() {
        const ITEMS: u64 = 200_000;
        let (unbound, receiver) = queue(8);
        let receiving = std::thread::spawn(move || {
            let mut next = 0;
            while let Some(item) = receiver.recv() {
                assert_eq!(item, next);
                next += 1;
            }
            next
        });
        let sender = unbound.to(receiving.thread().clone());
        let mut item = 0;
        while item < ITEMS {
            if sender.send(item).is_ok() {
                item += 1;
            } else {
                std::thread::yield_now();
            }
        }
        drop(sender);
        assert_eq!(receiving.join().unwrap(), ITEMS);
    }
}
