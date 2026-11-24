//! The reactor: the one task that owns what the daemon knows and serves.
//!
//! Pads connecting and leaving, and every rpc, arrive here and are
//! handled one at a time, so nothing it owns needs a lock. What it knows
//! goes out as state, each key from a cache so a client that connects
//! later still receives it.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ctrlassist_core::Level;
use ctrlassist_device::{Database, Found, InputThread, Pad};
use ctrlassist_protocol::endpoint::{daemon, pad, profile, setup};
use ctrlassist_protocol::error::{ErrorCode, WireError};
use ctrlassist_protocol::profile::Identity;
use ctrlassist_protocol::serve::{self, Responder};
use ctrlassist_protocol::{VERSION, keys, payload, rpc};
use serde::Serialize;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::oneshot;
use tokio::task::JoinSet;
use tracing::{error, info, warn};
use zenoh::Session;
use zenoh::bytes::Encoding;
use zenoh::handlers::FifoChannelHandler;
use zenoh::query::{Query, Queryable};
use zenoh_ext::{AdvancedPublisher, AdvancedPublisherBuilderExt, CacheConfig, MissDetectionConfig};

use crate::book::{Book, Changed};
use crate::copied::Copied;
use crate::device_identity;
use crate::hiding::Hiding;
use crate::hotplug::Seen;
use crate::profiles::Profiles;
use crate::setups::{Done, Owed, Owing, Setups};
use crate::store::Store;

/// Heartbeat period for sample-miss recovery on cached publishers: a
/// heartbeat goes out only in a period after a publish, so an idle key
/// costs a subscriber nothing.
const MISS_HEARTBEAT: Duration = Duration::from_millis(500);

/// How hard `identify` runs a pad's motors: plainly felt, short of full.
const BUZZ: Level = Level {
    strong: 0xa000,
    weak: 0xa000,
};

pub(crate) type Rpc = Queryable<FifoChannelHandler<Query>>;

/// The rpc families the reactor answers, one queryable each.
pub(crate) struct Rpcs {
    pub(crate) daemon: Rpc,
    pub(crate) pad: Rpc,
    pub(crate) profile: Rpc,
    pub(crate) setup: Rpc,
}

impl Rpcs {
    /// Declares every family on `session`.
    pub(crate) async fn declare(session: &Session) -> zenoh::Result<Self> {
        Ok(Self {
            daemon: session.declare_queryable(daemon::Incoming::wild()).await?,
            pad: session.declare_queryable(pad::Incoming::wild()).await?,
            profile: session.declare_queryable(profile::Incoming::wild()).await?,
            setup: session.declare_queryable(setup::Incoming::wild()).await?,
        })
    }
}

/// What the reactor owns.
pub(crate) struct Reactor {
    session: Session,
    pub(crate) book: Book,
    store: Store,
    pub(crate) profiles: Profiles,
    pub(crate) setups: Setups,
    /// The input thread, which runs the setups.
    pub(crate) input: Arc<InputThread>,
    /// The layouts of pads whose drivers don't present a gamepad's.
    pub(crate) database: Arc<Database>,
    /// How a setup's pads are kept from games.
    pub(crate) hiding: Arc<Hiding>,
    /// Blocking work under way for the setups, each piece coming back as
    /// what it did.
    pub(crate) work: JoinSet<Done>,
    /// One cached publisher per key published now.
    publishers: BTreeMap<String, AdvancedPublisher<'static>>,
    /// The buzzes under way; each ends itself.
    buzzes: JoinSet<()>,
    /// What has been copied from a connected pad for a virtual pad to
    /// pass as one.
    pub(crate) copied: Copied,
    /// Whether a connected pad is being copied now.
    pub(crate) copying: bool,
    /// Steam's controller blacklist, for pads that are only grabbed.
    pub(crate) blacklist: Arc<std::sync::Mutex<crate::blacklist::Blacklist>>,
    /// Whether the tests' fake pads are taken for pads to play with.
    pub(crate) test_pads: bool,
    /// The daemon's own state, as last published.
    pub(crate) state: Option<payload::Daemon>,
    /// The pads that connected while another was being copied, to try
    /// if that one can't be: a pad another program has hidden can't.
    pub(crate) candidates: Vec<Found>,
    /// Who is owed an answer once work off the reactor comes back.
    pub(crate) owed: BTreeMap<Owing, Owed>,
    /// The last [`Owing`] noted.
    pub(crate) owings: u64,
    /// Whether the hiding service is being looked for, and whether it is
    /// to be looked for again once that look is back.
    looking: bool,
    look_again: bool,
}

