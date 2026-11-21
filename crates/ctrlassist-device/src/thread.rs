//! The input thread: the whole data path, for every setup.
//!
//! It sleeps in `epoll_wait` with no timeout over every pad, every
//! virtual pad, one wake descriptor and one timer, and makes no wakeups
//! while idle. Everything it works on is built elsewhere and handed over
//! whole, and what it retires is handed back, so that it neither
//! allocates nor frees, and it never waits for the side that commands it.

use std::io;
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread::JoinHandle;

use ctrlassist_core::{EffectKey, Graph, NodeId, Router, SinkId, SourceId};
use rustix::event::epoll::{self, EventData, EventFlags};
use rustix::event::{EventfdFlags, eventfd};
use rustix::time::{
    ClockId, Itimerspec, TimerfdClockId, TimerfdFlags, TimerfdTimerFlags, Timespec, clock_gettime,
    timerfd_create, timerfd_settime,
};

use crate::{Pad, PadError, Request, VirtualPad, realtime};

/// The most setups that run at once.
const SETUPS: usize = 8;
/// How many commands, and how many reports, may wait unread.
const QUEUE: usize = 64;
/// The most selections reported after one piece of a setup's handling.
/// A setup with more nodes that select reports the rest after its next.
const SWITCHED: usize = 8;
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
    /// When a rumble level next changes by itself.
    next_ms: Option<u64>,
    /// Each node's selection as last reported, by node number.
    selected: Vec<Option<usize>>,
    failed: bool,
}

impl Setup {
    /// A setup from a built graph and the pads and virtual pads it names,
    /// in the graph's own order.
    #[must_use]
    pub fn new(graph: Graph, pads: Vec<Option<Pad>>, sinks: Vec<VirtualPad>) -> Self {
        let router = Router::new(sinks.len(), pads.len());
        let pads = pads.into_iter().map(|pad| pad.map(Box::new)).collect();
        // Nothing reported yet, so each node that selects is reported
        // once as the setup starts.
        let selected = vec![None; graph.nodes()];
        Self {
            graph,
            router,
            pads,
            sinks,
            next_ms: None,
            selected,
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
        if let Some(place) = self.pads.get_mut(index) {
            *place = Some(pad);
        }
        self.write();
        None
    }

    /// A game has asked something of a virtual pad.
    fn sink_ready(&mut self, index: usize, now_ms: u64) {
        let Self {
            router,
            sinks,
            failed,
            ..
        } = self;
        let Some(sink) = sinks.get_mut(index) else {
            return;
        };
        let id = SinkId(index);
        let served = sink.serve(|request| match request {
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
            ..
        } = self;
        for index in 0..sinks.len() {
            router.set_targets(SinkId(index), graph.feedback_targets(SinkId(index)));
        }
        *next_ms = router.poll(now_ms, &mut |source, level| {
            if let Some(Some(pad)) = pads.get_mut(source.0) {
                // A pad that refuses rumble still plays; the control side
                // knows from the pad itself which can't.
                let _ = pad.rumble(level);
            }
        });
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
}

/// What the input thread tells the control side. Reports are sent without
/// waiting and dropped if nobody is reading them.
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
    realtime: bool,
    thread: Option<JoinHandle<()>>,
}

impl InputThread {
    /// Starts the input thread, and returns it with where its reports
    /// arrive.
    ///
    /// # Errors
    ///
    /// Fails if the kernel refuses the descriptors it sleeps on.
    pub fn spawn() -> io::Result<(Self, Receiver<Report>)> {
        let epoll = epoll::create(epoll::CreateFlags::CLOEXEC)?;
        let wake = Arc::new(eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)?);
        let timer = timerfd_create(
            TimerfdClockId::Monotonic,
            TimerfdFlags::CLOEXEC | TimerfdFlags::NONBLOCK,
        )?;
        epoll::add(&epoll, &*wake, EventData::new_u64(WAKE), EventFlags::IN)?;
        epoll::add(&epoll, &timer, EventData::new_u64(TIMER), EventFlags::IN)?;

        let (commands, inbox) = sync_channel(QUEUE);
        let (outbox, reports) = sync_channel(QUEUE);
        let (granted_tx, granted) = sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let mut state = State {
            epoll,
            wake: Arc::clone(&wake),
            timer,
            inbox,
            outbox,
            stop: Arc::clone(&stop),
            slots: std::array::from_fn(|_| None),
        };
        let thread = std::thread::Builder::new()
            .name("input".to_string())
            .spawn(move || {
                let _ = granted_tx.send(realtime::request(PRIORITY));
                state.run();
            })?;
        let realtime = granted.recv().unwrap_or(false);
        let thread = Self {
            commands: Some(commands),
            wake,
            stop,
            realtime,
            thread: Some(thread),
        };
        Ok((thread, reports))
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
    stop: Arc<AtomicBool>,
    slots: [Option<Running>; SETUPS],
}

fn now_ms() -> u64 {
    let now = clock_gettime(ClockId::Monotonic);
    (now.tv_sec as u64)
        .saturating_mul(1000)
        .saturating_add(now.tv_nsec as u64 / 1_000_000)
}

impl State {
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

    fn report(&self, report: Report) {
        // Never waits: a control side that isn't reading loses reports,
        // and whatever a lost report carried is dropped here.
        let _ = self.outbox.try_send(report);
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
        let Some(running) = self.slots.get_mut(slot).and_then(Option::as_mut) else {
            return;
        };
        let id = running.id;
        let epoll = &self.epoll;
        let setup = &mut running.setup;
        // What changed selection, kept here until it can be reported: a
        // step changes one node's at most, and a start each node's once.
        let mut switched = [None; SWITCHED];
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            let gone = work(setup, epoll);
            setup.feedback(now);
            for place in &mut switched {
                *place = setup.selection_changed();
            }
            gone
        }));
        match outcome {
            Ok(gone) => {
                let failed = setup.failed;
                for (node, index) in switched.into_iter().flatten() {
                    self.report(Report::Selected {
                        setup: id,
                        node,
                        index,
                    });
                }
                if let Some((source, pad)) = gone {
                    self.report(Report::PadGone {
                        setup: id,
                        source,
                        pad,
                    });
                }
                if failed {
                    self.retire(slot, Report::Failed);
                }
            }
            Err(_) => self.retire(slot, Report::Failed),
        }
    }

    fn commands(&mut self, now: u64) {
        let mut count = [0u8; 8];
        let _ = rustix::io::read(&*self.wake, &mut count);
        while let Ok(command) = self.inbox.try_recv() {
            match command {
                Command::Start(id, setup) => {
                    let free = self.slots.iter().position(Option::is_none);
                    let duplicate = self.slot_of(id).is_some();
                    match free {
                        Some(slot) if !duplicate && setup.register(&self.epoll, slot).is_ok() => {
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
