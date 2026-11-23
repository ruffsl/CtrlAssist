//! Writing to a pad off the input thread.
//!
//! What a pad is sent can wait on something the input thread must never
//! wait on. A write to a USB pad's raw node is a synchronous transfer:
//! the kernel sends it at the pad's next interval and waits, up to five
//! seconds for a pad that stalls, whatever the descriptor's flags. An
//! update of a pad's kernel effect, for a pad another program makes
//! through uinput, waits until that program answers, up to 30 seconds;
//! and every write and request on a pad's event node waits while anyone
//! else's request holds that node, as such an update does. So each pad
//! that is sent anything has a thread of its own that sends it: whole
//! output reports to its raw node, or its rumble level to its one kernel
//! effect. The input thread only leaves what the pad should be sent in
//! a one-slot mailbox and wakes the writer: neither waits for the other,
//! and nothing is allocated. What the input thread passes is everything
//! the games have set, not a change, so a newer one takes the place of
//! one not yet written and loses nothing; what a report asks once is
//! gathered beside it until it is written. Every report written carries
//! the latest rumble level, so a level and the games' settings that
//! change together are written once, together.

use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering, fence};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{JoinHandle, Thread};

use ctrlassist_core::Level;
use evdev::{FFEffect, FFEffectData, FFEffectKind, FFReplay, FFTrigger};

use crate::dualsense::{Composer, GAME_SIZE, Kind, Once};

/// The mailbox's size in words: a game's report, then one for the level
/// and the count of game reports.
const WORDS: usize = GAME_SIZE / 8 + 1;

/// What a pad should be sent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Wanted {
    /// The games' settings last passed to it, with nothing asked once.
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

    /// The sequence now: odd while a put is under way.
    fn sequence(&self) -> u64 {
        self.sequence.load(Ordering::Acquire)
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
    /// What was asked once and not yet taken, as [`Once`]'s bits: added
    /// after the settings it came with are put, and taken by the writer
    /// between two looks at the mailbox (see [`write_out`]).
    once: AtomicU32,
    /// Something handed over to go after the pad's last write: dropped
    /// by the writer as it ends, and never by whoever let the pad go.
    after: Mutex<Option<Box<dyn Send>>>,
}

/// Where a writer writes: what it does with what the pad should be sent,
/// and how it leaves the pad.
pub(crate) trait Target: Send + 'static {
    /// Sends the pad `level`, with the games' settings where they changed
    /// since they were last sent, and what is asked `once`.
    fn send(&mut self, game: Option<&[u8; GAME_SIZE]>, level: Level, once: Once) -> io::Result<()>;

    /// Leaves the pad as no game had it. The writer's last act on it.
    fn rest(self);
}

/// A PlayStation pad's raw HID node: the pad is sent whole output
/// reports. Left with its motors stopped, a DualSense's trigger effects
/// off, and the lights its kernel driver keeps (see
/// [`Kind::driver_lights`]).
struct RawNode<W> {
    out: W,
    kind: Kind,
    composer: Composer,
    /// The HID device the node is of, whose LED devices say what its
    /// driver shows: read as the writer ends.
    device: Option<PathBuf>,
}

impl<W: Write + Send + 'static> RawNode<W> {
    fn new(out: W, kind: Kind, device: Option<PathBuf>) -> Self {
        Self {
            out,
            kind,
            composer: Composer::new(kind),
            device,
        }
    }
}

impl<W: Write + Send + 'static> Target for RawNode<W> {
    fn send(&mut self, game: Option<&[u8; GAME_SIZE]>, level: Level, once: Once) -> io::Result<()> {
        let (report, length) = self.composer.compose(game, level, once);
        self.out.write_all(report.get(..length).unwrap_or(&report))
    }

    fn rest(mut self) {
        let lights = self.kind.driver_lights(self.device.as_deref());
        let (rest, length) = self.composer.rest(lights);
        let _ = self.out.write_all(rest.get(..length).unwrap_or(&rest));
    }
}

