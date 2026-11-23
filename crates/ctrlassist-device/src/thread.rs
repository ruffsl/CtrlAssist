//! The input thread: the whole data path, for every setup.
//!
//! It sleeps in `epoll_wait` with no timeout over every pad, every
//! virtual pad, one wake descriptor and one timer, and makes no wakeups
//! while idle, but for a virtual pad that passes as one which reports all
//! the time, and so must itself: a virtual DualSense wakes it up to 100
//! times a second for as long as it exists. Everything it works on is
//! built elsewhere and handed over whole, and what it retires is handed
//! back, so that it neither allocates nor frees, and it never waits for
//! the side that commands it.

use std::io;
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, channel, sync_channel};
use std::thread::JoinHandle;

use ctrlassist_core::{
    Effect, EffectKey, Graph, Level, MAX_SOURCES, NodeId, Router, SinkId, SourceId, SourceSet,
};
use rustix::event::epoll::{self, EventData, EventFlags};
use rustix::event::{EventfdFlags, eventfd};
use rustix::time::{
    ClockId, Itimerspec, TimerfdClockId, TimerfdFlags, TimerfdTimerFlags, Timespec, clock_gettime,
    timerfd_create, timerfd_settime,
};

use crate::{Pad, PadError, Request, VirtualPad, realtime};

/// The most setups that run at once.
const SETUPS: usize = 8;
/// How many commands may wait unread.
const QUEUE: usize = 64;
/// The most reports that hand something back which the thread can owe
/// at once: one for each setup it runs and each pad those hold, which
/// leave it one report apiece at most, and those of the one command
/// taken next. A command is taken only when the reports it may cause
/// fit beside these, so none of them is ever dropped.
const OWED: usize = (SETUPS + 1) * (1 + MAX_SOURCES);
/// How many reports may wait unread: room for every one the thread can
/// owe, and as many again as commands for those that only tell.
const REPORTS: usize = OWED + QUEUE;
/// Room kept, beside what the thread runs, for the one report that hands
/// something back which a piece of handling has in hand when it tells
/// something too.
const IN_HAND: usize = 1;
/// The most selections reported after one piece of a setup's handling.
/// A setup with more nodes that select reports the rest after its next.
const SWITCHED: usize = 8;
/// The effect a virtual pad that is sent levels holds its level as: a
/// number no game's effect has.
const LEVEL_EFFECT: i16 = -1;
/// The real-time priority asked for: just above ordinary threads.
const PRIORITY: i32 = 10;

const WAKE: u64 = u64::MAX;
const TIMER: u64 = u64::MAX - 1;
const PAD: u64 = 0;
const SINK: u64 = 1;

/// A descriptor's place: which setup, a pad or a virtual pad, and which.
fn token(slot: usize, kind: u64, index: usize) -> u64 {
    ((slot as u64) << 32) | (kind << 24) | (index as u64 & 0xff_ffff)
}

/// Where a source of a new graph gets its pad.
#[derive(Debug)]
pub enum Place {
    /// The pad the setup has at this source of the graph it runs now,
    /// there or missing.
    Keep(SourceId),
    /// A pad the setup doesn't have yet; none is one still missing.
    Take(Option<Box<Pad>>),
}

/// A new graph for a running setup, with the pads it brings and what
/// carries over from the graph it replaces. Built off the input thread
/// and handed to it whole; the thread hands it back holding the graph it
/// replaced and the pads the new one has no place for.
#[derive(Debug)]
pub struct NewGraph {
    graph: Graph,
    /// Room for each node's selection, by node number: the thread keeps
    /// it in place of the one it hands back.
    selected: Vec<Option<usize>>,
    /// By the new graph's node number: the node of the old graph it
    /// carries on from, and where each of that node's inputs or outputs
    /// is among its own, by the old node's numbering.
    carry: Vec<Option<(NodeId, Vec<Option<usize>>)>>,
    /// By the new graph's source number: a pad brought, and room for one
    /// kept.
    pads: Vec<Option<Box<Pad>>>,
    /// By the new graph's source number: the old source whose pad it
    /// keeps.
    keep: Vec<Option<SourceId>>,
    /// Room for what the thread keeps by source number.
    wanted: Vec<Level>,
    buzz: Vec<Option<(Level, u64)>>,
    reported: Vec<Option<Level>>,
}

impl NewGraph {
    /// A graph to take the place of a running setup's.
    ///
    /// `carry` says, by the new graph's node number, which node of the
    /// old graph each carries on from, and where each of the old node's
    /// inputs or outputs is among the new one's: a node that selects
    /// then goes on selecting what it had, where that still exists.
    ///
    /// `places` says, by the new graph's source number, where each
    /// source's pad comes from. A pad the setup has that no source
    /// keeps is handed back.
    #[must_use]
    pub fn new(
        graph: Graph,
        carry: Vec<Option<(NodeId, Vec<Option<usize>>)>>,
        places: Vec<Place>,
    ) -> Self {
        let selected = vec![None; graph.nodes()];
        let sources = places.len();
        let (pads, keep) = places
            .into_iter()
            .map(|place| match place {
                Place::Keep(old) => (None, Some(old)),
                Place::Take(pad) => (pad, None),
            })
            .unzip();
        Self {
            graph,
            selected,
            carry,
            pads,
            keep,
            wanted: vec![Level::default(); sources],
            buzz: vec![None; sources],
            reported: vec![None; sources],
        }
    }
}

/// Something that passed through a setup's feedback, for whoever is
/// working out why a rumble did or didn't arrive. Reported only while
/// the input thread is [tapped](InputThread::tap).
#[derive(Clone, Copy, PartialEq, Debug)]
#[non_exhaustive]
pub enum Tap {
    /// A game asked this of a virtual pad.
    Asked {
        /// The virtual pad.
        sink: SinkId,
        /// What it asked.
        request: Request,
    },
    /// A game sent a virtual pad an output report.
    Output {
        /// The virtual pad.
        sink: SinkId,
        /// The report, as far as a DualSense's goes over USB.
        report: [u8; 48],
    },
    /// A pad was told to run its motors at this level.
    Told {
        /// The pad.
        source: SourceId,
        /// The level.
        level: Level,
    },
    /// A pad that was told a level didn't take it: the write to it
    /// failed, and its motors are as they were.
    Refused {
        /// The pad.
        source: SourceId,
        /// The level it didn't take.
        level: Level,
    },
}

/// The most taps kept from one piece of a setup's handling.
const TAPS: usize = 8;

/// Keeps a tap, if there is room for one more.
fn note(taps: &mut [Option<Tap>; TAPS], tap: Tap) {
    if let Some(place) = taps.iter_mut().find(|place| place.is_none()) {
        *place = Some(tap);
    }
}

/// Names a setup to the input thread.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct SetupId(pub u32);

