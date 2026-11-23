//! Writing to a pad's raw HID node, off the input thread.
//!
//! A write to a USB pad's raw node is a synchronous transfer: the kernel
//! sends it at the pad's next interval and waits, up to five seconds for
//! a pad that stalls, whatever the descriptor's flags. The input thread
//! can't wait, so each pad sent whole output reports has a thread of its
//! own that writes them. The input thread only leaves what the pad should
//! be sent in a one-slot mailbox and wakes the writer: neither waits for
//! the other, and nothing is allocated. A newer report takes the place of
//! one not yet written, and every report written carries the latest
//! rumble level, so a level and a game's report that change together
//! are written once, together.

use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering, fence};
use std::thread::{JoinHandle, Thread};

use ctrlassist_core::Level;

use crate::dualsense::{Composer, GAME_SIZE, Kind};

/// The mailbox's size in words: a game's report, then one for the level
/// and the count of game reports.
const WORDS: usize = GAME_SIZE / 8 + 1;

/// What a pad should be sent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Wanted {
    /// The game's report last passed to it.
    game: [u8; GAME_SIZE],
    /// How many game reports have been passed, counting round: the last
    /// is yet to be written if its count isn't the one last written.
    games: u32,
    /// The level its motors should be at.
    level: Level,
}

impl Wanted {
    fn to_words(self) -> [u64; WORDS] {
        let mut words = [0u64; WORDS];
        for (word, bytes) in words.iter_mut().zip(self.game.as_chunks::<8>().0) {
            *word = u64::from_le_bytes(*bytes);
        }
        if let Some(last) = words.last_mut() {
            *last = u64::from(self.level.strong)
                | (u64::from(self.level.weak) << 16)
                | (u64::from(self.games) << 32);
        }
        words
    }

    fn from_words(words: [u64; WORDS]) -> Self {
        let mut game = [0u8; GAME_SIZE];
        for (bytes, word) in game.as_chunks_mut::<8>().0.iter_mut().zip(words) {
            *bytes = word.to_le_bytes();
        }
        let last = words.last().copied().unwrap_or(0);
        Self {
            game,
            games: (last >> 32) as u32,
            level: Level {
                strong: last as u16,
                weak: (last >> 16) as u16,
            },
        }
    }
}

/// One slot holding the words last put, read whole without a lock: a
/// sequence lock over atomics. One thread puts and one reads; the one
/// that puts never waits, and the one that reads tries again while a put
/// is under way.
struct Mailbox {
    /// Odd while a put is under way; zero before the first.
    sequence: AtomicU64,
    words: [AtomicU64; WORDS],
}

impl Mailbox {
    fn new() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            words: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }

    /// Puts `words` in place of what was there.
    fn put(&self, words: [u64; WORDS]) {
        let at = self.sequence.load(Ordering::Relaxed);
        self.sequence.store(at.wrapping_add(1), Ordering::Relaxed);
        // No word below can be seen before the sequence above is.
        fence(Ordering::Release);
        for (slot, word) in self.words.iter().zip(words) {
            slot.store(word, Ordering::Relaxed);
        }
        self.sequence.store(at.wrapping_add(2), Ordering::Release);
    }

    /// The words last put, whole, and the sequence they were put under.
    fn get(&self) -> (u64, [u64; WORDS]) {
        loop {
            let before = self.sequence.load(Ordering::Acquire);
            if before.is_multiple_of(2) {
                let mut words = [0u64; WORDS];
                for (word, slot) in words.iter_mut().zip(&self.words) {
                    *word = slot.load(Ordering::Relaxed);
                }
                // No word above can be seen after the sequence below is.
                fence(Ordering::Acquire);
                if self.sequence.load(Ordering::Relaxed) == before {
                    return (before, words);
                }
            }
            std::hint::spin_loop();
        }
    }
}

