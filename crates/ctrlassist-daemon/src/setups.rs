//! Setups: profiles being run.
//!
//! A setup goes by the name of its profile. Starting one finds a pad
//! for each of the profile's sources, then, off the reactor, opens those
//! pads, makes the virtual pads and builds the graph, and hands the
//! whole to the input thread. From then on the reactor follows it by the
//! input thread's reports and by the pads that come and go: a setup
//! missing a pad is held, and takes the pad back when it returns.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::os::fd::{AsFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ctrlassist_core::{Graph, Input, Level, NodeId, NodeSpec, SourceId};
use ctrlassist_device::{
    Command, Database, DualSenseData, Found, NewGraph, Pad, Place, Report, SendError, Setup,
    SetupId, VirtualPad, own_address,
};
use ctrlassist_protocol::endpoint::setup;
use ctrlassist_protocol::error::{ErrorCode, WireError};
use ctrlassist_protocol::payload::{self, Phase};
use ctrlassist_protocol::profile::{Hide, Identity, PadRef, Plan, Profile};
use ctrlassist_protocol::serve::Responder;
use ctrlassist_protocol::{keys, rpc};
use tracing::{error, info, warn};
use zenoh::query::Query;

use crate::blacklist::Blacklist;
use crate::book::Book;
use crate::device_identity;
use crate::hiding::{Errand, Hiding, Token, Unhidden};
use crate::reactor::Reactor;

/// One of a setup's pads.
#[derive(Debug)]
struct Slot {
    /// Its name in the profile.
    name: String,
    /// Which pad the profile wants there.
    wanted: PadRef,
    /// The pad it is, once one that fits has been found.
    pad: Option<String>,
    /// Whether the input thread holds that pad now.
    present: bool,
    /// Whether the pad is being opened to go back.
    returning: bool,
    /// Whether it was asked for again while it was being opened: the
    /// pad may have come back meanwhile, so an opening that fails is
    /// tried once more.
    again: bool,
    /// The hiding service's name for the pad, while it has it hidden.
    token: Option<Token>,
}

impl Slot {
    fn new(name: String, wanted: PadRef, pad: Option<String>) -> Self {
        Self {
            name,
            wanted,
            pad,
            present: false,
            returning: false,
            again: false,
            token: None,
        }
    }
}

/// A setup, as the reactor follows it.
#[derive(Debug)]
struct Entry {
    /// Its name to the input thread.
    id: SetupId,
    phase: Phase,
    reason: Option<String>,
    slots: Vec<Slot>,
    sinks: Vec<payload::SetupSink>,
    /// Every node, by node number: its name, and what it would select
    /// between if it selects.
    nodes: Vec<(String, Vec<String>)>,
    /// What each node has selected, by node number, as the input thread
    /// last reported; none for a node that doesn't select.
    selected: Vec<Option<usize>>,
    /// Who is owed something once the input thread hands it back.
    stopping: Option<Waiting>,
    /// A change it is taking as it runs, until the input thread has the
    /// new graph.
    taking: Option<Taking>,
    /// The profile's text it runs.
    toml: String,
    /// Whether the profile as kept now is another text, or gone.
    stale: bool,
    /// How its pads are kept from games, as its profile says: how each
    /// was taken, and how one that returns is taken again.
    hide: Hide,
    /// By source number, when the pad it starts with last connected, as
    /// the book counts: hiding it may make it go and come back, which is
    /// then told from a pad plugged in by whether it has been seen.
    arrivals: Vec<Option<u64>>,
    /// Whether, failed, its pads are still being shown again: a stop or
    /// a start afresh waits for that.
    retiring: bool,
    /// Its requests to the hiding service, ended as it stops or fails.
    errand: Arc<Errand>,
}

/// A change a running setup is taking: what it will be once the input
/// thread has the new graph. Until then the setup is as it was, and what
/// the input thread reports is numbered by the old graph, so nothing
/// that names a pad or a node by number is sent meanwhile.
#[derive(Debug)]
struct Taking {
    responder: Updating,
    /// The profile's text it will run.
    toml: String,
    /// Its pads as they will be, by the new graph's source number.
    slots: Vec<Slot>,
    /// By the new graph's source number: which slot it has now whose pad
    /// that source keeps.
    keep: Vec<Option<usize>>,
    /// Its virtual pads' names, as the profile has them now.
    sinks: Vec<String>,
    /// Its nodes as they will be.
    nodes: Vec<(String, Vec<String>)>,
    /// Whether the input thread has been sent the graph. Before that,
    /// the pads it brings are being taken.
    sent: bool,
}

impl Taking {
    /// The pads this brings that the setup doesn't have.
    fn fresh(&self) -> impl Iterator<Item = &Slot> {
        let places = self.slots.iter().zip(&self.keep);
        places
            .filter(|(_, keep)| keep.is_none())
            .map(|(slot, _)| slot)
    }
}

/// The refusal of a profile that has one pad as two of its sources.
fn twice(source: &str, id: &str) -> WireError {
    WireError::refusal(
        ErrorCode::InvalidProfile,
        format!("source.{source}.pad: {id} is another source's pad too"),
    )
}

/// Works out a running setup's pads for `plan`: the slots as they will
/// be, and, by the plan's source number, which slot it has now whose pad
/// each source keeps. A source keeps the pad of the slot of its name
/// that wants the same pad; failing that, of a slot that wants the same
/// pad under another name; failing that, a pad the setup has that the
/// source could mean. So a pad stays open wherever the new profile has a
/// use for it. Any other source is found a pad as at a start.
fn rebind(
    book: &Book,
    name: &str,
    has: &[Slot],
    plan: &Plan,
) -> Result<(Vec<Slot>, Vec<Option<usize>>), WireError> {
    let mut keep: Vec<Option<usize>> = vec![None; plan.sources.len()];
    let mut kept = vec![false; has.len()];
    for pass in 0..3 {
        for (place, source) in keep.iter_mut().zip(&plan.sources) {
            if place.is_some() {
                continue;
            }
            let found = has.iter().zip(&kept).position(|(slot, kept)| {
                !kept
                    && match pass {
                        0 => slot.name == source.name && slot.wanted == source.pad,
                        1 => slot.wanted == source.pad,
                        _ => slot
                            .pad
                            .as_deref()
                            .is_some_and(|pad| book.fits(pad, &source.pad)),
                    }
            });
            if let Some(found) = found {
                *place = Some(found);
                kept[found] = true;
            }
        }
    }
    let mut slots: Vec<Slot> = Vec::new();
    for (source, keep) in plan.sources.iter().zip(&keep) {
        let taken = |id: &str| slots.iter().any(|slot| slot.pad.as_deref() == Some(id));
        let kept = keep.and_then(|kept| has.get(kept));
        let pad = match (kept, &source.pad) {
            (Some(kept), _) => kept.pad.clone(),
            (None, PadRef::Exact(id)) => {
                match book.user(id) {
                    // Its own pad, which another source keeps.
                    Some(user) if user == name => return Err(twice(&source.name, id)),
                    Some(user) => {
                        return Err(WireError::refusal(
                            ErrorCode::PadInUse,
                            format!("{id} is part of the setup {user}"),
                        ));
                    }
                    None if taken(id) => return Err(twice(&source.name, id)),
                    None => {}
                }
                Some(id.clone())
            }
            (None, wanted) => book
                .fitting(wanted)
                .into_iter()
                .find(|id| !taken(id))
                .map(str::to_string),
        };
        slots.push(Slot {
            present: kept.is_some_and(|kept| kept.present),
            token: kept.and_then(|kept| kept.token),
            ..Slot::new(source.name.clone(), source.pad.clone(), pad)
        });
    }
    Ok((slots, keep))
}

/// Who asked for an update, and is told how it went.
#[derive(Debug)]
pub(crate) enum Updating {
    /// A client, which is owed a reply.
    Client(Responder<setup::Update>),
    /// The daemon itself, for a profile a reload found changed.
    Reload,
}

impl Updating {
    async fn ok(self, updated: &rpc::Updated) {
        match self {
            Self::Client(responder) => responder.ok(updated).await,
            Self::Reload => {
                info!(
                    disruptive = updated.disruptive,
                    start = updated.start,
                    "a setup took up its reloaded profile"
                );
            }
        }
    }

    async fn err(self, refusal: &WireError) {
        match self {
            Self::Client(responder) => responder.err(refusal).await,
            Self::Reload => {
                warn!(code = %refusal.code, reason = refusal.message, "a setup couldn't take up its reloaded profile");
            }
        }
    }
}

/// Who waits for a setup the input thread was asked to stop.
#[derive(Debug)]
pub(crate) enum Waiting {
    /// A stop, owed its reply.
    Stop(Responder<setup::Stop>),
    /// An update, for which the setup is started again.
    Update(Updating),
}

/// What an update by stopping and starting costs, and the start it comes
/// to.
const fn restarted(start: SetupId) -> rpc::Updated {
    rpc::Updated {
        disruptive: true,
        start: start.0,
    }
}

/// What an update by a new graph costs, and the start that takes it.
const fn taken(start: SetupId) -> rpc::Updated {
    rpc::Updated {
        disruptive: false,
        start: start.0,
    }
}

/// For each node of a new graph, the node of the old one it carries on
/// from, which is the one of its name, and where each of the old node's
/// choices is among its own, by what the profile calls them. A node that
/// selects then goes on selecting what it had.
fn carry(
    old: &[(String, Vec<String>)],
    new: &[(String, Vec<String>)],
) -> Vec<Option<(NodeId, Vec<Option<usize>>)>> {
    new.iter()
        .map(|(name, choices)| {
            let (node, (_, were)) = old.iter().enumerate().find(|(_, (was, _))| was == name)?;
            let places = were
                .iter()
                .map(|was| choices.iter().position(|choice| choice == was))
                .collect();
            Some((NodeId(node), places))
        })
        .collect()
}

/// Each node of `plan`, by node number: its name, and what it selects
/// between, in the words of the profile.
fn choosers(plan: &Plan) -> Vec<(String, Vec<String>)> {
    let named = |input: &Input| match *input {
        Input::Source(SourceId(source)) => plan
            .sources
            .get(source)
            .map_or_else(String::new, |source| source.name.clone()),
        Input::Node(NodeId(node), output) => {
            let name = plan.nodes.get(node).cloned().unwrap_or_default();
            match plan.graph.nodes.get(node) {
                // A demux's outputs are told apart by number.
                Some(NodeSpec::Demux { .. }) => format!("{name}.{output}"),
                _ => name,
            }
        }
    };
    plan.graph
        .nodes
        .iter()
        .zip(&plan.nodes)
        .map(|(spec, name)| {
            let choices = match spec {
                NodeSpec::Mux { inputs, .. } => inputs.iter().map(named).collect(),
                NodeSpec::Demux { config, .. } => (0..config.outputs)
                    .map(|output| output.to_string())
                    .collect(),
            };
            (name.clone(), choices)
        })
        .collect()
}

impl Entry {
    /// Whether `plan` is of the virtual pads this setup has: as many,
    /// of the same identities in the same order. Then the setup can take
    /// it as it runs, whatever else differs, its pads included: a
    /// virtual pad is made anew only to be another thing to a game.
    fn has_the_virtual_pads_of(&self, plan: &Plan) -> bool {
        self.sinks.len() == plan.sinks.len()
            && self
                .sinks
                .iter()
                .zip(&plan.sinks)
                .all(|(has, sink)| has.identity == sink.identity.as_str())
    }

    /// Running with every pad there, held without one.
    fn settle(&mut self) {
        self.phase = if self.slots.iter().all(|slot| slot.present) {
            Phase::Running
        } else {
            Phase::Held
        };
    }
}

/// The setups that exist, by name.
#[derive(Debug, Default)]
pub(crate) struct Setups {
    /// How many have been started, for the next one's [`SetupId`].
    started: u32,
    entries: BTreeMap<String, Entry>,
}

impl Setups {
    fn name_of(&self, id: SetupId) -> Option<String> {
        self.entries
            .iter()
            .find(|(_, entry)| entry.id == id)
            .map(|(name, _)| name.clone())
    }

    pub(crate) fn setup(&self, name: &str) -> Option<payload::Setup> {
        let entry = self.entries.get(name)?;
        Some(payload::Setup {
            name: name.to_string(),
            start: entry.id.0,
            phase: entry.phase,
            reason: entry.reason.clone(),
            sources: entry
                .slots
                .iter()
                .map(|slot| payload::SetupSource {
                    name: slot.name.clone(),
                    pad: slot.pad.clone(),
                    present: slot.present,
                })
                .collect(),
            sinks: entry.sinks.clone(),
            stale: entry.stale,
            nodes: entry
                .nodes
                .iter()
                .zip(&entry.selected)
                .filter_map(|((name, choices), selected)| {
                    Some(payload::SetupNode {
                        name: name.clone(),
                        choices: choices.clone(),
                        selected: (*selected)?,
                    })
                })
                .collect(),
        })
    }
}

/// Runs `work`, or if it panics, logs that and runs `instead`: work done
/// off the reactor comes back to it whatever happens, or what waits on
/// it waits for good.
fn caught<T>(what: &str, work: impl FnOnce() -> T, instead: impl FnOnce() -> T) -> T {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or_else(|_| {
        error!(work = what, "work done off the reactor panicked");
        instead()
    })
}

/// What the blocking work of a start made.
#[derive(Debug)]
pub(crate) struct Made {
    setup: Box<Setup>,
    /// Each virtual pad's event node, by sink number.
    nodes: Vec<PathBuf>,
    /// Which sources' pads were opened, by source number, and the hiding
    /// service's name for each it hid.
    pads: Vec<(bool, Option<Token>)>,
}

/// Why a start's blocking work made nothing, and what of it is left to
/// undo: the pads it had the hiding service hide, which are shown again
/// once the reactor expects them to come back from that.
#[derive(Debug)]
pub(crate) struct Unmade {
    reason: String,
    /// The hiding service's name for each pad it hid, by source number.
    tokens: Vec<Option<Token>>,
}

impl Unmade {
    fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            tokens: Vec::new(),
        }
    }
}

/// Who asked for a setup to start.
#[derive(Debug)]
pub(crate) enum Asker {
    /// A client, which is owed a reply.
    Client(Responder<setup::Start>),
    /// The daemon itself, for a profile that starts by itself.
    Daemon,
    /// A client that asked for an update, which is owed its reply once
    /// the setup runs again.
    Update(Updating),
}

impl Asker {
    /// The setup started, as the start `start`.
    async fn ok(self, start: SetupId) {
        match self {
            Self::Client(responder) => responder.ok(&rpc::Ack {}).await,
            Self::Update(responder) => responder.ok(&restarted(start)).await,
            Self::Daemon => {}
        }
    }

    async fn err(self, refusal: &WireError) {
        match self {
            Self::Client(responder) => responder.err(refusal).await,
            Self::Update(responder) => responder.err(refusal).await,
            Self::Daemon => {
                warn!(code = %refusal.code, reason = refusal.message, "a profile that starts by itself couldn't");
            }
        }
    }
}

/// Who is owed an answer once work off the reactor comes back. Kept by
/// the reactor and not by the work, so that a daemon that stops answers
/// it that it is stopping, whatever has become of the work.
#[derive(Debug)]
pub(crate) enum Owed {
    /// Whoever asked for a start.
    Start(Asker),
    /// Whoever asked for an update the setup took as it ran.
    Update(Updating),
    /// Whoever waits for a setup to have stopped.
    Wait(Waiting),
}

impl Owed {
    /// Answers that the daemon is stopping.
    pub(crate) async fn stopping(self) {
        let refusal = WireError::daemon_shutting_down();
        match self {
            Self::Start(asker) => asker.err(&refusal).await,
            Self::Update(responder) | Self::Wait(Waiting::Update(responder)) => {
                responder.err(&refusal).await;
            }
            Self::Wait(Waiting::Stop(responder)) => responder.err(&refusal).await,
        }
    }
}

/// The reactor's note of what it [owes](Owed) while work is out.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct Owing(u64);