/// Everything one setup runs on: its graph, its pads and its virtual
/// pads. Built off the input thread and handed to it whole.
#[derive(Debug)]
pub struct Setup {
    graph: Graph,
    router: Router,
    /// By source number. None is a pad that is missing. Each is boxed so
    /// that one can leave or return on the input thread without anything
    /// being allocated or freed there.
    pads: Vec<Option<Box<Pad>>>,
    /// By sink number.
    sinks: Vec<VirtualPad>,
    /// When a rumble level next changes by itself, or a virtual pad
    /// that reports all the time is next due.
    next_ms: Option<u64>,
    /// Each node's selection as last reported, by node number.
    selected: Vec<Option<usize>>,
    /// The rumble level the games want of each pad, by source number.
    wanted: Vec<Level>,
    /// A buzz under way on a pad, by source number: its level, and when
    /// it ends. While it lasts it is what the pad plays.
    buzz: Vec<Option<(Level, u64)>>,
    /// The pads that have each virtual pad's last output report, by sink
    /// number.
    passed: Vec<SourceSet>,
    /// Whether each virtual pad was sent an output report in the
    /// handling under way, by sink number.
    fresh: Vec<bool>,
    /// What passed through its feedback in the handling under way.
    taps: [Option<Tap>; TAPS],
    failed: bool,
}

impl Setup {
    /// A setup from a built graph and the pads and virtual pads it names,
    /// in the graph's own order.
    #[must_use]
    pub fn new(graph: Graph, pads: Vec<Option<Pad>>, sinks: Vec<VirtualPad>) -> Self {
        let router = Router::new(sinks.len(), pads.len());
        let pads: Vec<Option<Box<Pad>>> = pads.into_iter().map(|pad| pad.map(Box::new)).collect();
        // Nothing reported yet, so each node that selects is reported
        // once as the setup starts.
        let selected = vec![None; graph.nodes()];
        let wanted = vec![Level::default(); pads.len()];
        let buzz = vec![None; pads.len()];
        let passed = vec![SourceSet::EMPTY; sinks.len()];
        let fresh = vec![false; sinks.len()];
        Self {
            graph,
            router,
            pads,
            sinks,
            next_ms: None,
            selected,
            wanted,
            buzz,
            passed,
            fresh,
            taps: [None; TAPS],
            failed: false,
        }
    }

    /// The pads, by source number.
    #[must_use]
    pub fn pads(&self) -> &[Option<Box<Pad>>] {
        &self.pads
    }

    /// The virtual pads, by sink number.
    #[must_use]
    pub fn sinks(&self) -> &[VirtualPad] {
        &self.sinks
    }

    /// A node whose selection isn't what was last reported, and what it
    /// is now; that then counts as reported.
    fn selection_changed(&mut self) -> Option<(NodeId, usize)> {
        let Self {
            graph, selected, ..
        } = self;
        selected.iter_mut().enumerate().find_map(|(index, known)| {
            let now = graph.selected(NodeId(index));
            if now == *known {
                return None;
            }
            *known = now;
            now.map(|now| (NodeId(index), now))
        })
    }

    fn register(&self, epoll: &OwnedFd, slot: usize) -> io::Result<()> {
        for (index, pad) in self.pads.iter().enumerate() {
            if let Some(pad) = pad {
                epoll::add(
                    epoll,
                    pad,
                    EventData::new_u64(token(slot, PAD, index)),
                    EventFlags::IN,
                )?;
            }
        }
        for (index, sink) in self.sinks.iter().enumerate() {
            epoll::add(
                epoll,
                sink,
                EventData::new_u64(token(slot, SINK, index)),
                EventFlags::IN,
            )?;
        }
        Ok(())
    }

    fn unregister(&self, epoll: &OwnedFd) {
        for pad in self.pads.iter().flatten() {
            let _ = epoll::delete(epoll, pad);
        }
        for sink in &self.sinks {
            let _ = epoll::delete(epoll, sink);
        }
    }

    /// Brings the virtual pads to what the pads hold now.
    fn start(&mut self, now_ms: u64) {
        for (index, pad) in self.pads.iter().enumerate() {
            match pad {
                Some(pad) => self
                    .graph
                    .update(SourceId(index), *pad.state(), &mut |_| {}),
                None => self.graph.set_present(SourceId(index), false, &mut |_| {}),
            }
        }
        self.write();
        self.feedback(now_ms);
    }

    /// Makes every virtual pad hold what the graph says it should.
    fn write(&mut self) {
        for (index, sink) in self.sinks.iter_mut().enumerate() {
            if sink.write(self.graph.sink(SinkId(index))).is_err() {
                self.failed = true;
            }
        }
    }

    /// A pad has something to read. Returns the pad if it has gone.
    fn pad_ready(&mut self, index: usize, epoll: &OwnedFd) -> Option<Box<Pad>> {
        let Self {
            graph,
            pads,
            sinks,
            failed,
            ..
        } = self;
        let slot = pads.get_mut(index)?;
        let pad = slot.as_mut()?;
        let read = pad.read(|state| {
            // One report frame is one step, written at once, so a press
            // and a release in separate frames both reach the game.
            graph.update(SourceId(index), *state, &mut |_| {});
            for (index, sink) in sinks.iter_mut().enumerate() {
                if sink.write(graph.sink(SinkId(index))).is_err() {
                    *failed = true;
                }
            }
        });
        match read {
            Ok(()) => None,
            Err(PadError::Gone | PadError::Io(_)) => {
                let _ = epoll::delete(epoll, &**pad);
                let gone = slot.take();
                // A buzz goes with the pad it was on.
                if let Some(buzz) = self.buzz.get_mut(index) {
                    *buzz = None;
                }
                graph.set_present(SourceId(index), false, &mut |_| {});
                self.write();
                gone
            }
        }
    }

    /// A missing pad has returned. It takes its place again holding
    /// whatever it holds now, and is told the rumble level it missed.
    /// Returns the pad if the setup has no such place free.
    fn pad_back(
        &mut self,
        index: usize,
        pad: Box<Pad>,
        epoll: &OwnedFd,
        slot: usize,
    ) -> Option<Box<Pad>> {
        let free = self.pads.get(index).is_some_and(Option::is_none);
        if !free
            || epoll::add(
                epoll,
                &*pad,
                EventData::new_u64(token(slot, PAD, index)),
                EventFlags::IN,
            )
            .is_err()
        {
            return Some(pad);
        }
        self.graph.set_present(SourceId(index), true, &mut |_| {});
        self.graph
            .update(SourceId(index), *pad.state(), &mut |_| {});
        self.router.source_back(SourceId(index));
        // And what the games last sent beyond rumble, which it missed.
        self.passed.fill(SourceSet::EMPTY);
        if let Some(place) = self.pads.get_mut(index) {
            *place = Some(pad);
        }
        self.write();
        None
    }

