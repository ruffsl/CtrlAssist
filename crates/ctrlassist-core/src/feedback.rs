//! Rumble routing, as levels.
//!
//! A game uploads effects to a virtual pad and plays them by number. The
//! router keeps those effects and their timing itself and answers one
//! question: how hard should each pad's two motors be running right now?
//! The caller keeps a single effect on each pad set to that level.
//!
//! This is what the kernel does for a pad that can only rumble, moved to
//! where the routing is known. A hand-off part-way through a rumble gives
//! the new pad the current level and it ends on time; a rumble that has
//! ended contributes nothing; a pad uses one effect slot however many
//! games rumble it.
//!
//! The router holds no clock. The caller passes the time in milliseconds
//! and is told when to ask again.

/// Effects a game can hold on one virtual pad at a time, as the kernel
/// offers for a rumble-only pad.
pub const EFFECTS: usize = 16;

/// A rumble effect, as a game uploads it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Effect {
    /// Strong motor strength.
    pub strong: u16,
    /// Weak motor strength.
    pub weak: u16,
    /// How long it runs each time it is played, in milliseconds. Zero
    /// means until it is stopped.
    pub length_ms: u16,
    /// How long after being played it starts, in milliseconds.
    pub delay_ms: u16,
}

/// How hard a pad's two motors should run.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Level {
    /// Strong motor strength.
    pub strong: u16,
    /// Weak motor strength.
    pub weak: u16,
}

/// An effect's identity: a game numbers its effects per virtual pad.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EffectKey {
    /// The virtual pad the game uploaded it to.
    pub sink: usize,
    /// The game's number for it.
    pub id: i16,
}

/// Why a feedback request was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum FeedbackError {
    /// The virtual pad doesn't exist.
    NoSuchSink,
    /// The effect was never uploaded, or has been erased.
    NoSuchEffect,
    /// The virtual pad already holds as many effects as it can.
    Full,
}

impl std::fmt::Display for FeedbackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NoSuchSink => "the virtual pad doesn't exist",
            Self::NoSuchEffect => "the effect was never uploaded, or has been erased",
            Self::Full => "the virtual pad already holds as many effects as it can",
        })
    }
}

impl std::error::Error for FeedbackError {}

#[derive(Clone, Copy)]
struct Play {
    started_ms: u64,
    /// How many times to run.
    count: u32,
}

#[derive(Clone, Copy)]
struct Slot {
    id: i16,
    effect: Effect,
    play: Option<Play>,
}

/// Where a playing effect stands at one instant.
enum Phase {
    /// Waiting out its delay; it starts at this time.
    Waiting(u64),
    /// Running; `Some` is when this run ends.
    Running(Option<u64>),
    /// Every run is over.
    Finished,
}

impl Slot {
    /// The play to carry on with if the effect is replaced at `now_ms`:
    /// its current run started over, with the runs it has left; or none
    /// if it has already finished.
    fn restarted(&self, play: Play, now_ms: u64) -> Option<Play> {
        let period = u64::from(self.effect.delay_ms) + u64::from(self.effect.length_ms);
        let done = if self.effect.length_ms == 0 {
            0
        } else {
            now_ms.saturating_sub(play.started_ms) / period
        };
        let left = u64::from(play.count)
            .checked_sub(done)
            .filter(|left| *left > 0)?;
        Some(Play {
            started_ms: now_ms,
            count: u32::try_from(left).unwrap_or(u32::MAX),
        })
    }

    fn phase(&self, play: Play, now_ms: u64) -> Phase {
        let elapsed = now_ms.saturating_sub(play.started_ms);
        let delay = u64::from(self.effect.delay_ms);
        let length = u64::from(self.effect.length_ms);
        if length == 0 {
            return if elapsed < delay {
                Phase::Waiting(play.started_ms + delay)
            } else {
                Phase::Running(None)
            };
        }
        let period = delay + length;
        let run = elapsed / period;
        if run >= u64::from(play.count) {
            return Phase::Finished;
        }
        let run_start = play.started_ms + run * period;
        if elapsed % period < delay {
            Phase::Waiting(run_start + delay)
        } else {
            Phase::Running(Some(run_start + period))
        }
    }
}

struct Sink {
    /// The pads this virtual pad's feedback goes to, as a mask.
    targets: u32,
    /// The game's overall gain, where `u16::MAX` is full strength.
    gain: u16,
    slots: [Option<Slot>; EFFECTS],
}

/// Rumble state for every virtual pad of a graph.
pub struct Router {
    sinks: Vec<Sink>,
    /// What each pad was last told, or `None` if it must be told again.
    reported: Vec<Option<Level>>,
}

impl std::fmt::Debug for Router {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Router")
            .field("sinks", &self.sinks.len())
            .field("reported", &self.reported)
            .finish()
    }
}

/// The most pads a router serves: targets are a 32-bit mask.
const MAX_SOURCES: usize = 32;

fn scale(strength: u16, gain: u16) -> u32 {
    u32::from(strength) * u32::from(gain) / u32::from(u16::MAX)
}

impl Router {
    /// A router for this many virtual pads and pads, with no targets yet
    /// and every gain at full.
    #[must_use]
    pub fn new(sinks: usize, sources: usize) -> Self {
        Self {
            sinks: (0..sinks)
                .map(|_| Sink {
                    targets: 0,
                    gain: u16::MAX,
                    slots: [None; EFFECTS],
                })
                .collect(),
            reported: vec![Some(Level::default()); sources.min(MAX_SOURCES)],
        }
    }

    fn sink(&mut self, sink: usize) -> Result<&mut Sink, FeedbackError> {
        self.sinks.get_mut(sink).ok_or(FeedbackError::NoSuchSink)
    }