/// Work done off the reactor, come back to it.
#[derive(Debug)]
pub(crate) enum Done {
    /// A start's pads are open and its virtual pads made, or not.
    Started {
        name: String,
        outcome: Result<Made, Unmade>,
        asker: Owing,
    },
    /// The pads a change brings are open, each that could be, by the
    /// new graph's source number, with the hiding service's name for
    /// each it hid; `ids` says what each source's pad is known by.
    Retaken {
        name: String,
        graph: Box<Graph>,
        ids: Vec<Option<String>>,
        pads: Vec<Option<(Pad, Option<Token>)>>,
        /// When each pad last connected before it was taken, as the book
        /// counts.
        arrivals: Vec<Option<u64>>,
    },
    /// A graph the input thread replaced is gone, and the pads it had no
    /// place for are shown again: the update is answered. `unshown` are
    /// the pads the hiding service didn't show again.
    Taken {
        responder: Owing,
        /// The setup's start, which took the change.
        start: SetupId,
        unshown: Vec<String>,
    },
    /// A pad that returned is open again, or couldn't be opened.
    Returned {
        name: String,
        /// The setup that sent for it.
        setup: SetupId,
        /// The pad, by what it is known by.
        id: String,
        pad: Option<Box<Pad>>,
        /// The hiding service's name for it, if it hid it just now; not
        /// one the setup had, which it opened the pad again under.
        token: Option<Token>,
        /// When it last connected before it was sent for, as the book
        /// counts.
        arrival: Option<u64>,
    },
    /// A setup the input thread handed back is gone, virtual pads and
    /// all, and its pads are shown again, but for `unshown`; a stop
    /// that was waiting for that is seen to.
    Retired {
        name: String,
        setup: SetupId,
        waiting: Option<Owing>,
        unshown: Vec<String>,
    },
    /// Pads let go were shown again, but for these, which the hiding
    /// service didn't show again.
    Shown(Vec<String>),
    /// A connected pad was copied, or couldn't be.
    Copied(Option<DualSenseData>),
    /// The hiding service has been looked for, as the daemon starts.
    Looked,
    /// Something was dropped where dropping may block.
    Dropped,
}

/// Opens a pad for writing too: rumble is written to the node it is
/// read from.
fn open(node: &Path) -> Result<OwnedFd, String> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(node)
        .or_else(|_| File::open(node))
        .map(OwnedFd::from)
        .map_err(|error| format!("opening {}: {error}", node.display()))
}

fn read(fd: OwnedFd, node: &Path, database: &Database) -> Result<Pad, String> {
    Pad::with_database(fd, database).map_err(|error| format!("reading {}: {error}", node.display()))
}

/// A pad to take: what it is known by, and the node it was last seen on.
#[derive(Debug)]
pub(crate) struct Wanted {
    id: String,
    node: PathBuf,
}

/// The hiding service, and the errand of the setup a piece of work asks
/// it for.
type Asking<'a> = (&'a Hiding, &'a Arc<Errand>);

/// Why a setup's pads weren't taken: it was stopped first.
const STOPPED: &str = "it was stopped as it started";

/// Gives a hidden DualSense on USB, or a hidden DualShock 4, its raw
/// node, which only the hiding service can open: what a game sends a
/// virtual DualSense beyond rumble is written there, the whole of it to
/// a DualSense and the lightbar's color to a DualShock 4. Any other
/// pad, and one whose raw node isn't to be had, is left to rumble
/// through the kernel and is sent nothing else.
fn give_raw(pad: &mut Pad, token: Token, (hiding, errand): Asking<'_>) {
    if !pad.could_take_output() {
        return;
    }
    if let Some(raw) = hiding.raw_of(errand, token)
        && let Err(error) = pad.set_raw(raw)
    {
        warn!(%error, "a pad's raw node can't be written to; it gets rumble and nothing else");
    }
}

/// Takes a pad for a setup, keeping it from games as far as the setup's
/// profile asks: hidden by the hiding service, which hands back the
/// only descriptor left open on it, or else grabbed; grabbed only; or
/// left as it is. Returns the daemon's name for the pad if the service
/// hid it. A setup stopped meanwhile takes nothing more.
fn take(
    wanted: &Wanted,
    database: &Database,
    asking: Asking<'_>,
    hide: Hide,
) -> Result<(Pad, Option<Token>), String> {
    let (hiding, errand) = asking;
    let node = wanted.node.as_path();
    let fd = open(node)?;
    // The node is where the pad was last seen, and the kernel hands a
    // freed node to the next pad to connect: what was opened is looked at
    // before it is taken for the pad wanted.
    let dup = fd.try_clone().map_err(|error| error.to_string())?;
    let opened = read(dup, node, database)?;
    let found = opened.identifiers().id().to_string();
    if found != wanted.id {
        return Err(format!(
            "{} is {found} now, not {}",
            node.display(),
            wanted.id
        ));
    }
    // Holding the pad open is what shows the service it is ours to hide.
    if hide == Hide::Full {
        match hiding.hide(errand, &wanted.id, fd.as_fd()) {
            Ok(hidden) => {
                drop(opened);
                drop(fd);
                return match read(hidden.event, node, database) {
                    Ok(mut pad) => {
                        give_raw(&mut pad, hidden.token, asking);
                        Ok((pad, Some(hidden.token)))
                    }
                    Err(error) => {
                        hiding.restore_token(hidden.token);
                        Err(error)
                    }
                };
            }
            Err(Unhidden::Ended) => return Err(STOPPED.to_string()),
            // Grabbed instead.
            Err(Unhidden::Not) => {}
        }
    }
    drop(fd);
    let mut pad = opened;
    if hide != Hide::None
        && let Err(error) = pad.grab()
    {
        warn!(node = %node.display(), %error, "a pad couldn't be grabbed; games will see it beside the virtual pad");
    }
    Ok((pad, None))
}

/// Takes back a pad that returned. One the service still has hidden is
/// opened through it: the pad was never visible in between. Returns the
/// daemon's name for the pad only if the service hid it afresh.
fn retake(
    wanted: &Wanted,
    token: Option<Token>,
    database: &Database,
    asking: Asking<'_>,
    hide: Hide,
) -> Result<(Pad, Option<Token>), String> {
    let (hiding, errand) = asking;
    if let Some(token) = token
        && let Some(event) = hiding.reopen(errand, token)
    {
        // The service knows the pad by what it is, not by its node.
        return read(event, &wanted.node, database).map(|mut pad| {
            give_raw(&mut pad, token, asking);
            (pad, None)
        });
    }
    let taken = take(wanted, database, asking, hide)?;
    // Hidden afresh, under a new name: the old one is given up, so that
    // what it hid isn't left for the connection's end to restore.
    if let (Some(old), Some(new)) = (token, taken.1)
        && old != new
    {
        hiding.restore_token(old);
    }
    Ok(taken)
}

/// The address of the virtual pad `sink` of the setup `setup`: the same
/// each time that setup runs, since Steam keeps what it knows of a pad
/// by its address, and another for every other sink. `home` is where
/// this daemon keeps its state, so that two daemons on one machine, as
/// two users', don't give a pad the same address: the kernel's driver
/// refuses the second.
fn address_of(home: &Path, setup: &str, sink: &str) -> [u8; 6] {
    let hash = keys::pad_id(&format!("{}/{setup}/{sink}", home.display()));
    let number = u64::from_str_radix(&hash, 16).unwrap_or(0);
    own_address((number & 0x00ff_ffff) as u32)
}

/// Makes one virtual pad of a setup.
fn make_sink(
    setup: &str,
    name: &str,
    identity: Identity,
    dualsense: &(PathBuf, DualSenseData),
) -> std::io::Result<VirtualPad> {
    let (home, data) = dualsense;
    match identity {
        Identity::DualSense => VirtualPad::dualsense(data, address_of(home, setup, name)),
        identity => VirtualPad::create(device_identity(identity)),
    }
}

/// The blocking part of a start.
fn make(
    plan: Plan,
    setup: &str,
    nodes: &[Option<Wanted>],
    database: &Database,
    asking: Asking<'_>,
    dualsense: &(PathBuf, DualSenseData),
    blacklist: &Mutex<Blacklist>,
) -> Result<Made, Unmade> {
    let hide = plan.hide;
    let graph = Graph::build(plan.graph).map_err(|error| Unmade::new(error.to_string()))?;
    let mut pads = Vec::new();
    let mut taken = Vec::new();
    let stopped = |taken: &[(bool, Option<Token>)]| Unmade {
        reason: STOPPED.to_string(),
        tokens: taken.iter().map(|(_, token)| *token).collect(),
    };
    for node in nodes {
        // A setup stopped as it starts goes no further: what it has
        // taken is let go here, and shown again once the reactor expects
        // it to come back.
        if asking.1.has_ended() {
            return Err(stopped(&taken));
        }
        // A pad that went between being chosen and being taken is one
        // the setup starts without.
        let pad = node.as_ref().and_then(|node| {
            caught(
                "taking a pad",
                || take(node, database, asking, hide),
                || Err("taking it panicked".to_string()),
            )
            .inspect_err(|error| warn!(error, "a setup starts without a pad"))
            .ok()
        });
        let (pad, token) = pad.unzip();
        taken.push((pad.is_some(), token.flatten()));
        pads.push(pad);
    }
    if asking.1.has_ended() {
        return Err(stopped(&taken));
    }
    let mut sinks = Vec::new();
    for sink in &plan.sinks {
        let made = caught(
            "making a virtual pad",
            || make_sink(setup, &sink.name, sink.identity, dualsense),
            || Err(std::io::Error::other("making it panicked")),
        );
        match made {
            Ok(made) => sinks.push(made),
            Err(error) => {
                // Nothing of a start that fails is left: its pads are
                // let go here, and shown again once the reactor expects
                // them to come back.
                drop(pads);
                return Err(Unmade {
                    reason: format!(
                        "making the virtual pad `{}` ({}): {error}",
                        sink.name,
                        sink.identity.as_str()
                    ),
                    tokens: taken.iter().map(|(_, token)| *token).collect(),
                });
            }
        }
    }
    // A pad that is only grabbed is still Steam's to read some other
    // way, so Steam is told to leave its model alone, unless a virtual
    // pad of the setup is of that model too: Steam goes by the numbers.
    // That is for a pad that was to be hidden and couldn't be: a
    // profile that asks for less than hiding is left with what it asks.
    let made: Vec<(u16, u16)> = sinks.iter().map(|sink| sink.identity().model()).collect();
    let grabbed = pads.iter().zip(&taken).filter_map(|(pad, (_, token))| {
        let ids = pad
            .as_ref()
            .filter(|_| hide == Hide::Full && token.is_none())?
            .identifiers();
        let model = (ids.vendor, ids.product);
        (!made.contains(&model)).then(|| crate::blacklist::entry(model.0, model.1))
    });
    if let Ok(mut blacklist) = blacklist.lock() {
        blacklist.set(setup, grabbed.collect());
    }
    let nodes = sinks
        .iter()
        .map(|sink| sink.event_node().to_path_buf())
        .collect();
    Ok(Made {
        setup: Box::new(Setup::new(graph, pads, sinks)),
        nodes,
        pads: taken,
    })
}

/// The pad a slot holds hidden, and the hiding service's name for it.
fn hidden_by(slot: &Slot) -> Option<(String, Token)> {
    Some((slot.pad.clone()?, slot.token?))
}

/// Drops `dropped`, then has `hiding` show `pads` again, off the reactor.
/// Returns those it didn't show again; all of them if that panicked.
fn shown_again<T>(hiding: &Hiding, dropped: T, pads: &[(String, Token)]) -> Vec<String> {
    caught("dropping", || drop(dropped), || ());
    caught(
        "showing pads again",
        || hiding.restore(pads),
        || pads.iter().map(|(pad, _)| pad.clone()).collect(),
    )
}

fn unknown(name: &str) -> WireError {
    WireError::refusal(
        ErrorCode::UnknownSetup,
        format!("no setup is called {}", name.escape_debug()),
    )
}

impl Reactor {
    /// Notes that `owed` is owed an answer once work comes back.
    pub(crate) fn owe(&mut self, owed: Owed) -> Owing {
        self.owings += 1;
        let owing = Owing(self.owings);
        self.owed.insert(owing, owed);
        owing
    }

    /// Who was owed the answer to `owing`, now that its work is back.
    fn paid(&mut self, owing: Owing) -> Option<Owed> {
        self.owed.remove(&owing)
    }

    /// Answers each caller the reactor holds that the daemon is stopping,
    /// whatever has become of the work it waits on, which is given up.
    pub(crate) async fn answer_as_stopping(&mut self) {
        let mut owed: Vec<Owed> = std::mem::take(&mut self.owed).into_values().collect();
        for entry in self.setups.entries.values_mut() {
            owed.extend(entry.stopping.take().map(Owed::Wait));
            owed.extend(
                entry
                    .taking
                    .take()
                    .map(|taking| Owed::Update(taking.responder)),
            );
        }
        for owed in owed {
            owed.stopping().await;
        }
    }

