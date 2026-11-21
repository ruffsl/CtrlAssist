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
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ctrlassist_core::{Graph, SourceId};
use ctrlassist_device::{Command, Database, Pad, Report, SendError, Setup, SetupId, VirtualPad};
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
    /// The reply owed once the input thread hands it back.
    stopping: Option<Responder<setup::Stop>>,
}

impl Entry {
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
        })
    }
}

/// What the blocking work of a start made.
#[derive(Debug)]
pub(crate) struct Made {
    setup: Box<Setup>,
    /// Each virtual pad's event node, by sink number.
    nodes: Vec<PathBuf>,
    /// Which sources' pads were opened, by source number.
    present: Vec<bool>,
}

/// Work done off the reactor, come back to it.
#[derive(Debug)]
pub(crate) enum Done {
    /// A start's pads are open and its virtual pads made, or not.
    Started {
        name: String,
        outcome: Result<Made, String>,
        responder: Responder<setup::Start>,
    },
    /// A pad that returned is open again, or couldn't be opened.
    Returned {
        name: String,
        source: usize,
        pad: Option<Box<Pad>>,
    },
    /// Something was dropped where dropping may block.
    Dropped,
}

/// Opens a pad for a setup: for writing too, since rumble is written to
/// the node it is read from.
fn open(node: &Path, database: &Database) -> Result<Pad, String> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(node)
        .or_else(|_| File::open(node))
        .map_err(|error| format!("opening {}: {error}", node.display()))?;
    Pad::with_database(OwnedFd::from(file), database)
        .map_err(|error| format!("reading {}: {error}", node.display()))
}