/// A rumble at `level` with no length: it runs until it is changed or
/// stopped. When each rumble ends is the router's business.
pub(crate) fn endless(level: Level) -> FFEffectData {
    FFEffectData {
        direction: 0,
        trigger: FFTrigger::default(),
        replay: FFReplay {
            length: 0,
            delay: 0,
        },
        kind: FFEffectKind::Rumble {
            strong_magnitude: level.strong,
            weak_magnitude: level.weak,
        },
    }
}

/// The one kernel effect a pad holds however many games rumble it,
/// uploaded at rest as the pad was opened: a new level updates it in
/// place and plays it, with no gap and no second slot, and a level of
/// nothing stops it. Left erased, which stops it.
struct KernelEffect {
    effect: FFEffect,
    /// The level it is at, zero while it is stopped.
    level: Level,
}

impl Target for KernelEffect {
    fn send(&mut self, _: Option<&[u8; GAME_SIZE]>, level: Level, _: Once) -> io::Result<()> {
        if level == self.level {
            return Ok(());
        }
        if level == Level::default() {
            self.effect.stop()?;
        } else {
            // Played again as well as updated: a memless driver, as a
            // rumble-only pad's is, goes on at the new strength either
            // way, and another program's uinput pad may act only on a
            // play.
            self.effect.update(endless(level))?;
            self.effect.play(1)?;
        }
        self.level = level;
        Ok(())
    }

    fn rest(self) {
        // Dropping the effect erases it, and the kernel stops an effect
        // before it erases it, whatever the pad's driver then answers: so
        // the pad's last write is a level of nothing. For a pad another
        // program makes, the erase waits for that program; here, that
        // holds nobody else.
        drop(self.effect);
    }
}

/// What a pad is sent, as the input thread tells it: its rumble level,
/// and for a PlayStation pad given its raw node, what the games set on
/// a virtual DualSense. A DualSense gets the game's report as it is,
/// lights and trigger effects; a DualShock 4 gets the lightbar's color.
/// Rumble then goes that way too, and not through the kernel's own
/// effects, so every report written carries the level the pad should be
/// at. Any other pad that can rumble is sent its level through its one
/// kernel effect.
///
/// The writes are made by a thread of the pad's own, at ordinary
/// priority. Dropping this has it write what is left, then leave the pad
/// as no game had it (see [`Target::rest`]), and then end; whoever drops
/// this doesn't wait for any of it.
pub(crate) struct Output {
    shared: Arc<Shared>,
    writer: Thread,
    /// The kind of pad whose raw node this writes to; none for an effect.
    kind: Option<Kind>,
    wanted: Wanted,
    /// What was asked once and not yet put.
    once: Once,
    /// Whether what is wanted is kept back until it is let go.
    held: bool,
    /// Whether what is wanted changed while kept back.
    changed: bool,
}

impl Output {
    /// Takes a pad's raw node, if it is one this knows how to write to:
    /// a DualSense's or a DualShock 4's, over USB or Bluetooth.
    pub(crate) fn raw(raw: OwnedFd) -> io::Result<Self> {
        let kind = Kind::of(raw.as_fd())?;
        // The HID device the node is of, whose LED devices say what its
        // driver shows: read as the writer ends, off the input thread.
        let device = rustix::fs::fstat(&raw).ok().map(|stat| {
            let (major, minor) = (
                rustix::fs::major(stat.st_rdev),
                rustix::fs::minor(stat.st_rdev),
            );
            PathBuf::from(format!("/sys/dev/char/{major}:{minor}/device"))
        });
        let target = RawNode::new(File::from(raw), kind, device);
        let (output, _) = Self::start(target, Some(kind), "pad-output")?;
        Ok(output)
    }