/// What the input thread's side and the writer share.
struct Shared {
    mailbox: Mailbox,
    /// Set once the input thread's side is dropped: the writer writes
    /// what is left and ends.
    stop: AtomicBool,
    /// The error number of a write the pad refused, until it is said.
    refused: AtomicI32,
}

/// A PlayStation pad's raw HID node, as somewhere to write: the pad
/// is sent whole output reports, so that what a game sends a virtual
/// DualSense beyond rumble reaches the pad behind it. A DualSense gets
/// the game's report as it is, lights and trigger effects; a DualShock 4
/// gets the lightbar's color. Rumble then goes this way too, and not
/// through the kernel's own effects, so every report written carries the
/// level the pad should be at.
///
/// The writes are made by a thread of the pad's own, at ordinary
/// priority. Dropping this has it write what is left, the last level
/// included, close the node and end.
pub(crate) struct RawOutput {
    shared: Arc<Shared>,
    writer: Thread,
    kind: Kind,
    wanted: Wanted,
    /// Whether what is wanted is kept back until it is let go.
    held: bool,
    /// Whether what is wanted changed while kept back.
    changed: bool,
}

impl RawOutput {
    /// Takes a pad's raw node, if it is one this knows how to write to:
    /// a DualSense's or a DualShock 4's, over USB or Bluetooth.
    pub(crate) fn open(raw: OwnedFd) -> io::Result<Self> {
        let kind = Kind::of(raw.as_fd())?;
        let (output, _) = Self::start(File::from(raw), kind)?;
        Ok(output)
    }

    /// Starts the writer on `out`.
    fn start<W: Write + Send + 'static>(out: W, kind: Kind) -> io::Result<(Self, JoinHandle<()>)> {
        let shared = Arc::new(Shared {
            mailbox: Mailbox::new(),
            stop: AtomicBool::new(false),
            refused: AtomicI32::new(0),
        });
        let writer = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("pad-output".to_string())
                .spawn(move || write_out(&shared, out, Composer::new(kind)))?
        };
        let output = Self {
            shared,
            writer: writer.thread().clone(),
            kind,
            wanted: Wanted {
                game: [0; GAME_SIZE],
                games: 0,
                level: Level::default(),
            },
            held: false,
            changed: false,
        };
        Ok((output, writer))
    }

    /// Makes the pad's motors run at `level`, with nothing else changed.
    ///
    /// Fails if the pad refused a write since a refusal was last said.
    pub(crate) fn rumble(&mut self, level: Level) -> io::Result<()> {
        if level == self.wanted.level {
            return Ok(());
        }
        self.wanted.level = level;
        self.change();
        self.refusal()
    }

    /// Sends the pad what a game sent a virtual DualSense, with the
    /// rumble the pad should be at in place of the report's: the whole
    /// report for a DualSense, and for a DualShock 4 the lightbar's
    /// color, if the report sets it.
    ///
    /// Fails if the pad refused a write since a refusal was last said.
    pub(crate) fn pass(&mut self, report: &[u8]) -> io::Result<()> {
        let mut game = [0u8; GAME_SIZE];
        for (place, byte) in game.iter_mut().zip(report) {
            *place = *byte;
        }
        if !self.kind.wants(&game) {
            return Ok(());
        }
        self.wanted.game = game;
        self.wanted.games = self.wanted.games.wrapping_add(1);
        self.change();
        self.refusal()
    }

    /// Keeps back what the pad is told until [`release`](Self::release),
    /// so that a level and a report told together are written once.
    pub(crate) fn hold(&mut self) {
        self.held = true;
    }

    /// Has what was kept back written.
    pub(crate) fn release(&mut self) {
        self.held = false;
        if std::mem::take(&mut self.changed) {
            self.publish();
        }
    }

    fn change(&mut self) {
        if self.held {
            self.changed = true;
        } else {
            self.publish();
        }
    }

    fn publish(&self) {
        self.shared.mailbox.put(self.wanted.to_words());
        self.writer.unpark();
    }

    fn refusal(&self) -> io::Result<()> {
        match self.shared.refused.swap(0, Ordering::Relaxed) {
            0 => Ok(()),
            errno => Err(io::Error::from_raw_os_error(errno)),
        }
    }
}