    /// Takes `new`'s graph and pads in place of this setup's, which
    /// `new` then holds. The virtual pads stay as they are. A pad a
    /// source keeps moves to that source; one no source keeps stops
    /// rumbling and goes back in `new`. The new graph is brought to what
    /// the pads hold now, each node that carries on selects what its old
    /// one had, and the virtual pads change only where the two graphs
    /// disagree. Returns false, with nothing changed, for a graph of
    /// other virtual pads or one whose parts don't fit each other.
    fn take_graph(&mut self, new: &mut NewGraph, epoll: &OwnedFd, slot: usize) -> bool {
        let sources = new.graph.sources();
        let fits = [
            new.pads.len(),
            new.keep.len(),
            new.wanted.len(),
            new.buzz.len(),
            new.reported.len(),
        ]
        .iter()
        .all(|length| *length == sources)
            && new.graph.sinks() == self.sinks.len()
            && new.selected.len() == new.graph.nodes()
            && new.keep.iter().flatten().all(|old| old.0 < self.pads.len());
        if !fits {
            return false;
        }
        // What each node goes on selecting, read before the old graph
        // goes; `new.selected` is the room for it.
        for (index, place) in new.selected.iter_mut().enumerate() {
            *place = new
                .carry
                .get(index)
                .and_then(Option::as_ref)
                .and_then(|(old, places)| {
                    let was = self.graph.selected(*old)?;
                    places.get(was).copied().flatten()
                });
        }

        // The pads: each is listened to under its source number, so all
        // are let go of and then taken up again where they now are.
        for pad in self.pads.iter().flatten() {
            let _ = epoll::delete(epoll, &**pad);
        }
        let places = new.pads.iter_mut().zip(&mut new.wanted).zip(&mut new.buzz);
        for (((place, wanted), buzz), old) in places.zip(&new.keep) {
            let Some(SourceId(old)) = *old else { continue };
            if let Some(has) = self.pads.get_mut(old) {
                *place = has.take();
            }
            *wanted = self.wanted.get(old).copied().unwrap_or_default();
            *buzz = self.buzz.get(old).copied().flatten();
        }
        // What is left is given up, and plays nothing more of ours.
        for pad in self.pads.iter_mut().flatten() {
            let _ = pad.rumble(Level::default());
        }
        std::mem::swap(&mut self.pads, &mut new.pads);
        std::mem::swap(&mut self.wanted, &mut new.wanted);
        std::mem::swap(&mut self.buzz, &mut new.buzz);
        // Every pad is told its level again, whichever source it is now,
        // and sent what the games last sent beyond rumble.
        self.router.swap_sources(&mut new.reported);
        self.passed.fill(SourceSet::EMPTY);
        for (index, pad) in self.pads.iter().enumerate() {
            let Some(pad) = pad else { continue };
            let data = EventData::new_u64(token(slot, PAD, index));
            if epoll::add(epoll, &**pad, data, EventFlags::IN).is_err() {
                self.failed = true;
            }
        }

        std::mem::swap(&mut self.graph, &mut new.graph);
        for (index, pad) in self.pads.iter().enumerate() {
            match pad {
                Some(pad) => self
                    .graph
                    .update(SourceId(index), *pad.state(), &mut |_| {}),
                None => self.graph.set_present(SourceId(index), false, &mut |_| {}),
            }
        }
        // After the pads' states, so that a switch button held through
        // the change can't undo what is carried over.
        for (index, place) in new.selected.iter_mut().enumerate() {
            if let Some(choice) = place.take() {
                self.graph.select(NodeId(index), choice, &mut |_| {});
            }
            // Counted as reported, so that nothing is said of the new
            // nodes until the control side has been told of the graph.
            *place = self.graph.selected(NodeId(index));
        }
        std::mem::swap(&mut self.selected, &mut new.selected);
        self.write();
        true
    }

    /// Has every node that selects reported afresh.
    fn forget_reported(&mut self) {
        self.selected.fill(None);
    }

    /// A game has asked something of a virtual pad.
    fn sink_ready(&mut self, index: usize, now_ms: u64) {
        let Self {
            router,
            sinks,
            failed,
            taps,
            ..
        } = self;
        let Some(sink) = sinks.get_mut(index) else {
            return;
        };
        let id = SinkId(index);
        let served = sink.serve(|request| {
            note(taps, Tap::Asked { sink: id, request });
            match request {
                Request::Upload {
                    id: effect,
                    effect: data,
                } => router.upload(
                    EffectKey {
                        sink: id,
                        id: effect,
                    },
                    data,
                    now_ms,
                ),
                Request::Erase { id: effect } => router.erase(EffectKey {
                    sink: id,
                    id: effect,
                }),
                Request::Play { id: effect, count } => router.play(
                    EffectKey {
                        sink: id,
                        id: effect,
                    },
                    count,
                    now_ms,
                ),
                Request::Gain(gain) => {
                    router.set_gain(id, gain);
                    Ok(())
                }
                // Only to be seen by whoever is tapping.
                Request::Feature { .. } => Ok(()),
                // A level is one endless effect of the pad's own, replaced
                // as the level changes and stopped at none.
                Request::Level(level) => {
                    let key = EffectKey {
                        sink: id,
                        id: LEVEL_EFFECT,
                    };
                    let effect = Effect {
                        strong: level.strong,
                        weak: level.weak,
                        length_ms: 0,
                        delay_ms: 0,
                    };
                    let on = u32::from(level != Level::default());
                    router
                        .upload(key, effect, now_ms)
                        .and_then(|()| router.play(key, on, now_ms))
                }
            }
        });
        if served.is_err() {
            *failed = true;
        }
    }