/// The blocking part of a start.
fn make(plan: Plan, nodes: &[Option<PathBuf>], database: &Database) -> Result<Made, String> {
    let graph = Graph::build(plan.graph).map_err(|error| error.to_string())?;
    let pads: Vec<Option<Pad>> = nodes
        .iter()
        .map(|node| {
            let node = node.as_deref()?;
            // A pad that went between being chosen and being opened is
            // one the setup starts without.
            open(node, database)
                .inspect_err(|error| warn!(error, "a setup starts without a pad"))
                .ok()
        })
        .collect();
    let present = pads.iter().map(Option::is_some).collect();
    let mut sinks = Vec::new();
    for sink in &plan.sinks {
        let made = VirtualPad::create(device_identity(sink.identity)).map_err(|error| {
            format!(
                "making the virtual pad `{}` ({}): {error}",
                sink.name,
                sink.identity.as_str()
            )
        })?;
        sinks.push(made);
    }
    let nodes = sinks
        .iter()
        .map(|sink| sink.event_node().to_path_buf())
        .collect();
    Ok(Made {
        setup: Box::new(Setup::new(graph, pads, sinks)),
        nodes,
        present,
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
            });
        }
        for slot in &slots {
            if let Some(pad) = &slot.pad {
                self.book.claim(pad, name);
            }
        }
        Ok(slots)
    }

    async fn start(&mut self, name: String, responder: Responder<setup::Start>) {
        if self.setups.entries.contains_key(&name) {
            let refusal = WireError::refusal(
                ErrorCode::SetupExists,
                format!("the setup {name} exists already; stop it first"),
            );
            return responder.err(&refusal).await;
        }
        let Some(profile) = self.profiles.profile(&name) else {
            let refusal = WireError::refusal(
                ErrorCode::UnknownProfile,
                format!("no profile is called {}", name.escape_debug()),
            );
            return responder.err(&refusal).await;
        };
        let plan = match Profile::check(&profile.toml) {
            Ok(plan) => plan,
            Err(problem) => {
                let refusal = WireError::refusal(ErrorCode::InvalidProfile, problem.to_string());
                return responder.err(&refusal).await;
            }
        };
        let slots = match self.bind(&name, &plan) {
            Ok(slots) => slots,
            Err(refusal) => return responder.err(&refusal).await,
        };
        let nodes: Vec<Option<PathBuf>> = slots
            .iter()
            .map(|slot| {
                let pad = slot.pad.as_deref()?;
                self.book.node(pad).map(Path::to_path_buf)
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
            stopping: None,
        };
        self.setups.entries.insert(name.clone(), entry);
        self.publish_setup(&name).await;
        for pad in pads {
            self.publish_pad_logged(&pad).await;
        }
        let database = Arc::clone(&self.database);
        self.work.spawn_blocking(move || Done::Started {
            outcome: make(plan, &nodes, &database),
            name,
            responder,
        });
    }

    async fn started(
        &mut self,
        name: String,
        outcome: Result<Made, String>,
        responder: Responder<setup::Start>,
    ) {
        let Some(entry) = self.setups.entries.get_mut(&name) else {
            // Nothing removes a setup that is starting.
            return responder
                .err(&WireError::internal("the setup is gone"))
                .await;
        };
        let sent = outcome.and_then(|made| {
            let Made {
                setup,
                nodes,
                present,
            } = made;
            match self.input.send(Command::Start(entry.id, setup)) {
                Ok(()) => Ok((nodes, present)),
                Err(error) => {
                    let reason = error.to_string();
                    if let SendError::Full(command) | SendError::Stopped(command) = error {
                        self.work.spawn_blocking(move || {
                            drop(command);
                            Done::Dropped
                        });
                    }
                    Err(format!("the input thread didn't take it: {reason}"))
                }
            }
        });
        match sent {
            Ok((nodes, present)) => {
                for (slot, present) in entry.slots.iter_mut().zip(present) {
                    slot.present = present;
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
                self.publish_setup(&name).await;
                responder.ok(&rpc::Ack {}).await;
            }
            Err(reason) => {
                // A start that fails leaves nothing behind.
                self.setups.entries.remove(&name);
                self.release(&name).await;
                self.publish_setup(&name).await;
                warn!(setup = name, reason, "a setup couldn't start");
                let refusal = WireError::refusal(ErrorCode::SetupFailed, reason);
                responder.err(&refusal).await;
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
                    Ok(()) => entry.stopping = Some(responder),
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

    pub(crate) async fn setup_rpc(&mut self, query: Query) {
        match setup::Incoming::receive(query).await {
            Some(((), setup::Incoming::Start(request, responder))) => {
                self.start(request.name, responder).await;
            }
            Some(((), setup::Incoming::Stop(request, responder))) => {
                self.stop(request.name, responder).await;
            }
            // Refused already, with a typed error.
            None => {}
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
        let Some(node) = slot.pad.as_deref().and_then(|pad| self.book.node(pad)) else {
            return;
        };
        slot.returning = true;
        let node = node.to_path_buf();
        let database = Arc::clone(&self.database);
        let name = name.to_string();
        self.work.spawn_blocking(move || Done::Returned {
            // A node from before the pad left doesn't open; the pad's
            // arrival will bring it back.
            pad: open(&node, &database).ok().map(Box::new),
            name,
            source,
        });
    }

    async fn returned(&mut self, name: String, source: usize, pad: Option<Box<Pad>>) {
        let slot = self
            .setups
            .entries
            .get_mut(&name)
            .and_then(|entry| Some((entry.id, entry.slots.get_mut(source)?)));
        let Some((id, slot)) = slot else {
            // The setup went while the pad was being opened.
            return self.drop_off(pad);
        };
        slot.returning = false;
        let Some(pad) = pad else { return };
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
                responder,
            } => self.started(name, outcome, responder).await,
            Done::Returned { name, source, pad } => self.returned(name, source, pad).await,
            Done::Dropped => {}
        }
    }

    /// The input thread has said something.
    pub(crate) async fn reported(&mut self, report: Report) {
        match report {
            Report::Stopped(id, setup) => {
                self.drop_off(setup);
                let Some(name) = self.setups.name_of(id) else {
                    return;
                };
                let entry = self.setups.entries.remove(&name);
                self.release(&name).await;
                self.publish_setup(&name).await;
                info!(setup = name, "a setup stopped");
                if let Some(responder) = entry.and_then(|entry| entry.stopping) {
                    responder.ok(&rpc::Ack {}).await;
                }
            }
            Report::Failed(id, setup) => {
                self.drop_off(setup);
                let Some(name) = self.setups.name_of(id) else {
                    return;
                };
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
                self.release(&name).await;
                warn!(setup = name, "a setup failed");
                if let Some(responder) = stopping {
                    // It was asked to stop, and it has.
                    self.setups.entries.remove(&name);
                    self.publish_setup(&name).await;
                    responder.ok(&rpc::Ack {}).await;
                } else {
                    self.publish_setup(&name).await;
                }
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
            stopping: None,
        };
        reactor.setups.entries.insert("couch".to_string(), entry);
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
        stopping.await.unwrap().unwrap();
        assert!(rig.reactor.setups.setup("couch").is_none());

        // One that fails while stopping has stopped all the same.
        follow(&mut rig.reactor, 9, Phase::Running);
        let stopping = ask(&mut rig, true).await;
        rig.reactor
            .reported(Report::Failed(SetupId(9), handed_back()))
            .await;
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
                responder,
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
            })
            .await;
        rig.reactor.done(Done::Dropped).await;
        assert_eq!(
            rig.reactor.setups.setup("couch").unwrap().phase,
            Phase::Held
        );
    }
}