impl std::fmt::Debug for Reactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reactor")
            .field("book", &self.book)
            .finish_non_exhaustive()
    }
}

/// Starts a pad buzzing and keeps it buzzing for `millis`. The reply
/// goes once the pad has taken its effect and been told the level,
/// which a thread of the pad's own then sends it, or with why it can't.
async fn buzz(node: PathBuf, millis: u32, responder: Responder<pad::Identify>) {
    let started = tokio::task::spawn_blocking(move || {
        let file = OpenOptions::new().read(true).write(true).open(&node)?;
        let mut pad = Pad::from_fd(OwnedFd::from(file))?;
        pad.rumble(BUZZ)?;
        Ok::<_, ctrlassist_device::PadError>(pad)
    })
    .await;
    let pad = match started {
        Ok(Ok(pad)) => pad,
        Ok(Err(error)) => {
            let refusal =
                WireError::refusal(ErrorCode::PadRefused, format!("the pad refused: {error}"));
            return responder.err(&refusal).await;
        }
        Err(error) => {
            return responder.err(&WireError::internal(error.to_string())).await;
        }
    };
    responder.ok(&rpc::Ack {}).await;
    // The caller's own duration. Dropping the pad stops its motors, here
    // or wherever this task is dropped.
    tokio::time::sleep(Duration::from_millis(u64::from(millis))).await;
    let _ = tokio::task::spawn_blocking(move || drop(pad)).await;
}