    /// Sends each pad the rumble level it should now have, and notes when
    /// one next changes by itself. Run after anything that could change a
    /// level or where it goes: a request, a step, the timer.
    fn feedback(&mut self, now_ms: u64) {
        let Self {
            graph,
            router,
            pads,
            sinks,
            next_ms,
            wanted,
            buzz,
            passed,
            fresh,
            taps,
            failed,
            ..
        } = self;
        // A virtual pad that passes as one that reports all the time
        // does so too. This is the one thing that wakes the thread with
        // nothing to carry, and only while such a pad exists.
        // Each says when it is next due, which a report of real input
        // puts off, so input that keeps coming replaces these.
        let mut due = None;
        for sink in sinks.iter_mut() {
            match sink.beat(now_ms) {
                Ok(Some(at)) => due = Some(due.map_or(at, |due: u64| due.min(at))),
                Ok(None) => {}
                Err(_) => *failed = true,
            }
        }
        // What each pad's raw node is told here is written at once, so a
        // level and a report that change together are one write.
        for pad in pads.iter_mut().flatten() {
            pad.hold_output();
        }
        for (index, sink) in sinks.iter_mut().enumerate() {
            router.set_targets(SinkId(index), graph.feedback_targets(SinkId(index)));
            let new = sink.take_output().is_some();
            if let Some(place) = fresh.get_mut(index) {
                *place = new;
            }
            if let (true, Some(sent)) = (new, sink.last_output()) {
                let mut report = [0u8; 48];
                for (place, byte) in report.iter_mut().zip(sent) {
                    *place = *byte;
                }
                let sink = SinkId(index);
                note(taps, Tap::Output { sink, report });
            }
        }
        // The levels before the reports, so that a report that changes a
        // level is passed on with the level it brought.
        *next_ms = router.poll(now_ms, &mut |source, level| {
            note(taps, Tap::Told { source, level });
            if let Some(want) = wanted.get_mut(source.0) {
                *want = level;
            }
            // A buzz outranks the games until it ends.
            let buzzing = buzz.get(source.0).is_some_and(Option::is_some);
            if !buzzing
                && let Some(Some(pad)) = pads.get_mut(source.0)
                // A pad that refuses rumble still plays; the control side
                // knows from the pad itself which can't. That it refused
                // is kept for whoever is watching.
                && pad.rumble(level).is_err()
            {
                note(taps, Tap::Refused { source, level });
            }
        });
        for (index, slot) in buzz.iter_mut().enumerate() {
            let Some((_, until)) = *slot else { continue };
            if now_ms < until {
                *next_ms = Some(next_ms.map_or(until, |next| next.min(until)));
                continue;
            }
            // Over: the pad goes back to what the games want of it.
            *slot = None;
            if let (Some(Some(pad)), Some(level)) = (pads.get_mut(index), wanted.get(index)) {
                let _ = pad.rumble(*level);
            }
        }
        // What a game sent a virtual pad beyond rumble goes to the pads
        // its feedback goes to: to all of them when it is new, and to one
        // that has just become one of them.
        for (index, sink) in sinks.iter().enumerate() {
            let targets = graph.feedback_targets(SinkId(index));
            let new = fresh.get(index).copied().unwrap_or(false);
            let had = passed.get(index).copied().unwrap_or(SourceSet::EMPTY);
            if let Some(report) = sink.last_output() {
                for source in targets.iter() {
                    if (new || !had.contains(source))
                        && let Some(Some(pad)) = pads.get_mut(source.0)
                    {
                        let _ = pad.pass(report);
                    }
                }
            }
            if let Some(had) = passed.get_mut(index) {
                *had = targets;
            }
        }
        for pad in pads.iter_mut().flatten() {
            pad.release_output();
        }
        if let Some(due) = due {
            *next_ms = Some(next_ms.map_or(due, |next| next.min(due)));
        }
    }

    /// Starts a buzz on a pad: for `millis` it plays `level`, whatever
    /// the games want of it. A pad that is missing has nothing to buzz.
    fn buzz(&mut self, index: usize, level: Level, millis: u32, now_ms: u64) {
        if let (Some(Some(pad)), Some(slot)) = (self.pads.get_mut(index), self.buzz.get_mut(index))
        {
            let _ = pad.rumble(level);
            *slot = Some((level, now_ms + u64::from(millis)));
        }
    }
}

/// What the control side asks of the input thread.
#[derive(Debug)]
#[non_exhaustive]
pub enum Command {
    /// Run a setup.
    Start(SetupId, Box<Setup>),
    /// Stop a setup; it comes back in a [`Report::Stopped`].
    Stop(SetupId),
    /// Give a running setup back a pad that went missing.
    PadBack {
        /// The setup.
        setup: SetupId,
        /// Which of its pads.
        source: SourceId,
        /// The pad, opened again.
        pad: Box<Pad>,
    },
    /// Buzz one of a running setup's pads, so a person can tell which it
    /// is. For that long the buzz is what the pad plays, and then it goes
    /// back to what the games want of it.
    Buzz {
        /// The setup.
        setup: SetupId,
        /// Which of its pads.
        source: SourceId,
        /// How hard.
        level: Level,
        /// For how long, in milliseconds: the asker's own duration.
        millis: u32,
    },
    /// Move a toggle or unicast node to its next input or output.
    Switch {
        /// The setup.
        setup: SetupId,
        /// The node.
        node: NodeId,
    },
    /// Move a toggle or unicast node to one input or output by number.
    Select {
        /// The setup.
        setup: SetupId,
        /// The node.
        node: NodeId,
        /// Which input or output.
        index: usize,
    },
    /// Have a running setup take a new graph, and with it other pads if
    /// it says so, its virtual pads staying as they are. The graph it
    /// replaces comes back in a [`Report::GraphTaken`].
    NewGraph {
        /// The setup.
        setup: SetupId,
        /// The graph.
        graph: Box<NewGraph>,
    },
}

/// What the input thread tells the control side. Reports are sent without
/// waiting. One that hands something back is never dropped: the thread
/// takes a command only when every report it might cause has room to
/// wait, and holds the commands after it until then. One that only
/// tells waits for room: a selection is told once there is some, and a
/// tap that finds none is dropped.
#[derive(Debug)]
#[non_exhaustive]
pub enum Report {
    /// A setup was stopped as asked. It is handed back to be dropped.
    Stopped(SetupId, Box<Setup>),
    /// A setup couldn't be started, or failed while running: a virtual
    /// pad refused a write, or its handling panicked. It is handed back.
    Failed(SetupId, Box<Setup>),
    /// A returning pad couldn't be taken: the setup isn't running, or
    /// that place in it isn't empty. The pad is handed back.
    PadRefused {
        /// The setup.
        setup: SetupId,
        /// Which of its pads.
        source: SourceId,
        /// The pad.
        pad: Box<Pad>,
    },
    /// A node of a running setup selects another input or output than
    /// was last reported: its switch was pressed, it was asked to, or the
    /// setup has just started. Only nodes that select are reported.
    Selected {
        /// The setup.
        setup: SetupId,
        /// The node.
        node: NodeId,
        /// Which of its inputs, for a mux, or its outputs, for a demux.
        index: usize,
    },
    /// A running setup took a new graph. The one it replaced is handed
    /// back to be dropped, with the pads the new one has no place for.
    /// Its nodes and pads are numbered by the new graph from here on,
    /// and each node that selects is reported after this.
    GraphTaken {
        /// The setup.
        setup: SetupId,
        /// The graph it had.
        old: Box<NewGraph>,
    },
    /// A new graph wasn't taken: the setup isn't running, or the graph
    /// is of other virtual pads. It is handed back, with the pads it
    /// brought.
    GraphRefused {
        /// The setup.
        setup: SetupId,
        /// The graph.
        graph: Box<NewGraph>,
    },
    /// Something passed through a setup's feedback, while the thread is
    /// tapped.
    Tapped {
        /// The setup.
        setup: SetupId,
        /// What passed.
        tap: Tap,
    },
    /// A pad in a running setup has gone. The setup carries on with the
    /// pad reading as at rest. The pad is handed back to be dropped.
    PadGone {
        /// The setup.
        setup: SetupId,
        /// Which of its pads.
        source: SourceId,
        /// The pad.
        pad: Box<Pad>,
    },
}

impl Report {
    /// Whether it hands something back, and so must never be dropped.
    fn hands_back(&self) -> bool {
        !matches!(self, Self::Selected { .. } | Self::Tapped { .. })
    }
}