impl Drop for RawOutput {
    fn drop(&mut self) {
        if std::mem::take(&mut self.changed) {
            self.shared.mailbox.put(self.wanted.to_words());
        }
        self.shared.stop.store(true, Ordering::Release);
        self.writer.unpark();
    }
}

/// The writer: writes what is wanted each time it changes, and sleeps
/// between. Ends once the input thread's side is gone and what it left
/// is written.
fn write_out<W: Write>(shared: &Shared, mut out: W, mut composer: Composer) {
    let (mut written, mut games) = (0u64, 0u32);
    loop {
        // Before the mailbox is looked in, so that what was put before
        // the stop is written.
        let stopping = shared.stop.load(Ordering::Acquire);
        let (sequence, words) = shared.mailbox.get();
        if sequence != written {
            written = sequence;
            let wanted = Wanted::from_words(words);
            let game = (wanted.games != games).then_some(&wanted.game);
            games = wanted.games;
            let (report, length) = composer.compose(game, wanted.level);
            let report = report.get(..length).unwrap_or(&report);
            if let Err(error) = out.write_all(report) {
                let errno = error
                    .raw_os_error()
                    .unwrap_or(rustix::io::Errno::IO.raw_os_error());
                shared.refused.store(errno, Ordering::Relaxed);
            }
            continue;
        }
        if stopping {
            return;
        }
        // Woken by every put; a wake with nothing new only looks again.
        std::thread::park();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    const USB: Kind = Kind::DualSense {
        newer: true,
        bluetooth: false,
    };

    fn level(strong: u16, weak: u16) -> Level {
        Level { strong, weak }
    }

    /// A game's report that sets the lightbar's color.
    fn lights(red: u8) -> [u8; GAME_SIZE] {
        let mut report = [0u8; GAME_SIZE];
        report[0] = 0x02;
        (report[2], report[45], report[46], report[47]) = (0x04, red, 0x22, 0x33);
        report
    }

    /// Stands in for a raw node: says each report written, and if gated,
    /// doesn't finish the write until the test says so.
    struct Recorder {
        written: mpsc::Sender<Vec<u8>>,
        gate: Option<mpsc::Receiver<()>>,
        refuse: Option<i32>,
    }

    impl Write for Recorder {
        fn write(&mut self, report: &[u8]) -> io::Result<usize> {
            self.written.send(report.to_vec()).unwrap();
            if let Some(gate) = &self.gate {
                gate.recv().unwrap();
            }
            match self.refuse {
                Some(errno) => Err(io::Error::from_raw_os_error(errno)),
                None => Ok(report.len()),
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn recorded(
        gated: bool,
        refuse: Option<i32>,
    ) -> (
        RawOutput,
        JoinHandle<()>,
        mpsc::Receiver<Vec<u8>>,
        mpsc::Sender<()>,
    ) {
        let (written, reports) = mpsc::channel();
        let (open, gate) = mpsc::channel();
        let recorder = Recorder {
            written,
            gate: gated.then_some(gate),
            refuse,
        };
        let (output, writer) = RawOutput::start(recorder, USB).unwrap();
        (output, writer, reports, open)
    }

    #[test]
    fn what_is_wanted_comes_out_of_the_mailbox_as_it_went_in() {
        let wanted = Wanted {
            game: std::array::from_fn(|at| at as u8),
            games: 0xdead_beef,
            level: level(0x8000, 0x0102),
        };
        assert_eq!(Wanted::from_words(wanted.to_words()), wanted);
        let mailbox = Mailbox::new();
        assert_eq!(mailbox.get().0, 0, "nothing put yet");
        mailbox.put(wanted.to_words());
        let (sequence, words) = mailbox.get();
        assert_eq!((sequence, Wanted::from_words(words)), (2, wanted));
    }

    /// Read while being put, what comes out is always one put whole.
    #[test]
    fn the_mailbox_is_never_read_half_put() {
        let mailbox = Arc::new(Mailbox::new());
        let putting = {
            let mailbox = Arc::clone(&mailbox);
            std::thread::spawn(move || {
                for round in 1..=200_000u64 {
                    mailbox.put([round; WORDS]);
                }
            })
        };
        let mut last = 0;
        while last < 200_000 {
            let (_, words) = mailbox.get();
            assert!(words.iter().all(|word| *word == words[0]), "{words:?}");
            assert!(words[0] >= last, "never older than what was read");
            last = words[0];
        }
        putting.join().unwrap();
    }

    /// A level and a report told together are one write carrying both;
    /// once the input thread's side is dropped, the writer writes what
    /// is left and ends.
    #[test]
    fn a_level_and_a_report_told_together_are_written_once() {
        let (mut output, writer, reports, _) = recorded(false, None);
        output.hold();
        output.pass(&lights(0x11)).unwrap();
        output.rumble(level(0x8000, 0x4000)).unwrap();
        output.release();
        let report = reports.recv().unwrap();
        assert_eq!(report.len(), 48);
        assert_eq!((report[1], report[39]), (0x02, 0x04), "rumble marked");
        assert_eq!(report[3..5], [0x40, 0x80]);
        assert_eq!(report[45..48], [0x11, 0x22, 0x33], "and the lights");
        // The same level again is nothing new.
        output.rumble(level(0x8000, 0x4000)).unwrap();
        output.rumble(Level::default()).unwrap();
        drop(output);
        writer.join().unwrap();
        let rest: Vec<Vec<u8>> = reports.try_iter().collect();
        assert_eq!(rest.len(), 1, "{rest:?}");
        let stop = &rest[0];
        assert_eq!((stop[1], stop[39], stop[3], stop[4]), (0, 0, 0, 0));
        assert_eq!(stop[45], 0, "a level alone changes nothing else");
    }

    /// While a write is under way, what the input thread says replaces
    /// what it said before: the newest report is written, once, with the
    /// newest level.
    #[test]
    fn a_newer_report_takes_the_place_of_one_not_yet_written() {
        let (mut output, writer, reports, open) = recorded(true, None);
        output.rumble(level(0x1000, 0)).unwrap();
        let first = reports.recv().unwrap();
        assert_eq!((first[4], first[45]), (0x10, 0), "the level alone");
        // The writer is in its first write, held there by the gate.
        output.pass(&lights(0xaa)).unwrap();
        output.pass(&lights(0xbb)).unwrap();
        output.rumble(level(0x2000, 0)).unwrap();
        open.send(()).unwrap();
        let second = reports.recv().unwrap();
        assert_eq!((second[4], second[45]), (0x20, 0xbb), "{second:?}");
        drop(output);
        open.send(()).unwrap();
        writer.join().unwrap();
        assert_eq!(reports.try_iter().count(), 0, "nothing was left");
    }

    /// A write the pad refuses is said at the next thing it is told.
    #[test]
    fn a_refused_write_is_said_next_time() {
        let enodev = rustix::io::Errno::NODEV.raw_os_error();
        let (mut output, writer, reports, _) = recorded(false, Some(enodev));
        output.rumble(level(0x1000, 0)).unwrap();
        reports.recv().unwrap();
        // The refusal is noted just after the write was tried.
        while output.shared.refused.load(Ordering::Relaxed) == 0 {
            std::thread::yield_now();
        }
        let refused = output.rumble(level(0x2000, 0)).unwrap_err();
        assert_eq!(refused.raw_os_error(), Some(enodev));
        drop(output);
        writer.join().unwrap();
    }
}
