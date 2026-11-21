//! Feedback routing: which pads get a virtual pad's effects, and keeping
//! each pad's effects in line with that as targets change.
//!
//! The router decides and remembers; it doesn't touch a device. It turns
//! requests from a virtual pad into [`Command`]s for the pads, and the
//! caller reports back which pads accepted an upload so the game can be
//! answered.

/// A rumble effect, as a game uploads it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Effect {
    /// Strong motor magnitude.
    pub strong: u16,
    /// Weak motor magnitude.
    pub weak: u16,
}

/// An effect's identity. A game numbers its effects per virtual pad, so
/// two virtual pads' effects never collide on a pad they share.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct EffectKey {
    /// The virtual pad the game uploaded it to.
    pub sink: usize,
    /// The game's number for it.
    pub id: i16,
}

/// Something to do to a pad.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    /// Give the pad this effect, replacing any it has under the key.
    Upload { target: usize, key: EffectKey, effect: Effect },
    /// Remove the effect from the pad.
    Erase { target: usize, key: EffectKey },
    /// Start the effect.
    Play { target: usize, key: EffectKey },
    /// Stop the effect.
    Stop { target: usize, key: EffectKey },
}

/// What to tell the game about an upload.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reply {
    /// The effect is in place, or has nowhere it needs to go.
    Accepted,
    /// Every pad it was meant for refused it.
    Rejected,
}

struct Slot {
    key: EffectKey,
    effect: Effect,
    playing: bool,
    /// The pads asked to take the last upload.
    asked: u32,
    /// The pads that hold the effect.
    on: u32,
}

/// Feedback state for every virtual pad of a graph.
pub struct Router {
    /// Per virtual pad, the pads its feedback goes to.
    targets: Vec<u32>,
    /// The pads that are connected.
    present: u32,
    effects: Vec<Slot>,
}

impl std::fmt::Debug for Router {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Router")
            .field("targets", &self.targets)
            .field("effects", &self.effects.len())
            .finish_non_exhaustive()
    }
}

fn each(mask: u32, mut action: impl FnMut(usize)) {
    let mut rest = mask;
    while rest != 0 {
        action(rest.trailing_zeros() as usize);
        rest &= rest - 1;
    }
}

impl Router {
    /// A router for this many virtual pads, with no targets yet and every
    /// pad taken as connected.
    #[must_use]
    pub fn new(sinks: usize) -> Self {
        Self {
            targets: vec![0; sinks],
            present: u32::MAX,
            effects: Vec::new(),
        }
    }

    fn slot(&mut self, key: EffectKey) -> Option<&mut Slot> {
        self.effects.iter_mut().find(|slot| slot.key == key)
    }

    /// Sets where a virtual pad's feedback goes. Pads that join get its
    /// effects, started if they are playing; pads that leave have them
    /// stopped and removed.
    pub fn retarget(&mut self, sink: usize, targets: u32, out: &mut Vec<Command>) {
        let old = std::mem::replace(&mut self.targets[sink], targets);
        let present = self.present;
        for slot in self.effects.iter_mut().filter(|slot| slot.key.sink == sink) {
            let key = slot.key;
            each(targets & !old & present, |target| {
                out.push(Command::Upload {
                    target,
                    key,
                    effect: slot.effect,
                });
                if slot.playing {
                    out.push(Command::Play { target, key });
                }
            });
            each(old & !targets & slot.on, |target| {
                if slot.playing {
                    out.push(Command::Stop { target, key });
                }
                out.push(Command::Erase { target, key });
            });
            slot.on = (slot.on | (targets & !old & present)) & targets;
        }
    }

    /// A game uploads or replaces an effect. Returns the pads asked to
    /// take it; the caller applies the commands and calls
    /// [`confirm`](Self::confirm) with the pads that did.
    pub fn upload(&mut self, key: EffectKey, effect: Effect, out: &mut Vec<Command>) -> u32 {
        let asked = self.targets[key.sink] & self.present;
        each(asked, |target| out.push(Command::Upload { target, key, effect }));
        if let Some(slot) = self.slot(key) {
            slot.effect = effect;
            slot.asked = asked;
        } else {
            self.effects.push(Slot {
                key,
                effect,
                playing: false,
                asked,
                on: 0,
            });
        }
        asked
    }

    /// Records which pads accepted an upload and says what to tell the
    /// game. An effect is accepted if any pad took it or none was asked;
    /// one every pad refused is forgotten. A pad that takes an effect the
    /// game is already playing, which it didn't hold before, is started.
    pub fn confirm(&mut self, key: EffectKey, accepted: u32, out: &mut Vec<Command>) -> Reply {
        let Some(index) = self.effects.iter().position(|slot| slot.key == key) else {
            return Reply::Rejected;
        };
        let slot = &mut self.effects[index];
        let before = slot.on;
        slot.on = accepted & slot.asked;
        if slot.asked != 0 && slot.on == 0 {
            self.effects.swap_remove(index);
            return Reply::Rejected;
        }
        if slot.playing {
            each(slot.on & !before, |target| out.push(Command::Play { target, key }));
        }
        Reply::Accepted
    }

    /// A game starts or stops an effect.
    pub fn play(&mut self, key: EffectKey, playing: bool, out: &mut Vec<Command>) {
        let present = self.present;
        if let Some(slot) = self.slot(key) {
            slot.playing = playing;
            each(slot.on & present, |target| {
                out.push(if playing {
                    Command::Play { target, key }
                } else {
                    Command::Stop { target, key }
                });
            });
        }
    }

    /// A game removes an effect.
    pub fn erase(&mut self, key: EffectKey, out: &mut Vec<Command>) {
        if let Some(index) = self.effects.iter().position(|slot| slot.key == key) {
            let slot = self.effects.swap_remove(index);
            each(slot.on & self.present, |target| out.push(Command::Erase { target, key }));
        }
    }

    /// A pad disconnected. Its effects went with it, so there is nothing
    /// to send.
    pub fn source_gone(&mut self, source: usize) {
        self.present &= !(1 << source);
        for slot in &mut self.effects {
            slot.on &= !(1 << source);
        }
    }

    /// A pad reconnected. It gets back every effect meant for it, started
    /// if playing.
    pub fn source_back(&mut self, source: usize, out: &mut Vec<Command>) {
        let bit = 1 << source;
        self.present |= bit;
        for slot in &mut self.effects {
            if self.targets[slot.key.sink] & bit != 0 {
                out.push(Command::Upload {
                    target: source,
                    key: slot.key,
                    effect: slot.effect,
                });
                if slot.playing {
                    out.push(Command::Play {
                        target: source,
                        key: slot.key,
                    });
                }
                slot.on |= bit;
            }
        }
    }
}