    /// Drops `thing` where dropping may block: a pad's close can wait on
    /// its driver, and a virtual pad's on the kernel.
    fn drop_off<T: Send + 'static>(&mut self, thing: T) {
        self.work.spawn_blocking(move || {
            caught("dropping", || drop(thing), || ());
            Done::Dropped
        });
    }

    /// Drops `dropped`, then has the hiding service show `pads` again:
    /// in that order, so that nothing of ours still reads a pad as it
    /// comes back. Each pad the book expects to go and come back as it
    /// is shown again is expected to by now.
    fn show_again<T: Send + 'static>(&mut self, dropped: T, pads: Vec<(String, Token)>) {
        let hiding = Arc::clone(&self.hiding);
        self.work
            .spawn_blocking(move || Done::Shown(shown_again(&hiding, dropped, &pads)));
    }

    /// Drops a setup the input thread handed back, then has the hiding
    /// service show the pads it hid: in that order, so that nothing of
    /// ours still reads a pad as it comes back.
    fn retire(
        &mut self,
        (name, id): (String, SetupId),
        setup: Box<Setup>,
        pads: Vec<(String, Token)>,
        waiting: Option<Waiting>,
    ) {
        let hiding = Arc::clone(&self.hiding);
        let blacklist = Arc::clone(&self.blacklist);
        let waiting = waiting.map(|waiting| self.owe(Owed::Wait(waiting)));
        self.work.spawn_blocking(move || {
            let unshown = shown_again(&hiding, setup, &pads);
            if let Ok(mut blacklist) = blacklist.lock() {
                blacklist.set(&name, std::collections::BTreeSet::new());
            }
            Done::Retired {
                name,
                setup: id,
                waiting,
                unshown,
            }
        });
    }

    /// Notes that the hiding service has just hidden `pad`, which last
    /// connected at `arrival`, as the book counted before the pad was
    /// taken. Hidden by re-binding its driver, the pad went and came
    /// back, or is about to: nobody touched it, so that isn't published
    /// as a pad leaving and one connecting, nor taken for a pad plugged
    /// in, and it isn't left for a later showing again to be taken for.
    /// Returns whether the pad is now published otherwise, as when its
    /// going has been seen already.
    fn hid(&mut self, pad: &str, arrival: Option<u64>) -> bool {
        self.hiding.rebinds(pad)
            && arrival.is_some_and(|arrival| self.book.expect_back_since(pad, arrival))
    }

    /// The pads the hiding service didn't show again, as it said: none
    /// of them goes and comes back now.
    async fn unshown(&mut self, pads: Vec<String>) {
        for pad in pads {
            if self.book.not_shown(&pad) {
                self.publish_pad_logged(&pad).await;
            }
        }
    }

    /// Copies a DualSense that has connected over USB, if none has been
    /// copied yet: a virtual one is then made from the user's own pad's
    /// data and not the shipped. Its raw node is the hiding service's to
    /// open, so without the service nothing is copied.
    pub(crate) fn copy_from(&mut self, found: &Found) {
        let ids = &found.identifiers;
        let usb = 3;
        let is_one = (ids.vendor, ids.product) == ctrlassist_device::Identity::DualSense.model()
            && ids.bus == usb
            && !found.is_own_output();
        let wanted = is_one && self.copied.dualsense().is_none();
        if !wanted {
            return;
        }
        if self.copying {
            // One at a time; this one is tried if that one can't be.
            return self.candidates.push(found.clone());
        }
        self.copying = true;
        let node = found.node.clone();
        let hiding = Arc::clone(&self.hiding);
        self.work.spawn_blocking(move || {
            let copy = || {
                let raw = open(&node)
                    .ok()
                    .and_then(|pad| hiding.raw(pad.as_fd()));
                raw.and_then(|raw| {
                    DualSenseData::read(raw.as_fd())
                        .inspect_err(|error| warn!(node = %node.display(), %error, "a connected DualSense couldn't be copied"))
                        .ok()
                })
            };
            Done::Copied(caught("copying a DualSense", copy, || None))
        });
    }

    /// Works out again whether a setup's profile, as kept now, is the
    /// one it runs, and says so if that changed: for when a profile is
    /// put, deleted or read again.
    pub(crate) async fn profile_changed(&mut self, name: &str) {
        let kept = self.profiles.profile(name).map(|profile| profile.toml);
        let Some(entry) = self.setups.entries.get_mut(name) else {
            return;
        };
        let stale = kept.as_deref() != Some(entry.toml.as_str());
        if entry.stale != stale {
            entry.stale = stale;
            self.publish_setup(name).await;
        }
    }

    async fn publish_setup(&mut self, name: &str) {
        let outcome = match self.setups.setup(name) {
            Some(setup) => self.publish(keys::setup(name), &setup).await,
            None => {
                self.unpublish(&keys::setup(name)).await;
                Ok(())
            }
        };
        if let Err(error) = outcome {
            warn!(setup = name, %error, "publishing a setup's state failed");
        }
    }

    /// Gives up a setup's pads, and says so. Those it hid by re-binding
    /// their drivers are expected to go and come back as they are shown
    /// again, which is done next.
    async fn release(&mut self, name: &str) {
        let hiding = Arc::clone(&self.hiding);
        for pad in self.book.release(name, |pad| hiding.rebinds(pad)) {
            self.publish_pad_logged(&pad).await;
        }
    }

    /// Gives up one of a setup's pads, as [`release`](Self::release)
    /// does.
    async fn let_go(&mut self, pad: &str) {
        self.book.let_go(pad, self.hiding.rebinds(pad));
        self.publish_pad_logged(pad).await;
    }

    /// Finds a pad for each source of `plan`, and notes them as `name`'s.
    fn bind(&mut self, name: &str, plan: &Plan) -> Result<Vec<Slot>, WireError> {
        let mut slots: Vec<Slot> = Vec::new();
        for source in &plan.sources {
            let taken = |id: &str| slots.iter().any(|slot| slot.pad.as_deref() == Some(id));
            let pad = match &source.pad {
                // That pad and no other, there or not.
                PadRef::Exact(id) => {
                    if let Some(user) = self.book.user(id) {
                        return Err(WireError::refusal(
                            ErrorCode::PadInUse,
                            format!("{id} is part of the setup {user}"),
                        ));
                    }
                    if taken(id) {
                        return Err(twice(&source.name, id));
                    }
                    Some(id.clone())
                }
                // The first of the model to have connected that nothing
                // else has taken; none yet is a pad to wait for.
                wanted => self
                    .book
                    .fitting(wanted)
                    .into_iter()
                    .find(|id| !taken(id))
                    .map(str::to_string),
            };
            slots.push(Slot::new(source.name.clone(), source.pad.clone(), pad));
        }
        for slot in &slots {
            if let Some(pad) = &slot.pad {
                self.book.claim(pad, name);
            }
        }
        Ok(slots)
    }

    async fn start(&mut self, name: String, asker: Asker) {
        if self.setups.entries.contains_key(&name) {
            let refusal = WireError::refusal(
                ErrorCode::SetupExists,
                format!("the setup {name} exists already; stop it first"),
            );
            return asker.err(&refusal).await;
        }
        let Some(profile) = self.profiles.profile(&name) else {
            let refusal = WireError::refusal(
                ErrorCode::UnknownProfile,
                format!("no profile is called {}", name.escape_debug()),
            );
            return asker.err(&refusal).await;
        };
        let plan = match Profile::check(&profile.toml) {
            Ok(plan) => plan,
            Err(problem) => {
                let refusal = WireError::refusal(ErrorCode::InvalidProfile, problem.to_string());
                return asker.err(&refusal).await;
            }
        };
        let slots = match self.bind(&name, &plan) {
            Ok(slots) => slots,
            Err(refusal) => return asker.err(&refusal).await,
        };
        let nodes: Vec<Option<Wanted>> = slots
            .iter()
            .map(|slot| {
                let pad = slot.pad.as_deref()?;
                Some(Wanted {
                    id: pad.to_string(),
                    node: self.book.node(pad)?.to_path_buf(),
                })
            })
            .collect();
        let pads: Vec<String> = slots.iter().filter_map(|slot| slot.pad.clone()).collect();
        let arrivals = slots
            .iter()
            .map(|slot| self.book.arrival(slot.pad.as_deref()?))
            .collect();
        self.setups.started += 1;
        let errand = Arc::new(Errand::default());
        let entry = Entry {
            id: SetupId(self.setups.started),
            phase: Phase::Starting,
            reason: None,
            slots,
            sinks: plan
                .sinks
                .iter()
                .map(|sink| payload::SetupSink {
                    name: sink.name.clone(),
                    identity: sink.identity.as_str().to_string(),
                    node: None,
                })
                .collect(),
            selected: vec![None; plan.nodes.len()],
            nodes: choosers(&plan),
            stopping: None,
            taking: None,
            toml: profile.toml,
            stale: false,
            hide: plan.hide,
            arrivals,
            retiring: false,
            errand: Arc::clone(&errand),
        };
        self.setups.entries.insert(name.clone(), entry);
        self.publish_setup(&name).await;
        for pad in pads {
            self.publish_pad_logged(&pad).await;
        }
        let asker = self.owe(Owed::Start(asker));
        let database = Arc::clone(&self.database);
        let hiding = Arc::clone(&self.hiding);
        let dualsense = (
            self.copied.path().to_path_buf(),
            self.copied.dualsense_or_shipped(),
        );
        let blacklist = Arc::clone(&self.blacklist);
        self.work.spawn_blocking(move || Done::Started {
            outcome: caught(
                "starting a setup",
                || {
                    let asking = (&*hiding, &errand);
                    make(
                        plan, &name, &nodes, &database, asking, &dualsense, &blacklist,
                    )
                },
                || Err(Unmade::new("starting it panicked")),
            ),
            name,
            asker,
        });
    }

    async fn started(&mut self, name: String, outcome: Result<Made, Unmade>, asker: Asker) {
        let Some(entry) = self.setups.entries.get_mut(&name) else {
            // Nothing removes a setup that is starting.
            return asker.err(&WireError::internal("the setup is gone")).await;
        };
        // Asked to stop as it started: what it made is let go, not run.
        let stopped = entry.stopping.is_some();
        let sent = outcome.map_err(|unmade| (unmade, None)).and_then(|made| {
            let Made { setup, nodes, pads } = made;
            let tokens = pads.iter().map(|(_, token)| *token).collect();
            if stopped {
                let unmade = Unmade {
                    reason: STOPPED.to_string(),
                    tokens,
                };
                return Err((unmade, Some(Box::new(setup) as Box<dyn Send>)));
            }
            match self.input.send(Command::Start(entry.id, setup)) {
                Ok(()) => Ok((nodes, pads)),
                Err(error) => {
                    let unmade = Unmade {
                        reason: format!("the input thread didn't take it: {error}"),
                        tokens,
                    };
                    let command = match error {
                        SendError::Full(command) | SendError::Stopped(command) => {
                            Some(Box::new(command) as Box<dyn Send>)
                        }
                        _ => None,
                    };
                    Err((unmade, command))
                }
            }
        });
        match sent {
            Ok((nodes, pads)) => {
                let mut hidden = Vec::new();
                for (source, (slot, (present, token))) in
                    entry.slots.iter_mut().zip(pads).enumerate()
                {
                    slot.present = present;
                    slot.token = token;
                    if let (Some(pad), Some(_)) = (&slot.pad, token) {
                        let arrival = entry.arrivals.get(source).copied().flatten();
                        hidden.push((pad.clone(), arrival));
                    }
                }
                for (sink, node) in entry.sinks.iter_mut().zip(nodes) {
                    sink.node = Some(node.display().to_string());
                }
                entry.settle();
                let sources = entry.slots.len();
                let start = entry.id;
                info!(
                    setup = name,
                    phase = entry.phase.as_str(),
                    "a setup started"
                );
                for (pad, arrival) in hidden {
                    self.hid(&pad, arrival);
                    self.book.set_hidden(&pad, true);
                    self.publish_pad_logged(&pad).await;
                }
                self.fill(&name).await;
                self.publish_setup(&name).await;
                asker.ok(start).await;
                // A pad may have connected while it started, and one it
                // has may have been in the middle of connecting: each is
                // taken now.
                for source in 0..sources {
                    self.bring_back(&name, source);
                }
            }
            Err((Unmade { reason, tokens }, leftover)) => {
                // A start that fails leaves nothing behind.
                let entry = self.setups.entries.remove(&name);
                let mut shown = Vec::new();
                for (source, slot) in entry
                    .iter()
                    .flat_map(|entry| entry.slots.iter().enumerate())
                {
                    let token = tokens.get(source).copied().flatten();
                    let (Some(pad), Some(token)) = (&slot.pad, token) else {
                        continue;
                    };
                    // Shown again, it goes and comes back once more, as it
                    // is let go below. Neither is a pad plugged in, to
                    // start this again by.
                    let arrival = entry
                        .as_ref()
                        .and_then(|entry| entry.arrivals.get(source).copied().flatten());
                    self.hid(pad, arrival);
                    self.book.set_hidden(pad, true);
                    shown.push((pad.clone(), token));
                }
                self.release(&name).await;
                self.publish_setup(&name).await;
                // A stop asked as it started is answered once its pads
                // are shown again, as any stop is.
                let (setup, waiting) =
                    entry.map_or((SetupId(0), None), |entry| (entry.id, entry.stopping));
                let waiting = waiting.map(|waiting| self.owe(Owed::Wait(waiting)));
                let hiding = Arc::clone(&self.hiding);
                let blacklist = Arc::clone(&self.blacklist);
                let stopped = name.clone();
                self.work.spawn_blocking(move || {
                    let unshown = shown_again(&hiding, leftover, &shown);
                    if let Ok(mut blacklist) = blacklist.lock() {
                        blacklist.set(&stopped, std::collections::BTreeSet::new());
                    }
                    Done::Retired {
                        name: stopped,
                        setup,
                        waiting,
                        unshown,
                    }
                });
                if reason == STOPPED {
                    info!(setup = name, "a setup was stopped as it started");
                } else {
                    warn!(setup = name, reason, "a setup couldn't start");
                }
                let refusal = WireError::refusal(ErrorCode::SetupFailed, reason);
                asker.err(&refusal).await;
            }
        }
    }

    /// Clears away a failed setup, and anything the book still notes as
    /// its own.
    async fn clear_failed(&mut self, name: &str) {
        self.setups.entries.remove(name);
        self.release(name).await;
        self.publish_setup(name).await;
    }

    /// Gives each of a setup's sources still without a pad one that fits
    /// and is free, as a start does, and notes as the setup's each pad it
    /// names that has come to be known since: for a setup that has just
    /// started, which pads that connected meanwhile were left to.
    async fn fill(&mut self, name: &str) {
        let Some(entry) = self.setups.entries.get_mut(name) else {
            return;
        };
        let mut claimed = Vec::new();
        for slot in &mut entry.slots {
            if slot.pad.is_none() {
                slot.pad = self
                    .book
                    .fitting(&slot.wanted)
                    .first()
                    .map(ToString::to_string);
            }
            if let Some(pad) = &slot.pad
                && self.book.user(pad).is_none()
            {
                self.book.claim(pad, name);
                claimed.push(pad.clone());
            }
        }
        for pad in claimed {
            self.publish_pad_logged(&pad).await;
        }
    }

    async fn stop(&mut self, name: String, responder: Responder<setup::Stop>) {
        let Some(entry) = self.setups.entries.get_mut(&name) else {
            return responder.err(&unknown(&name)).await;
        };
        match entry.phase {
            // The input thread handed it back when it failed; the stop is
            // answered once its pads are shown again.
            Phase::Failed if entry.stopping.is_none() && entry.retiring => {
                entry.stopping = Some(Waiting::Stop(responder));
            }
            Phase::Failed if entry.stopping.is_none() => {
                self.clear_failed(&name).await;
                responder.ok(&rpc::Ack {}).await;
            }
            // What it asks of the hiding service ends now, and it is
            // answered once what it took is let go and shown again.
            Phase::Starting if entry.stopping.is_none() => {
                self.hiding.end(&entry.errand);
                entry.stopping = Some(Waiting::Stop(responder));
            }
            Phase::Running | Phase::Held if entry.stopping.is_none() => {
                match self.input.send(Command::Stop(entry.id)) {
                    Ok(()) => {
                        // A pad being taken back, or for a change, is
                        // given up.
                        self.hiding.end(&entry.errand);
                        entry.stopping = Some(Waiting::Stop(responder));
                    }
                    Err(error) => {
                        let refusal = WireError::internal(format!(
                            "the input thread didn't take the request: {error}"
                        ));
                        responder.err(&refusal).await;
                    }
                }
            }
            _ => {
                let refusal = WireError::refusal(
                    ErrorCode::SetupBusy,
                    format!("the setup {name} is stopping or restarting; ask again once it has"),
                );
                responder.err(&refusal).await;
            }
        }
    }

    /// Has a setup take up its profile as it is kept now, or only says
    /// what that would cost. A profile of the same pads and virtual pads
    /// is taken as the setup runs: the input thread is handed its graph.
    /// Any other means stopping the setup and starting it again.
    async fn update(&mut self, request: rpc::UpdateRequest, responder: Updating) {
        let name = request.name;
        let Some(entry) = self.setups.entries.get_mut(&name) else {
            return responder.err(&unknown(&name)).await;
        };
        // What is kept now must be right before what runs is given up.
        let kept = self.profiles.profile(&name).map(|profile| profile.toml);
        let toml = kept.clone().unwrap_or_default();
        let plan = match kept.map(|toml| Profile::check(&toml)) {
            None => {
                let refusal = WireError::refusal(
                    ErrorCode::UnknownProfile,
                    format!("the profile {name} is no longer kept; the setup runs it as it was"),
                );
                return responder.err(&refusal).await;
            }
            Some(Err(problem)) => {
                let refusal = WireError::refusal(ErrorCode::InvalidProfile, problem.to_string());
                return responder.err(&refusal).await;
            }
            Some(Ok(plan)) => plan,
        };
        // Its pads are taken one way for as long as it runs: another
        // way is another start.
        let live = matches!(entry.phase, Phase::Running | Phase::Held)
            && entry.has_the_virtual_pads_of(&plan)
            && entry.hide == plan.hide;
        // The pads it would have, which a restart finds for itself.
        let bound = live.then(|| rebind(&self.book, &name, &entry.slots, &plan));
        let bound = match bound.transpose() {
            Ok(bound) => bound,
            Err(refusal) => return responder.err(&refusal).await,
        };
        if !request.apply {
            // Nothing changes: the start is the one it has.
            let cost = if live {
                taken(entry.id)
            } else {
                restarted(entry.id)
            };
            return responder.ok(&cost).await;
        }
        let free = entry.stopping.is_none() && entry.taking.is_none();
        match (entry.phase, bound) {
            // Nothing runs: it is cleared away and started afresh, once
            // its pads are shown again.
            (Phase::Failed, _) if free && entry.retiring => {
                entry.stopping = Some(Waiting::Update(responder));
            }
            (Phase::Failed, _) if free => {
                self.clear_failed(&name).await;
                self.start(name, Asker::Update(responder)).await;
            }
            (Phase::Running | Phase::Held, Some((slots, keep))) if free => {
                let nodes = choosers(&plan);
                let sinks = plan.sinks.iter().map(|sink| sink.name.clone()).collect();
                let graph = match Graph::build(plan.graph) {
                    Ok(graph) => graph,
                    Err(error) => {
                        let refusal =
                            WireError::internal(format!("the graph didn't build: {error}"));
                        return responder.err(&refusal).await;
                    }
                };
                // The pads it brings are its own from here, so that
                // nothing else takes them meanwhile.
                let mut brings = Vec::new();
                let fresh: Vec<Option<Wanted>> = slots
                    .iter()
                    .zip(&keep)
                    .map(|(slot, keep)| {
                        let pad = slot.pad.as_deref().filter(|_| keep.is_none())?;
                        brings.push(pad.to_string());
                        Some(Wanted {
                            id: pad.to_string(),
                            node: self.book.node(pad)?.to_path_buf(),
                        })
                    })
                    .collect();
                for pad in &brings {
                    self.book.claim(pad, &name);
                }
                let errand = Arc::clone(&entry.errand);
                entry.taking = Some(Taking {
                    responder,
                    toml,
                    slots,
                    keep,
                    sinks,
                    nodes,
                    sent: false,
                });
                for pad in &brings {
                    self.publish_pad_logged(pad).await;
                }
                let ids = fresh
                    .iter()
                    .map(|wanted| Some(wanted.as_ref()?.id.clone()))
                    .collect();
                let arrivals = fresh
                    .iter()
                    .map(|wanted| self.book.arrival(&wanted.as_ref()?.id))
                    .collect();
                if fresh.iter().all(Option::is_none) {
                    let none = fresh.iter().map(|_| None).collect();
                    return self.send_graph(name, graph, ids, none, arrivals).await;
                }
                let database = Arc::clone(&self.database);
                let hiding = Arc::clone(&self.hiding);
                let hide = plan.hide;
                self.work.spawn_blocking(move || {
                    let pads = fresh
                        .iter()
                        .map(|wanted| {
                            let wanted = wanted.as_ref()?;
                            caught(
                                "taking a pad",
                                || take(wanted, &database, (&hiding, &errand), hide),
                                || Err("taking it panicked".to_string()),
                            )
                            .inspect_err(|error| warn!(error, "a setup changes without a pad"))
                            .ok()
                        })
                        .collect();
                    Done::Retaken {
                        name,
                        graph: Box::new(graph),
                        ids,
                        pads,
                        arrivals,
                    }
                });
            }
            (Phase::Running | Phase::Held, None) if free => {
                match self.input.send(Command::Stop(entry.id)) {
                    Ok(()) => {
                        self.hiding.end(&entry.errand);
                        entry.stopping = Some(Waiting::Update(responder));
                    }
                    Err(error) => {
                        let refusal = WireError::internal(format!(
                            "the input thread didn't take the request: {error}"
                        ));
                        responder.err(&refusal).await;
                    }
                }
            }
            _ => {
                let refusal = WireError::refusal(
                    ErrorCode::SetupBusy,
                    format!("the setup {name} is starting or stopping; ask again once it has"),
                );
                responder.err(&refusal).await;
            }
        }
    }

    /// Brings the running setups among `names` up to their profiles as
    /// a reload found them, and says which it set about. One whose
    /// profile is now wrong, or that is busy, stays as it runs, and the
    /// log says why.
    pub(crate) async fn update_reloaded(&mut self, names: &[String]) -> Vec<String> {
        let mut updating = Vec::new();
        for name in names {
            if !self.setups.entries.contains_key(name) {
                continue;
            }
            updating.push(name.clone());
            let request = rpc::UpdateRequest {
                name: name.clone(),
                apply: true,
            };
            self.update(request, Updating::Reload).await;
        }
        updating
    }

    /// Hands the input thread the graph of a change a setup is taking,
    /// with the pads it brings, now that they are open.
    async fn send_graph(
        &mut self,
        name: String,
        graph: Graph,
        ids: Vec<Option<String>>,
        pads: Vec<Option<(Pad, Option<Token>)>>,
        arrivals: Vec<Option<u64>>,
    ) {
        // Whatever becomes of the change, a pad hidden for it was hidden.
        let hidden = pads.iter().zip(&ids).zip(arrivals);
        let hidden: Vec<(String, Option<u64>)> = hidden
            .filter(|((pad, _), _)| pad.as_ref().is_some_and(|(_, token)| token.is_some()))
            .filter_map(|((_, id), arrival)| Some((id.clone()?, arrival)))
            .collect();
        for (pad, arrival) in hidden {
            if self.hid(&pad, arrival) {
                self.publish_pad_logged(&pad).await;
            }
        }
        let ready = self.setups.entries.get_mut(&name).and_then(|entry| {
            let Entry {
                id,
                phase,
                stopping,
                nodes,
                taking,
                ..
            } = entry;
            let taking = taking.as_mut().filter(|taking| !taking.sent)?;
            let runs = matches!(phase, Phase::Running | Phase::Held) && stopping.is_none();
            runs.then_some((*id, &*nodes, taking))
        });
        let Some((setup, nodes, taking)) = ready else {
            // It stopped or failed while the pads were being opened.
            let hidden = pads
                .iter()
                .zip(ids)
                .filter_map(|(pad, id)| Some((id?, pad.as_ref()?.1?)))
                .collect();
            let refusal = WireError::refusal(
                ErrorCode::SetupBusy,
                format!("the setup {name} is stopping; the change wasn't made"),
            );
            return self.abandon(&name, pads, hidden, &refusal).await;
        };
        let mut places = Vec::with_capacity(pads.len());
        for ((slot, keep), pad) in taking.slots.iter_mut().zip(&taking.keep).zip(pads) {
            places.push(match keep {
                Some(kept) => Place::Keep(SourceId(*kept)),
                None => {
                    let (pad, token) = pad.unzip();
                    slot.present = pad.is_some();
                    slot.token = token.flatten();
                    Place::Take(pad.map(Box::new))
                }
            });
        }
        let graph = Box::new(NewGraph::new(graph, carry(nodes, &taking.nodes), places));
        match self.input.send(Command::NewGraph { setup, graph }) {
            Ok(()) => taking.sent = true,
            Err(error) => {
                let refusal = WireError::internal(format!(
                    "the input thread didn't take the request: {error}"
                ));
                let command = match error {
                    SendError::Full(command) | SendError::Stopped(command) => Some(command),
                    _ => None,
                };
                self.abandon(&name, command, Vec::new(), &refusal).await;
            }
        }
    }

    /// Gives up a change a setup was taking. `brought` holds the pads it
    /// brought, which are let go and then shown again, with any named in
    /// `hidden`; whoever asked for the change is told why it wasn't made.
    async fn abandon<T: Send + 'static>(
        &mut self,
        name: &str,
        brought: T,
        mut hidden: Vec<(String, Token)>,
        refusal: &WireError,
    ) {
        let taking = self
            .setups
            .entries
            .get_mut(name)
            .and_then(|entry| entry.taking.take());
        if let Some(taking) = taking {
            for slot in taking.fresh() {
                if let (Some(pad), Some(token)) = (&slot.pad, slot.token) {
                    hidden.push((pad.clone(), token));
                }
            }
            let freed: Vec<String> = taking.fresh().filter_map(|slot| slot.pad.clone()).collect();
            for pad in freed {
                // Hidden as it was brought: shown again as it is let go.
                if hidden.iter().any(|(was, _)| *was == pad) {
                    self.book.set_hidden(&pad, true);
                }
                self.let_go(&pad).await;
            }
            taking.responder.err(refusal).await;
        } else {
            // The setup let its pads go as it stopped; those hidden for
            // it since are shown again for no setup.
            for (pad, _) in &hidden {
                if self.hiding.rebinds(pad) {
                    self.book.expect_back(pad);
                }
            }
        }
        self.show_again(brought, hidden);
    }

    /// The input thread has a setup's new graph: the setup is now what
    /// the change made it. The pads the new graph had no place for come
    /// back in `old`; they are let go, then shown again, and then the
    /// update is answered.
    async fn took(&mut self, setup: SetupId, old: Box<NewGraph>) {
        let taking = self.setups.name_of(setup).and_then(|name| {
            let entry = self.setups.entries.get_mut(&name)?;
            let taking = entry.taking.take()?;
            Some((name, entry, taking))
        });
        let Some((name, entry, taking)) = taking else {
            return self.drop_off(old);
        };
        let Taking {
            responder,
            toml,
            mut slots,
            keep,
            sinks,
            nodes,
            ..
        } = taking;
        let mut hidden = Vec::new();
        let mut changed = Vec::new();
        for (index, slot) in entry.slots.iter().enumerate() {
            if !keep.contains(&Some(index))
                && let Some(pad) = &slot.pad
            {
                hidden.extend(slot.token.map(|token| (pad.clone(), token)));
                self.book.let_go(pad, self.hiding.rebinds(pad));
                changed.push(pad.clone());
            }
        }
        for (slot, keep) in slots.iter_mut().zip(&keep) {
            if let Some(has) = keep.and_then(|kept| entry.slots.get(kept)) {
                // What became of a kept pad since the change was asked.
                slot.pad.clone_from(&has.pad);
                slot.present = has.present;
                slot.returning = has.returning;
                slot.token = has.token;
                continue;
            }
            // A source still without a pad takes one that came meanwhile.
            if slot.pad.is_none() {
                let came = self.book.fitting(&slot.wanted).first().copied();
                slot.pad = came.map(str::to_string);
                if let Some(pad) = &slot.pad {
                    self.book.claim(pad, &name);
                }
            }
            if let Some(pad) = &slot.pad {
                self.book.set_hidden(pad, slot.token.is_some());
                changed.push(pad.clone());
            }
        }
        let sources = slots.len();
        let start = entry.id;
        entry.slots = slots;
        entry.toml = toml;
        entry.stale = false;
        for (sink, name) in entry.sinks.iter_mut().zip(sinks) {
            sink.name = name;
        }
        entry.selected = vec![None; nodes.len()];
        entry.nodes = nodes;
        entry.settle();
        info!(
            setup = name,
            phase = entry.phase.as_str(),
            "a setup took its profile as it runs"
        );
        for pad in changed {
            self.publish_pad_logged(&pad).await;
        }
        self.publish_setup(&name).await;
        let hiding = Arc::clone(&self.hiding);
        let responder = self.owe(Owed::Update(responder));
        self.work.spawn_blocking(move || {
            let unshown = shown_again(&hiding, old, &hidden);
            Done::Taken {
                responder,
                start,
                unshown,
            }
        });
        // A pad that isn't there may have come while the pads were
        // numbered by the old graph.
        for source in 0..sources {
            self.bring_back(&name, source);
        }
    }

    /// Buzzes a pad that a setup holds, through the input thread, which
    /// is the only thing that can reach it.
    pub(crate) fn buzz(
        &self,
        setup: &str,
        pad: &str,
        level: Level,
        millis: u32,
    ) -> Result<(), WireError> {
        let place = self.setups.entries.get(setup).and_then(|entry| {
            let source = entry
                .slots
                .iter()
                .position(|slot| slot.pad.as_deref() == Some(pad))?;
            // While it takes a change its pads are about to be numbered
            // anew, and a pad it is giving up isn't one to buzz.
            let there = entry.slots[source].present && entry.taking.is_none();
            Some((entry.id, source, there))
        });
        let Some((id, source, true)) = place else {
            return Err(WireError::refusal(
                ErrorCode::PadDisconnected,
                format!("{pad} isn't connected"),
            ));
        };
        let command = Command::Buzz {
            setup: id,
            source: SourceId(source),
            level,
            millis,
        };
        self.input.send(command).map_err(|error| {
            WireError::internal(format!("the input thread didn't take the request: {error}"))
        })
    }

    /// Asks the input thread to have a node select as `request` says.
    fn select(&self, request: &rpc::SelectRequest) -> Result<(), WireError> {
        let entry = self
            .setups
            .entries
            .get(&request.name)
            .ok_or_else(|| unknown(&request.name))?;
        if !matches!(entry.phase, Phase::Running | Phase::Held) {
            return Err(WireError::refusal(
                ErrorCode::SetupBusy,
                format!("the setup {} isn't running", request.name),
            ));
        }
        if entry.taking.is_some() {
            // Its nodes are about to be numbered anew.
            return Err(WireError::refusal(
                ErrorCode::SetupBusy,
                format!(
                    "the setup {} is taking a change; ask again once it has",
                    request.name
                ),
            ));
        }
        let selecting = entry
            .nodes
            .iter()
            .zip(&entry.selected)
            .enumerate()
            .find(|(_, ((name, _), _))| *name == request.node);
        let Some((node, ((_, choices), Some(_)))) = selecting else {
            let those: Vec<&str> = entry
                .nodes
                .iter()
                .zip(&entry.selected)
                .filter(|(_, selected)| selected.is_some())
                .map(|((name, _), _)| name.as_str())
                .collect();
            return Err(WireError::bad_request(if those.is_empty() {
                format!("no node of the setup {} selects", request.name)
            } else {
                format!(
                    "`{}` isn't a node of the setup {} that selects; those are {}",
                    request.node.escape_debug(),
                    request.name,
                    those.join(", ")
                )
            }));
        };
        let setup = entry.id;
        let node = NodeId(node);
        let command = match &request.choice {
            None => Command::Switch { setup, node },
            Some(choice) => {
                let index = choices
                    .iter()
                    .position(|known| known == choice)
                    .or_else(|| choice.parse().ok().filter(|index| *index < choices.len()))
                    .ok_or_else(|| {
                        WireError::bad_request(format!(
                            "`{}` isn't one of {}'s choices, which are {}",
                            choice.escape_debug(),
                            request.node,
                            choices.join(", ")
                        ))
                    })?;
                Command::Select { setup, node, index }
            }
        };
        self.input.send(command).map_err(|error| {
            WireError::internal(format!("the input thread didn't take the request: {error}"))
        })
    }

    pub(crate) async fn setup_rpc(&mut self, query: Query) {
        match setup::Incoming::receive(query).await {
            Some(((), setup::Incoming::Start(request, responder))) => {
                self.start(request.name, Asker::Client(responder)).await;
            }
            Some(((), setup::Incoming::Stop(request, responder))) => {
                self.stop(request.name, responder).await;
            }
            Some(((), setup::Incoming::Update(request, responder))) => {
                self.update(request, Updating::Client(responder)).await;
            }
            Some(((), setup::Incoming::Select(request, responder))) => {
                responder
                    .answer(self.select(&request).map(|()| rpc::Ack {}))
                    .await;
            }
            // Refused already, with a typed error.
            None => {}
        }
    }

    /// Whether every pad `plan` names is connected and free, so that
    /// started now it would run and not be held.
    fn ready(&self, plan: &Plan) -> bool {
        let mut taken: Vec<&str> = Vec::new();
        plan.sources.iter().all(|source| {
            let pad = match &source.pad {
                PadRef::Exact(id) => {
                    let free = self.book.node(id).is_some() && self.book.user(id).is_none();
                    (free && !taken.contains(&id.as_str())).then_some(id.as_str())
                }
                wanted => self
                    .book
                    .fitting(wanted)
                    .into_iter()
                    .find(|id| !taken.contains(id)),
            };
            pad.inspect(|pad| taken.push(pad)).is_some()
        })
    }

    /// Starts each kept profile that asks to start by itself and now
    /// can: one with `managed`, nothing wrong with it, no setup of its
    /// name, and every pad it names connected and free. `because` is the
    /// pad whose connecting prompted this, if one did: then only profiles
    /// that pad fits are looked at, so a setup the user stopped isn't
    /// started again by some other pad arriving.
    pub(crate) async fn start_what_can(&mut self, because: Option<&str>) {
        let names: Vec<String> = self.profiles.names().map(str::to_string).collect();
        for name in names {
            if self.setups.entries.contains_key(&name) {
                continue;
            }
            let Some(plan) = self
                .profiles
                .profile(&name)
                .and_then(|profile| Profile::check(&profile.toml).ok())
            else {
                continue;
            };
            let prompted = because.is_none_or(|pad| {
                plan.sources
                    .iter()
                    .any(|source| self.book.fits(pad, &source.pad))
            });
            if plan.managed && prompted && self.ready(&plan) {
                info!(
                    profile = name,
                    "a profile starts by itself: its pads are connected"
                );
                self.start(name, Asker::Daemon).await;
            }
        }
    }

    /// Starts the kept profile `name` if it asks to start by itself and
    /// can, as when it has just been put.
    pub(crate) async fn start_if_it_can(&mut self, name: &str) {
        let plan = self
            .profiles
            .profile(name)
            .and_then(|profile| Profile::check(&profile.toml).ok());
        if let Some(plan) = plan
            && plan.managed
            && !self.setups.entries.contains_key(name)
            && self.ready(&plan)
        {
            info!(
                profile = name,
                "a profile starts by itself: its pads are connected"
            );
            self.start(name.to_string(), Asker::Daemon).await;
        }
    }

    /// Opens the pad a held setup's source is waiting for, if it is
    /// connected, to hand it back to the input thread.
    fn bring_back(&mut self, name: &str, source: usize) {
        let Some(entry) = self.setups.entries.get_mut(name) else {
            return;
        };
        // While it takes a change its pads are about to be numbered anew;
        // each that is missing is brought back once it has.
        let waits = entry.stopping.is_some() || entry.taking.is_some();
        if !matches!(entry.phase, Phase::Running | Phase::Held) || waits {
            return;
        }
        let (setup, hide, errand) = (entry.id, entry.hide, Arc::clone(&entry.errand));
        let Some(slot) = entry.slots.get_mut(source) else {
            return;
        };
        if slot.present {
            return;
        }
        if slot.returning {
            // It may have come back since it was sent for, and the
            // opening under way fail for that.
            slot.again = true;
            return;
        }
        // A pad another setup has taken meanwhile is that setup's.
        let wanted = slot.pad.as_deref().and_then(|pad| {
            if self.book.user(pad).is_some_and(|user| user != name) {
                return None;
            }
            Some(Wanted {
                id: pad.to_string(),
                node: self.book.node(pad)?.to_path_buf(),
            })
        });
        let Some(wanted) = wanted else {
            return;
        };
        slot.returning = true;
        let token = slot.token;
        let arrival = self.book.arrival(&wanted.id);
        let database = Arc::clone(&self.database);
        let hiding = Arc::clone(&self.hiding);
        let name = name.to_string();
        self.work.spawn_blocking(move || {
            // A node from before the pad left doesn't open, or is some
            // other pad's by now; the pad's arrival will bring it back.
            let retaken = caught(
                "taking a pad back",
                || retake(&wanted, token, &database, (&hiding, &errand), hide),
                || Err("taking it back panicked".to_string()),
            );
            let (pad, token) = retaken.ok().unzip();
            Done::Returned {
                pad: pad.map(Box::new),
                token: token.flatten(),
                name,
                setup,
                id: wanted.id,
                arrival,
            }
        });
    }

    async fn returned(
        &mut self,
        (name, setup): (String, SetupId),
        id: String,
        pad: Option<Box<Pad>>,
        (token, arrival): (Option<Token>, Option<u64>),
    ) {
        // Whatever became of the setup, a pad hidden afresh was hidden.
        if token.is_some() && self.hid(&id, arrival) {
            self.publish_pad_logged(&id).await;
        }
        // By the pad, not by where it was when it was sent for: a setup
        // that took a change since has its pads numbered anew. Only to
        // the setup that sent for it, and only while that runs: one that
        // failed meanwhile has had its pads shown again.
        let place = self.setups.entries.get_mut(&name).and_then(|entry| {
            let runs = matches!(entry.phase, Phase::Running | Phase::Held);
            if entry.id != setup || !runs {
                return None;
            }
            let source = entry
                .slots
                .iter()
                .position(|slot| slot.pad.as_deref() == Some(id.as_str()))?;
            let taking = entry.taking.is_some();
            Some((taking, source, entry.slots.get_mut(source)?))
        });
        let Some((taking, source, slot)) = place else {
            // The setup went, failed, or gave the pad up, while the pad
            // was being opened: the pad is let go, and shown again if it
            // was hidden afresh for this.
            let hidden: Vec<(String, Token)> =
                token.map(|token| (id.clone(), token)).into_iter().collect();
            if !hidden.is_empty() && self.hiding.rebinds(&id) {
                self.book.expect_back(&id);
            }
            return self.show_again(pad, hidden);
        };
        slot.returning = false;
        let again = std::mem::take(&mut slot.again);
        let Some(pad) = pad else {
            // It may be back by now, where it wasn't when sent for.
            if again {
                self.bring_back(&name, source);
            }
            return;
        };
        let newly_hidden = token.is_some() && slot.token != token;
        slot.token = token.or(slot.token);
        if newly_hidden {
            self.book.set_hidden(&id, true);
            self.publish_pad_logged(&id).await;
        }
        if taking {
            // The input thread is about to number the setup's pads anew:
            // this one is let go, and brought back once it has.
            return self.drop_off(pad);
        }
        let command = Command::PadBack {
            setup,
            source: SourceId(source),
            pad,
        };
        match self.input.send(command) {
            Ok(()) => {
                if let Some(entry) = self.setups.entries.get_mut(&name) {
                    if let Some(slot) = entry.slots.get_mut(source) {
                        slot.present = true;
                    }
                    entry.settle();
                }
                info!(setup = name, source, "a pad returned to its setup");
                self.publish_setup(&name).await;
            }
            Err(error) => {
                warn!(setup = name, %error, "a returned pad couldn't be handed to the input thread");
                if let SendError::Full(command) | SendError::Stopped(command) = error {
                    self.drop_off(command);
                }
            }
        }
    }

    /// A pad connected: a setup waiting for it takes it.
    pub(crate) async fn pad_arrived(&mut self, id: &str) {
        // One pad, one place: its own, in a setup that hasn't failed and
        // that no other setup's use of it comes before; or else the first
        // place it fits that has no pad yet, in a setup that runs. A
        // setup that is starting looks for that itself once it has.
        let user = self.book.user(id);
        let own = self.setups.entries.iter().find_map(|(name, entry)| {
            let mine = user.is_none_or(|user| user == name);
            let source = entry
                .slots
                .iter()
                .position(|slot| slot.pad.as_deref() == Some(id))?;
            (mine && entry.phase != Phase::Failed).then(|| (name.clone(), source))
        });
        let place = own.or_else(|| {
            let free = user.is_none();
            self.setups.entries.iter().find_map(|(name, entry)| {
                let runs = matches!(entry.phase, Phase::Running | Phase::Held);
                let source = entry
                    .slots
                    .iter()
                    .position(|slot| slot.pad.is_none() && self.book.fits(id, &slot.wanted))?;
                (free && runs).then(|| (name.clone(), source))
            })
        });
        let Some((name, source)) = place else {
            return;
        };
        let mut placed = false;
        if let Some(slot) = self
            .setups
            .entries
            .get_mut(&name)
            .and_then(|entry| entry.slots.get_mut(source))
            && slot.pad.is_none()
        {
            slot.pad = Some(id.to_string());
            placed = true;
        }
        // Noted as the setup's, if it wasn't: an exact pad never seen
        // before had nothing to note until now.
        if self.book.user(id).is_none() {
            self.book.claim(id, &name);
            self.publish_pad_logged(id).await;
        }
        if placed {
            self.publish_setup(&name).await;
        }
        self.bring_back(&name, source);
    }

    /// Work done off the reactor has come back.
    pub(crate) async fn done(&mut self, done: Done) {
        self.done_inner(done).await;
        // Whatever was done may have found the hiding service.
        self.publish_hiding().await;
    }

    async fn done_inner(&mut self, done: Done) {
        match done {
            Done::Started {
                name,
                outcome,
                asker,
            } => {
                let asker = match self.paid(asker) {
                    Some(Owed::Start(asker)) => asker,
                    // Nothing else is owed under a start's note.
                    _ => Asker::Daemon,
                };
                self.started(name, outcome, asker).await;
            }
            Done::Returned {
                name,
                setup,
                id,
                pad,
                token,
                arrival,
            } => {
                self.returned((name, setup), id, pad, (token, arrival))
                    .await;
            }
            Done::Retaken {
                name,
                graph,
                ids,
                pads,
                arrivals,
            } => self.send_graph(name, *graph, ids, pads, arrivals).await,
            Done::Taken {
                responder,
                start,
                unshown,
            } => {
                self.unshown(unshown).await;
                if let Some(Owed::Update(responder)) = self.paid(responder) {
                    responder.ok(&taken(start)).await;
                }
            }
            Done::Retired {
                name,
                setup,
                waiting,
                unshown,
            } => {
                let waiting = waiting
                    .and_then(|waiting| self.paid(waiting))
                    .and_then(|owed| match owed {
                        Owed::Wait(waiting) => Some(waiting),
                        _ => None,
                    });
                self.retired(name, setup, waiting, unshown).await;
            }
            Done::Shown(unshown) => self.unshown(unshown).await,
            Done::Copied(data) => {
                self.copying = false;
                match data {
                    Some(data) => {
                        self.candidates.clear();
                        // Kept on disk off the async runtime's workers.
                        let mut copied = self.copied.clone();
                        let kept = tokio::task::spawn_blocking(move || {
                            copied.keep(data);
                            copied
                        });
                        match kept.await {
                            Ok(copied) => self.copied = copied,
                            Err(error) => error!(%error, "keeping a copied DualSense panicked"),
                        }
                        if let Err(error) = self.publish_identities().await {
                            warn!(%error, "publishing the identities failed");
                        }
                    }
                    // Another that connected meanwhile, if one did.
                    None => {
                        if let Some(next) = self.candidates.pop() {
                            self.copy_from(&next);
                        }
                    }
                }
            }
            // What was found is published below.
            Done::Dropped | Done::Looked => {}
        }
    }

    /// Adds to `hidden` the pads a change a setup gave up had hidden for
    /// it, which the book is told of as hidden: they are let go with the
    /// setup's own, and shown again the same way.
    fn hidden_for(&mut self, taking: &Taking, hidden: &mut Vec<(String, Token)>) {
        for (pad, token) in taking.fresh().filter_map(hidden_by) {
            self.book.set_hidden(&pad, true);
            hidden.push((pad, token));
        }
    }

    /// A setup the input thread handed back is gone and its pads shown
    /// again: whoever waited for that is seen to, whether they asked
    /// before it was handed back or, for one that failed, after.
    async fn retired(
        &mut self,
        name: String,
        setup: SetupId,
        waiting: Option<Waiting>,
        unshown: Vec<String>,
    ) {
        self.unshown(unshown).await;
        let failed = self
            .setups
            .entries
            .get_mut(&name)
            .filter(|entry| entry.id == setup && entry.phase == Phase::Failed);
        let mut asked = None;
        if let Some(entry) = failed {
            entry.retiring = false;
            asked = entry.stopping.take();
            if asked.is_some() {
                self.clear_failed(&name).await;
            }
        }
        match waiting.or(asked) {
            Some(Waiting::Stop(responder)) => responder.ok(&rpc::Ack {}).await,
            // The old setup is gone, pads restored: the new one starts
            // from the profile as it is kept now.
            Some(Waiting::Update(responder)) => {
                self.start(name, Asker::Update(responder)).await;
            }
            None => {}
        }
    }

    /// The input thread has said something.
    pub(crate) async fn reported(&mut self, report: Report) {
        match report {
            Report::Stopped(id, setup) => {
                let Some(name) = self.setups.name_of(id) else {
                    return self.drop_off(setup);
                };
                let mut entry = self.setups.entries.remove(&name);
                if let Some(entry) = &entry {
                    self.hiding.end(&entry.errand);
                }
                let mut hidden: Vec<(String, Token)> = entry
                    .iter()
                    .flat_map(|entry| entry.slots.iter().filter_map(hidden_by))
                    .collect();
                if let Some(taking) = entry.as_mut().and_then(|entry| entry.taking.take()) {
                    // Stopped before the pads a change brings were open,
                    // or with the report of the graph lost.
                    self.hidden_for(&taking, &mut hidden);
                    let refusal = WireError::refusal(
                        ErrorCode::SetupBusy,
                        format!("the setup {name} stopped; the change wasn't made"),
                    );
                    taking.responder.err(&refusal).await;
                }
                self.release(&name).await;
                // The stop is answered once the virtual pads are gone.
                self.retire(
                    (name.clone(), id),
                    setup,
                    hidden,
                    entry.and_then(|entry| entry.stopping),
                );
                self.publish_setup(&name).await;
                info!(setup = name, "a setup stopped");
            }
            Report::Failed(id, setup) => {
                let Some(name) = self.setups.name_of(id) else {
                    return self.drop_off(setup);
                };
                if let Some(entry) = self.setups.entries.get(&name) {
                    // A pad being taken back, or for a change, is given up.
                    self.hiding.end(&entry.errand);
                }
                let mut hidden: Vec<(String, Token)> = self
                    .setups
                    .entries
                    .get_mut(&name)
                    .map(|entry| {
                        let hidden = entry.slots.iter().filter_map(hidden_by).collect();
                        for slot in &mut entry.slots {
                            slot.token = None;
                        }
                        hidden
                    })
                    .unwrap_or_default();
                let stopping = self.setups.entries.get_mut(&name).and_then(|entry| {
                    entry.phase = Phase::Failed;
                    entry.reason = Some(
                        "the input thread gave it up: a virtual pad refused a write, or handling it failed"
                            .to_string(),
                    );
                    for slot in &mut entry.slots {
                        slot.present = false;
                    }
                    for sink in &mut entry.sinks {
                        sink.node = None;
                    }
                    entry.stopping.take()
                });
                let taking = self
                    .setups
                    .entries
                    .get_mut(&name)
                    .and_then(|entry| entry.taking.take());
                if let Some(taking) = taking {
                    // The pads a change brought are shown again with the
                    // rest; the graph that holds them follows, refused.
                    self.hidden_for(&taking, &mut hidden);
                    let refusal = WireError::refusal(
                        ErrorCode::SetupFailed,
                        "the setup failed as it took the change",
                    );
                    taking.responder.err(&refusal).await;
                }
                self.release(&name).await;
                warn!(setup = name, "a setup failed");
                if stopping.is_some() {
                    // It was asked to stop, and it has.
                    self.setups.entries.remove(&name);
                } else if let Some(entry) = self.setups.entries.get_mut(&name) {
                    // Stopped or started afresh once its pads are shown
                    // again.
                    entry.retiring = true;
                }
                self.publish_setup(&name).await;
                self.retire((name, id), setup, hidden, stopping);
            }
            Report::PadGone { setup, source, pad } => {
                self.drop_off(pad);
                let Some(name) = self.setups.name_of(setup) else {
                    return;
                };
                if let Some(entry) = self.setups.entries.get_mut(&name)
                    && let Some(slot) = entry.slots.get_mut(source.0)
                {
                    slot.present = false;
                    entry.settle();
                    info!(
                        setup = name,
                        source = source.0,
                        "a setup lost a pad and is held"
                    );
                }
                self.publish_setup(&name).await;
                // It may be back already: its arrival can come before
                // the input thread notices it gone.
                self.bring_back(&name, source.0);
            }
            Report::Selected { setup, node, index } => {
                let Some(name) = self.setups.name_of(setup) else {
                    return;
                };
                if let Some(entry) = self.setups.entries.get_mut(&name)
                    && let Some(selected) = entry.selected.get_mut(node.0)
                {
                    *selected = Some(index);
                }
                self.publish_setup(&name).await;
            }
            Report::GraphTaken { setup, old } => self.took(setup, old).await,
            // Asked for only when this is being logged.
            Report::Tapped { setup, tap } => {
                let name = self.setups.name_of(setup).unwrap_or_default();
                match tap {
                    ctrlassist_device::Tap::Output { sink, report } => {
                        let hex = report.iter().fold(String::new(), |mut text, byte| {
                            use std::fmt::Write as _;
                            let _ = write!(text, "{byte:02x}");
                            text
                        });
                        tracing::debug!(setup = name, sink = sink.0, report = hex, "feedback");
                    }
                    tap => tracing::debug!(setup = name, ?tap, "feedback"),
                }
            }
            Report::GraphRefused { setup, graph } => {
                // Only a setup the input thread no longer runs refuses
                // one, and its own report says what became of it.
                let name = self.setups.name_of(setup).unwrap_or_default();
                warn!(setup = name, "the input thread refused a new graph");
                let refusal = WireError::internal("the input thread refused the new graph");
                self.abandon(&name, graph, Vec::new(), &refusal).await;
            }
            Report::PadRefused { setup, source, pad } => {
                self.drop_off(pad);
                let Some(name) = self.setups.name_of(setup) else {
                    return;
                };
                if let Some(entry) = self.setups.entries.get_mut(&name)
                    && let Some(slot) = entry.slots.get_mut(source.0)
                {
                    // The input thread still holds a pad there, or the
                    // setup has gone; either way this one wasn't taken.
                    warn!(
                        setup = name,
                        source = source.0,
                        "the input thread refused a returning pad"
                    );
                    slot.present = entry.phase != Phase::Failed;
                    if entry.phase != Phase::Failed {
                        entry.settle();
                    }
                }
                self.publish_setup(&name).await;
            }
            // A report this release doesn't know says nothing it can act
            // on.
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    //! What a setup does when the input thread or a client does something
    //! no test can provoke from outside: the reactor is handed the report
    //! or the request directly, and asked through a real session.

    use std::time::Duration;

    use ctrlassist_device::InputThread;
    use ctrlassist_protocol::bootstrap;
    use ctrlassist_protocol::caller::{Caller, RpcError};
    use tempfile::TempDir;
    use zenoh::Session;
    use zenoh::handlers::FifoChannelHandler;
    use zenoh::query::Queryable;

    use super::*;
    use crate::book::Book;
    use crate::hotplug::Seen;
    use crate::profiles::Profiles;
    use crate::store::Store;

    const PROFILE: &str =
        "mode = \"priority\"\nprimary = \"0a:1b:2c:3d:4e:14\"\nassist = \"model:054c:09cc\"\n";

    struct Rig {
        _dir: TempDir,
        reactor: Reactor,
        requests: Queryable<FifoChannelHandler<Query>>,
        client: Session,
        // Held so the input thread's reports have somewhere to go.
        _reports: std::sync::mpsc::Receiver<Report>,
    }

    async fn rig() -> Rig {
        let dir = tempfile::Builder::new()
            .prefix("ca")
            .tempdir_in("/tmp")
            .unwrap();
        let socket = dir.path().join("zenoh.sock");
        let session = zenoh::open(bootstrap::router_config(&socket).unwrap())
            .await
            .unwrap();
        let requests = session
            .declare_queryable(setup::Incoming::wild())
            .await
            .unwrap();
        let (input, reports) = InputThread::spawn().unwrap();
        let (store, known) = Store::open(&dir.path().join("state"));
        let mut profiles = Profiles::open(&dir.path().join("profiles"));
        profiles.put("couch", PROFILE).unwrap();
        let reactor = Reactor::new(
            session,
            Book::new(known),
            store,
            profiles,
            Arc::new(input),
            Database::parse(""),
            Hiding::grab(),
            crate::copied::Copied::open(&dir.path().join("state")),
        );
        let mut reactor = reactor;
        reactor.test_pads = true;
        let client = zenoh::open(bootstrap::client_config(&socket).unwrap())
            .await
            .unwrap();
        Rig {
            _dir: dir,
            reactor,
            requests,
            client,
            _reports: reports,
        }
    }

    /// A setup nothing runs, as the input thread would hand one back.
    fn handed_back() -> Box<Setup> {
        let plan = Profile::check(PROFILE).unwrap();
        let graph = Graph::build(plan.graph).unwrap();
        Box::new(Setup::new(graph, vec![None, None], Vec::new()))
    }

    /// Follows `couch` as a setup the input thread was given as `id`.
    fn follow(reactor: &mut Reactor, id: u32, phase: Phase) {
        let plan = Profile::check(PROFILE).unwrap();
        let slots = reactor.bind("couch", &plan).unwrap();
        let entry = Entry {
            id: SetupId(id),
            phase,
            reason: None,
            slots,
            sinks: vec![payload::SetupSink {
                name: "virtual".to_string(),
                identity: "generic".to_string(),
                node: Some("/dev/input/event99".to_string()),
            }],
            selected: vec![None; plan.nodes.len()],
            nodes: choosers(&plan),
            stopping: None,
            taking: None,
            toml: String::new(),
            stale: false,
            hide: plan.hide,
            arrivals: Vec::new(),
            retiring: false,
            errand: Arc::default(),
        };
        reactor.setups.entries.insert("couch".to_string(), entry);
    }

    /// Lets the work the reactor started off itself come back to it, as
    /// its loop would.
    async fn finish(reactor: &mut Reactor) {
        while let Some(done) = reactor.work.join_next().await {
            reactor.done(done.unwrap()).await;
        }
    }

    type Asked<R> = tokio::task::JoinHandle<Result<R, RpcError>>;

    /// Asks `request` of the reactor as a client would, and hands the
    /// reactor the request. The answer comes when the reactor replies.
    async fn ask(rig: &mut Rig, stop: bool) -> Asked<rpc::Ack> {
        let caller = Caller::new(rig.client.clone());
        let request = rpc::SetupRequest {
            name: "couch".to_string(),
        };
        let asked = tokio::spawn(async move {
            if stop {
                caller.call::<setup::Stop>(&(), &request).await
            } else {
                caller.call::<setup::Start>(&(), &request).await
            }
        });
        let query = tokio::time::timeout(Duration::from_secs(20), rig.requests.recv_async())
            .await
            .unwrap()
            .unwrap();
        rig.reactor.setup_rpc(query).await;
        asked
    }

    fn refusal(outcome: Result<rpc::Ack, RpcError>) -> WireError {
        match outcome {
            Err(RpcError::Service(refusal)) => refusal,
            other => unreachable!("expected a refusal, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_setup_the_input_thread_gives_up_is_failed_until_stopped() {
        let mut rig = rig().await;
        follow(&mut rig.reactor, 7, Phase::Running);
        assert_eq!(
            rig.reactor.setups.name_of(SetupId(7)).as_deref(),
            Some("couch")
        );

        // A report for a setup the reactor doesn't follow changes nothing.
        rig.reactor
            .reported(Report::Failed(SetupId(8), handed_back()))
            .await;
        rig.reactor
            .reported(Report::Stopped(SetupId(8), handed_back()))
            .await;
        assert_eq!(
            rig.reactor.setups.setup("couch").unwrap().phase,
            Phase::Running
        );

        rig.reactor
            .reported(Report::Failed(SetupId(7), handed_back()))
            .await;
        let failed = rig.reactor.setups.setup("couch").unwrap();
        assert_eq!(failed.phase, Phase::Failed);
        assert!(failed.reason.unwrap().contains("gave it up"));
        assert_eq!(failed.sinks[0].node, None, "its virtual pads are gone");
        assert!(failed.sources.iter().all(|source| !source.present));
        assert_eq!(rig.reactor.book.user("address:0a:1b:2c:3d:4e:14"), None);
        finish(&mut rig.reactor).await;

        // It can't be started over until it is stopped, which only
        // clears it away.
        let again = ask(&mut rig, false).await;
        assert_eq!(refusal(again.await.unwrap()).code, ErrorCode::SetupExists);
        let stopped = ask(&mut rig, true).await;
        stopped.await.unwrap().unwrap();
        assert!(rig.reactor.setups.setup("couch").is_none());
        let gone = ask(&mut rig, true).await;
        assert_eq!(refusal(gone.await.unwrap()).code, ErrorCode::UnknownSetup);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_stop_is_answered_when_the_input_thread_hands_the_setup_back() {
        let mut rig = rig().await;
        follow(&mut rig.reactor, 7, Phase::Held);
        // The input thread runs no such setup, so it reports nothing, and
        // the stop waits.
        let stopping = ask(&mut rig, true).await;
        assert!(rig.reactor.setups.entries["couch"].stopping.is_some());
        // A second request has to wait for the first.
        let second = ask(&mut rig, true).await;
        assert_eq!(refusal(second.await.unwrap()).code, ErrorCode::SetupBusy);
        // A pad that turns up meanwhile isn't brought into it.
        rig.reactor.bring_back("couch", 0);
        assert!(!rig.reactor.setups.entries["couch"].slots[0].returning);

        rig.reactor
            .reported(Report::Stopped(SetupId(7), handed_back()))
            .await;
        finish(&mut rig.reactor).await;
        stopping.await.unwrap().unwrap();
        assert!(rig.reactor.setups.setup("couch").is_none());

        // One that fails while stopping has stopped all the same.
        follow(&mut rig.reactor, 9, Phase::Running);
        let stopping = ask(&mut rig, true).await;
        rig.reactor
            .reported(Report::Failed(SetupId(9), handed_back()))
            .await;
        finish(&mut rig.reactor).await;
        stopping.await.unwrap().unwrap();
        assert!(rig.reactor.setups.setup("couch").is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_setup_that_is_starting_selects_nothing_and_takes_no_pad_yet() {
        let mut rig = rig().await;
        follow(&mut rig.reactor, 7, Phase::Starting);
        // No node of it selects yet.
        let caller = Caller::new(rig.client.clone());
        let asked = tokio::spawn(async move {
            let request = rpc::SelectRequest {
                name: "couch".to_string(),
                node: "mux".to_string(),
                choice: None,
            };
            caller.call::<setup::Select>(&(), &request).await
        });
        let query = rig.requests.recv_async().await.unwrap();
        rig.reactor.setup_rpc(query).await;
        let refused = refusal(asked.await.unwrap());
        assert_eq!(refused.code, ErrorCode::SetupBusy);
        assert!(refused.message.contains("isn't running"));
        // A selection reported for a setup that has gone is nothing.
        rig.reactor
            .reported(Report::Selected {
                setup: SetupId(99),
                node: NodeId(0),
                index: 1,
            })
            .await;
        // Nor is a pad brought into it before it runs.
        rig.reactor.bring_back("couch", 0);
        rig.reactor.bring_back("couch", 5);
        rig.reactor.bring_back("nothing", 0);
        assert!(!rig.reactor.setups.entries["couch"].slots[0].returning);
    }

    /// A stop asked while a setup starts ends what the start asks of the
    /// hiding service, which may never answer, and is answered once the
    /// start's work has come back and what it took is let go and shown
    /// again; the start is refused, and nothing it made runs.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_setup_stopped_as_it_starts_ends_its_start() {
        let mut rig = rig().await;
        follow(&mut rig.reactor, 7, Phase::Starting);
        let errand = Arc::clone(&rig.reactor.setups.entries["couch"].errand);
        let caller = Caller::new(rig.client.clone());
        let starting = tokio::spawn(async move {
            let request = rpc::SetupRequest {
                name: "couch".to_string(),
            };
            caller.call::<setup::Start>(&(), &request).await
        });
        let query = rig.requests.recv_async().await.unwrap();
        let Some(((), setup::Incoming::Start(_, responder))) =
            setup::Incoming::receive(query).await
        else {
            unreachable!("the request is a start")
        };

        let stopping = ask(&mut rig, true).await;
        assert!(errand.has_ended(), "what it asks of the service ends");
        assert!(rig.reactor.setups.entries["couch"].stopping.is_some());
        let second = ask(&mut rig, true).await;
        assert_eq!(refusal(second.await.unwrap()).code, ErrorCode::SetupBusy);
        assert!(!stopping.is_finished(), "answered once the start is back");

        // The start's work comes back, having made all it was to, as a
        // start the stop came too late to end has.
        let asker = rig.reactor.owe(Owed::Start(Asker::Client(responder)));
        rig.reactor
            .done(Done::Started {
                name: "couch".to_string(),
                outcome: made_without_pads(),
                asker,
            })
            .await;
        let refused = refusal(starting.await.unwrap());
        assert_eq!(refused.code, ErrorCode::SetupFailed);
        assert_eq!(refused.message, STOPPED);
        assert!(rig.reactor.setups.setup("couch").is_none());
        assert_eq!(rig.reactor.book.user(PRIMARY), None);
        finish(&mut rig.reactor).await;
        stopping.await.unwrap().unwrap();
    }

    /// As the daemon stops, every rpc the reactor holds or has queued is
    /// answered that it is stopping, and nothing is waited on: not the
    /// work of a start held up by a hiding service that never answers.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_daemon_that_stops_answers_what_it_holds_that_it_is_stopping() {
        use rustix::net::{AddressFamily, SocketAddrUnix, SocketType, bind, listen, socket};

        let mut rig = rig().await;
        // A service that takes connections and never answers.
        let path = rig._dir.path().join("hider.sock");
        let listener = socket(AddressFamily::UNIX, SocketType::SEQPACKET, None).unwrap();
        bind(&listener, &SocketAddrUnix::new(&path).unwrap()).unwrap();
        listen(&listener, 16).unwrap();
        rig.reactor.hiding = Arc::new(Hiding::new(&path));
        rig.reactor.look_for_the_hiding_service();
        follow(&mut rig.reactor, 7, Phase::Starting);

        // A start whose work waits on the service.
        let caller = Caller::new(rig.client.clone());
        let starting = tokio::spawn(async move {
            let request = rpc::SetupRequest {
                name: "couch".to_string(),
            };
            caller.call::<setup::Start>(&(), &request).await
        });
        let query = rig.requests.recv_async().await.unwrap();
        let Some(((), setup::Incoming::Start(_, responder))) =
            setup::Incoming::receive(query).await
        else {
            unreachable!("the request is a start")
        };
        let asker = rig.reactor.owe(Owed::Start(Asker::Client(responder)));
        let (hiding, errand) = (
            Arc::clone(&rig.reactor.hiding),
            Arc::clone(&rig.reactor.setups.entries["couch"].errand),
        );
        rig.reactor.work.spawn_blocking(move || {
            // Given up, and so never read: what it says is nothing.
            let pad = File::open("/dev/null").unwrap();
            drop(hiding.hide(&errand, PRIMARY, pad.as_fd()));
            Done::Started {
                name: "couch".to_string(),
                outcome: Err(Unmade::new(STOPPED)),
                asker,
            }
        });

        // And a stop the reactor has yet to read.
        let caller = Caller::new(rig.client.clone());
        let stopping = tokio::spawn(async move {
            let request = rpc::SetupRequest {
                name: "couch".to_string(),
            };
            caller.call::<setup::Stop>(&(), &request).await
        });
        while rig.requests.is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let requests = &rig.requests;
        rig.reactor.wind_down(&[requests]).await;
        for asked in [starting, stopping] {
            let refused = refusal(asked.await.unwrap());
            assert_eq!(refused.code, ErrorCode::DaemonShuttingDown);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_start_whose_making_fails_is_refused_with_why_and_leaves_nothing() {
        let mut rig = rig().await;
        follow(&mut rig.reactor, 7, Phase::Starting);
        assert_eq!(
            rig.reactor.book.user("address:0a:1b:2c:3d:4e:14"),
            None,
            "a pad never seen has nothing to note"
        );
        let caller = Caller::new(rig.client.clone());
        let asked = tokio::spawn(async move {
            let request = rpc::SetupRequest {
                name: "couch".to_string(),
            };
            caller.call::<setup::Start>(&(), &request).await
        });
        let query = rig.requests.recv_async().await.unwrap();
        let Some(((), setup::Incoming::Start(_, responder))) =
            setup::Incoming::receive(query).await
        else {
            unreachable!("the request is a start")
        };
        let asker = rig.reactor.owe(Owed::Start(Asker::Client(responder)));
        rig.reactor
            .done(Done::Started {
                name: "couch".to_string(),
                outcome: Err(Unmade::new(
                    "making the virtual pad `virtual` (generic): no uinput",
                )),
                asker,
            })
            .await;
        let refused = refusal(asked.await.unwrap());
        assert_eq!(refused.code, ErrorCode::SetupFailed);
        assert!(refused.message.contains("no uinput"));
        assert!(rig.reactor.setups.setup("couch").is_none());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_pad_that_could_not_be_opened_again_is_waited_for_still() {
        let mut rig = rig().await;
        follow(&mut rig.reactor, 7, Phase::Held);
        // The sources go by name: the assist, then the primary.
        rig.reactor.setups.entries.get_mut("couch").unwrap().slots[1].returning = true;
        rig.reactor
            .done(Done::Returned {
                name: "couch".to_string(),
                setup: SetupId(7),
                id: "address:0a:1b:2c:3d:4e:14".to_string(),
                pad: None,
                token: None,
                arrival: None,
            })
            .await;
        let slot = &rig.reactor.setups.entries["couch"].slots[1];
        assert!(!slot.returning && !slot.present);
        // For a setup that went meanwhile, there is nothing to do.
        rig.reactor
            .done(Done::Returned {
                name: "gone".to_string(),
                setup: SetupId(7),
                id: "address:0a:1b:2c:3d:4e:14".to_string(),
                pad: None,
                token: None,
                arrival: None,
            })
            .await;
        rig.reactor.done(Done::Dropped).await;
        assert_eq!(
            rig.reactor.setups.setup("couch").unwrap().phase,
            Phase::Held
        );
    }

    /// A pad as the input thread hands one back, where pads can be made.
    fn a_pad(test: &str, name: &str) -> Option<(ctrlassist_test::FakePad, Box<Pad>)> {
        if !ctrlassist_test::can_make_pads(test) {
            return None;
        }
        let fake = ctrlassist_test::FakePad::new(name, 0x1209, 0x8301).unwrap();
        let pad = open(fake.event_node())
            .and_then(|fd| read(fd, fake.event_node(), &Database::parse("")))
            .unwrap();
        Some((fake, Box::new(pad)))
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_pad_the_input_thread_hands_back_is_let_go_whatever_became_of_its_setup() {
        const TEST: &str =
            "a_pad_the_input_thread_hands_back_is_let_go_whatever_became_of_its_setup";
        let mut rig = rig().await;
        follow(&mut rig.reactor, 7, Phase::Running);
        for slot in &mut rig.reactor.setups.entries.get_mut("couch").unwrap().slots {
            slot.present = true;
        }
        let Some((_fake, pad)) = a_pad(TEST, "Daemon Gone Pad") else {
            return;
        };
        // Gone from a setup the reactor follows: the setup is held.
        rig.reactor
            .reported(Report::PadGone {
                setup: SetupId(7),
                source: SourceId(0),
                pad,
            })
            .await;
        let held = rig.reactor.setups.setup("couch").unwrap();
        assert_eq!(held.phase, Phase::Held);
        assert!(!held.sources[0].present && held.sources[1].present);

        // Refused on its way back: the input thread has a pad there
        // still, so the place counts as filled.
        let (_fake, pad) = a_pad(TEST, "Daemon Refused Pad").unwrap();
        rig.reactor
            .reported(Report::PadRefused {
                setup: SetupId(7),
                source: SourceId(0),
                pad,
            })
            .await;
        assert_eq!(
            rig.reactor.setups.setup("couch").unwrap().phase,
            Phase::Running
        );

        // From a setup that has gone, either is only a pad to let go.
        for refused in [false, true] {
            let (_fake, pad) = a_pad(TEST, "Daemon Late Pad").unwrap();
            let (setup, source) = (SetupId(99), SourceId(0));
            rig.reactor
                .reported(if refused {
                    Report::PadRefused { setup, source, pad }
                } else {
                    Report::PadGone { setup, source, pad }
                })
                .await;
        }
        finish(&mut rig.reactor).await;
        assert_eq!(
            rig.reactor.setups.setup("couch").unwrap().phase,
            Phase::Running
        );
    }

    /// Asks for an update of `couch` as a client would.
    async fn ask_update(rig: &mut Rig, apply: bool) -> Asked<rpc::Updated> {
        let caller = Caller::new(rig.client.clone());
        let asked = tokio::spawn(async move {
            let request = rpc::UpdateRequest {
                name: "couch".to_string(),
                apply,
            };
            caller.call::<setup::Update>(&(), &request).await
        });
        let query = rig.requests.recv_async().await.unwrap();
        rig.reactor.setup_rpc(query).await;
        asked
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_update_waits_its_turn_and_a_buzz_needs_the_pad_there() {
        let mut rig = rig().await;
        follow(&mut rig.reactor, 7, Phase::Starting);
        // What an update costs can be asked at any time; doing it can't.
        // Asked, it says the start the setup is.
        let cost = ask_update(&mut rig, false).await.await.unwrap().unwrap();
        assert!(cost.disruptive);
        assert_eq!(cost.start, 7);
        let busy = ask_update(&mut rig, true).await.await.unwrap();
        assert!(matches!(
            busy,
            Err(RpcError::Service(refusal)) if refusal.code == ErrorCode::SetupBusy
        ));

        // Running, an update to another identity waits for the input
        // thread to hand the setup back, and a second one for the first.
        rig.reactor.setups.entries.get_mut("couch").unwrap().phase = Phase::Running;
        rig.reactor
            .profiles
            .put("couch", &format!("{PROFILE}identity = \"xbox\"\n"))
            .unwrap();
        assert!(
            ask_update(&mut rig, false)
                .await
                .await
                .unwrap()
                .unwrap()
                .disruptive
        );
        let updating = ask_update(&mut rig, true).await;
        assert!(matches!(
            rig.reactor.setups.entries["couch"].stopping,
            Some(Waiting::Update(_))
        ));
        let second = ask_update(&mut rig, true).await.await.unwrap();
        assert!(matches!(
            second,
            Err(RpcError::Service(refusal)) if refusal.code == ErrorCode::SetupBusy
        ));
        drop(updating);

        // A buzz reaches a pad its setup holds, and no other.
        let pad = "address:0a:1b:2c:3d:4e:14";
        let level = Level { strong: 1, weak: 1 };
        let absent = rig.reactor.buzz("couch", pad, level, 100).unwrap_err();
        assert_eq!(absent.code, ErrorCode::PadDisconnected);
        for slot in &mut rig.reactor.setups.entries.get_mut("couch").unwrap().slots {
            slot.present = true;
        }
        rig.reactor.buzz("couch", pad, level, 100).unwrap();
        assert!(
            rig.reactor
                .buzz("couch", "address:other", level, 100)
                .is_err()
        );
        assert!(rig.reactor.buzz("nothing", pad, level, 100).is_err());
    }

    /// What a node selects between is said in the profile's own words: a
    /// source or a mux by its name, a demux's output by name and number.
    #[test]
    fn a_node_s_choices_are_named_as_the_profile_names_them() {
        let full = include_str!("../../ctrlassist-protocol/tests/it/fixtures/full.toml");
        let named = choosers(&Profile::check(full).unwrap());
        let choices = |node: &str| -> Vec<&str> {
            let (_, choices) = named.iter().find(|(name, _)| name == node).unwrap();
            choices.iter().map(String::as_str).collect()
        };
        assert_eq!(choices("first"), ["one", "share.0"]);
        assert_eq!(choices("second"), ["two", "share.1"]);
        assert_eq!(choices("share"), ["0", "1"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_update_of_a_failed_setup_starts_it_afresh() {
        if !ctrlassist_test::can_make_pads("an_update_of_a_failed_setup_starts_it_afresh") {
            return;
        }
        let mut rig = rig().await;
        follow(&mut rig.reactor, 7, Phase::Failed);
        let updating = ask_update(&mut rig, true).await;
        // Its pads aren't connected, so it starts held, on a setup of its
        // own making this time.
        finish(&mut rig.reactor).await;
        let updated = updating.await.unwrap().unwrap();
        assert!(updated.disruptive);
        let started = rig.reactor.setups.setup("couch").unwrap();
        assert_eq!(started.phase, Phase::Held);
        assert_ne!(started.start, 7, "started afresh, it is another start");
        assert_eq!(updated.start, started.start, "the answer names it");
        assert!(
            started.sinks[0]
                .node
                .as_deref()
                .is_some_and(|node| node != "/dev/input/event99")
        );
    }

    /// A profile of the same pads and virtual pad as [`PROFILE`], as a
    /// toggle.
    const TOGGLE: &str =
        "mode = \"toggle\"\nprimary = \"0a:1b:2c:3d:4e:14\"\nassist = \"model:054c:09cc\"\n";

    /// A graph as the input thread hands one back.
    fn graph_of(profile: &str) -> Box<NewGraph> {
        let graph = Graph::build(Profile::check(profile).unwrap().graph).unwrap();
        Box::new(NewGraph::new(graph, Vec::new(), Vec::new()))
    }

    /// How a setup's pads are kept from games is how they were taken,
    /// so a profile that asks for another way costs a start afresh,
    /// though its pads and its virtual pad are the same.
    #[tokio::test(flavor = "multi_thread")]
    async fn another_way_of_hiding_is_another_start() {
        let mut rig = rig().await;
        follow(&mut rig.reactor, 7, Phase::Held);
        assert_eq!(rig.reactor.setups.entries["couch"].hide, Hide::Full);
        let said_again = format!("{PROFILE}hide = \"full\"\n");
        rig.reactor.profiles.put("couch", &said_again).unwrap();
        let cost = ask_update(&mut rig, false).await.await.unwrap().unwrap();
        assert!(!cost.disruptive, "the same way, said outright");
        let grabbed = format!("{PROFILE}hide = \"grab\"\n");
        rig.reactor.profiles.put("couch", &grabbed).unwrap();
        let cost = ask_update(&mut rig, false).await.await.unwrap().unwrap();
        assert!(cost.disruptive, "its pads would be taken another way");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_profile_of_the_same_virtual_pads_is_taken_as_the_setup_runs() {
        let mut rig = rig().await;
        follow(&mut rig.reactor, 7, Phase::Held);
        // Another mode, and another pad as the primary.
        let other = TOGGLE.replace("0a:1b:2c:3d:4e:14", "0a:1b:2c:3d:4e:15");
        rig.reactor.profiles.put("couch", &other).unwrap();
        let cost = ask_update(&mut rig, false).await.await.unwrap().unwrap();
        assert!(!cost.disruptive, "its virtual pad is the same");
        assert!(rig.reactor.setups.entries["couch"].taking.is_none());

        let updating = ask_update(&mut rig, true).await;
        let entry = &rig.reactor.setups.entries["couch"];
        let taking = entry.taking.as_ref().unwrap();
        assert!(taking.sent && entry.stopping.is_none());
        // The sources go by name: the assist, then the primary.
        assert_eq!(taking.keep, [Some(0), None], "the assist's place is kept");
        // Until the input thread has the graph, the setup is as it was,
        // and nothing that goes by a pad's or a node's number is asked.
        let before = rig.reactor.setups.setup("couch").unwrap();
        assert_eq!(
            before.sources[1].pad.as_deref(),
            Some("address:0a:1b:2c:3d:4e:14")
        );
        let second = ask_update(&mut rig, true).await.await.unwrap();
        assert!(matches!(
            second,
            Err(RpcError::Service(refusal)) if refusal.code == ErrorCode::SetupBusy
        ));
        let request = rpc::SelectRequest {
            name: "couch".to_string(),
            node: "mux".to_string(),
            choice: None,
        };
        let busy = rig.reactor.select(&request).unwrap_err();
        assert!(busy.message.contains("taking a change"), "{busy:?}");
        let level = Level { strong: 1, weak: 1 };
        let pad = "address:0a:1b:2c:3d:4e:14";
        rig.reactor.setups.entries.get_mut("couch").unwrap().slots[1].present = true;
        assert!(rig.reactor.buzz("couch", pad, level, 100).is_err());
        rig.reactor.bring_back("couch", 0);
        assert!(!rig.reactor.setups.entries["couch"].slots[0].returning);
        // A pad sent for before the change, open only now, is let go.
        rig.reactor.setups.entries.get_mut("couch").unwrap().slots[1].present = false;
        rig.reactor
            .done(Done::Returned {
                name: "couch".to_string(),
                setup: SetupId(7),
                id: pad.to_string(),
                pad: None,
                token: None,
                arrival: None,
            })
            .await;

        rig.reactor
            .reported(Report::GraphTaken {
                setup: SetupId(7),
                old: graph_of(PROFILE),
            })
            .await;
        finish(&mut rig.reactor).await;
        let updated = updating.await.unwrap().unwrap();
        assert!(!updated.disruptive);
        assert_eq!(updated.start, 7, "the start that took it");
        rig.reactor
            .reported(Report::Selected {
                setup: SetupId(7),
                node: NodeId(0),
                index: 0,
            })
            .await;
        let taken = rig.reactor.setups.setup("couch").unwrap();
        assert_eq!(taken.phase, Phase::Held, "it never stopped");
        assert_eq!(
            taken.start, 7,
            "a change taken as it runs is the same start"
        );
        assert_eq!(
            taken.sources[1].pad.as_deref(),
            Some("address:0a:1b:2c:3d:4e:15")
        );
        assert_eq!(taken.nodes[0].choices, ["primary", "assist"]);
        assert_eq!(taken.nodes[0].selected, 0);
        assert_eq!(
            taken.sinks[0].node.as_deref(),
            Some("/dev/input/event99"),
            "its virtual pad is the one it had"
        );
        // A pad it gave up that turns up open is let go.
        rig.reactor
            .done(Done::Returned {
                name: "couch".to_string(),
                setup: SetupId(7),
                id: pad.to_string(),
                pad: None,
                token: None,
                arrival: None,
            })
            .await;
        // A report of a graph nobody waits for is only a graph to drop.
        rig.reactor
            .reported(Report::GraphTaken {
                setup: SetupId(7),
                old: graph_of(PROFILE),
            })
            .await;
        finish(&mut rig.reactor).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_update_under_way_is_answered_whatever_becomes_of_the_setup() {
        let refused = |outcome: Result<rpc::Updated, RpcError>, code| {
            assert!(matches!(
                outcome,
                Err(RpcError::Service(refusal)) if refusal.code == code
            ));
        };
        let mut rig = rig().await;
        rig.reactor.profiles.put("couch", TOGGLE).unwrap();

        // Refused by the input thread, which runs no such setup.
        follow(&mut rig.reactor, 7, Phase::Running);
        let updating = ask_update(&mut rig, true).await;
        rig.reactor
            .reported(Report::GraphRefused {
                setup: SetupId(7),
                graph: graph_of(TOGGLE),
            })
            .await;
        refused(updating.await.unwrap(), ErrorCode::Internal);

        // Failed as it took it.
        let updating = ask_update(&mut rig, true).await;
        rig.reactor
            .reported(Report::Failed(SetupId(7), handed_back()))
            .await;
        refused(updating.await.unwrap(), ErrorCode::SetupFailed);
        finish(&mut rig.reactor).await;
        rig.reactor.setups.entries.remove("couch");

        // Stopped with the report of the graph lost.
        follow(&mut rig.reactor, 8, Phase::Running);
        let updating = ask_update(&mut rig, true).await;
        rig.reactor
            .reported(Report::Stopped(SetupId(8), handed_back()))
            .await;
        refused(updating.await.unwrap(), ErrorCode::SetupBusy);
        finish(&mut rig.reactor).await;

        // Asked to stop while the pads a change brings were being opened:
        // the graph isn't sent, and the pads are let go.
        follow(&mut rig.reactor, 9, Phase::Running);
        let updating = ask_update(&mut rig, true).await;
        let entry = rig.reactor.setups.entries.get_mut("couch").unwrap();
        entry.taking.as_mut().unwrap().sent = false;
        let stopping = ask(&mut rig, true).await;
        let graph = Graph::build(Profile::check(TOGGLE).unwrap().graph).unwrap();
        rig.reactor
            .done(Done::Retaken {
                name: "couch".to_string(),
                graph: Box::new(graph),
                ids: vec![None, None],
                pads: vec![None, None],
                arrivals: vec![None, None],
            })
            .await;
        refused(updating.await.unwrap(), ErrorCode::SetupBusy);
        rig.reactor
            .reported(Report::Stopped(SetupId(9), handed_back()))
            .await;
        finish(&mut rig.reactor).await;
        stopping.await.unwrap().unwrap();

        // For a setup that has gone, either report is only a graph to drop.
        rig.reactor
            .reported(Report::GraphRefused {
                setup: SetupId(99),
                graph: graph_of(TOGGLE),
            })
            .await;
        rig.reactor
            .reported(Report::GraphTaken {
                setup: SetupId(99),
                old: graph_of(TOGGLE),
            })
            .await;
        finish(&mut rig.reactor).await;
        assert!(rig.reactor.setups.setup("couch").is_none());
    }

    /// A pad stays with the setup wherever the new profile has a use for
    /// it: under its source's name, under another name, or as a pad
    /// another source could mean.
    #[test]
    fn a_change_keeps_each_pad_the_new_profile_has_a_use_for() {
        let slot = |name: &str, wanted: &str, pad: Option<&str>| Slot {
            present: pad.is_some(),
            ..Slot::new(
                name.to_string(),
                PadRef::parse(wanted).unwrap(),
                pad.map(str::to_string),
            )
        };
        let a = "address:0a:1b:2c:3d:4e:14";
        let b = "address:0a:1b:2c:3d:4e:15";
        let has = [slot("primary", a, Some(a)), slot("assist", b, Some(b))];
        let book = Book::new(Vec::new());
        let plan = |primary: &str, assist: &str| {
            let toml =
                format!("mode = \"priority\"\nprimary = \"{primary}\"\nassist = \"{assist}\"\n");
            Profile::check(&toml).unwrap()
        };
        let bound = |plan: &Plan| {
            let (slots, keep) = rebind(&book, "couch", &has, plan).unwrap();
            let pads: Vec<Option<String>> = slots.into_iter().map(|slot| slot.pad).collect();
            (pads, keep)
        };
        // The preset's sources are the assist, then the primary, by name.
        let same = bound(&plan(a, b));
        assert_eq!(same.1, [Some(1), Some(0)]);
        // The two change parts: each pad is kept, under the other name.
        let swapped = bound(&plan(b, a));
        assert_eq!(swapped.1, [Some(0), Some(1)]);
        assert_eq!(swapped.0, [Some(a.to_string()), Some(b.to_string())]);
        // Another pad as the assist: the primary's is kept, the assist's
        // given up, and the new one named though it was never seen.
        let c = "address:0a:1b:2c:3d:4e:16";
        let other = bound(&plan(a, c));
        assert_eq!(other.1, [None, Some(0)]);
        assert_eq!(other.0, [Some(c.to_string()), Some(a.to_string())]);
        // One pad as both is no profile at all.
        let both = format!("mode = \"priority\"\nprimary = \"{c}\"\nassist = \"{c}\"\n");
        let refused = Profile::check(&both).unwrap_err();
        assert_eq!(refused.at, "source.primary.pad");
    }

    /// A node carries on from the one of its name, and what it selected
    /// is found among its choices by what the profile calls it.
    #[test]
    fn a_node_carries_on_from_the_one_of_its_name() {
        let names = |names: &[&str]| names.iter().map(ToString::to_string).collect::<Vec<_>>();
        let old = vec![
            ("mux".to_string(), names(&["primary", "assist"])),
            ("gone".to_string(), names(&["0", "1"])),
        ];
        let new = vec![
            ("added".to_string(), names(&["0", "1"])),
            ("mux".to_string(), names(&["assist", "third", "primary"])),
        ];
        assert_eq!(
            carry(&old, &new),
            [None, Some((NodeId(0), vec![Some(2), Some(0)]))]
        );
    }

    /// [`PROFILE`]'s primary.
    const PRIMARY: &str = "address:0a:1b:2c:3d:4e:14";
    /// A pad of the model [`PROFILE`]'s assist wants.
    const ASSIST: &str = "address:0a:1b:2c:3d:4e:20";

    /// One of those two connecting on `node`, which nothing can open:
    /// none of these tests makes a pad.
    fn connecting(id: &str, node: &str) -> Found {
        let address = id.strip_prefix("address:").unwrap();
        Found {
            node: PathBuf::from(node),
            identifiers: ctrlassist_device::Identifiers {
                name: "Wireless Controller".to_string(),
                bus: 3,
                vendor: 0x054c,
                product: if id == ASSIST { 0x09cc } else { 0x0ce6 },
                version: 1,
                address: Some(address.to_string()),
                serial: None,
                port: None,
            },
        }
    }

    /// A start's making, done with no pads opened and no virtual pads.
    fn made_without_pads() -> Result<Made, Unmade> {
        Ok(Made {
            setup: handed_back(),
            nodes: Vec::new(),
            pads: vec![(false, None); 2],
        })
    }

    /// A pad that connects while its setup starts has nowhere to go
    /// until the setup runs; then it is taken, whether the setup named
    /// it or it fits a place still without a pad.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_pad_that_connects_while_its_setup_starts_is_taken_once_it_has() {
        let mut rig = rig().await;
        follow(&mut rig.reactor, 7, Phase::Starting);
        for (id, node) in [(PRIMARY, "/nowhere/event1"), (ASSIST, "/nowhere/event2")] {
            rig.reactor.book.arrived(&connecting(id, node), None);
            rig.reactor.pad_arrived(id).await;
        }
        // The sources go by name: the assist, then the primary.
        let entry = &rig.reactor.setups.entries["couch"];
        assert!(entry.slots.iter().all(|slot| !slot.returning));
        assert_eq!(entry.slots[0].pad, None, "the assist's waits for the start");
        assert_eq!(rig.reactor.book.user(PRIMARY), Some("couch"), "named");
        assert_eq!(rig.reactor.book.user(ASSIST), None);

        let asker = rig.reactor.owe(Owed::Start(Asker::Daemon));
        rig.reactor
            .done(Done::Started {
                name: "couch".to_string(),
                outcome: made_without_pads(),
                asker,
            })
            .await;
        let entry = &rig.reactor.setups.entries["couch"];
        assert_eq!(entry.phase, Phase::Held);
        assert_eq!(entry.slots[0].pad.as_deref(), Some(ASSIST));
        assert_eq!(rig.reactor.book.user(ASSIST), Some("couch"));
        assert!(
            entry.slots.iter().all(|slot| slot.returning),
            "each is being opened"
        );
        // Nothing is there to open: each is waited for still.
        finish(&mut rig.reactor).await;
        let entry = &rig.reactor.setups.entries["couch"];
        assert!(
            entry
                .slots
                .iter()
                .all(|slot| !slot.returning && !slot.present)
        );
    }

    /// A pad asked for while it is being opened may have come back
    /// meanwhile: an opening that fails is tried again, once.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_pad_that_returns_while_it_is_opened_is_opened_again_if_that_fails() {
        let mut rig = rig().await;
        follow(&mut rig.reactor, 7, Phase::Held);
        let primary = connecting(PRIMARY, "/nowhere/event1");
        rig.reactor.book.arrived(&primary, None);
        rig.reactor.bring_back("couch", 1);
        rig.reactor.pad_arrived(PRIMARY).await;
        assert!(rig.reactor.setups.entries["couch"].slots[1].again);

        let first = rig.reactor.work.join_next().await.unwrap().unwrap();
        rig.reactor.done(first).await;
        let slot = &rig.reactor.setups.entries["couch"].slots[1];
        assert!(slot.returning && !slot.again, "sent for again");
        finish(&mut rig.reactor).await;
        let slot = &rig.reactor.setups.entries["couch"].slots[1];
        assert!(!slot.returning, "and not again after that");
    }

    /// A failed setup runs nothing: a pad that connects isn't taken into
    /// it, and nothing in the book is its own once it is stopped.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_setup_takes_no_pad_that_connects() {
        let mut rig = rig().await;
        follow(&mut rig.reactor, 7, Phase::Running);
        rig.reactor
            .reported(Report::Failed(SetupId(7), handed_back()))
            .await;
        for (id, node) in [(ASSIST, "/nowhere/event2"), (PRIMARY, "/nowhere/event1")] {
            rig.reactor.book.arrived(&connecting(id, node), None);
            rig.reactor.pad_arrived(id).await;
            assert_eq!(rig.reactor.book.user(id), None, "{id}");
        }
        let entry = &rig.reactor.setups.entries["couch"];
        assert_eq!(entry.slots[0].pad, None);
        assert!(entry.slots.iter().all(|slot| !slot.returning));
        finish(&mut rig.reactor).await;
        ask(&mut rig, true).await.await.unwrap().unwrap();
        assert!(rig.reactor.setups.setup("couch").is_none());
    }

    /// A stop of a setup that failed is answered once its pads are shown
    /// again, as a stop of one that runs is, so that a start straight
    /// after finds them free.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_setup_is_stopped_once_its_pads_are_shown_again() {
        let mut rig = rig().await;
        follow(&mut rig.reactor, 7, Phase::Running);
        rig.reactor
            .reported(Report::Failed(SetupId(7), handed_back()))
            .await;
        let stopping = ask(&mut rig, true).await;
        let entry = &rig.reactor.setups.entries["couch"];
        assert!(entry.retiring && entry.stopping.is_some());
        let second = ask(&mut rig, true).await;
        assert_eq!(refusal(second.await.unwrap()).code, ErrorCode::SetupBusy);
        assert!(!stopping.is_finished());

        finish(&mut rig.reactor).await;
        stopping.await.unwrap().unwrap();
        assert!(rig.reactor.setups.setup("couch").is_none());
    }

    /// A pad sent for by a setup that has since gone is nothing to the
    /// setup of that name started afresh, which sends for its own.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_pad_sent_for_by_a_setup_that_has_gone_is_not_its_successor_s() {
        let mut rig = rig().await;
        follow(&mut rig.reactor, 8, Phase::Held);
        rig.reactor.setups.entries.get_mut("couch").unwrap().slots[1].returning = true;
        rig.reactor
            .done(Done::Returned {
                name: "couch".to_string(),
                setup: SetupId(7),
                id: PRIMARY.to_string(),
                pad: None,
                token: None,
                arrival: None,
            })
            .await;
        assert!(rig.reactor.setups.entries["couch"].slots[1].returning);
        finish(&mut rig.reactor).await;
    }

    /// A start that fails after the hiding service hid a pad by
    /// re-binding its driver shows the pad again the same way. The pad
    /// goes and comes back for each, and may not have been seen to go
    /// for the first yet; neither is a pad plugged in, which would start
    /// a profile that starts by itself again, and fail again.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_start_that_fails_after_hiding_a_pad_is_not_started_again_by_it() {
        let mut rig = rig().await;
        let node = "/nowhere/event1";
        rig.reactor.book.arrived(&connecting(PRIMARY, node), None);
        follow(&mut rig.reactor, 7, Phase::Starting);
        let arrival = rig.reactor.book.arrival(PRIMARY);
        rig.reactor
            .setups
            .entries
            .get_mut("couch")
            .unwrap()
            .arrivals = vec![None, arrival];
        rig.reactor.hiding.rebound(PRIMARY);
        let asker = rig.reactor.owe(Owed::Start(Asker::Daemon));
        rig.reactor
            .done(Done::Started {
                name: "couch".to_string(),
                outcome: Err(Unmade {
                    reason: "making the virtual pad `virtual` (dualsense): no uhid".to_string(),
                    tokens: vec![None, Some(Token(1))],
                }),
                asker,
            })
            .await;
        assert!(rig.reactor.setups.setup("couch").is_none());
        assert_eq!(rig.reactor.book.user(PRIMARY), None);
        let book = &mut rig.reactor.book;
        for went in ["as it was hidden", "as it is shown again"] {
            assert_eq!(book.left(Path::new(node)), None, "{went}");
            book.arrived(&connecting(PRIMARY, node), None);
            assert!(book.was_restored(PRIMARY), "{went}");
        }
        assert!(book.left(Path::new(node)).is_some(), "then unplugged");
        finish(&mut rig.reactor).await;
    }

    /// A start that hid a pad by re-binding its driver expects it to go
    /// and come back, as a start that fails does: its going isn't
    /// published, nor its return taken for a pad plugged in.
    ///
    /// And the race that left: a managed profile, started and stopped by
    /// hand before the hiding's own going and coming back was heard of.
    /// Showing the pad again expects one going and coming back, and the
    /// hiding's used it up, so the second, the showing again, was taken
    /// for the pad plugged in, which started the profile again.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_pad_a_start_hides_comes_back_from_that_by_itself() {
        let mut rig = rig().await;
        let (node, other) = ("/nowhere/event1", "/nowhere/event2");
        rig.reactor.book.arrived(&connecting(PRIMARY, node), None);
        rig.reactor.book.arrived(&connecting(ASSIST, other), None);
        let managed = format!("{PROFILE}managed = true\n");
        rig.reactor.profiles.put("couch", &managed).unwrap();
        follow(&mut rig.reactor, 7, Phase::Starting);
        let arrival = rig.reactor.book.arrival(PRIMARY);
        rig.reactor
            .setups
            .entries
            .get_mut("couch")
            .unwrap()
            .arrivals = vec![None, arrival];
        rig.reactor.hiding.rebound(PRIMARY);
        let made = Made {
            setup: handed_back(),
            nodes: Vec::new(),
            pads: vec![(false, None), (true, Some(Token(1)))],
        };
        let asker = rig.reactor.owe(Owed::Start(Asker::Daemon));
        rig.reactor
            .done(Done::Started {
                name: "couch".to_string(),
                outcome: Ok(made),
                asker,
            })
            .await;
        assert_eq!(rig.reactor.setups.entries["couch"].phase, Phase::Held);
        assert!(rig.reactor.book.pad(PRIMARY).unwrap().hidden);

        // Stopped by hand before anything of the hiding was heard of.
        rig.reactor
            .reported(Report::Stopped(SetupId(7), handed_back()))
            .await;
        assert!(rig.reactor.setups.setup("couch").is_none());
        for went in ["as it was hidden", "as it is shown again"] {
            rig.reactor.saw(Seen::Left(PathBuf::from(node))).await;
            assert!(rig.reactor.book.pad(PRIMARY).unwrap().connected, "{went}");
            rig.reactor
                .saw(Seen::Arrived(connecting(PRIMARY, node), None))
                .await;
            assert!(
                rig.reactor.setups.setup("couch").is_none(),
                "not started again {went}"
            );
        }
        assert_eq!(
            rig.reactor.book.left(Path::new(node)).as_deref(),
            Some(PRIMARY),
            "then unplugged"
        );
        finish(&mut rig.reactor).await;
    }

    /// A pad the hiding service doesn't show again, as when it isn't
    /// there to ask, doesn't go and come back: its next going is told.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_pad_the_service_does_not_show_again_is_not_expected_back() {
        let mut rig = rig().await;
        let node = "/nowhere/event1";
        rig.reactor.book.arrived(&connecting(PRIMARY, node), None);
        follow(&mut rig.reactor, 7, Phase::Running);
        rig.reactor.setups.entries.get_mut("couch").unwrap().slots[1].token = Some(Token(1));
        rig.reactor.book.set_hidden(PRIMARY, true);
        rig.reactor.hiding.rebound(PRIMARY);
        rig.reactor
            .reported(Report::Stopped(SetupId(7), handed_back()))
            .await;
        finish(&mut rig.reactor).await;
        assert_eq!(
            rig.reactor.book.left(Path::new(node)).as_deref(),
            Some(PRIMARY)
        );
    }

    /// Work done off the reactor comes back to it if it panics.
    #[test]
    fn work_that_panics_comes_back_all_the_same() {
        assert_eq!(caught("a test", || 1, || 2), 1);
        assert_eq!(caught("a test", || -> i32 { panic!("as a test") }, || 2), 2);
    }

    /// A pad that returns to a setup that failed while it was being
    /// opened is let go: the failed setup isn't brought back to running.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_pad_that_returns_to_a_setup_that_failed_meanwhile_is_let_go() {
        const TEST: &str = "a_pad_that_returns_to_a_setup_that_failed_meanwhile_is_let_go";
        let mut rig = rig().await;
        follow(&mut rig.reactor, 7, Phase::Held);
        rig.reactor.setups.entries.get_mut("couch").unwrap().slots[1].returning = true;
        rig.reactor
            .reported(Report::Failed(SetupId(7), handed_back()))
            .await;
        let Some((_fake, pad)) = a_pad(TEST, "Daemon Failed Pad") else {
            return;
        };
        rig.reactor
            .done(Done::Returned {
                name: "couch".to_string(),
                setup: SetupId(7),
                id: PRIMARY.to_string(),
                pad: Some(pad),
                token: None,
                arrival: None,
            })
            .await;
        let failed = rig.reactor.setups.setup("couch").unwrap();
        assert_eq!(failed.phase, Phase::Failed);
        assert!(failed.sources.iter().all(|source| !source.present));
        finish(&mut rig.reactor).await;
        ask(&mut rig, true).await.await.unwrap().unwrap();
        assert!(rig.reactor.setups.setup("couch").is_none());
    }
}
