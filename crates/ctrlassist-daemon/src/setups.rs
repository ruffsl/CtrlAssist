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
    Command, Database, NewGraph, Pad, Report, SendError, Setup, SetupId, VirtualPad,
};
use ctrlassist_hider::{Client, Token};
use ctrlassist_protocol::endpoint::setup;
use ctrlassist_protocol::error::{ErrorCode, WireError};
use ctrlassist_protocol::payload::{self, Phase};
use ctrlassist_protocol::profile::{PadRef, Plan, Profile};
use ctrlassist_protocol::serve::Responder;
use ctrlassist_protocol::{keys, rpc};
use tracing::{info, warn};
use zenoh::query::Query;

use crate::device_identity;
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
    /// The hiding service's name for the pad, while it has it hidden.
    token: Option<Token>,
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
    /// Who is owed a reply once the input thread has taken a new graph
    /// for it. Until then `nodes` is the new graph's and what the input
    /// thread reports of selections is the old one's.
    taking: Option<Responder<setup::Update>>,
}

/// Who waits for a setup the input thread was asked to stop.
#[derive(Debug)]
pub(crate) enum Waiting {
    /// A stop, owed its reply.
    Stop(Responder<setup::Stop>),
    /// An update, for which the setup is started again.
    Update(Responder<setup::Update>),
}