impl Reactor {
    #[expect(
        clippy::too_many_arguments,
        reason = "each is one of the things the reactor owns; a struct of them would only move the list"
    )]
    pub(crate) fn new(
        session: Session,
        book: Book,
        store: Store,
        profiles: Profiles,
        input: Arc<InputThread>,
        database: Database,
        hiding: Hiding,
        copied: Copied,
    ) -> Self {
        Self {
            session,
            book,
            store,
            profiles,
            setups: Setups::default(),
            input,
            database: Arc::new(database),
            hiding: Arc::new(hiding),
            work: JoinSet::new(),
            publishers: BTreeMap::new(),
            buzzes: JoinSet::new(),
            copied,
            copying: false,
            blacklist: Arc::new(std::sync::Mutex::new(crate::blacklist::Blacklist::none())),
            test_pads: false,
            state: None,
            candidates: Vec::new(),
            owed: BTreeMap::new(),
            owings: 0,
            looking: false,
            look_again: false,
        }
    }

    /// Publishes the daemon's own state.
    pub(crate) async fn publish_daemon(&mut self, state: payload::Daemon) -> zenoh::Result<()> {
        self.publish(keys::daemon(), &state).await?;
        self.state = Some(state);
        Ok(())
    }

    /// Says how pads are hidden now, if that isn't what was last said:
    /// the hiding service may be found after the daemon started, and its
    /// connection may break. What was found is read as it stands, never
    /// waiting on a request to the service under way.
    pub(crate) async fn publish_hiding(&mut self) {
        let (now, release) = (self.hiding.mechanism(), self.hiding.service_release());
        let Some(state) = self
            .state
            .clone()
            .filter(|state| state.hiding != now || state.hider_version != release)
        else {
            return;
        };
        let state = payload::Daemon {
            hiding: now,
            hider_version: release,
            ..state
        };
        if let Err(error) = self.publish_daemon(state).await {
            warn!(%error, "publishing the daemon's state failed");
        }
    }

    /// Looks for the hiding service, off the reactor: as the daemon
    /// starts, and each time the service's socket is made or made to take
    /// connections while it isn't known to be there. What was found is published when the look comes
    /// back, and the pads grabbed for want of the service are hidden if
    /// it is there. One look at a time: a socket made during one has
    /// another once it is back.
    pub(crate) fn look_for_the_hiding_service(&mut self) {
        if self.looking {
            self.look_again = true;
            return;
        }
        self.looking = true;
        let hiding = Arc::clone(&self.hiding);
        self.work.spawn_blocking(move || {
            hiding.look();
            Done::Looked
        });
    }

    /// A look for the hiding service is back.
    pub(crate) fn looked(&mut self) {
        self.looking = false;
        if std::mem::take(&mut self.look_again) {
            self.look_for_the_hiding_service();
        }
    }

    /// Publishes what a virtual pad can present itself as, and whether
    /// each can be made now: through uinput, or for a DualSense through
    /// uhid.
    pub(crate) async fn publish_identities(&mut self) -> zenoh::Result<()> {
        let writable = |node: &str| OpenOptions::new().write(true).open(node).is_ok();
        let uinput = writable("/dev/uinput");
        for identity in Identity::ALL {
            let device = device_identity(identity);
            let (vendor, product) = device.model();
            let available = match identity {
                Identity::DualSense => writable("/dev/uhid"),
                _ => uinput,
            };
            let body = payload::Identity {
                id: identity.as_str().to_string(),
                name: device.name().to_string(),
                vendor,
                product,
                available,
            };
            self.publish(keys::identity(identity.as_str()), &body)
                .await?;
        }
        Ok(())
    }

    /// Publishes `body` on `key`, from a cache.
    pub(crate) async fn publish(
        &mut self,
        key: String,
        body: &impl Serialize,
    ) -> zenoh::Result<()> {
        let bytes = serde_json::to_vec(body)?;
        if !self.publishers.contains_key(&key) {
            let publisher = self
                .session
                .declare_publisher(key.clone())
                .encoding(Encoding::APPLICATION_JSON)
                .cache(CacheConfig::default().max_samples(1))
                .sample_miss_detection(
                    MissDetectionConfig::default().sporadic_heartbeat(MISS_HEARTBEAT),
                )
                .publisher_detection()
                .await?;
            self.publishers.insert(key.clone(), publisher);
        }
        self.publishers[&key].put(bytes).await
    }

    /// Takes `key` back: subscribers see it deleted.
    pub(crate) async fn unpublish(&mut self, key: &str) {
        if let Some(publisher) = self.publishers.remove(key)
            && let Err(error) = publisher.delete().await
        {
            warn!(key, %error, "withdrawing state failed");
        }
    }

    /// Publishes every pad in the book.
    pub(crate) async fn publish_pads(&mut self) -> zenoh::Result<()> {
        let ids: Vec<String> = self.book.ids().map(str::to_string).collect();
        for id in ids {
            self.publish_pad(&id).await?;
        }
        Ok(())
    }

    async fn publish_pad(&mut self, id: &str) -> zenoh::Result<()> {
        match self.book.pad(id) {
            Some(pad) => self.publish(keys::pad(&keys::pad_id(id)), &pad).await,
            None => Ok(()),
        }
    }

    /// Publishes every profile kept.
    pub(crate) async fn publish_profiles(&mut self) -> zenoh::Result<()> {
        let names: Vec<String> = self.profiles.names().map(str::to_string).collect();
        for name in names {
            self.publish_profile(&name).await?;
        }
        Ok(())
    }

    async fn publish_profile(&mut self, name: &str) -> zenoh::Result<()> {
        match self.profiles.profile(name) {
            Some(profile) => self.publish(keys::profile(name), &profile).await,
            None => Ok(()),
        }
    }

    async fn profile_rpc(&mut self, query: Query) {
        match profile::Incoming::receive(query).await {
            Some(((), profile::Incoming::Validate(request, responder))) => {
                let validation = rpc::Validation {
                    problem: Profiles::problem(&request.toml),
                };
                responder.ok(&validation).await;
            }
            Some(((), profile::Incoming::Schema((), responder))) => {
                let form = rpc::PresetForm {
                    fields: ctrlassist_protocol::form::fields(),
                };
                responder.ok(&form).await;
            }
            Some(((), profile::Incoming::Put(request, responder))) => {
                let (name, toml) = (request.name.clone(), request.toml.clone());
                let put = self
                    .keep_profiles(move |profiles| profiles.put(&name, &toml))
                    .await;
                match put {
                    Ok(()) => {
                        info!(profile = request.name, "a profile was put");
                        if let Err(error) = self.publish_profile(&request.name).await {
                            warn!(profile = request.name, %error, "publishing a profile failed");
                        }
                        responder.ok(&rpc::Ack {}).await;
                        self.profile_changed(&request.name).await;
                        self.start_if_it_can(&request.name).await;
                    }
                    Err(refusal) => responder.err(&refusal).await,
                }
            }
            Some(((), profile::Incoming::Delete(request, responder))) => {
                let name = request.name.clone();
                let deleted = self
                    .keep_profiles(move |profiles| profiles.delete(&name))
                    .await;
                match deleted {
                    Ok(()) => {
                        info!(profile = request.name, "a profile was deleted");
                        self.unpublish(&keys::profile(&request.name)).await;
                        responder.ok(&rpc::Ack {}).await;
                        self.profile_changed(&request.name).await;
                    }
                    Err(refusal) => responder.err(&refusal).await,
                }
            }
            // Refused already, with a typed error.
            None => {}
        }
    }

    /// Publishes a pad's state; a failure is logged.
    pub(crate) async fn publish_pad_logged(&mut self, id: &str) {
        if let Err(error) = self.publish_pad(id).await {
            warn!(pad = id, %error, "publishing a pad's state failed");
        }
    }

    /// Changes the profiles off the async runtime's workers, since that
    /// writes and syncs files: on a copy, which is kept once changed. The
    /// reactor waits for it, so nothing else sees the profiles meanwhile.
    /// What the change says comes back; nothing if it panicked, which
    /// leaves them as they were.
    async fn change_profiles<T: Send + 'static>(
        &mut self,
        change: impl FnOnce(&mut Profiles) -> T + Send + 'static,
    ) -> Option<T> {
        let mut profiles = self.profiles.clone();
        let changed = tokio::task::spawn_blocking(move || {
            let said = change(&mut profiles);
            (profiles, said)
        })
        .await;
        match changed {
            Ok((profiles, said)) => {
                self.profiles = profiles;
                Some(said)
            }
            Err(error) => {
                error!(%error, "changing the profiles panicked; they are as they were");
                None
            }
        }
    }

    /// Puts or deletes a profile, as [`change_profiles`](Self::change_profiles).
    async fn keep_profiles(
        &mut self,
        change: impl FnOnce(&mut Profiles) -> Result<(), WireError> + Send + 'static,
    ) -> Result<(), WireError> {
        self.change_profiles(change)
            .await
            .unwrap_or_else(|| Err(WireError::internal("changing the profiles failed")))
    }

    /// Saves the known pads, off the async runtime's workers.
    async fn save(&self) {
        let (store, known) = (self.store.clone(), self.book.known());
        match tokio::task::spawn_blocking(move || store.save(&known)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                warn!(%error, "the known pads couldn't be saved; they are kept until the daemon stops");
            }
            Err(error) => error!(%error, "saving the known pads panicked"),
        }
    }

    /// Takes in what the hotplug watch saw.
    pub(crate) async fn saw(&mut self, seen: Seen) {
        match seen {
            Seen::Arrived(found) => self.took_in(found).await,
            // A pad whose node went for a re-bind the hiding service
            // asked is away, and nothing is said of it unless its anchor
            // goes.
            Seen::Left(syspath) => {
                let gone = self.book.left(&syspath).into_iter().collect();
                self.gone(gone).await;
            }
            Seen::Unplugged(device) => {
                let gone = self.book.unplugged(&device);
                self.gone(gone).await;
            }
            Seen::Rebinding { anchor, present } => self.book.rebinding(&anchor, &present),
            // Looked for unless it is there as far as the daemon knows: a
            // look would start a service nothing needs. A service gone
            // from under hidden pads is known gone as they come back.
            Seen::HiderSocket => {
                if self.hiding.mechanism() != payload::Hiding::Service {
                    self.look_for_the_hiding_service();
                }
            }
            Seen::Rumble {
                node,
                syspath,
                rumble,
            } => {
                if let Some(id) = self.book.learned(&node, &syspath, rumble) {
                    self.save().await;
                    self.publish_pad_logged(&id).await;
                }
            }
            Seen::Overran { connected, anchors } => {
                warn!(
                    "udev's events overran the hotplug watch; what is connected was looked at again"
                );
                let gone = self.book.reconcile(&connected, &anchors);
                self.gone(gone).await;
                for found in connected {
                    self.took_in(found).await;
                }
            }
        }
    }

    /// Takes in a pad connected, or one already connected changed.
    async fn took_in(&mut self, found: Found) {
        if found.identifiers.is_test_pad() && !self.test_pads {
            return;
        }
        self.copy_from(&found);
        let changed = self.book.arrived(&found);
        // The last is the pad that connected, if one did. One back on the
        // anchor it was away from, as from the hiding service hiding it or
        // showing it again, came back by itself: nobody plugged it in, and
        // to its clients it never left.
        let restored = changed
            .last()
            .is_some_and(|pad| self.book.was_restored(&pad.id));
        match changed.last() {
            Some(pad) if restored => info!(
                pad = pad.id,
                name = found.identifiers.name,
                "a pad is back from a re-bind of its driver"
            ),
            Some(pad) => info!(
                pad = pad.id,
                name = found.identifiers.name,
                "a pad connected"
            ),
            None => {}
        }
        if changed.iter().any(|changed| changed.remembered) {
            self.save().await;
        }
        for changed in &changed {
            self.publish_pad_logged(&changed.id).await;
        }
        if let Some(Changed { id, .. }) = changed.last() {
            // One back from a re-bind prompts nothing: a setup stopped by
            // hand would otherwise start again at once.
            self.pad_arrived(id).await;
            if !restored {
                // A profile that was waiting for this pad may start now.
                self.start_what_can(Some(id)).await;
            }
        }
    }

    /// Says that each of `gone` has disconnected.
    async fn gone(&mut self, gone: Vec<String>) {
        for pad in &gone {
            info!(pad, "a pad disconnected");
            self.publish_pad_logged(pad).await;
        }
    }

    async fn daemon_rpc(&mut self, query: Query) {
        match daemon::Incoming::receive(query).await {
            Some(((), daemon::Incoming::Info((), responder))) => {
                let info = rpc::ServiceInfo {
                    version: VERSION.to_string(),
                    keys: self.publishers.keys().cloned().collect(),
                    config: String::new(),
                };
                responder.ok(&info).await;
            }
            Some(((), daemon::Incoming::Reload((), responder))) => {
                let reloaded = self.reload().await;
                responder.ok(&reloaded).await;
            }
            // Refused already, with a typed error.
            None => {}
        }
    }

    /// Reads the daemon's files again, publishes what changed, starts
    /// what can now start by itself, and sets the running setups whose
    /// profiles changed to taking them up.
    async fn reload(&mut self) -> rpc::Reloaded {
        let (changed, removed) = self
            .change_profiles(Profiles::reload)
            .await
            .unwrap_or_default();
        info!(?changed, ?removed, "the profiles were read again");
        for name in &removed {
            self.unpublish(&keys::profile(name)).await;
        }
        let mut wrong = Vec::new();
        for name in &changed {
            if let Err(error) = self.publish_profile(name).await {
                warn!(profile = name, %error, "publishing a profile failed");
            }
            wrong.extend(
                self.profiles
                    .profile(name)
                    .filter(|profile| profile.problem.is_some()),
            );
        }
        let updating = self.update_reloaded(&changed).await;
        // One that wasn't updated, or whose profile is gone, runs
        // another text than is kept.
        for name in changed.iter().chain(&removed) {
            self.profile_changed(name).await;
        }
        for name in &changed {
            if !updating.contains(name) {
                self.start_if_it_can(name).await;
            }
        }
        rpc::Reloaded {
            changed,
            removed,
            wrong,
            updating,
        }
    }

    /// The node to buzz for an `identify`, or why not; no node for a pad
    /// a setup holds, which has been buzzed through the input thread.
    pub(crate) fn buzzable(
        &self,
        request: &rpc::IdentifyRequest,
    ) -> Result<Option<PathBuf>, WireError> {
        if !(1..=rpc::IDENTIFY_MAX_MS).contains(&request.millis) {
            return Err(WireError::bad_request(format!(
                "a buzz lasts from 1 to {} ms, not {}",
                rpc::IDENTIFY_MAX_MS,
                request.millis
            )));
        }
        // A setup's virtual pad is rumbled as a game rumbles it, on its
        // own node: what the setup makes of that is the point.
        if let Some((setup, sink)) = rpc::parse_virtual_pad(&request.pad) {
            let node = self
                .setups
                .setup(setup)
                .and_then(|setup| setup.sinks.into_iter().find(|made| made.name == sink))
                .and_then(|made| made.node);
            return match node {
                Some(node) => Ok(Some(PathBuf::from(node))),
                None => Err(WireError::refusal(
                    ErrorCode::UnknownPad,
                    format!("no setup has a virtual pad {setup}/{sink} now"),
                )),
            };
        }
        let id = self.book.resolve(&request.pad)?;
        // A pad that isn't there is said to be that, before anything it
        // lacks: one away is there in a moment, as its driver is
        // re-bound. One a setup holds is reached through the setup.
        let node = match self.book.user(id) {
            Some(_) => None,
            None if self.book.is_away(id) => {
                return Err(WireError::refusal(
                    ErrorCode::PadRefused,
                    format!("{id} is away for a moment, its driver being re-bound; try again"),
                ));
            }
            None => Some(self.book.node(id).ok_or_else(|| {
                WireError::refusal(ErrorCode::PadDisconnected, format!("{id} isn't connected"))
            })?),
        };
        if self.book.rumble(id) == Some(false) {
            return Err(WireError::refusal(
                ErrorCode::PadWithoutRumble,
                format!("{id} has no rumble to buzz with"),
            ));
        }
        let Some(node) = node else {
            let setup = self.book.user(id).unwrap_or_default();
            return self.buzz(setup, id, BUZZ, request.millis).map(|()| None);
        };
        Ok(Some(node.to_path_buf()))
    }

    async fn pad_rpc(&mut self, query: Query) {
        match pad::Incoming::receive(query).await {
            Some(((), pad::Incoming::Identify(request, responder))) => {
                match self.buzzable(&request) {
                    Ok(Some(node)) => {
                        self.buzzes.spawn(buzz(node, request.millis, responder));
                    }
                    Ok(None) => responder.ok(&rpc::Ack {}).await,
                    Err(refusal) => responder.err(&refusal).await,
                }
            }
            Some(((), pad::Incoming::Forget(request, responder))) => {
                match self.book.forget(&request.pad) {
                    Ok(id) => {
                        self.save().await;
                        self.unpublish(&keys::pad(&keys::pad_id(&id))).await;
                        info!(pad = id, "a pad was forgotten");
                        responder.ok(&rpc::Ack {}).await;
                    }
                    Err(refusal) => responder.err(&refusal).await,
                }
            }
            // Refused already, with a typed error.
            None => {}
        }
    }

    /// Runs until told to stop. What it published goes with it.
    pub(crate) async fn run(
        mut self,
        rpcs: Rpcs,
        mut seen: UnboundedReceiver<Seen>,
        mut reports: crate::Reports,
        mut stop: oneshot::Receiver<()>,
    ) {
        loop {
            tokio::select! {
                _ = &mut stop => break,

                Ok(query) = rpcs.daemon.recv_async() => self.daemon_rpc(query).await,
                Ok(query) = rpcs.pad.recv_async() => self.pad_rpc(query).await,
                Ok(query) = rpcs.profile.recv_async() => self.profile_rpc(query).await,
                Ok(query) = rpcs.setup.recv_async() => self.setup_rpc(query).await,
                report = reports.recv() => match report {
                    Some(report) => self.reported(report).await,
                    // The input thread ended though nothing stopped it:
                    // nothing it was asked will be reported, so a stop or
                    // an update waiting on it would hold its caller for as
                    // long as the daemon lives. The daemon ends instead,
                    // as when its reactor panics: every rpc held is
                    // dropped, which ends its call.
                    None => {
                        error!("the input thread ended by itself; no setup can run");
                        break;
                    }
                },
                Some(joined) = self.work.join_next() => match joined {
                    Ok(done) => self.done(done).await,
                    // Each piece catches its own panics and comes back
                    // all the same; one that didn't is lost.
                    Err(error) => error!(%error, "work done off the reactor was lost"),
                },
                Some(seen) = seen.recv() => self.saw(seen).await,
                // A finished buzz has nothing to say, unless it panicked.
                Some(joined) = self.buzzes.join_next() => {
                    if let Err(error) = joined {
                        error!(%error, "a buzz was lost");
                    }
                }
            }
        }
        let Rpcs {
            daemon,
            pad,
            profile,
            setup,
        } = &rpcs;
        self.wind_down(&[daemon, pad, profile, setup]).await;
    }

    /// Ends the reactor's work, waiting on nothing: every rpc it holds,
    /// and every one queued on `queued`, is answered that the daemon is
    /// stopping, and the work still out is given up. A setup's start or
    /// update caught by a restart says so, and isn't dropped unanswered.
    pub(crate) async fn wind_down(&mut self, queued: &[&Rpc]) {
        // Nothing waits on the hiding service from here: every request
        // under way is hung up, which ends it at once, and the service
        // shows again what each connection hid as it closes.
        self.hiding.close();
        self.answer_as_stopping().await;
        let refusal = WireError::daemon_shutting_down();
        for rpcs in queued {
            while let Ok(Some(query)) = rpcs.try_recv() {
                serve::refuse(query, &refusal).await;
            }
        }
        // Pads still buzzing stop as their tasks are dropped.
        self.buzzes.shutdown().await;
        // What is being made or dropped is left to finish by itself, off
        // the runtime's workers, and what it made is dropped there; the
        // setups themselves go with the input thread.
        self.work.detach_all();
    }
}
