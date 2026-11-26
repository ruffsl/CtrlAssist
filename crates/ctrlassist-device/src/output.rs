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
//!
//! A Steam Controller's writer also writes when nothing has changed, as
//! the pad's protocol asks: lizard mode off, with its motion sensors on,
//! as it starts and every [`steam::LIZARD_MODE_AGAIN`] after, and its
//! rumble again every [`steam::RUMBLE_AGAIN`] while it is not at rest.
//! It sleeps until the next of those is due, or something changes; the
//! input thread knows nothing of them.

use std::fs::File;
use std::io::{self, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering, fence};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{JoinHandle, Thread};
use std::time::Instant;

use ctrlassist_core::Level;
use evdev::{FFEffect, FFEffectData, FFEffectKind, FFReplay, FFTrigger};

use crate::dualsense::{Composer, GAME_SIZE, Kind, Once};
use crate::{hidraw, steam};

/// The mailbox's size in words: a game's report, then one for the level
/// and the count of game reports.
const WORDS: usize = GAME_SIZE / 8 + 1;
/// How many times the mailbox's reader spins on a put under way before
/// it yields between looks.
const SPINS: u32 = 8;

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
    ///
    /// A put takes a few dozen instructions, so a look that finds one
    /// under way spins a few times, then gives the processor up between
    /// looks: the one putting may have been put aside on this processor.
    fn get(&self) -> (u64, [u64; WORDS]) {
        let mut spins = 0u32;
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
            if spins < SPINS {
                spins += 1;
                std::hint::spin_loop();
            } else {
                std::thread::yield_now();
            }
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
    /// Set once the writer has panicked: it writes nothing more, and
    /// every later thing the pad is told fails.
    broken: AtomicBool,
    /// What was asked once and not yet taken, as [`Once`]'s bits: added
    /// after the settings it came with are put, and taken by the writer
    /// between two looks at the mailbox (see [`write_out`]).
    once: AtomicU32,
    /// Something handed over to go after the pad's last write: dropped
    /// by the writer as it ends, and never by whoever let the pad go.
    after: Mutex<Option<Box<dyn Send>>>,
    /// Said once the writer has left the pad at rest.
    rested: Arc<Done>,
}

/// Whether a writer has left its pad at rest, and a way to wait for it.
#[derive(Debug, Default)]
struct Done {
    rested: Mutex<bool>,
    said: Condvar,
}

/// Says the writer has left the pad at rest as it is dropped: after the
/// rest is written, or as a panic unwinds past it, so that nobody waits
/// for a writer that has gone.
struct Finish(Arc<Done>);

impl Drop for Finish {
    fn drop(&mut self) {
        *self.0.rested.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.0.said.notify_all();
    }
}

/// What a pad's writer writes, and so what [`Rested::wait`] waits for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Writes {
    /// Nothing: the pad is sent nothing, or its writer couldn't be
    /// started. There is nothing to wait for.
    Nothing,
    /// Whole output reports to the pad's raw node: a DualSense or a
    /// DualShock 4 given its raw node. Its last report turns off what a
    /// game set beyond rumble, a DualSense's trigger effects among them,
    /// which a re-bind of the pad's driver doesn't: so this is the write
    /// that someone about to have the driver re-bound waits for. On USB,
    /// that write done is the pad at rest; over Bluetooth, only the rest
    /// on its way (see [`Rested`]).
    ///
    /// A Steam Controller given its raw node too: its rumble reports, the
    /// last of them stopping its motors. Its raw node closed is what has
    /// the kernel's driver give the pad its input devices back.
    Reports,
    /// Levels to the pad's one kernel effect. Its last act erases the
    /// effect, which the kernel stops first.
    Effect,
}

/// Says when the writer of a pad that has been let go has left the pad
/// at rest, and waits for it. Got from [`Pad::rested`](crate::Pad::rested)
/// or [`Pad::let_go`](crate::Pad::let_go).
///
/// What waiting covers is by what the writer [writes](Self::writes):
///
/// - For [`Writes::Reports`], the writer has written what was left and
///   then the report that leaves the pad at rest, each written or refused
///   as by a pad that has gone, and has closed the raw node: nothing is
///   written to the pad after. Written means what the kernel's write
///   means on the pad's transport. On USB the write waits until the pad
///   has taken the report, up to the kernel's five seconds for a pad
///   that stalls, so the pad is at rest. Over Bluetooth it doesn't wait:
///   through the kernel's own Bluetooth HID the report is queued for the
///   session's thread to send, and through BlueZ's uhid it is handed to
///   BlueZ, so the pad is at rest once that is sent. A re-bind of the
///   driver straight after may drop what the kernel has queued, which is
///   suspected and not yet seen (the project's concern,
///   `docs/concerns/rest-over-bluetooth.md`).
/// - For [`Writes::Effect`], the writer has sent the level left and
///   erased the effect. For a pad another program makes through uinput,
///   which has stopped answering, the update under way and the erase can
///   take 30 seconds each. Not covered: the pad's grab let go and its
///   event node closed, which come after on the same thread and can take
///   30 seconds more.
/// - For [`Writes::Nothing`], it is done at once.
///
/// The writer starts on what is left once the pad is dropped, so a wait
/// for a pad that is never dropped never ends. Waiting is a choice:
/// dropping this changes nothing, and never waits. It is bounded by
/// nothing but what the writer is doing, as above; a writer that panics
/// is done once it has tried the rest.
#[derive(Clone, Debug)]
pub struct Rested {
    done: Option<Arc<Done>>,
    writes: Writes,
}