/// How many reports that hand something back a command may cause, at
/// most: one for itself, and one for each pad it brings, which may
/// leave again before the control side reads anything.
fn owed_by(command: &Command) -> usize {
    match command {
        // A setup with more pads than a graph has is refused, with the
        // one report.
        Command::Start(_, setup) if setup.pads.len() > MAX_SOURCES => 1,
        Command::Start(_, setup) => 1 + setup.pads.iter().flatten().count(),
        Command::PadBack { .. } => 1,
        // Pads beyond a graph's are refused with the graph, at once.
        Command::NewGraph { graph, .. } => 1 + graph.pads.iter().flatten().count().min(MAX_SOURCES),
        // What these end or fail is already counted with the setup.
        Command::Stop(_)
        | Command::Buzz { .. }
        | Command::Switch { .. }
        | Command::Select { .. } => 0,
    }
}

/// Why a command couldn't be sent.
#[derive(Debug)]
#[non_exhaustive]
pub enum SendError {
    /// The input thread has more commands waiting than it holds; the
    /// command is handed back.
    Full(Command),
    /// The input thread has stopped; the command is handed back.
    Stopped(Command),
    /// The wake descriptor refused a write.
    Wake(io::Error),
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full(_) => f.write_str("the input thread has too many commands waiting"),
            Self::Stopped(_) => f.write_str("the input thread has stopped"),
            Self::Wake(error) => write!(f, "the input thread couldn't be woken: {error}"),
        }
    }
}

impl std::error::Error for SendError {}

/// A running input thread.
#[derive(Debug)]
#[must_use = "dropping the input thread stops it, which removes every virtual pad"]
pub struct InputThread {
    commands: Option<SyncSender<Command>>,
    wake: Arc<OwnedFd>,
    stop: Arc<AtomicBool>,
    tap: Arc<AtomicBool>,
    realtime: bool,
    thread: Option<JoinHandle<()>>,
    forwarder: Option<JoinHandle<()>>,
}

/// What the input thread and the thread that passes its reports on
/// share: how many reports wait unread, and whether the input thread is
/// waiting for one to be read, to be woken when one is.
#[derive(Debug, Default)]
struct Owed {
    waiting: AtomicUsize,
    short: AtomicBool,
}

/// Passes the input thread's reports on to the control side through
/// `deliver`, in order, counting each as read once it leaves the bounded
/// queue, and wakes the input thread when it was waiting for room. The
/// control side's own queue has no bound, and whatever it no longer
/// takes is dropped in `deliver`, off the input thread. Ends when the
/// input thread does.
fn forward(queued: Receiver<Report>, owed: &Owed, wake: &OwnedFd, mut deliver: impl FnMut(Report)) {
    for report in queued {
        owed.read(wake);
        deliver(report);
    }
}

impl Owed {
    /// Counts a report as read, and wakes the input thread if it was
    /// waiting for room.
    fn read(&self, wake: &OwnedFd) {
        self.waiting.fetch_sub(1, Ordering::SeqCst);
        if self.short.swap(false, Ordering::SeqCst) {
            let _ = rustix::io::write(wake, &1u64.to_ne_bytes());
        }
    }
}

impl InputThread {
    /// Starts the input thread, and returns it with where its reports
    /// arrive, as [`spawn_with`](Self::spawn_with) does into a channel.
    ///
    /// # Errors
    ///
    /// Fails if the kernel refuses the descriptors it sleeps on, or
    /// either thread.
    pub fn spawn() -> io::Result<(Self, Receiver<Report>)> {
        let (passed, reports) = channel();
        let thread = Self::spawn_with(move |report| {
            // Not taken any more: dropped here, off the input thread.
            let _ = passed.send(report);
        })?;
        Ok((thread, reports))
    }

    /// Starts the input thread, handing each of its reports to `deliver`
    /// in the order the thread sent them: into the control side's own
    /// queue, which has no bound, so that `deliver` never waits.
    ///
    /// Its reports pass through a thread of their own at ordinary
    /// priority, which only counts them as read and calls `deliver`: the
    /// input thread neither waits for room nor allocates any, and so
    /// takes no command until the reports that command may cause fit.
    /// `deliver` runs on that thread, and goes with it once the input
    /// thread has stopped and what it handed back on its way out has
    /// been delivered.
    ///
    /// # Errors
    ///
    /// Fails if the kernel refuses the descriptors it sleeps on, or
    /// either thread.
    pub fn spawn_with(deliver: impl FnMut(Report) + Send + 'static) -> io::Result<Self> {
        let epoll = epoll::create(epoll::CreateFlags::CLOEXEC)?;
        let wake = Arc::new(eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)?);
        let timer = timerfd_create(
            TimerfdClockId::Monotonic,
            TimerfdFlags::CLOEXEC | TimerfdFlags::NONBLOCK,
        )?;
        epoll::add(&epoll, &*wake, EventData::new_u64(WAKE), EventFlags::IN)?;
        epoll::add(&epoll, &timer, EventData::new_u64(TIMER), EventFlags::IN)?;

        let (commands, inbox) = sync_channel(QUEUE);
        let (outbox, queued) = sync_channel(REPORTS);
        let owed = Arc::new(Owed::default());
        let forwarder = {
            let (owed, wake) = (Arc::clone(&owed), Arc::clone(&wake));
            std::thread::Builder::new()
                .name("input-reports".to_string())
                .spawn(move || forward(queued, &owed, &wake, deliver))?
        };
        let (granted_tx, granted) = sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let tap = Arc::new(AtomicBool::new(false));
        let mut state = State::new(
            epoll,
            Arc::clone(&wake),
            timer,
            inbox,
            outbox,
            owed,
            Arc::clone(&stop),
            Arc::clone(&tap),
        );
        let thread = std::thread::Builder::new()
            .name("input".to_string())
            .spawn(move || {
                let _ = granted_tx.send(realtime::request(PRIORITY));
                state.run();
            })?;
        let realtime = granted.recv().unwrap_or(false);
        Ok(Self {
            commands: Some(commands),
            wake,
            stop,
            tap,
            realtime,
            thread: Some(thread),
            forwarder: Some(forwarder),
        })
    }

    /// Has the thread report what passes through each setup's feedback,
    /// as [`Report::Tapped`], or stop: for finding out why a rumble did
    /// or didn't arrive. Off unless asked for.
    pub fn tap(&self, on: bool) {
        self.tap.store(on, Ordering::Relaxed);
    }

    /// Whether the kernel granted the thread real-time scheduling.
    /// Without it, latency under load is not dependable.
    #[must_use]
    pub fn is_realtime(&self) -> bool {
        self.realtime
    }

    fn wake(&self) -> io::Result<()> {
        rustix::io::write(&*self.wake, &1u64.to_ne_bytes())?;
        Ok(())
    }

    /// Sends a command, without waiting for it to be carried out.
    ///
    /// # Errors
    ///
    /// Fails, handing the command back, if the thread has stopped or has
    /// too many commands waiting.
    pub fn send(&self, command: Command) -> Result<(), SendError> {
        let Some(commands) = &self.commands else {
            return Err(SendError::Stopped(command));
        };
        match commands.try_send(command) {
            Ok(()) => self.wake().map_err(SendError::Wake),
            Err(TrySendError::Full(command)) => Err(SendError::Full(command)),
            Err(TrySendError::Disconnected(command)) => Err(SendError::Stopped(command)),
        }
    }
}