    fn slot(&mut self, key: EffectKey) -> Result<&mut Slot, FeedbackError> {
        self.sink(key.sink)?
            .slots
            .iter_mut()
            .flatten()
            .find(|slot| slot.id == key.id)
            .ok_or(FeedbackError::NoSuchEffect)
    }

    /// Sets which pads a virtual pad's feedback goes to. Takes effect at
    /// the next [`poll`](Self::poll); a virtual pad that doesn't exist is
    /// ignored.
    pub fn set_targets(&mut self, sink: usize, targets: u32) {
        if let Ok(sink) = self.sink(sink) {
            sink.targets = targets;
        }
    }

    /// Sets a virtual pad's overall gain, where `u16::MAX` is full.
    pub fn set_gain(&mut self, sink: usize, gain: u16) {
        if let Ok(sink) = self.sink(sink) {
            sink.gain = gain;
        }
    }

    /// A game uploads an effect, or replaces one it uploaded before.
    ///
    /// Replacing an effect that is playing restarts its current run from
    /// now with the new strengths and timing, keeping the runs it has
    /// left, as the kernel does. Replacing one that has finished does not
    /// start it again.
    ///
    /// This succeeds whether or not any pad can rumble: the game is not
    /// told its rumble goes nowhere.
    ///
    /// # Errors
    ///
    /// Returns [`FeedbackError::Full`] if the virtual pad has no free
    /// effect, or [`FeedbackError::NoSuchSink`].
    pub fn upload(
        &mut self,
        key: EffectKey,
        effect: Effect,
        now_ms: u64,
    ) -> Result<(), FeedbackError> {
        if let Ok(slot) = self.slot(key) {
            slot.play = slot.play.and_then(|play| slot.restarted(play, now_ms));
            slot.effect = effect;
            return Ok(());
        }
        let free = self
            .sink(key.sink)?
            .slots
            .iter_mut()
            .find(|slot| slot.is_none())
            .ok_or(FeedbackError::Full)?;
        *free = Some(Slot {
            id: key.id,
            effect,
            play: None,
        });
        Ok(())
    }

    /// A game removes an effect, stopping it.
    ///
    /// # Errors
    ///
    /// Returns [`FeedbackError::NoSuchEffect`] or
    /// [`FeedbackError::NoSuchSink`].
    pub fn erase(&mut self, key: EffectKey) -> Result<(), FeedbackError> {
        let slot = self
            .sink(key.sink)?
            .slots
            .iter_mut()
            .find(|slot| slot.is_some_and(|slot| slot.id == key.id))
            .ok_or(FeedbackError::NoSuchEffect)?;
        *slot = None;
        Ok(())
    }

    /// A game plays an effect `count` times from now, or stops it with a
    /// count of zero.
    ///
    /// # Errors
    ///
    /// Returns [`FeedbackError::NoSuchEffect`] or
    /// [`FeedbackError::NoSuchSink`].
    pub fn play(&mut self, key: EffectKey, count: u32, now_ms: u64) -> Result<(), FeedbackError> {
        self.slot(key)?.play = (count > 0).then_some(Play {
            started_ms: now_ms,
            count,
        });
        Ok(())
    }

    /// Stops and removes everything a virtual pad holds, as when the game
    /// closes it.
    pub fn clear(&mut self, sink: usize) {
        if let Ok(sink) = self.sink(sink) {
            sink.slots = [None; EFFECTS];
        }
    }

    /// A pad reconnected: whatever it was told before is lost, so it is
    /// told its level again at the next [`poll`](Self::poll).
    pub fn source_back(&mut self, source: usize) {
        if let Some(reported) = self.reported.get_mut(source) {
            *reported = None;
        }
    }

    /// Reports each pad whose level has changed, and returns when a level
    /// will next change by itself, if ever. Nothing needs to call this
    /// again before then unless a request arrives.
    ///
    /// Within one virtual pad, playing effects add and clamp, after gain.
    /// Across virtual pads sharing a pad, the strongest wins on each motor.
    pub fn poll(&mut self, now_ms: u64, emit: &mut impl FnMut(usize, Level)) -> Option<u64> {
        let mut levels = [Level::default(); MAX_SOURCES];
        let mut next: Option<u64> = None;
        let mut sooner = |at: u64| next = Some(next.map_or(at, |next| next.min(at)));

        for sink in &mut self.sinks {
            let (mut strong, mut weak) = (0u32, 0u32);
            for entry in &mut sink.slots {
                let Some(slot) = entry else { continue };
                let Some(play) = slot.play else { continue };
                match slot.phase(play, now_ms) {
                    Phase::Waiting(until) => sooner(until),
                    Phase::Running(until) => {
                        strong += scale(slot.effect.strong, sink.gain);
                        weak += scale(slot.effect.weak, sink.gain);
                        if let Some(until) = until {
                            sooner(until);
                        }
                    }
                    Phase::Finished => slot.play = None,
                }
            }
            let strong = strong.min(u32::from(u16::MAX)) as u16;
            let weak = weak.min(u32::from(u16::MAX)) as u16;
            for (source, level) in levels.iter_mut().enumerate() {
                if sink.targets & (1 << source) != 0 {
                    level.strong = level.strong.max(strong);
                    level.weak = level.weak.max(weak);
                }
            }
        }

        for (source, reported) in self.reported.iter_mut().enumerate() {
            let level = levels[source];
            // A pad that just returned and is at rest needs no telling.
            let stale = reported.map_or(level != Level::default(), |before| before != level);
            if stale {
                emit(source, level);
            }
            *reported = Some(level);
        }
        next
    }
}