    /// Takes a pad's one kernel effect, uploaded at rest.
    pub(crate) fn effect(effect: FFEffect) -> io::Result<Self> {
        let target = KernelEffect {
            effect,
            level: Level::default(),
        };
        let (output, _) = Self::start(target, None, "pad-rumble")?;
        Ok(output)
    }

    /// Starts a writer, named `name`, on `target`: a raw node of a pad of
    /// `kind`, or an effect.
    fn start<T: Target>(
        target: T,
        kind: Option<Kind>,
        name: &str,
    ) -> io::Result<(Self, JoinHandle<()>)> {
        Self::start_with(target, kind, name, || {})
    }

    /// As [`start`](Self::start), with `between` called by the writer between
    /// its look in the mailbox and its taking what was asked once: for a
    /// test to hold it there.
    fn start_with<T: Target>(
        mut target: T,
        kind: Option<Kind>,
        name: &str,
        between: impl Fn() + Send + 'static,
    ) -> io::Result<(Self, JoinHandle<()>)> {
        let shared = Arc::new(Shared {
            mailbox: Mailbox::new(),
            stop: AtomicBool::new(false),
            refused: AtomicI32::new(0),
            once: AtomicU32::new(0),
            after: Mutex::new(None),
        });
        let writer = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name(name.to_string())
                .spawn(move || {
                    write_out(&shared, &mut target, &between);
                    // What the pad is left with, after whatever was left.
                    target.rest();
                    let after = shared
                        .after
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .take();
                    drop(after);
                })?
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
            once: Once::default(),
            held: false,
            changed: false,
        };
        Ok((output, writer))
    }

    /// Whether this writes whole output reports to a raw node.
    pub(crate) fn is_raw(&self) -> bool {
        self.kind.is_some()
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

    /// Sends the pad what the games have set on a virtual DualSense, as
    /// one report, with the rumble the pad should be at in place of the
    /// report's: the whole of it for a DualSense, with what it asks once,
    /// and for a DualShock 4 the lightbar's color, if it is set. The same
    /// settings again are nothing new, and so is anything for a pad that
    /// is sent only its level.
    ///
    /// Fails if the pad refused a write since a refusal was last said.
    pub(crate) fn pass(&mut self, report: &[u8]) -> io::Result<()> {
        let Some(kind) = self.kind else {
            return Ok(());
        };
        let mut game = [0u8; GAME_SIZE];
        for (place, byte) in game.iter_mut().zip(report) {
            *place = *byte;
        }
        if !kind.wants(&game) {
            return Ok(());
        }
        let once = match kind {
            Kind::DualSense { .. } => Once::of(&game),
            Kind::DualShock4 { .. } => Once::default(),
        };
        let game = Once::strip(game);
        if game == self.wanted.game && once.is_empty() {
            return self.refusal();
        }
        if game != self.wanted.game {
            self.wanted.game = game;
            self.wanted.games = self.wanted.games.wrapping_add(1);
        }
        self.once = self.once.then(once);
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

    /// Hands the writer `after`, to drop once it has left the pad at
    /// rest, as it ends, so that whoever drops this never waits on it.
    pub(crate) fn leave_after(&mut self, after: Box<dyn Send>) {
        *self
            .shared
            .after
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(after);
    }

    fn change(&mut self) {
        if self.held {
            self.changed = true;
        } else {
            self.publish();
        }
    }

    fn publish(&mut self) {
        self.shared.mailbox.put(self.wanted.to_words());
        let once = std::mem::take(&mut self.once);
        if !once.is_empty() {
            // The writer only ever empties it, so this goes round again
            // only when it just has.
            let mut was = self.shared.once.load(Ordering::Acquire);
            loop {
                let now = Once::from_bits(was).then(once).bits();
                match self.shared.once.compare_exchange_weak(
                    was,
                    now,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => break,
                    Err(seen) => was = seen,
                }
            }
        }
        self.writer.unpark();
    }

    fn refusal(&self) -> io::Result<()> {
        match self.shared.refused.swap(0, Ordering::Relaxed) {
            0 => Ok(()),
            errno => Err(io::Error::from_raw_os_error(errno)),
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        if std::mem::take(&mut self.changed) {
            self.publish();
        }
        self.shared.stop.store(true, Ordering::Release);
        self.writer.unpark();
    }
}

/// The writer: sends what is wanted each time it changes, and sleeps
/// between. Ends once the input thread's side is gone and what it left
/// is sent.
///
/// What is asked once goes in the first report written whose settings
/// came with it or after it, and never in a later one: no settings put
/// after it are written before it. The input thread puts the settings,
/// then adds what they ask once; this looks in the mailbox, then takes
/// what was asked once, then looks at the mailbox's sequence again, and
/// writes only if it hasn't moved. Taken by the swap, anything asked once
/// was added after its own settings were put, and the swap's acquiring
/// it makes that put's sequence seen by the look after: unmoved, it is
/// the one read, so those settings or later ones are what is written
/// with it. Moved, what was taken is kept and the mailbox looked in
/// again. And settings put after something asked once were put after it
/// was added, so a look that reads them is followed by a swap that sees
/// it, if it isn't already kept: it goes with them.
fn write_out<T: Target>(shared: &Shared, target: &mut T, between: &impl Fn()) {
    let (mut written, mut games) = (0u64, 0u32);
    // What was asked once, taken and not yet written.
    let mut once = Once::default();
    loop {
        // Before the mailbox is looked in, so that what was put before
        // the stop is written.
        let stopping = shared.stop.load(Ordering::Acquire);
        let (sequence, words) = shared.mailbox.get();
        between();
        once = once.then(Once::from_bits(shared.once.swap(0, Ordering::AcqRel)));
        if shared.mailbox.sequence() != sequence {
            // Put since the look: what was just taken may have come with
            // what was put, which is to be read first.
            continue;
        }
        if sequence != written || !once.is_empty() {
            written = sequence;
            let wanted = Wanted::from_words(words);
            let game = (wanted.games != games).then_some(&wanted.game);
            games = wanted.games;
            if let Err(error) = target.send(game, wanted.level, std::mem::take(&mut once)) {
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
        Output,
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
        let target = RawNode::new(recorder, USB, None);
        let (output, writer) = Output::start(target, Some(USB), "pad-output").unwrap();
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

    /// Read while being put, what comes out is always one put whole, on
    /// the machine the test runs on. On x86, which neither reorders one
    /// thread's stores nor its loads, this can't catch a fence left out:
    /// that the fences are right for a weaker machine rests on the
    /// sequence lock's argument in `put` and `get`, not on this. A model
    /// checker such as loom would show it, but is a new dependency for
    /// one test.
    #[test]
    fn the_mailbox_is_never_read_half_put_on_this_machine() {
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

    /// The trigger effects off, the motors stopped and the driver's own
    /// lights: what a DualSense is left with, here with nothing in sysfs
    /// to say its player LEDs, which are then left as they are.
    fn is_rest(report: &[u8]) -> bool {
        report.len() == 48
            && (report[1], report[2], report[39]) == (0x0c, 0x04, 0)
            && (report[11], report[22]) == (0x05, 0x05)
            && report[3..5] == [0, 0]
            && report[45..48] == [0, 0, 128]
    }

    /// A level and a report told together are one write carrying both;
    /// once the input thread's side is dropped, the writer writes what
    /// is left, then the pad's rest, and ends.
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
        assert_eq!(rest.len(), 2, "{rest:?}");
        let stop = &rest[0];
        assert_eq!((stop[1], stop[39], stop[3], stop[4]), (0, 0, 0, 0));
        assert_eq!(stop[45], 0, "a level alone changes nothing else");
        assert!(is_rest(&rest[1]), "{rest:?}");
    }

    /// Dropped with its motors running and a trigger effect set, the
    /// pad's last write stops the one and turns off the other.
    #[test]
    fn a_pad_let_go_is_left_at_rest() {
        let (mut output, writer, reports, _) = recorded(false, None);
        let mut trigger = lights(0x11);
        (trigger[1], trigger[11], trigger[12]) = (0x04, 0x01, 0x80);
        output.pass(&trigger).unwrap();
        output.rumble(level(0x8000, 0x4000)).unwrap();
        drop(output);
        writer.join().unwrap();
        let written: Vec<Vec<u8>> = reports.try_iter().collect();
        let last = written.last().unwrap();
        assert!(is_rest(last), "{written:?}");
        assert_eq!(last[12], 0, "nothing of the effect is left");
        // A DualShock 4 is left with its rumble stopped and the color its
        // driver keeps, or with its color as it was where that's unknown.
        let mut ds4 = Composer::new(Kind::DualShock4 { bluetooth: false });
        let lights = crate::dualsense::Lights {
            color: Some([0, 0, 0x40]),
            players: None,
        };
        let (report, length) = ds4.rest(lights);
        assert_eq!(report[..length][..9], [0x05, 0x03, 0, 0, 0, 0, 0, 0, 0x40]);
        let (report, _) = ds4.rest(crate::dualsense::Lights::default());
        assert_eq!(report[1], 0x01, "the motors alone");
        // A DualSense's player LEDs, where known, are the driver's.
        let mut usb = Composer::new(USB);
        let lights = crate::dualsense::Lights {
            color: Some([0, 0, 128]),
            players: Some(0x04),
        };
        let (report, _) = usb.rest(lights);
        assert_eq!((report[2], report[44]), (0x14, 0x04));
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
        open.send(()).unwrap();
        writer.join().unwrap();
        let rest: Vec<Vec<u8>> = reports.try_iter().collect();
        assert!(
            rest.len() == 1 && is_rest(&rest[0]),
            "only the rest: {rest:?}"
        );
    }

    /// Settings that replace others not yet written lose nothing, being
    /// everything the games set; what each asked once is gathered until
    /// a write carries it; and the same settings again write nothing.
    #[test]
    fn what_is_asked_once_is_gathered_until_it_is_written() {
        let (mut output, writer, reports, open) = recorded(true, None);
        output.pass(&lights(0x11)).unwrap();
        let first = reports.recv().unwrap();
        assert_eq!(first[45], 0x11);
        // The writer is held in its first write. The LEDs are given back,
        // and the newer settings, which no longer set them, come after.
        let mut release = [0u8; GAME_SIZE];
        (release[0], release[2]) = (0x02, 0x08);
        output.pass(&release).unwrap();
        let mut trigger = release;
        (trigger[1], trigger[2], trigger[11]) = (0x04, 0, 0x01);
        output.pass(&trigger).unwrap();
        open.send(()).unwrap();
        let second = reports.recv().unwrap();
        assert_eq!((second[1], second[2], second[11]), (0x04, 0x08, 0x01));
        open.send(()).unwrap();
        // The same settings again are nothing new.
        output.pass(&trigger).unwrap();
        output.rumble(level(0x1000, 0)).unwrap();
        let third = reports.recv().unwrap();
        assert_eq!(
            (third[2], third[4]),
            (0, 0x10),
            "the level alone: {third:?}"
        );
        open.send(()).unwrap();
        drop(output);
        open.send(()).unwrap();
        writer.join().unwrap();
    }

    /// What is asked once is never written after settings that followed
    /// it. The writer is held between its two looks, at the mailbox and
    /// at what was asked once, while the lights are let go and then a
    /// color set: the first report with the color lets the lights go too,
    /// and nothing after lets them go again.
    #[test]
    fn what_is_asked_once_never_follows_the_settings_after_it() {
        let (written, reports) = mpsc::channel();
        let recorder = Recorder {
            written,
            gate: None,
            refuse: None,
        };
        let (arrived, held) = mpsc::channel();
        let (go, gate) = mpsc::channel::<()>();
        let gate = Mutex::new(gate);
        let armed = AtomicBool::new(true);
        let between = move || {
            if armed.swap(false, Ordering::SeqCst) {
                arrived.send(()).unwrap();
                gate.lock().unwrap().recv().unwrap();
            }
        };
        let target = RawNode::new(recorder, USB, None);
        let (mut output, writer) =
            Output::start_with(target, Some(USB), "pad-output", between).unwrap();
        // The writer's first look, held between its two reads.
        held.recv().unwrap();
        let mut release = [0u8; GAME_SIZE];
        (release[0], release[2]) = (0x02, 0x08);
        output.pass(&release).unwrap();
        output.pass(&lights(0xbb)).unwrap();
        go.send(()).unwrap();
        drop(output);
        writer.join().unwrap();
        let written: Vec<Vec<u8>> = reports.try_iter().collect();
        let lets_go = |report: &Vec<u8>| report[2] & 0x08 != 0;
        let colored = written
            .iter()
            .position(|report| report[45] == 0xbb)
            .expect("the color is written");
        assert!(lets_go(&written[colored]), "{written:?}");
        assert_eq!(
            written.iter().filter(|report| lets_go(report)).count(),
            1,
            "{written:?}"
        );
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

    /// What a pad sent only its level is told, as the writer says it.
    #[derive(PartialEq, Debug)]
    enum Told {
        Level(Level),
        Rest,
        /// What was left with the writer was dropped.
        After,
    }

    /// Stands in for a pad whose every write waits until the test says,
    /// as one another program makes waits for that program to answer.
    struct Stalling {
        told: mpsc::Sender<Told>,
        gate: mpsc::Receiver<()>,
    }

    impl Target for Stalling {
        fn send(&mut self, _: Option<&[u8; GAME_SIZE]>, level: Level, _: Once) -> io::Result<()> {
            self.told.send(Told::Level(level)).unwrap();
            self.gate.recv().unwrap();
            Ok(())
        }

        fn rest(self) {
            self.told.send(Told::Rest).unwrap();
        }
    }

    struct Left(mpsc::Sender<Told>);

    impl Drop for Left {
        fn drop(&mut self) {
            self.0.send(Told::After).unwrap();
        }
    }

    /// While a pad's write waits, whoever tells it a level, or drops it,
    /// goes on at once: the newest level is sent once the write is let
    /// go, then the pad is left at rest, and only then is what was left
    /// with the writer dropped.
    #[test]
    fn a_pad_whose_write_waits_holds_nobody_who_tells_it_or_drops_it() {
        let (told_by, told) = mpsc::channel();
        let (open, gate) = mpsc::channel();
        let target = Stalling {
            told: told_by.clone(),
            gate,
        };
        let (mut output, writer) = Output::start(target, None, "pad-rumble").unwrap();
        assert!(!output.is_raw());
        output.rumble(level(0x1000, 0)).unwrap();
        assert_eq!(told.recv().unwrap(), Told::Level(level(0x1000, 0)));
        // The writer is held in that write.
        output.rumble(level(0x2000, 0)).unwrap();
        output.rumble(level(0x3000, 0x3000)).unwrap();
        // Nothing for a pad that is sent only its level.
        output.pass(&lights(0x11)).unwrap();
        output.leave_after(Box::new(Left(told_by)));
        drop(output);
        assert!(told.try_recv().is_err(), "{:?}", told.try_recv());
        open.send(()).unwrap();
        assert_eq!(told.recv().unwrap(), Told::Level(level(0x3000, 0x3000)));
        open.send(()).unwrap();
        assert_eq!(told.recv().unwrap(), Told::Rest);
        assert_eq!(told.recv().unwrap(), Told::After);
        writer.join().unwrap();
        assert!(told.try_recv().is_err(), "nothing after");
    }
}