/// What an update by stopping and starting costs.
const RESTARTED: rpc::Updated = rpc::Updated { disruptive: true };
/// What an update by a new graph costs.
const TAKEN: rpc::Updated = rpc::Updated { disruptive: false };

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
    /// Whether `plan` is of the pads and the virtual pads this setup
    /// has: the same sources under the same names, and the same sinks of
    /// the same identities. Then only the graph between them differs,
    /// and the setup can take it as it runs.
    fn has_the_pads_of(&self, plan: &Plan) -> bool {
        let sources = self.slots.len() == plan.sources.len()
            && self
                .slots
                .iter()
                .zip(&plan.sources)
                .all(|(slot, source)| slot.name == source.name && slot.wanted == source.pad);
        let sinks =
            self.sinks.len() == plan.sinks.len()
                && self.sinks.iter().zip(&plan.sinks).all(|(has, sink)| {
                    has.name == sink.name && has.identity == sink.identity.as_str()
                });
        sources && sinks
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

    fn setup(&self, name: &str) -> Option<payload::Setup> {
        let entry = self.entries.get(name)?;
        Some(payload::Setup {
            name: name.to_string(),
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

/// How a setup's pads are kept from games.
#[derive(Debug)]
pub(crate) enum Hiding {
    /// Through the hiding service, one request at a time.
    Service(Mutex<Client>),
    /// By a grab on each pad, where the service isn't there.
    Grab,
}

impl Hiding {
    /// Connects to the hiding service at `socket`, or settles for grabs.
    pub(crate) fn find(socket: &Path) -> Self {
        match Client::connect(socket) {
            Ok(client) => Self::Service(Mutex::new(client)),
            Err(error) => {
                info!(socket = %socket.display(), %error, "the hiding service isn't there; a setup's pads will be grabbed, which hides them from most games but not from a program that reads them some other way");
                Self::Grab
            }
        }
    }

    /// The mechanism, for the daemon's state.
    pub(crate) fn mechanism(&self) -> payload::Hiding {
        match self {
            Self::Service(_) => payload::Hiding::Service,
            Self::Grab => payload::Hiding::Grab,
        }
    }

    /// Asks one thing of the hiding service, if it is there.
    fn ask<T>(&self, ask: impl FnOnce(&Client) -> Result<T, ctrlassist_hider::Error>) -> Option<T> {
        let Self::Service(client) = self else {
            return None;
        };
        let client = client
            .lock()
            .expect("nothing panics while the hiding service's client is held");
        ask(&client)
            .inspect_err(|error| warn!(%error, "the hiding service didn't do as asked"))
            .ok()
    }

    /// Restores each pad named, once what read them has been dropped.
    fn restore(&self, tokens: &[Token]) {
        for token in tokens {
            self.ask(|client| client.restore(*token));
        }
    }
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

/// Who asked for a setup to start.
#[derive(Debug)]
pub(crate) enum Asker {
    /// A client, which is owed a reply.
    Client(Responder<setup::Start>),
    /// The daemon itself, for a profile that starts by itself.
    Daemon,
    /// A client that asked for an update, which is owed its reply once
    /// the setup runs again.
    Update(Responder<setup::Update>),
}

impl Asker {
    async fn ok(self) {
        match self {
            Self::Client(responder) => responder.ok(&rpc::Ack {}).await,
            Self::Update(responder) => responder.ok(&RESTARTED).await,
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

/// Work done off the reactor, come back to it.
#[derive(Debug)]
pub(crate) enum Done {
    /// A start's pads are open and its virtual pads made, or not.
    Started {
        name: String,
        outcome: Result<Made, String>,
        asker: Asker,
    },
    /// A pad that returned is open again, or couldn't be opened.
    Returned {
        name: String,
        source: usize,
        pad: Option<Box<Pad>>,
        /// The hiding service's name for it, if it hid it just now.
        token: Option<Token>,
    },
    /// A setup the input thread handed back is gone, virtual pads and
    /// all, and its pads are shown again; a stop that was waiting for
    /// that is seen to.
    Retired {
        name: String,
        waiting: Option<Waiting>,
    },
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

/// Takes a pad for a setup, keeping it from games: hidden by the hiding
/// service, which hands back the only descriptor left open on it, or
/// else grabbed. Returns the service's name for the pad if it hid it.
fn take(
    wanted: &Wanted,
    database: &Database,
    hiding: &Hiding,
) -> Result<(Pad, Option<Token>), String> {
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
    if let Some(hidden) = hiding.ask(|client| client.hide(fd.as_fd())) {
        drop(opened);
        drop(fd);
        return match read(hidden.event, node, database) {
            Ok(pad) => Ok((pad, Some(hidden.token))),
            Err(error) => {
                hiding.restore(&[hidden.token]);
                Err(error)
            }
        };
    }
    drop(fd);
    let mut pad = opened;
    if let Err(error) = pad.grab() {
        warn!(node = %node.display(), %error, "a pad couldn't be grabbed; games will see it beside the virtual pad");
    }
    Ok((pad, None))
}

/// Takes back a pad that returned. One the service still has hidden is
/// opened through it: the pad was never visible in between.
fn retake(
    wanted: &Wanted,
    token: Option<Token>,
    database: &Database,
    hiding: &Hiding,
) -> Result<(Pad, Option<Token>), String> {
    if let Some(token) = token
        && let Some(hidden) = hiding.ask(|client| client.reopen(token))
    {
        // The service knows the pad by what it is, not by its node.
        return read(hidden.event, &wanted.node, database).map(|pad| (pad, Some(token)));
    }
    let taken = take(wanted, database, hiding)?;
    // Hidden afresh, under a new name: the old one is given up, so that
    // what it hid isn't left for the connection's end to restore.
    if let (Some(old), Some(new)) = (token, taken.1)
        && old != new
    {
        hiding.restore(&[old]);
    }
    Ok(taken)
}

/// The blocking part of a start.
fn make(
    plan: Plan,
    nodes: &[Option<Wanted>],
    database: &Database,
    hiding: &Hiding,
) -> Result<Made, String> {
    let graph = Graph::build(plan.graph).map_err(|error| error.to_string())?;
    let mut pads = Vec::new();
    let mut taken = Vec::new();
    for node in nodes {
        // A pad that went between being chosen and being taken is one
        // the setup starts without.
        let pad = node.as_ref().and_then(|node| {
            take(node, database, hiding)
                .inspect_err(|error| warn!(error, "a setup starts without a pad"))
                .ok()
        });
        let (pad, token) = pad.unzip();
        taken.push((pad.is_some(), token.flatten()));
        pads.push(pad);
    }
    let mut sinks = Vec::new();
    for sink in &plan.sinks {
        match VirtualPad::create(device_identity(sink.identity)) {
            Ok(made) => sinks.push(made),
            Err(error) => {
                // Nothing of a start that fails is left: its pads are
                // let go, then shown again.
                drop(pads);
                let tokens: Vec<Token> = taken.iter().filter_map(|(_, token)| *token).collect();
                hiding.restore(&tokens);
                return Err(format!(
                    "making the virtual pad `{}` ({}): {error}",
                    sink.name,
                    sink.identity.as_str()
                ));
            }
        }
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

fn unknown(name: &str) -> WireError {
    WireError::refusal(
        ErrorCode::UnknownSetup,
        format!("no setup is called {}", name.escape_debug()),
    )
}

impl Reactor {
    /// Drops `thing` where dropping may block: a pad's close can wait on
    /// its driver, and a virtual pad's on the kernel.
    fn drop_off<T: Send + 'static>(&mut self, thing: T) {
        self.work.spawn_blocking(move || {
            drop(thing);
            Done::Dropped
        });
    }

    /// Drops a setup the input thread handed back, then has the hiding
    /// service show the pads it hid: in that order, so that nothing of
    /// ours still reads a pad as it comes back.
    fn retire(
        &mut self,
        name: String,
        setup: Box<Setup>,
        tokens: Vec<Token>,
        waiting: Option<Waiting>,
    ) {
        let hiding = Arc::clone(&self.hiding);
        self.work.spawn_blocking(move || {
            drop(setup);
            hiding.restore(&tokens);
            Done::Retired { name, waiting }
        });
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

    /// Gives up a setup's pads, and says so.
    async fn release(&mut self, name: &str) {
        for pad in self.book.release(name) {
            self.publish_pad_logged(&pad).await;
        }
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
                        return Err(WireError::refusal(
                            ErrorCode::InvalidProfile,
                            format!(
                                "source.{}.pad: {id} is another source's pad too",
                                source.name
                            ),
                        ));
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
            slots.push(Slot {
                name: source.name.clone(),
                wanted: source.pad.clone(),
                pad,
                present: false,
                returning: false,
                token: None,
            });
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
        self.setups.started += 1;
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
        };
        self.setups.entries.insert(name.clone(), entry);
        self.publish_setup(&name).await;
        for pad in pads {
            self.publish_pad_logged(&pad).await;
        }
        let database = Arc::clone(&self.database);
        let hiding = Arc::clone(&self.hiding);
        self.work.spawn_blocking(move || Done::Started {
            outcome: make(plan, &nodes, &database, &hiding),
            name,
            asker,
        });
    }

    async fn started(&mut self, name: String, outcome: Result<Made, String>, asker: Asker) {
        let Some(entry) = self.setups.entries.get_mut(&name) else {
            // Nothing removes a setup that is starting.
            return asker.err(&WireError::internal("the setup is gone")).await;
        };
        let sent = outcome.and_then(|made| {
            let Made { setup, nodes, pads } = made;
            match self.input.send(Command::Start(entry.id, setup)) {
                Ok(()) => Ok((nodes, pads)),
                Err(error) => {
                    let reason = error.to_string();
                    if let SendError::Full(command) | SendError::Stopped(command) = error {
                        let hiding = Arc::clone(&self.hiding);
                        let tokens: Vec<Token> =
                            pads.iter().filter_map(|(_, token)| *token).collect();
                        self.work.spawn_blocking(move || {
                            drop(command);
                            hiding.restore(&tokens);
                            Done::Dropped
                        });
                    }
                    Err(format!("the input thread didn't take it: {reason}"))
                }
            }
        });
        match sent {
            Ok((nodes, pads)) => {
                let mut hidden = Vec::new();
                for (slot, (present, token)) in entry.slots.iter_mut().zip(pads) {
                    slot.present = present;
                    slot.token = token;
                    if let (Some(pad), Some(_)) = (&slot.pad, token) {
                        hidden.push(pad.clone());
                    }
                }
                for (sink, node) in entry.sinks.iter_mut().zip(nodes) {
                    sink.node = Some(node.display().to_string());
                }
                entry.settle();
                info!(
                    setup = name,
                    phase = entry.phase.as_str(),
                    "a setup started"
                );
                for pad in hidden {
                    self.book.set_hidden(&pad, true);
                    self.publish_pad_logged(&pad).await;
                }
                self.publish_setup(&name).await;
                asker.ok().await;
            }
            Err(reason) => {
                // A start that fails leaves nothing behind.
                self.setups.entries.remove(&name);
                self.release(&name).await;
                self.publish_setup(&name).await;
                warn!(setup = name, reason, "a setup couldn't start");
                let refusal = WireError::refusal(ErrorCode::SetupFailed, reason);
                asker.err(&refusal).await;
            }
        }
    }

    async fn stop(&mut self, name: String, responder: Responder<setup::Stop>) {
        let Some(entry) = self.setups.entries.get_mut(&name) else {
            return responder.err(&unknown(&name)).await;
        };
        match entry.phase {
            Phase::Failed => {
                // The input thread handed it back when it failed.
                self.setups.entries.remove(&name);
                self.publish_setup(&name).await;
                responder.ok(&rpc::Ack {}).await;
            }
            Phase::Running | Phase::Held if entry.stopping.is_none() => {
                match self.input.send(Command::Stop(entry.id)) {
                    Ok(()) => entry.stopping = Some(Waiting::Stop(responder)),
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

    /// Has a setup take up its profile as it is kept now, or only says
    /// what that would cost. A profile of the same pads and virtual pads
    /// is taken as the setup runs: the input thread is handed its graph.
    /// Any other means stopping the setup and starting it again.
    async fn update(&mut self, request: rpc::UpdateRequest, responder: Responder<setup::Update>) {
        let name = request.name;
        let Some(entry) = self.setups.entries.get_mut(&name) else {
            return responder.err(&unknown(&name)).await;
        };
        // What is kept now must be right before what runs is given up.
        let kept = self
            .profiles
            .profile(&name)
            .map(|profile| Profile::check(&profile.toml));
        let plan = match kept {
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
        let live =
            matches!(entry.phase, Phase::Running | Phase::Held) && entry.has_the_pads_of(&plan);
        if !request.apply {
            let cost = if live { TAKEN } else { RESTARTED };
            return responder.ok(&cost).await;
        }
        let free = entry.stopping.is_none() && entry.taking.is_none();
        match entry.phase {
            Phase::Failed => {
                // Nothing runs: clear it away and start afresh.
                self.setups.entries.remove(&name);
                self.publish_setup(&name).await;
                self.start(name, Asker::Update(responder)).await;
            }
            Phase::Running | Phase::Held if free && live => {
                let nodes = choosers(&plan);
                let sent = Graph::build(plan.graph)
                    .map_err(|error| error.to_string())
                    .and_then(|graph| {
                        let graph = Box::new(NewGraph::new(graph, carry(&entry.nodes, &nodes)));
                        let command = Command::NewGraph {
                            setup: entry.id,
                            graph,
                        };
                        self.input.send(command).map_err(|error| error.to_string())
                    });
                match sent {
                    Ok(()) => {
                        // Numbered by the new graph from here on: a
                        // selection asked for now reaches the input
                        // thread after the graph does.
                        entry.selected = vec![None; nodes.len()];
                        entry.nodes = nodes;
                        entry.taking = Some(responder);
                        self.publish_setup(&name).await;
                    }
                    Err(error) => {
                        let refusal = WireError::internal(format!(
                            "the input thread didn't take the request: {error}"
                        ));
                        responder.err(&refusal).await;
                    }
                }
            }
            Phase::Running | Phase::Held if free => {
                match self.input.send(Command::Stop(entry.id)) {
                    Ok(()) => entry.stopping = Some(Waiting::Update(responder)),
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
            Some((entry.id, source, entry.slots[source].present))
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
                self.update(request, responder).await;
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
    /// can: one with `auto-start`, nothing wrong with it, no setup of its
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
            if plan.auto_start && prompted && self.ready(&plan) {
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
            && plan.auto_start
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
        if !matches!(entry.phase, Phase::Running | Phase::Held) || entry.stopping.is_some() {
            return;
        }
        let Some(slot) = entry.slots.get_mut(source) else {
            return;
        };
        if slot.present || slot.returning {
            return;
        }
        let wanted = slot.pad.as_deref().and_then(|pad| {
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
        let database = Arc::clone(&self.database);
        let hiding = Arc::clone(&self.hiding);
        let name = name.to_string();
        self.work.spawn_blocking(move || {
            // A node from before the pad left doesn't open, or is some
            // other pad's by now; the pad's arrival will bring it back.
            let (pad, token) = retake(&wanted, token, &database, &hiding).ok().unzip();
            Done::Returned {
                pad: pad.map(Box::new),
                token: token.flatten(),
                name,
                source,
            }
        });
    }

    async fn returned(
        &mut self,
        name: String,
        source: usize,
        pad: Option<Box<Pad>>,
        token: Option<Token>,
    ) {
        let slot = self
            .setups
            .entries
            .get_mut(&name)
            .and_then(|entry| Some((entry.id, entry.slots.get_mut(source)?)));
        let Some((id, slot)) = slot else {
            // The setup went while the pad was being opened: the pad is
            // let go, and shown again if it was hidden for this.
            let hiding = Arc::clone(&self.hiding);
            self.work.spawn_blocking(move || {
                drop(pad);
                hiding.restore(&token.into_iter().collect::<Vec<_>>());
                Done::Dropped
            });
            return;
        };
        slot.returning = false;
        let Some(pad) = pad else { return };
        let newly_hidden = token.is_some() && slot.token != token;
        slot.token = token.or(slot.token);
        let its_pad = slot.pad.clone();
        let command = Command::PadBack {
            setup: id,
            source: SourceId(source),
            pad,
        };
        match self.input.send(command) {
            Ok(()) => {
                slot.present = true;
                if let Some(entry) = self.setups.entries.get_mut(&name) {
                    entry.settle();
                }
                info!(setup = name, source, "a pad returned to its setup");
                if let (true, Some(pad)) = (newly_hidden, its_pad) {
                    self.book.set_hidden(&pad, true);
                    self.publish_pad_logged(&pad).await;
                }
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
        // One pad, one place: its own if a setup is waiting for it, or
        // else the first place it fits that has no pad yet.
        let waiting = self.setups.entries.iter().find_map(|(name, entry)| {
            entry.slots.iter().enumerate().find_map(|(source, slot)| {
                let its_own = slot.pad.as_deref() == Some(id);
                let free = slot.pad.is_none()
                    && self.book.user(id).is_none()
                    && self.book.fits(id, &slot.wanted);
                (!slot.present && (its_own || free)).then(|| (name.clone(), source))
            })
        });
        let Some((name, source)) = waiting else {
            return;
        };
        if let Some(slot) = self
            .setups
            .entries
            .get_mut(&name)
            .and_then(|entry| entry.slots.get_mut(source))
            && slot.pad.is_none()
        {
            slot.pad = Some(id.to_string());
            self.book.claim(id, &name);
            self.publish_pad_logged(id).await;
            self.publish_setup(&name).await;
        }
        self.bring_back(&name, source);
    }

    /// Work done off the reactor has come back.
    pub(crate) async fn done(&mut self, done: Done) {
        match done {
            Done::Started {
                name,
                outcome,
                asker,
            } => self.started(name, outcome, asker).await,
            Done::Returned {
                name,
                source,
                pad,
                token,
            } => self.returned(name, source, pad, token).await,
            Done::Retired { name, waiting } => match waiting {
                Some(Waiting::Stop(responder)) => responder.ok(&rpc::Ack {}).await,
                // The old setup is gone, pads restored: the new one starts
                // from the profile as it is kept now.
                Some(Waiting::Update(responder)) => {
                    self.start(name, Asker::Update(responder)).await;
                }
                None => {}
            },
            Done::Dropped => {}
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
                let tokens = entry
                    .iter()
                    .flat_map(|entry| entry.slots.iter().filter_map(|slot| slot.token))
                    .collect();
                if let Some(responder) = entry.as_mut().and_then(|entry| entry.taking.take()) {
                    // Only if the report of the graph was lost.
                    let refusal = WireError::internal("the setup stopped");
                    responder.err(&refusal).await;
                }
                // The stop is answered once the virtual pads are gone.
                self.retire(
                    name.clone(),
                    setup,
                    tokens,
                    entry.and_then(|entry| entry.stopping),
                );
                self.release(&name).await;
                self.publish_setup(&name).await;
                info!(setup = name, "a setup stopped");
            }
            Report::Failed(id, setup) => {
                let Some(name) = self.setups.name_of(id) else {
                    return self.drop_off(setup);
                };
                let tokens = self
                    .setups
                    .entries
                    .get_mut(&name)
                    .map(|entry| {
                        entry
                            .slots
                            .iter_mut()
                            .filter_map(|slot| slot.token.take())
                            .collect()
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
                if let Some(responder) = taking {
                    let refusal = WireError::refusal(
                        ErrorCode::SetupFailed,
                        "the setup failed as it took the change",
                    );
                    responder.err(&refusal).await;
                }
                self.release(&name).await;
                warn!(setup = name, "a setup failed");
                if stopping.is_some() {
                    // It was asked to stop, and it has.
                    self.setups.entries.remove(&name);
                }
                self.publish_setup(&name).await;
                self.retire(name, setup, tokens, stopping);
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
                // While a new graph is on its way, what arrives is of
                // the old one's nodes; the new one's follow its report.
                if let Some(entry) = self.setups.entries.get_mut(&name)
                    && entry.taking.is_none()
                    && let Some(selected) = entry.selected.get_mut(node.0)
                {
                    *selected = Some(index);
                }
                self.publish_setup(&name).await;
            }
            Report::GraphTaken { setup, old } => {
                // A graph holds only memory: dropping it never blocks.
                drop(old);
                let Some(name) = self.setups.name_of(setup) else {
                    return;
                };
                let taking = self
                    .setups
                    .entries
                    .get_mut(&name)
                    .and_then(|entry| entry.taking.take());
                info!(setup = name, "a setup took its profile as it runs");
                if let Some(responder) = taking {
                    responder.ok(&TAKEN).await;
                }
            }
            Report::GraphRefused { setup, graph } => {
                drop(graph);
                let Some(name) = self.setups.name_of(setup) else {
                    return;
                };
                // Only a setup the input thread no longer runs refuses
                // one, and its own report says what became of it.
                warn!(setup = name, "the input thread refused a new graph");
                let taking = self
                    .setups
                    .entries
                    .get_mut(&name)
                    .and_then(|entry| entry.taking.take());
                if let Some(responder) = taking {
                    let refusal = WireError::internal("the input thread refused the new graph");
                    responder.err(&refusal).await;
                }
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
    use crate::profiles::Profiles;
    use crate::store::Store;

    const PROFILE: &str =
        "preset = \"priority\"\nprimary = \"0c:27:56:4f:bd:14\"\nassist = \"model:054c:09cc\"\n";

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
        let session = zenoh::open(bootstrap::peer_config(&socket).unwrap())
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
            Hiding::Grab,
        );
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
        assert_eq!(rig.reactor.book.user("address:0c:27:56:4f:bd:14"), None);

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
    async fn a_setup_that_is_starting_cannot_be_stopped_yet() {
        let mut rig = rig().await;
        follow(&mut rig.reactor, 7, Phase::Starting);
        let stopped = ask(&mut rig, true).await;
        let refused = refusal(stopped.await.unwrap());
        assert_eq!(refused.code, ErrorCode::SetupBusy);
        assert!(refused.message.contains("starting or stopping"));
        // Nor does a node of it select yet.
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

    #[tokio::test(flavor = "multi_thread")]
    async fn a_start_whose_making_fails_is_refused_with_why_and_leaves_nothing() {
        let mut rig = rig().await;
        follow(&mut rig.reactor, 7, Phase::Starting);
        assert_eq!(
            rig.reactor.book.user("address:0c:27:56:4f:bd:14"),
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
        rig.reactor
            .done(Done::Started {
                name: "couch".to_string(),
                outcome: Err("making the virtual pad `virtual` (generic): no uinput".to_string()),
                asker: Asker::Client(responder),
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
        rig.reactor.setups.entries.get_mut("couch").unwrap().slots[0].returning = true;
        rig.reactor
            .done(Done::Returned {
                name: "couch".to_string(),
                source: 0,
                pad: None,
                token: None,
            })
            .await;
        let slot = &rig.reactor.setups.entries["couch"].slots[0];
        assert!(!slot.returning && !slot.present);
        // For a setup that went meanwhile, there is nothing to do.
        rig.reactor
            .done(Done::Returned {
                name: "gone".to_string(),
                source: 0,
                pad: None,
                token: None,
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
        assert!(
            ask_update(&mut rig, false)
                .await
                .await
                .unwrap()
                .unwrap()
                .disruptive
        );
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
        let pad = "address:0c:27:56:4f:bd:14";
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
        assert!(updating.await.unwrap().unwrap().disruptive);
        let started = rig.reactor.setups.setup("couch").unwrap();
        assert_eq!(started.phase, Phase::Held);
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
        "preset = \"toggle\"\nprimary = \"0c:27:56:4f:bd:14\"\nassist = \"model:054c:09cc\"\n";

    #[tokio::test(flavor = "multi_thread")]
    async fn a_profile_of_the_same_pads_is_taken_as_the_setup_runs() {
        let mut rig = rig().await;
        follow(&mut rig.reactor, 7, Phase::Held);
        rig.reactor.profiles.put("couch", TOGGLE).unwrap();
        let cost = ask_update(&mut rig, false).await.await.unwrap().unwrap();
        assert!(!cost.disruptive, "only the graph differs");

        let updating = ask_update(&mut rig, true).await;
        let entry = &rig.reactor.setups.entries["couch"];
        assert!(entry.taking.is_some() && entry.stopping.is_none());
        // A second update waits for the first; a stop needn't.
        let second = ask_update(&mut rig, true).await.await.unwrap();
        assert!(matches!(
            second,
            Err(RpcError::Service(refusal)) if refusal.code == ErrorCode::SetupBusy
        ));
        // A selection still on its way is of the old graph's nodes.
        let selected = |index| Report::Selected {
            setup: SetupId(7),
            node: NodeId(0),
            index,
        };
        rig.reactor.reported(selected(1)).await;
        assert!(rig.reactor.setups.setup("couch").unwrap().nodes.is_empty());

        let old = Box::new(NewGraph::new(
            Graph::build(Profile::check(PROFILE).unwrap().graph).unwrap(),
            Vec::new(),
        ));
        rig.reactor
            .reported(Report::GraphTaken {
                setup: SetupId(7),
                old,
            })
            .await;
        assert!(!updating.await.unwrap().unwrap().disruptive);
        rig.reactor.reported(selected(0)).await;
        let taken = rig.reactor.setups.setup("couch").unwrap();
        assert_eq!(taken.phase, Phase::Held, "it never stopped");
        assert_eq!(taken.nodes[0].choices, ["primary", "assist"]);
        assert_eq!(taken.nodes[0].selected, 0);
        assert_eq!(
            taken.sinks[0].node.as_deref(),
            Some("/dev/input/event99"),
            "its virtual pad is the one it had"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_update_under_way_is_answered_whatever_becomes_of_the_setup() {
        let graph = || {
            Box::new(NewGraph::new(
                Graph::build(Profile::check(TOGGLE).unwrap().graph).unwrap(),
                Vec::new(),
            ))
        };
        let internal = |outcome: Result<rpc::Updated, RpcError>, code| {
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
                graph: graph(),
            })
            .await;
        internal(updating.await.unwrap(), ErrorCode::Internal);

        // Failed as it took it.
        let updating = ask_update(&mut rig, true).await;
        rig.reactor
            .reported(Report::Failed(SetupId(7), handed_back()))
            .await;
        internal(updating.await.unwrap(), ErrorCode::SetupFailed);
        finish(&mut rig.reactor).await;
        rig.reactor.setups.entries.remove("couch");

        // Stopped with the report of the graph lost.
        follow(&mut rig.reactor, 8, Phase::Running);
        let updating = ask_update(&mut rig, true).await;
        rig.reactor
            .reported(Report::Stopped(SetupId(8), handed_back()))
            .await;
        internal(updating.await.unwrap(), ErrorCode::Internal);
        finish(&mut rig.reactor).await;

        // For a setup that has gone, either report is only a graph to drop.
        rig.reactor
            .reported(Report::GraphRefused {
                setup: SetupId(99),
                graph: graph(),
            })
            .await;
        rig.reactor
            .reported(Report::GraphTaken {
                setup: SetupId(99),
                old: graph(),
            })
            .await;
        assert!(rig.reactor.setups.setup("couch").is_none());
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
}