impl Drop for InputThread {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.wake();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        // What the input thread handed back on its way out has been
        // passed on, or dropped here, once this ends.
        if let Some(forwarder) = self.forwarder.take() {
            let _ = forwarder.join();
        }
    }
}

struct Running {
    id: SetupId,
    setup: Box<Setup>,
}

struct State {
    epoll: OwnedFd,
    wake: Arc<OwnedFd>,
    timer: OwnedFd,
    inbox: Receiver<Command>,
    outbox: SyncSender<Report>,
    owed: Arc<Owed>,
    stop: Arc<AtomicBool>,
    tap: Arc<AtomicBool>,
    slots: [Option<Running>; SETUPS],
    /// A command taken from the queue that waits for its reports to fit.
    held: Option<Command>,
    /// Whether a selection is waiting to be told for want of room.
    behind: bool,
}

fn now_ms() -> u64 {
    let now = clock_gettime(ClockId::Monotonic);
    (now.tv_sec as u64)
        .saturating_mul(1000)
        .saturating_add(now.tv_nsec as u64 / 1_000_000)
}

impl State {
    #[expect(
        clippy::too_many_arguments,
        reason = "each is one of the thread's own descriptors or queues, made by its caller"
    )]
    fn new(
        epoll: OwnedFd,
        wake: Arc<OwnedFd>,
        timer: OwnedFd,
        inbox: Receiver<Command>,
        outbox: SyncSender<Report>,
        owed: Arc<Owed>,
        stop: Arc<AtomicBool>,
        tap: Arc<AtomicBool>,
    ) -> Self {
        Self {
            epoll,
            wake,
            timer,
            inbox,
            outbox,
            owed,
            stop,
            tap,
            slots: std::array::from_fn(|_| None),
            held: None,
            behind: false,
        }
    }

    fn run(&mut self) {
        let mut ready = [MaybeUninit::<epoll::Event>::uninit(); 32];
        while !self.stop.load(Ordering::Acquire) {
            let Ok((events, _)) = epoll::wait(&self.epoll, &mut ready, None) else {
                // Interrupted by a signal; nothing was lost.
                continue;
            };
            let now = now_ms();
            for event in events.iter() {
                match event.data.u64() {
                    WAKE => self.commands(now),
                    TIMER => self.timer_fired(now),
                    token => self.descriptor_ready(token, now),
                }
            }
            self.arm_timer();
        }
        // Stopping: every setup goes back to be dropped off this thread,
        // or is dropped here if nobody takes it.
        for slot in 0..SETUPS {
            self.retire(slot, Report::Stopped);
        }
    }

    /// Sends a report that hands something back. Never waits, and always
    /// finds room: what may be owed is counted before each command is
    /// taken.
    fn report(&self, report: Report) {
        debug_assert!(report.hands_back());
        self.send(report);
    }

    /// Sends a report that only tells, if there is room for it beside
    /// everything that may yet be handed back, and says whether it went.
    fn tell(&self, report: Report) -> bool {
        debug_assert!(!report.hands_back());
        if !self.fits(1 + IN_HAND) {
            return false;
        }
        self.send(report);
        true
    }

    fn send(&self, report: Report) {
        // Counted before it can be read, so the count never falls below
        // what waits.
        self.owed.waiting.fetch_add(1, Ordering::SeqCst);
        if self.outbox.try_send(report).is_err() {
            // Only if the count is wrong, or the thread passing reports
            // on has gone: what the report carried is dropped here.
            self.owed.waiting.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// How many reports that hand something back the running setups may
    /// yet cause: each setup one, and each pad it has one.
    fn may_owe(&self) -> usize {
        self.slots
            .iter()
            .flatten()
            .map(|running| 1 + running.setup.pads.iter().flatten().count())
            .sum()
    }

    /// Whether `more` reports fit beside those waiting and those the
    /// running setups may yet cause.
    fn fits(&self, more: usize) -> bool {
        let waiting = self.owed.waiting.load(Ordering::SeqCst);
        waiting + self.may_owe() + more <= REPORTS
    }

    /// As [`fits`](Self::fits), and when they don't, has the thread
    /// woken as soon as a report is read.
    fn room(&self, more: usize) -> bool {
        if self.fits(more) {
            return true;
        }
        self.owed.short.store(true, Ordering::SeqCst);
        // A report read between the look above and the flag being set
        // found it unset and woke nobody: look again.
        self.fits(more)
    }

    fn slot_of(&self, id: SetupId) -> Option<usize> {
        self.slots
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|running| running.id == id))
    }

    /// Takes a setup out and hands it back.
    fn retire(&mut self, slot: usize, how: fn(SetupId, Box<Setup>) -> Report) {
        if let Some(running) = self.slots.get_mut(slot).and_then(Option::take) {
            running.setup.unregister(&self.epoll);
            self.report(how(running.id, running.setup));
        }
    }

    /// Runs one setup's handling. A panic inside it takes that setup
    /// down, not the thread and every other setup with it.
    fn guarded(
        &mut self,
        slot: usize,
        now: u64,
        work: impl FnOnce(&mut Setup, &OwnedFd) -> Option<(SourceId, Box<Pad>)>,
    ) {
        if self.handle(slot, now, work) {
            self.retire(slot, Report::Failed);
        }
    }

    /// As [`guarded`](Self::guarded), but a setup that failed or panicked
    /// is left for the caller to retire, which this says, so that what
    /// the handling did can be reported first.
    fn handle(
        &mut self,
        slot: usize,
        now: u64,
        work: impl FnOnce(&mut Setup, &OwnedFd) -> Option<(SourceId, Box<Pad>)>,
    ) -> bool {
        let Some(running) = self.slots.get_mut(slot).and_then(Option::as_mut) else {
            return false;
        };
        let id = running.id;
        let epoll = &self.epoll;
        let setup = &mut running.setup;
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let gone = work(setup, epoll);
            setup.feedback(now);
            gone
        }));
        // Said only to someone listening: a game can ask for rumble
        // many times a second.
        let taps = std::mem::take(&mut setup.taps);
        if self.tap.load(Ordering::Relaxed) {
            for tap in taps.into_iter().flatten() {
                if !self.tell(Report::Tapped { setup: id, tap }) {
                    break;
                }
            }
        }
        let Ok(gone) = outcome else {
            return true;
        };
        // Told before a pad that has gone, whose report is in hand.
        self.tell_selections(slot);
        if let Some((source, pad)) = gone {
            self.report(Report::PadGone {
                setup: id,
                source,
                pad,
            });
        }
        self.slots
            .get(slot)
            .and_then(Option::as_ref)
            .is_some_and(|running| running.setup.failed)
    }

    /// Tells what a setup's nodes now select, where it isn't what was
    /// last told: a step changes one node's at most, and a start each
    /// node's once. With no room to tell one, it is told after a report
    /// is read. A panic while looking takes the setup down.
    fn tell_selections(&mut self, slot: usize) {
        for _ in 0..SWITCHED {
            if !self.room(1 + IN_HAND) {
                self.behind = true;
                return;
            }
            let Some(running) = self.slots.get_mut(slot).and_then(Option::as_mut) else {
                return;
            };
            let id = running.id;
            let setup = &mut running.setup;
            match catch_unwind(AssertUnwindSafe(|| setup.selection_changed())) {
                Ok(Some((node, index))) => {
                    self.tell(Report::Selected {
                        setup: id,
                        node,
                        index,
                    });
                }
                Ok(None) => return,
                Err(_) => {
                    self.retire(slot, Report::Failed);
                    return;
                }
            }
        }
    }

    /// Takes the commands waiting, in order, each once every report it
    /// may cause has room; and tells what waited for room to be told.
    fn commands(&mut self, now: u64) {
        let mut count = [0u8; 8];
        let _ = rustix::io::read(&*self.wake, &mut count);
        if std::mem::take(&mut self.behind) {
            for slot in 0..SETUPS {
                self.tell_selections(slot);
            }
        }
        loop {
            let command = match self.held.take() {
                Some(command) => command,
                None => match self.inbox.try_recv() {
                    Ok(command) => command,
                    Err(_) => return,
                },
            };
            if !self.room(owed_by(&command)) {
                // Woken again as soon as a report is read.
                self.held = Some(command);
                return;
            }
            self.carry_out(command, now);
        }
    }

    fn carry_out(&mut self, command: Command, now: u64) {
        match command {
            Command::Start(id, setup) => {
                let free = self.slots.iter().position(Option::is_none);
                let duplicate = self.slot_of(id).is_some();
                // More pads than a graph has would break the count of
                // what may be owed.
                let fits = setup.pads.len() <= MAX_SOURCES;
                match free {
                    Some(slot)
                        if fits && !duplicate && setup.register(&self.epoll, slot).is_ok() =>
                    {
                        self.slots[slot] = Some(Running { id, setup });
                        self.guarded(slot, now, |setup, _| {
                            setup.start(now);
                            None
                        });
                    }
                    _ => {
                        setup.unregister(&self.epoll);
                        self.report(Report::Failed(id, setup));
                    }
                }
            }
            Command::Stop(id) => {
                if let Some(slot) = self.slot_of(id) {
                    self.retire(slot, Report::Stopped);
                }
            }
            Command::PadBack { setup, source, pad } => {
                let mut pad = Some(pad);
                if let Some(slot) = self.slot_of(setup) {
                    self.guarded(slot, now, |running, epoll| {
                        if let Some(back) = pad.take() {
                            pad = running.pad_back(source.0, back, epoll, slot);
                        }
                        None
                    });
                }
                if let Some(pad) = pad {
                    self.report(Report::PadRefused { setup, source, pad });
                }
            }
            Command::Buzz {
                setup,
                source,
                level,
                millis,
            } => {
                if let Some(slot) = self.slot_of(setup) {
                    self.guarded(slot, now, |setup, _| {
                        setup.buzz(source.0, level, millis, now);
                        None
                    });
                }
            }
            Command::Switch { setup, node } => {
                if let Some(slot) = self.slot_of(setup) {
                    self.guarded(slot, now, |setup, _| {
                        setup.graph.switch(node, &mut |_| {});
                        setup.write();
                        None
                    });
                }
            }
            Command::Select { setup, node, index } => {
                if let Some(slot) = self.slot_of(setup) {
                    self.guarded(slot, now, |setup, _| {
                        setup.graph.select(node, index, &mut |_| {});
                        setup.write();
                        None
                    });
                }
            }
            Command::NewGraph { setup, mut graph } => {
                let mut taken = false;
                let mut failed = None;
                if let Some(slot) = self.slot_of(setup) {
                    let failing = self.handle(slot, now, |running, epoll| {
                        taken = running.take_graph(&mut graph, epoll, slot);
                        None
                    });
                    failed = failing.then_some(slot);
                }
                if !taken {
                    // Its own report says first what became of a setup
                    // that failed taking it.
                    if let Some(slot) = failed {
                        self.retire(slot, Report::Failed);
                    }
                    self.report(Report::GraphRefused { setup, graph });
                    return;
                }
                // Told of the graph first, then of the setup failing on
                // it or of what its nodes select: the control side
                // numbers them by it.
                self.report(Report::GraphTaken { setup, old: graph });
                if let Some(slot) = failed {
                    self.retire(slot, Report::Failed);
                    return;
                }
                if let Some(slot) = self.slot_of(setup) {
                    if let Some(running) = self.slots.get_mut(slot).and_then(Option::as_mut) {
                        running.setup.forget_reported();
                    }
                    self.guarded(slot, now, |_, _| None);
                }
            }
        }
    }

    fn descriptor_ready(&mut self, token: u64, now: u64) {
        let slot = (token >> 32) as usize;
        let kind = (token >> 24) & 0xff;
        let index = (token & 0xff_ffff) as usize;
        self.guarded(slot, now, |setup, epoll| {
            if kind == PAD {
                setup
                    .pad_ready(index, epoll)
                    .map(|pad| (SourceId(index), pad))
            } else {
                setup.sink_ready(index, now);
                None
            }
        });
    }

    fn timer_fired(&mut self, now: u64) {
        let mut expirations = [0u8; 8];
        let _ = rustix::io::read(&self.timer, &mut expirations);
        for slot in 0..SETUPS {
            self.guarded(slot, now, |_, _| None);
        }
    }

    /// Sets the one timer to the soonest moment any setup's rumble level
    /// changes by itself, or stops it when none will. The times are the
    /// games' own; nothing here adds one.
    fn arm_timer(&self) {
        let next = self
            .slots
            .iter()
            .flatten()
            .filter_map(|running| running.setup.next_ms)
            .min();
        let at = next.map_or(
            Timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
            // A time already past must still fire: zero would disarm it.
            |ms| Timespec {
                tv_sec: (ms / 1000) as i64,
                tv_nsec: ((ms % 1000) * 1_000_000).max(1) as i64,
            },
        );
        let spec = Itimerspec {
            it_interval: Timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
            it_value: at,
        };
        let _ = timerfd_settime(&self.timer, TimerfdTimerFlags::ABSTIME, &spec);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ctrlassist_core::{GraphSpec, Input, Layout, Mode, MuxConfig, NodeSpec, SourceSpec};

    const FIRST: SetupId = SetupId(1);
    const SECOND: SetupId = SetupId(2);

    /// Two pads into a toggle and no virtual pad: a setup that needs no
    /// device to run.
    fn toggle() -> Graph {
        let source = || SourceSpec {
            deadzone: 0.1,
            layout: Layout::gamepad(),
        };
        Graph::build(GraphSpec {
            sources: vec![source(), source()],
            nodes: vec![NodeSpec::Mux {
                inputs: vec![Input::Source(SourceId(0)), Input::Source(SourceId(1))],
                config: MuxConfig::new(Mode::Toggle, &Layout::gamepad()),
            }],
            sinks: Vec::new(),
        })
        .unwrap()
    }

    fn setup() -> Box<Setup> {
        Box::new(Setup::new(toggle(), vec![None, None], Vec::new()))
    }

    /// An input thread's state, run here by the test, with what it is
    /// sent and what it reports.
    struct Rig {
        state: State,
        commands: SyncSender<Command>,
        queued: Receiver<Report>,
    }

    fn rig() -> Rig {
        let epoll = epoll::create(epoll::CreateFlags::CLOEXEC).unwrap();
        let wake = Arc::new(eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK).unwrap());
        let timer = timerfd_create(
            TimerfdClockId::Monotonic,
            TimerfdFlags::CLOEXEC | TimerfdFlags::NONBLOCK,
        )
        .unwrap();
        let (commands, inbox) = sync_channel(QUEUE);
        let (outbox, queued) = sync_channel(REPORTS);
        let state = State::new(
            epoll,
            wake,
            timer,
            inbox,
            outbox,
            Arc::new(Owed::default()),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(true)),
        );
        Rig {
            state,
            commands,
            queued,
        }
    }

    impl Rig {
        /// Reads what waits, as the thread that passes reports on does.
        fn read(&self) -> Vec<Report> {
            let mut read = Vec::new();
            while let Ok(report) = self.queued.try_recv() {
                self.state.owed.read(&self.state.wake);
                read.push(report);
            }
            read
        }

        fn send(&mut self, command: Command) {
            self.commands.try_send(command).unwrap();
            self.state.commands(0);
        }
    }

    /// A game asking for rumble many times a second, while the reports
    /// go unread, fills the queue with taps; the reports that hand
    /// something back still find room.
    #[test]
    fn a_flood_of_taps_leaves_room_for_what_is_handed_back() {
        let mut rig = rig();
        rig.send(Command::Start(FIRST, setup()));
        let told = Tap::Told {
            source: SourceId(0),
            level: Level::default(),
        };
        for _ in 0..REPORTS {
            rig.state.guarded(0, 0, |setup, _| {
                for _ in 0..TAPS {
                    note(&mut setup.taps, told);
                }
                None
            });
        }
        let graph = NewGraph::new(
            toggle(),
            vec![None],
            vec![Place::Keep(SourceId(0)), Place::Keep(SourceId(1))],
        );
        rig.send(Command::NewGraph {
            setup: FIRST,
            graph: Box::new(graph),
        });
        rig.send(Command::Stop(FIRST));
        let read = rig.read();
        let taps = read
            .iter()
            .filter(|report| matches!(report, Report::Tapped { .. }))
            .count();
        assert!(taps > QUEUE, "the taps filled what they may: {taps}");
        assert!(
            read.iter()
                .any(|report| matches!(report, Report::GraphTaken { setup: FIRST, .. }))
        );
        assert!(matches!(read.last(), Some(Report::Stopped(FIRST, _))));
    }

    /// With nothing read, commands wait for room rather than have their
    /// reports dropped, and are taken in order once there is some.
    #[test]
    fn commands_wait_for_room_and_none_of_their_reports_is_lost() {
        let mut rig = rig();
        rig.send(Command::Start(FIRST, setup()));
        let mut read = rig.read();
        // Each start under a name already running fails, and hands its
        // setup back. Sent until the thread holds one for want of room.
        let mut sent = 0;
        while rig.state.held.is_none() {
            while rig
                .commands
                .try_send(Command::Start(FIRST, setup()))
                .is_ok()
            {
                sent += 1;
            }
            rig.state.commands(0);
        }
        let waiting = rig.state.owed.waiting.load(Ordering::SeqCst);
        assert!(waiting > QUEUE && waiting <= REPORTS, "{waiting}");
        // Reading wakes the thread, which takes the rest.
        read.extend(rig.read());
        let mut count = [0u8; 8];
        assert!(rustix::io::read(&*rig.state.wake, &mut count).is_ok());
        loop {
            rig.state.commands(0);
            let more = rig.read();
            if more.is_empty() && rig.state.held.is_none() {
                break;
            }
            read.extend(more);
        }
        let failed = read
            .iter()
            .filter(|report| matches!(report, Report::Failed(FIRST, _)))
            .count();
        assert_eq!(failed, sent, "every refused setup came back");
        assert!(rig.state.slot_of(FIRST).is_some(), "the first still runs");
    }

    /// A setup that fails as it takes a new graph says it took the graph
    /// first, so that what follows is numbered by it.
    #[test]
    fn a_setup_failing_on_a_new_graph_says_it_took_it_first() {
        let mut rig = rig();
        rig.send(Command::Start(FIRST, setup()));
        rig.read();
        // As when a pad it keeps can't be listened to again.
        let slot = rig.state.slot_of(FIRST).unwrap();
        if let Some(running) = rig.state.slots[slot].as_mut() {
            running.setup.failed = true;
        }
        let graph = NewGraph::new(
            toggle(),
            vec![None],
            vec![Place::Keep(SourceId(0)), Place::Keep(SourceId(1))],
        );
        rig.send(Command::NewGraph {
            setup: FIRST,
            graph: Box::new(graph),
        });
        let read = rig.read();
        assert!(
            matches!(
                read[..],
                [
                    Report::GraphTaken { setup: FIRST, .. },
                    Report::Failed(FIRST, _)
                ]
            ),
            "{read:?}"
        );
        assert!(rig.state.slot_of(FIRST).is_none());
    }

    /// A setup whose handling panics is taken down alone: it is handed
    /// back as failed, and the setup beside it carries on, its virtual
    /// pads with it. Shipped builds unwind for this, and a build that
    /// aborts on panic can't be made: the crate's root refuses it, since
    /// the test harness unwinds whatever the profile says.
    #[test]
    fn a_setup_whose_handling_panics_is_taken_down_alone() {
        let mut rig = rig();
        rig.send(Command::Start(FIRST, setup()));
        rig.send(Command::Start(SECOND, setup()));
        let (first, second) = (
            rig.state.slot_of(FIRST).unwrap(),
            rig.state.slot_of(SECOND).unwrap(),
        );
        rig.read();
        rig.state
            .guarded(first, 0, |_, _| panic!("a step that panics"));
        let read = rig.read();
        assert!(matches!(read[..], [Report::Failed(FIRST, _)]), "{read:?}");
        assert!(rig.state.slot_of(FIRST).is_none());
        assert_eq!(rig.state.slot_of(SECOND), Some(second));
        // And the other is handled as before.
        let told = Tap::Told {
            source: SourceId(1),
            level: Level::default(),
        };
        rig.state.guarded(second, 0, |setup, _| {
            note(&mut setup.taps, told);
            None
        });
        let read = rig.read();
        assert!(
            matches!(
                read[..],
                [Report::Tapped {
                    setup: SECOND,
                    tap: Tap::Told { .. }
                }]
            ),
            "{read:?}"
        );
    }
}