impl Rested {
    /// For a pad with no writer: done already.
    pub(crate) fn already() -> Self {
        Self {
            done: None,
            writes: Writes::Nothing,
        }
    }

    /// What the writer writes, which says what [`wait`](Self::wait)
    /// waits for.
    #[must_use]
    pub fn writes(&self) -> Writes {
        self.writes
    }

    /// Whether the writer has left the pad at rest.
    #[must_use]
    pub fn is_done(&self) -> bool {
        self.done
            .as_ref()
            .is_none_or(|done| *done.rested.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Waits until the writer has left the pad at rest. Blocks the
    /// calling thread, for as long as the writer takes; see [`Rested`].
    pub fn wait(&self) {
        let Some(done) = &self.done else {
            return;
        };
        let mut rested = done.rested.lock().unwrap_or_else(PoisonError::into_inner);
        while !*rested {
            rested = done
                .said
                .wait(rested)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

/// Where a writer writes: what it does with what the pad should be sent,
/// and how it leaves the pad.
pub(crate) trait Target: Send + 'static {
    /// Sends the pad `level`, with the games' settings where they changed
    /// since they were last sent, and what is asked `once`.
    fn send(&mut self, game: Option<&[u8; GAME_SIZE]>, level: Level, once: Once) -> io::Result<()>;

    /// When the pad next wants a write that nothing tells it, as its own
    /// protocol asks: none while it wants nothing until it is told.
    fn due(&self) -> Option<Instant> {
        None
    }

    /// Writes what is due by `now`. A write the pad refuses is its own
    /// business: it is tried again when next due, if the pad is still
    /// there, and the input thread is told nothing of it.
    fn keep_up(&mut self, _now: Instant) {}

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

/// What a Steam Controller's raw node is written: output reports, and
/// feature reports, which are requests on the node.
pub(crate) trait SteamOut: Send + 'static {
    /// Writes an output report, its number first.
    fn output(&mut self, report: &[u8]) -> io::Result<()>;
    /// Sets a feature report, its number first.
    fn feature(&mut self, report: &[u8]) -> io::Result<()>;
}

impl SteamOut for File {
    fn output(&mut self, report: &[u8]) -> io::Result<()> {
        self.write_all(report)
    }

    fn feature(&mut self, report: &[u8]) -> io::Result<()> {
        hidraw::set_feature(self.as_fd(), report)
    }
}

/// A Steam Controller's raw node: the pad is sent its rumble level as
/// output report `0x80`, and nothing a game sets beyond rumble.
///
/// The pad's protocol asks two things of whoever holds the node, at
/// intervals that are the pad's own and not CtrlAssist's (see
/// `steam.rs`): its rumble again every [`steam::RUMBLE_AGAIN`] while it
/// isn't at rest, since the pad stops by itself shortly after the last;
/// and lizard mode off every [`steam::LIZARD_MODE_AGAIN`], since its own
/// watchdog puts it back, the motion sensors asked for each time as SDL
/// asks for them, since Steam turns them off when it has no use for them.
/// The first is written as the writer starts. Nothing is written to put
/// lizard mode back as the pad is let go: the raw node closed, as it is
/// however the daemon ends, has the kernel's driver set the pad up as it
/// was (`steam_work_connect_cb`, `hid-steam.c:1460-1484`), and the pad's
/// own watchdog puts lizard mode back besides once nothing keeps it off.
///
/// The pad's mappings of its buttons to keys and its pointer are not
/// cleared first, as the kernel's driver clears them as it turns lizard
/// mode off (`clear_digital_mappings`): lizard mode is kept off SDL's
/// way, its watchdog left on so that a daemon that hangs leaves the pad
/// to type and point again, and SDL's driver, written for this pad,
/// sends lizard mode off alone. Whether the pad types or points with
/// lizard mode off and its mappings kept only a real pad can show.
///
/// Rumble is the plain level every pad is sent. The pad's richer haptics
/// (a pulse on either trackpad, output report `0x81`, and the reports
/// after it that Steam Input uses for its own effects) are not sent: a
/// game's force feedback, the router's levels and a virtual DualSense's
/// output report have nothing to carry them, and a Steam Controller
/// identity that passed them through would be where they start.
struct SteamNode<O> {
    out: O,
    /// The level the motors were last sent.
    level: Level,
    /// When the rumble is due again: none while the motors are at rest.
    rumble_due: Option<Instant>,
    /// When lizard mode off is due again.
    lizard_due: Option<Instant>,
    /// Whether the node has said the pad has gone (`ENODEV`): nothing
    /// written there will reach it again, so nothing is due.
    gone: bool,
}

impl<O: SteamOut> SteamNode<O> {
    /// The node of a pad starting to be held at `now`: lizard mode off is
    /// due at once.
    fn new(out: O, now: Instant) -> Self {
        Self {
            out,
            level: Level::default(),
            rumble_due: None,
            lizard_due: Some(now),
            gone: false,
        }
    }

    /// Notes what a write said of the pad.
    fn heard(&mut self, result: &io::Result<()>) {
        if let Err(error) = result
            && error.raw_os_error() == Some(rustix::io::Errno::NODEV.raw_os_error())
        {
            self.gone = true;
        }
    }

    /// Sends the motors `level` at `now`, if it isn't what they were last
    /// sent.
    fn rumble_at(&mut self, level: Level, now: Instant) -> io::Result<()> {
        if level == self.level {
            return Ok(());
        }
        self.level = level;
        self.write_rumble(now)
    }

    /// Writes the level the motors are at, and has it due again while it
    /// isn't at rest. A write refused is tried again when due, as a level
    /// at rest needs no write again: the pad stops by itself.
    fn write_rumble(&mut self, now: Instant) -> io::Result<()> {
        let result = self.out.output(&steam::rumble(self.level));
        self.heard(&result);
        self.rumble_due = (self.level != Level::default())
            .then(|| now.checked_add(steam::RUMBLE_AGAIN))
            .flatten();
        result
    }
}

impl<O: SteamOut> Target for SteamNode<O> {
    fn send(&mut self, _: Option<&[u8; GAME_SIZE]>, level: Level, _: Once) -> io::Result<()> {
        self.rumble_at(level, Instant::now())
    }

    fn due(&self) -> Option<Instant> {
        if self.gone {
            return None;
        }
        match (self.rumble_due, self.lizard_due) {
            (Some(rumble), Some(lizard)) => Some(rumble.min(lizard)),
            (rumble, lizard) => rumble.or(lizard),
        }
    }

    fn keep_up(&mut self, now: Instant) {
        if self.lizard_due.is_some_and(|due| due <= now) {
            // One try, as SDL makes it, and the next an interval later. The
            // kernel's driver tries a feature report again on `EPIPE`, up to
            // 50 times 20 ms apart, because "the wireless controller fails
            // with EPIPE" (`steam_send_report_id`, `hid-steam.c:549-562`):
            // whether a pad behind a Puck or over Bluetooth needs that, so
            // that lizard mode isn't left on for an interval, only a real
            // one can show.
            let result = self.out.feature(&steam::lizard_mode_off_motion_on());
            self.heard(&result);
            // From now, not from when it was due: a write that waited on
            // the pad is followed by a whole interval, not a burst.
            self.lizard_due = now.checked_add(steam::LIZARD_MODE_AGAIN);
        }
        if self.rumble_due.is_some_and(|due| due <= now) {
            let _ = self.write_rumble(now);
        }
    }

    fn rest(mut self) {
        if !self.gone {
            let _ = self.out.output(&steam::rumble(Level::default()));
        }
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
/// at. A Steam Controller given its raw node is sent its level there, and
/// nothing the games set beyond rumble (see [`SteamNode`]). Any other pad
/// that can rumble is sent its level through its one kernel effect.
///
/// The writes are made by a thread of the pad's own, at ordinary
/// priority. Dropping this has it write what is left, then leave the pad
/// as no game had it (see [`Target::rest`]), and then end; whoever drops
/// this doesn't wait for any of it. A writer that panics leaves the pad
/// at rest at once, and everything the pad is told after fails.
pub(crate) struct Output {
    shared: Arc<Shared>,
    writer: Thread,
    /// What this writes to.
    to: To,
    wanted: Wanted,
    /// What was asked once and not yet put.
    once: Once,
    /// Whether what is wanted is kept back until it is let go.
    held: bool,
    /// Whether what is wanted changed while kept back.
    changed: bool,
}

/// What a writer writes to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum To {
    /// A pad's one kernel effect.
    Effect,
    /// The raw node of a PlayStation pad of this kind, sent whole output
    /// reports.
    Reports(Kind),
    /// A Steam Controller's raw node, sent its rumble.
    Steam,
}

impl Output {
    /// Takes the raw node of a pad of `kind`, one this knows how to write
    /// to: a DualSense's or a DualShock 4's, over USB or Bluetooth.
    pub(crate) fn raw(raw: OwnedFd, kind: Kind) -> io::Result<Self> {
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
        let (output, _) = Self::start(target, To::Reports(kind), "pad-output")?;
        Ok(output)
    }

    /// Takes a Steam Controller's raw node. Its writer sets lizard mode
    /// off as it starts, and keeps it off.
    pub(crate) fn steam(raw: OwnedFd) -> io::Result<Self> {
        let target = SteamNode::new(File::from(raw), Instant::now());
        let (output, _) = Self::start(target, To::Steam, "pad-output")?;
        Ok(output)
    }

    /// Takes a pad's one kernel effect, uploaded at rest.
    pub(crate) fn effect(effect: FFEffect) -> io::Result<Self> {
        let target = KernelEffect {
            effect,
            level: Level::default(),
        };
        let (output, _) = Self::start(target, To::Effect, "pad-rumble")?;
        Ok(output)
    }

    /// Starts a writer, named `name`, on `target`, which writes `to`.
    fn start<T: Target>(target: T, to: To, name: &str) -> io::Result<(Self, JoinHandle<()>)> {
        Self::start_with(target, to, name, || {})
    }

    /// As [`start`](Self::start), with `between` called by the writer between
    /// its look in the mailbox and its taking what was asked once: for a
    /// test to hold it there.
    fn start_with<T: Target>(
        mut target: T,
        to: To,
        name: &str,
        between: impl Fn() + Send + 'static,
    ) -> io::Result<(Self, JoinHandle<()>)> {
        let shared = Arc::new(Shared {
            mailbox: Mailbox::new(),
            stop: AtomicBool::new(false),
            refused: AtomicI32::new(0),
            broken: AtomicBool::new(false),
            once: AtomicU32::new(0),
            after: Mutex::new(None),
            rested: Arc::new(Done::default()),
        });
        let writer = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name(name.to_string())
                .spawn(move || {
                    // Said however this thread goes from here.
                    let finish = Finish(Arc::clone(&shared.rested));
                    // A writer that panics writes nothing more, and says
                    // so at everything the pad is told after.
                    let wrote = catch_unwind(AssertUnwindSafe(|| {
                        write_out(&shared, &mut target, &between);
                    }));
                    if wrote.is_err() {
                        shared.broken.store(true, Ordering::Release);
                    }
                    // What the pad is left with, after whatever was left:
                    // at once, if the writer broke off.
                    if catch_unwind(AssertUnwindSafe(move || target.rest())).is_err() {
                        shared.broken.store(true, Ordering::Release);
                    }
                    // The pad's last write is made, and its raw node, if
                    // that is what was written, closed.
                    drop(finish);
                    // What was handed over goes here, once it has been:
                    // as the pad is let go, which a writer that broke off
                    // waits for.
                    while !shared.stop.load(Ordering::Acquire) {
                        std::thread::park();
                    }
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
            to,
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

    /// Whether this writes to a raw node.
    pub(crate) fn is_raw(&self) -> bool {
        self.to != To::Effect
    }

    /// Whether this writes whole output reports, which carry what the
    /// games set on a virtual DualSense.
    pub(crate) fn takes_reports(&self) -> bool {
        matches!(self.to, To::Reports(_))
    }

    /// Says when the writer has left the pad at rest, after this is
    /// dropped.
    pub(crate) fn rested(&self) -> Rested {
        Rested {
            done: Some(Arc::clone(&self.shared.rested)),
            writes: if self.is_raw() {
                Writes::Reports
            } else {
                Writes::Effect
            },
        }
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
    /// is sent only its level: through a kernel effect, or a Steam
    /// Controller, whose lights and triggers are nothing a DualSense's
    /// report sets.
    ///
    /// Fails if the pad refused a write since a refusal was last said.
    pub(crate) fn pass(&mut self, report: &[u8]) -> io::Result<()> {
        let To::Reports(kind) = self.to else {
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

    /// Says a write the pad refused, or a writer that has broken off.
    /// Called on the input thread, so neither error allocates: an error
    /// number and a bare kind are held in place, where a message would be
    /// boxed, at every call once the writer has broken off.
    fn refusal(&self) -> io::Result<()> {
        match self.shared.refused.swap(0, Ordering::Relaxed) {
            0 if self.shared.broken.load(Ordering::Acquire) => {
                Err(io::Error::from(io::ErrorKind::Other))
            }
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

/// The writer: sends what is wanted each time it changes, and what the
/// pad's protocol asks of it when that is due, and sleeps between. Ends
/// once the input thread's side is gone and what it left is sent; what
/// would have been due after that isn't written.
///
/// What is asked once is written with its own settings or later ones,
/// possibly in the report just after its own; never after settings put
/// after it. The input thread puts the settings, then adds what they ask
/// once; this looks in the mailbox, then takes what was asked once, then
/// looks at the mailbox's sequence again, and writes only if it hasn't
/// moved. Taken by the swap, anything asked once was added after its own
/// settings were put, and the swap's acquiring it makes that put's
/// sequence seen by the look after: unmoved, it is the one read, so those
/// settings or later ones are what is written with it. Moved, what was
/// taken is kept and the mailbox looked in again. Not yet added as its
/// own settings were read, it is taken at the next look, and goes in the
/// report after theirs. And settings put after something asked once were
/// put after it was added, so a look that reads them is followed by a
/// swap that sees it, if it isn't already kept: it goes with them.
///
/// A write the pad refuses is said to the input thread, and isn't tried
/// again by itself: a pad that refuses one write may refuse the next at
/// once, for as long as it likes. What it carried goes with the next
/// write, which the next change brings: what it asked once is kept for
/// that write, and the games' settings are taken as not written, so that
/// write carries them again. So a pad that stalls for a moment, as one
/// on USB that times out, loses no setting until a game next changes it.
/// A pad that has gone, whose node answers every write so, is kept
/// nothing: nothing written there again will reach it.
fn write_out<T: Target>(shared: &Shared, target: &mut T, between: &impl Fn()) {
    // The sequence last written or refused, and the count of the game
    // reports whose settings the pad took last.
    let (mut written, mut games) = (0u64, 0u32);
    // What was asked once, taken and not yet written.
    let mut once = Once::default();
    // What a refused write asked once: carried by the next write, and no
    // reason for one by itself.
    let mut unsent = Once::default();
    let gone = rustix::io::Errno::NODEV.raw_os_error();
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
            let asked = std::mem::take(&mut unsent).then(std::mem::take(&mut once));
            match target.send(game, wanted.level, asked) {
                Ok(()) => games = wanted.games,
                Err(error) => {
                    let errno = error
                        .raw_os_error()
                        .unwrap_or(rustix::io::Errno::IO.raw_os_error());
                    shared.refused.store(errno, Ordering::Relaxed);
                    if errno == gone {
                        games = wanted.games;
                    } else {
                        unsent = asked;
                    }
                }
            }
            // What the pad's protocol has due goes before the next change:
            // changes that come faster than writes are made would put it
            // off for as long as they came.
            let now = Instant::now();
            if !stopping && target.due().is_some_and(|due| due <= now) {
                target.keep_up(now);
            }
            continue;
        }
        if stopping {
            return;
        }
        // Woken by every put; a wake with nothing new only looks again.
        // A pad whose protocol asks for writes of its own is woken for
        // the next of those too, which it then makes.
        match target.due() {
            None => std::thread::park(),
            Some(due) => {
                let now = Instant::now();
                match due.checked_duration_since(now) {
                    Some(wait) if !wait.is_zero() => std::thread::park_timeout(wait),
                    _ => target.keep_up(now),
                }
            }
        }
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
    /// doesn't finish the write until the test says so, and how: taken,
    /// as zero, or refused with that error number. Ungated, it refuses
    /// every write with `refuse`, if given.
    struct Recorder {
        written: mpsc::Sender<Vec<u8>>,
        gate: Option<mpsc::Receiver<i32>>,
        refuse: Option<i32>,
    }

    impl Write for Recorder {
        fn write(&mut self, report: &[u8]) -> io::Result<usize> {
            self.written.send(report.to_vec()).unwrap();
            let errno = match &self.gate {
                Some(gate) => gate.recv().unwrap(),
                None => self.refuse.unwrap_or(0),
            };
            match errno {
                0 => Ok(report.len()),
                errno => Err(io::Error::from_raw_os_error(errno)),
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
        mpsc::Sender<i32>,
    ) {
        let (written, reports) = mpsc::channel();
        let (open, gate) = mpsc::channel();
        let recorder = Recorder {
            written,
            gate: gated.then_some(gate),
            refuse,
        };
        let target = RawNode::new(recorder, USB, None);
        let (output, writer) = Output::start(target, To::Reports(USB), "pad-output").unwrap();
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
        open.send(0).unwrap();
        let second = reports.recv().unwrap();
        assert_eq!((second[4], second[45]), (0x20, 0xbb), "{second:?}");
        drop(output);
        open.send(0).unwrap();
        open.send(0).unwrap();
        writer.join().unwrap();
        let rest: Vec<Vec<u8>> = reports.try_iter().collect();
        assert!(
            rest.len() == 1 && is_rest(&rest[0]),
            "only the rest: {rest:?}"
        );
    }

    /// Settings that replace others not yet written lose nothing, being
    /// everything the games set; what two asked once, one each, is
    /// gathered until a write carries both; and the same settings again
    /// write nothing.
    #[test]
    fn what_is_asked_once_is_gathered_until_it_is_written() {
        let (mut output, writer, reports, open) = recorded(true, None);
        output.pass(&lights(0x11)).unwrap();
        let first = reports.recv().unwrap();
        assert_eq!(first[45], 0x11);
        // The writer is held in its first write. The LEDs are given back,
        // then the lightbar set up, and the newer settings, which ask
        // neither, come after.
        let mut release = [0u8; GAME_SIZE];
        (release[0], release[2]) = (0x02, 0x08);
        output.pass(&release).unwrap();
        let mut setup = [0u8; GAME_SIZE];
        (setup[0], setup[39], setup[42]) = (0x02, 0x02, 0x02);
        output.pass(&setup).unwrap();
        let mut trigger = release;
        (trigger[1], trigger[2], trigger[11]) = (0x04, 0, 0x01);
        output.pass(&trigger).unwrap();
        open.send(0).unwrap();
        let second = reports.recv().unwrap();
        assert_eq!((second[1], second[2], second[11]), (0x04, 0x08, 0x01));
        assert_eq!((second[39] & 0x02, second[42]), (0x02, 0x02), "both");
        open.send(0).unwrap();
        // The same settings again are nothing new.
        output.pass(&trigger).unwrap();
        output.rumble(level(0x1000, 0)).unwrap();
        let third = reports.recv().unwrap();
        assert_eq!(
            (third[2], third[4]),
            (0, 0x10),
            "the level alone: {third:?}"
        );
        open.send(0).unwrap();
        drop(output);
        open.send(0).unwrap();
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
            Output::start_with(target, To::Reports(USB), "pad-output", between).unwrap();
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

    /// Whoever lets a pad go can wait, on another thread, until its last
    /// report is written: not while that write is held, and at once
    /// after, with nothing written to the pad after it.
    #[test]
    fn a_pad_let_go_is_rested_once_its_last_report_is_written() {
        let (mut output, writer, reports, open) = recorded(true, None);
        output.rumble(level(0x1000, 0)).unwrap();
        reports.recv().unwrap();
        // The writer is held in that write.
        let rested = output.rested();
        assert_eq!(rested.writes(), Writes::Reports);
        drop(output);
        let (said, waited) = mpsc::channel();
        let waiting = {
            let rested = rested.clone();
            std::thread::spawn(move || {
                rested.wait();
                said.send(()).unwrap();
            })
        };
        assert!(!rested.is_done());
        open.send(0).unwrap();
        // The rest, held as it is written.
        let rest = reports.recv().unwrap();
        assert!(is_rest(&rest), "{rest:?}");
        assert!(!rested.is_done(), "not until the rest is written");
        assert!(waited.try_recv().is_err());
        open.send(0).unwrap();
        waited.recv().unwrap();
        assert!(rested.is_done());
        waiting.join().unwrap();
        writer.join().unwrap();
        assert!(reports.try_recv().is_err(), "nothing after the rest");
        // Dropping a handle waits for nothing and changes nothing.
        drop(rested);
        // A pad with no writer is done already.
        let nothing = Rested::already();
        assert_eq!(nothing.writes(), Writes::Nothing);
        assert!(nothing.is_done());
        nothing.wait();
        fn shared<T: Send + Sync>(_: &T) {}
        shared(&nothing);
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

    /// Waits until the writer has noted a refusal, just after the write
    /// it refused.
    fn noted(output: &Output) {
        while output.shared.refused.load(Ordering::Relaxed) == 0 {
            std::thread::yield_now();
        }
    }

    /// A write refused for a moment, as by a pad on USB that times out,
    /// is tried again by nothing but the next change, and loses nothing:
    /// that write carries what the refused one asked once, and the games'
    /// settings again. One refused by a pad that has gone is kept nothing.
    #[test]
    fn a_write_refused_for_a_moment_loses_nothing_and_one_gone_is_kept_nothing() {
        let timed_out = rustix::io::Errno::TIMEDOUT.raw_os_error();
        let enodev = rustix::io::Errno::NODEV.raw_os_error();
        let (mut output, writer, reports, open) = recorded(true, None);
        let lets_go = |report: &[u8]| report[2] & 0x08 != 0;
        // The lights set, and the LEDs given back first.
        let mut lit = lights(0x11);
        lit[2] |= 0x08;
        output.pass(&lit).unwrap();
        let first = reports.recv().unwrap();
        assert!(lets_go(&first) && first[45] == 0x11, "{first:?}");
        open.send(timed_out).unwrap();
        noted(&output);
        // Not tried again: the next write is the next change's.
        let refused = output.rumble(level(0x1000, 0)).unwrap_err();
        assert_eq!(refused.raw_os_error(), Some(timed_out));
        let second = reports.recv().unwrap();
        assert_eq!(second[4], 0x10, "the change's, not one tried again");
        assert!(lets_go(&second) && second[45] == 0x11, "{second:?}");
        open.send(0).unwrap();
        // Taken, nothing of it is carried again.
        output.rumble(level(0x2000, 0)).unwrap();
        let third = reports.recv().unwrap();
        assert!(!lets_go(&third) && third[45] == 0, "{third:?}");
        open.send(0).unwrap();

        // The pad has gone.
        let mut lit = lights(0x22);
        lit[2] |= 0x08;
        output.pass(&lit).unwrap();
        let fourth = reports.recv().unwrap();
        assert!(lets_go(&fourth) && fourth[45] == 0x22, "{fourth:?}");
        open.send(enodev).unwrap();
        noted(&output);
        let refused = output.rumble(level(0x3000, 0)).unwrap_err();
        assert_eq!(refused.raw_os_error(), Some(enodev));
        let fifth = reports.recv().unwrap();
        assert!(!lets_go(&fifth) && fifth[45] == 0, "{fifth:?}");
        open.send(enodev).unwrap();
        drop(output);
        // The rest.
        assert!(is_rest(&reports.recv().unwrap()));
        open.send(enodev).unwrap();
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

    /// Handed over with the pad, and said to be dropped only once the pad
    /// is said to be at rest.
    struct Left(mpsc::Sender<Told>, Rested);

    impl Drop for Left {
        fn drop(&mut self) {
            assert!(self.1.is_done(), "dropped after the pad is said at rest");
            self.0.send(Told::After).unwrap();
        }
    }

    /// Stands in for a pad whose writer panics as it writes a level.
    struct Panicking {
        told: mpsc::Sender<(Told, Option<String>)>,
    }

    impl Target for Panicking {
        fn send(&mut self, _: Option<&[u8; GAME_SIZE]>, _: Level, _: Once) -> io::Result<()> {
            panic!("a writer that panics");
        }

        fn rest(self) {
            let thread = std::thread::current().name().map(str::to_string);
            self.told.send((Told::Rest, thread)).unwrap();
        }
    }

    struct LeftOn(mpsc::Sender<(Told, Option<String>)>);

    impl Drop for LeftOn {
        fn drop(&mut self) {
            let thread = std::thread::current().name().map(str::to_string);
            self.0.send((Told::After, thread)).unwrap();
        }
    }

    /// A writer that panics leaves the pad at rest at once, on its own
    /// thread; everything the pad is told after fails; and what is handed
    /// over as the pad is let go is still dropped there.
    #[test]
    fn a_writer_that_panics_says_so_and_still_leaves_the_pad_at_rest() {
        let (told_by, told) = mpsc::channel();
        let target = Panicking {
            told: told_by.clone(),
        };
        let (mut output, writer) = Output::start(target, To::Effect, "pad-rumble").unwrap();
        let rested = output.rested();
        assert_eq!(rested.writes(), Writes::Effect);
        output.rumble(level(0x1000, 0)).unwrap();
        let writer_name = Some("pad-rumble".to_string());
        assert_eq!(told.recv().unwrap(), (Told::Rest, writer_name.clone()));
        // Said as soon as the rest is tried, the pad not yet let go.
        rested.wait();
        for _ in 0..2 {
            let refused = output.rumble(level(0x2000, 0)).unwrap_err();
            assert!(refused.raw_os_error().is_none(), "{refused}");
            assert_eq!(refused.kind(), io::ErrorKind::Other);
            // Made on the input thread: nothing boxed, as a message is.
            assert!(refused.get_ref().is_none(), "{refused:?}");
            output.rumble(level(0x1000, 0)).unwrap_err();
        }
        output.leave_after(Box::new(LeftOn(told_by)));
        drop(output);
        assert_eq!(told.recv().unwrap(), (Told::After, writer_name));
        writer.join().unwrap();
    }

    /// While a pad's write waits, whoever tells it a level, or drops it,
    /// goes on at once: the newest level is sent once the write is let
    /// go, then the pad is left at rest and said to be, and only then is
    /// what was left with the writer dropped.
    #[test]
    fn a_pad_whose_write_waits_holds_nobody_who_tells_it_or_drops_it() {
        let (told_by, told) = mpsc::channel();
        let (open, gate) = mpsc::channel();
        let target = Stalling {
            told: told_by.clone(),
            gate,
        };
        let (mut output, writer) = Output::start(target, To::Effect, "pad-rumble").unwrap();
        assert!(!output.is_raw());
        output.rumble(level(0x1000, 0)).unwrap();
        assert_eq!(told.recv().unwrap(), Told::Level(level(0x1000, 0)));
        // The writer is held in that write.
        output.rumble(level(0x2000, 0)).unwrap();
        output.rumble(level(0x3000, 0x3000)).unwrap();
        // Nothing for a pad that is sent only its level.
        output.pass(&lights(0x11)).unwrap();
        output.leave_after(Box::new(Left(told_by, output.rested())));
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

    /// What a Steam Controller's node was written.
    #[derive(Clone, PartialEq, Debug)]
    enum Steam {
        Output(Vec<u8>),
        Feature(Vec<u8>),
    }

    /// Stands in for a Steam Controller's raw node: says each write, and
    /// refuses each with `refuse`, if set, as a node whose pad has gone
    /// refuses them with `ENODEV`.
    struct SteamRecorder {
        written: mpsc::Sender<Steam>,
        refuse: Arc<AtomicI32>,
    }

    impl SteamRecorder {
        fn answer(&self) -> io::Result<()> {
            match self.refuse.load(Ordering::SeqCst) {
                0 => Ok(()),
                errno => Err(io::Error::from_raw_os_error(errno)),
            }
        }
    }

    impl SteamOut for SteamRecorder {
        fn output(&mut self, report: &[u8]) -> io::Result<()> {
            self.written.send(Steam::Output(report.to_vec())).unwrap();
            self.answer()
        }

        fn feature(&mut self, report: &[u8]) -> io::Result<()> {
            self.written.send(Steam::Feature(report.to_vec())).unwrap();
            self.answer()
        }
    }

    fn steam_recorder() -> (SteamRecorder, mpsc::Receiver<Steam>, Arc<AtomicI32>) {
        let (written, steam) = mpsc::channel();
        let refuse = Arc::new(AtomicI32::new(0));
        let recorder = SteamRecorder {
            written,
            refuse: Arc::clone(&refuse),
        };
        (recorder, steam, refuse)
    }

    fn lizard_off() -> Steam {
        Steam::Feature(steam::lizard_mode_off_motion_on().to_vec())
    }

    fn rumbled(level: Level) -> Steam {
        Steam::Output(steam::rumble(level).to_vec())
    }

    /// By the pad's own clock, as the writer's is read: lizard mode off at
    /// once and every interval of the pad's after, from when it was last
    /// written; a level written as it changes and again every interval of
    /// its own while it isn't at rest; at rest, written once and nothing
    /// due but lizard mode.
    #[test]
    fn a_steam_controller_s_node_is_kept_out_of_lizard_mode_and_its_rumble_going() {
        let (recorder, written, _) = steam_recorder();
        let start = Instant::now();
        let at = |ms: u64| start + std::time::Duration::from_millis(ms);
        let mut node = SteamNode::new(recorder, start);
        assert_eq!(node.due(), Some(start), "lizard mode off at once");
        node.keep_up(start);
        assert_eq!(written.try_recv().unwrap(), lizard_off());
        assert_eq!(node.due(), Some(at(3000)));
        // Nothing due is nothing written.
        node.keep_up(at(2999));
        assert!(written.try_recv().is_err());

        let on = level(0x8000, 0x4000);
        node.rumble_at(on, at(1000)).unwrap();
        assert_eq!(written.try_recv().unwrap(), rumbled(on));
        assert_eq!(node.due(), Some(at(1040)));
        node.rumble_at(on, at(1010)).unwrap();
        assert!(written.try_recv().is_err(), "the same level is nothing new");
        node.keep_up(at(1040));
        assert_eq!(written.try_recv().unwrap(), rumbled(on));
        assert_eq!(node.due(), Some(at(1080)));
        // A change is written at once, and due again from then.
        let other = level(0x1000, 0);
        node.rumble_at(other, at(1050)).unwrap();
        assert_eq!(written.try_recv().unwrap(), rumbled(other));
        assert_eq!(node.due(), Some(at(1090)));
        // At rest: once, and only lizard mode is due.
        node.rumble_at(Level::default(), at(1060)).unwrap();
        assert_eq!(written.try_recv().unwrap(), rumbled(Level::default()));
        assert_eq!(node.due(), Some(at(3000)));
        node.keep_up(at(2000));
        assert!(written.try_recv().is_err());

        // Both due at once: lizard mode first, then the rumble. A write
        // made late is followed by a whole interval.
        node.rumble_at(on, at(2990)).unwrap();
        assert_eq!(written.try_recv().unwrap(), rumbled(on));
        node.keep_up(at(3100));
        assert_eq!(written.try_recv().unwrap(), lizard_off());
        assert_eq!(written.try_recv().unwrap(), rumbled(on));
        assert_eq!(node.due(), Some(at(3140)));
        node.rumble_at(Level::default(), at(3110)).unwrap();
        assert_eq!(written.try_recv().unwrap(), rumbled(Level::default()));
        assert_eq!(node.due(), Some(at(6100)));

        // Let go, the motors are stopped, whatever they were at.
        node.rest();
        assert_eq!(written.try_recv().unwrap(), rumbled(Level::default()));
        assert!(written.try_recv().is_err());
    }

    /// A write refused for a moment is tried again when next due; a pad
    /// that has gone is written nothing more, its node refusing all.
    #[test]
    fn a_steam_controller_that_has_gone_is_written_nothing_more() {
        let (recorder, written, refuse) = steam_recorder();
        let start = Instant::now();
        let at = |ms: u64| start + std::time::Duration::from_millis(ms);
        let mut node = SteamNode::new(recorder, start);
        refuse.store(rustix::io::Errno::TIMEDOUT.raw_os_error(), Ordering::SeqCst);
        node.keep_up(start);
        assert_eq!(written.try_recv().unwrap(), lizard_off());
        let on = level(0x8000, 0);
        let refused = node.rumble_at(on, at(10)).unwrap_err();
        assert_eq!(
            refused.raw_os_error(),
            Some(rustix::io::Errno::TIMEDOUT.raw_os_error())
        );
        assert_eq!(written.try_recv().unwrap(), rumbled(on));
        assert_eq!(node.due(), Some(at(50)), "tried again when due");

        refuse.store(rustix::io::Errno::NODEV.raw_os_error(), Ordering::SeqCst);
        node.keep_up(at(50));
        assert_eq!(written.try_recv().unwrap(), rumbled(on));
        assert_eq!(node.due(), None, "nothing is due for a pad gone");
        node.rest();
        assert!(written.try_recv().is_err(), "nor is its rest written");
    }

    /// The writer itself: lizard mode off as it starts, with nothing told;
    /// a level, and the same level again, unbidden, while it runs; once at
    /// rest, nothing until lizard mode is next due; the motors stopped as
    /// it is let go; and nothing a game sets beyond rumble.
    #[test]
    fn a_steam_controller_s_writer_writes_what_its_protocol_asks_unbidden() {
        let (recorder, written, _) = steam_recorder();
        let target = SteamNode::new(recorder, Instant::now());
        let (mut output, writer) = Output::start(target, To::Steam, "pad-output").unwrap();
        assert!(output.is_raw() && !output.takes_reports());
        assert_eq!(output.rested().writes(), Writes::Reports);
        assert_eq!(written.recv().unwrap(), lizard_off());

        output.pass(&lights(0x11)).unwrap();
        let on = level(0x8000, 0x4000);
        output.rumble(on).unwrap();
        assert_eq!(written.recv().unwrap(), rumbled(on));
        // Again, and again: the writer's own, told nothing new.
        assert_eq!(written.recv().unwrap(), rumbled(on));
        assert_eq!(written.recv().unwrap(), rumbled(on));
        output.rumble(Level::default()).unwrap();
        // Whatever was due just before is the level that ran; then the
        // rest, once.
        let mut stopped = written.recv().unwrap();
        while stopped == rumbled(on) {
            stopped = written.recv().unwrap();
        }
        assert_eq!(stopped, rumbled(Level::default()));
        // At rest, nothing is due for the next three seconds; a writer
        // that went on rumbling would have by now.
        assert!(
            written
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err()
        );
        drop(output);
        writer.join().unwrap();
        let rest: Vec<Steam> = written.try_iter().collect();
        assert_eq!(rest, [rumbled(Level::default())]);
    }

    /// What a pad whose protocol asks for writes of its own was told.
    #[derive(PartialEq, Debug)]
    enum Done {
        Sent(Level),
        KeptUp,
    }

    /// Stands in for such a pad: something falls due as its first level
    /// is being written, which is held until the test lets it finish.
    struct Due {
        told: mpsc::Sender<Done>,
        gate: mpsc::Receiver<()>,
        due: Option<Instant>,
        sent: bool,
    }

    impl Target for Due {
        fn send(&mut self, _: Option<&[u8; GAME_SIZE]>, level: Level, _: Once) -> io::Result<()> {
            self.told.send(Done::Sent(level)).unwrap();
            if !std::mem::replace(&mut self.sent, true) {
                self.due = Some(Instant::now());
                self.gate.recv().unwrap();
            }
            Ok(())
        }

        fn due(&self) -> Option<Instant> {
            self.due
        }

        fn keep_up(&mut self, _: Instant) {
            self.due = None;
            self.told.send(Done::KeptUp).unwrap();
        }

        fn rest(self) {}
    }

    /// What falls due while a write is under way is written before the
    /// level told meanwhile: levels told faster than they are written
    /// can't put it off.
    #[test]
    fn what_falls_due_goes_before_the_next_change() {
        let (told_by, told) = mpsc::channel();
        let (open, gate) = mpsc::channel();
        let target = Due {
            told: told_by,
            gate,
            due: None,
            sent: false,
        };
        let (mut output, writer) = Output::start(target, To::Steam, "pad-output").unwrap();
        output.rumble(level(0x1000, 0)).unwrap();
        assert_eq!(told.recv().unwrap(), Done::Sent(level(0x1000, 0)));
        // The writer is held in that write, with something due.
        output.rumble(level(0x2000, 0)).unwrap();
        open.send(()).unwrap();
        assert_eq!(told.recv().unwrap(), Done::KeptUp);
        assert_eq!(told.recv().unwrap(), Done::Sent(level(0x2000, 0)));
        drop(output);
        writer.join().unwrap();
    }
}
