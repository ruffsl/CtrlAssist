#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

mod error;
mod handshake;
mod lifetime;
mod mirror;

use std::future::Future;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use ctrlassist_protocol::bootstrap;
/// The socket a client connects to when it isn't given one: the daemon's,
/// in the user's runtime directory.
pub use ctrlassist_protocol::bootstrap::default_socket_path;
use ctrlassist_protocol::caller::Caller;
use ctrlassist_protocol::endpoint::{Unary, daemon, pad, profile, setup, tray};
pub use ctrlassist_protocol::error::ErrorCode;
pub use ctrlassist_protocol::form;
pub use ctrlassist_protocol::payload::{
    Daemon as DaemonState, Hiding, Identity, Pad, Phase, Problem, Profile, Setup, SetupNode,
    SetupSink, SetupSource,
};
use ctrlassist_protocol::rpc;
pub use ctrlassist_protocol::rpc::{
    NotificationSettings, NotificationSettingsPatch, ReloadReport, Reloaded, ServiceInfo,
};
pub use error::{BoxError, Error, Result, Service};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, warn};

use handshake::Handshake;
use lifetime::LifetimeMirror;
use mirror::{DaemonMirror, TrayMirror};

/// Change notifications from the clients' mirrors and their services'
/// liveliness.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StateEvent {
    /// The daemon's own state changed in the mirror, or left it because
    /// the daemon went away. Re-read [`DaemonClient::daemon_state`].
    DaemonStateChanged,
    /// A pad's state changed in the mirror: it connected or left, the
    /// daemon came to know it or forgot it, or it left the mirror because
    /// the daemon went away. Re-read [`DaemonClient::list_pads`].
    PadChanged {
        /// The pad, by the id it is known by across reconnects.
        pad: String,
    },
    /// A profile's state changed in the mirror: it was put or deleted, or
    /// it left the mirror because the daemon went away. Re-read
    /// [`DaemonClient::list_profiles`].
    ProfileChanged {
        /// The profile's name.
        profile: String,
    },
    /// A setup's state changed in the mirror: it was started or stopped,
    /// it moved between phases, a pad of its came or went, or it left the
    /// mirror because the daemon went away. Re-read
    /// [`DaemonClient::list_setups`].
    SetupChanged {
        /// The setup's name: the profile it runs.
        setup: String,
    },
    /// What a virtual pad can present itself as changed, or left the
    /// mirror because the daemon went away. Re-read
    /// [`DaemonClient::list_identities`].
    IdentitiesChanged,
    /// The daemon's liveliness token appeared: it's running and serving
    /// rpcs. Can repeat for one lifetime, so treat it idempotently.
    DaemonAppeared,
    /// The daemon's liveliness token went away (stopped, crashed, or the
    /// link dropped). Its state has already left the mirror, announced
    /// before this event, and [`DaemonClient::service_info`] has cleared.
    DaemonDisappeared,
    /// The tray's liveliness token appeared: its settings service is
    /// running. Can repeat for one lifetime, so treat it idempotently.
    TrayAppeared,
    /// The tray's liveliness token went away: the tray stopped, or the
    /// daemon it serves through did. Its switches have already left the
    /// mirror, announced before this event, and
    /// [`TrayClient::service_info`] has cleared.
    TrayDisappeared,
    /// The tray's notification switches changed in the mirror, or left
    /// it because the tray went away. Re-read
    /// [`TrayClient::notification_settings`].
    TrayNotificationSettingsChanged,
    /// Re-read everything: the watcher's periodic tick
    /// ([`StateWatcherConfig::fallback_poll_interval`]), also sent when
    /// the watcher fell behind and dropped events.
    FallbackPoll,
    /// A service's release check settled for its current lifetime, so
    /// its client's `service_info` now answers. Sent once per service
    /// lifetime, after its `*Appeared`, whatever the verdict; a watcher
    /// built after the check settled gets it on start, and may then see
    /// it twice.
    ReleaseChecked(Service),
    /// A service's cached state has arrived in the mirror for the
    /// current lifetime, so an empty read now means empty rather than not
    /// yet known: until then, show the service as loading. Sent once per
    /// service lifetime, after its `*Appeared`; a watcher built later
    /// reads its client's `is_hydrated` instead.
    Hydrated(Service),
    /// A service runs a release this client can't talk to: every rpc to
    /// it fails until it's restarted on a matching release. Sent once per
    /// service lifetime, right after its [`Self::ReleaseChecked`]; a
    /// watcher built while the mismatch is already known gets it on
    /// start, and may then see it twice.
    VersionMismatch {
        /// The mismatched service.
        service: Service,
        /// The protocol release this SDK speaks.
        ours: String,
        /// The service's release.
        theirs: String,
    },
}

const EVENT_CAPACITY: usize = 256;

/// Client for the daemon: mirror reads, rpc writes.
#[derive(Debug, Clone)]
pub struct DaemonClient {
    session: zenoh::Session,
    caller: Arc<Caller>,
    mirror: LifetimeMirror<DaemonMirror>,
    events: broadcast::Sender<StateEvent>,
    handshake: Arc<Handshake>,
}

impl DaemonClient {
    /// Connect to the daemon socket and start mirroring.
    ///
    /// This succeeds whether or not the daemon is running (see the crate
    /// docs).
    ///
    /// # Errors
    ///
    /// Fails only with [`Error::Transport`], when the local session or
    /// its subscriptions can't be set up.
    pub async fn connect(socket: &Path) -> Result<Self> {
        let session = zenoh::open(
            bootstrap::client_config_resilient(socket)
                .map_err(|e| Error::transport("build the client config", e))?,
        )
        .await
        .map_err(|e| Error::transport("connect to the daemon socket", e))?;
        Self::with_session(session).await
    }

    /// Start mirroring over a zenoh `session` the caller already holds on
    /// the daemon's socket, rather than opening one. [`Self::connect`]
    /// opens a session that keeps dialing for as long as the client
    /// lives; a command that runs once and wants to fail at once when no
    /// daemon is listening opens its own, with
    /// `ctrlassist_protocol::bootstrap::client_config`, and hands it over.
    ///
    /// # Errors
    ///
    /// Fails only with [`Error::Transport`].
    pub async fn with_session(session: zenoh::Session) -> Result<Self> {
        let caller = Arc::new(Caller::new(session.clone()));
        let mirror = LifetimeMirror::<DaemonMirror>::start(&session, &caller).await?;
        Ok(Self {
            session,
            caller,
            events: mirror.events().clone(),
            handshake: Arc::clone(mirror.handshake()),
            mirror,
        })
    }

    /// Whether the daemon's liveliness token is present: a mirror read.
    /// `true` doesn't promise the next rpc succeeds (the daemon may be
    /// stopping, or run another release).
    #[must_use]
    pub fn is_available(&self) -> bool {
        self.mirror.is_available()
    }

    /// Whether the daemon's cached state has arrived in the mirror for
    /// its current lifetime ([`StateEvent::Hydrated`]): while it's up and
    /// this is `false`, an empty read means not yet known. A mirror read.
    #[must_use]
    pub fn is_hydrated(&self) -> bool {
        self.mirror.is_hydrated()
    }

    /// What this lifetime's release check learned: the daemon's release.
    /// `None` while the daemon is down and until the check settles
    /// ([`StateEvent::ReleaseChecked`]). Ungated, so it reports a
    /// mismatched release too. A mirror read; [`Self::version`] asks the
    /// daemon instead.
    #[must_use]
    pub fn service_info(&self) -> Option<ServiceInfo> {
        self.handshake.info()
    }

    /// The daemon's own state, as last published: a mirror read, no rpc.
    /// `None` until it arrives, and while the daemon is down.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::VersionMismatch`] when the daemon is known to
    /// run another release.
    pub fn daemon_state(&self) -> Result<Option<DaemonState>> {
        self.refuse_known_mismatch()?;
        Ok(self.mirror.read(|mirror| mirror.daemon.clone()))
    }

    /// The pads the daemon knows, connected or remembered, in the order
    /// of their ids: a mirror read, no rpc.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::VersionMismatch`] when the daemon is known to
    /// run another release.
    pub fn list_pads(&self) -> Result<Vec<Pad>> {
        self.refuse_known_mismatch()?;
        let mut pads: Vec<Pad> = self
            .mirror
            .read(|mirror| mirror.pads.values().cloned().collect());
        pads.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(pads)
    }

    /// What a virtual pad can present itself as: a mirror read, no rpc.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::VersionMismatch`] when the daemon is known to
    /// run another release.
    pub fn list_identities(&self) -> Result<Vec<Identity>> {
        self.refuse_known_mismatch()?;
        Ok(self
            .mirror
            .read(|mirror| mirror.identities.values().cloned().collect()))
    }

    /// The profiles the daemon keeps, in the order of their names: a
    /// mirror read, no rpc. One with a [`Profile::problem`] is kept but
    /// can't be run until it is put right.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::VersionMismatch`] when the daemon is known to
    /// run another release.
    pub fn list_profiles(&self) -> Result<Vec<Profile>> {
        self.refuse_known_mismatch()?;
        Ok(self
            .mirror
            .read(|mirror| mirror.profiles.values().cloned().collect()))
    }

    /// The profile kept under `name`, if one is: a mirror read, no rpc.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::VersionMismatch`] when the daemon is known to
    /// run another release.
    pub fn profile(&self, name: &str) -> Result<Option<Profile>> {
        self.refuse_known_mismatch()?;
        Ok(self
            .mirror
            .read(|mirror| mirror.profiles.get(name).cloned()))
    }

    /// Have the daemon check a profile's TOML without keeping it: what is
    /// wrong with it and where, or `None` for one it would keep.
    ///
    /// # Errors
    ///
    /// Fails as any rpc can.
    pub async fn validate_profile(&self, toml: &str) -> Result<Option<Problem>> {
        let request = rpc::ValidateRequest {
            toml: toml.to_string(),
        };
        Ok(self.call::<profile::Validate>(&request).await?.problem)
    }

    /// Keep `toml` as the profile `name`: a new one, or in place of the
    /// one that has the name.
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::InvalidProfile`], whose message says
    /// where the profile is wrong and what, or with
    /// [`ErrorCode::BadRequest`] for a name that isn't 1 to 32 lowercase
    /// letters, digits and hyphens; and fails as any rpc can.
    pub async fn put_profile(&self, name: &str, toml: &str) -> Result<()> {
        let request = rpc::PutProfileRequest {
            name: name.to_string(),
            toml: toml.to_string(),
        };
        self.call::<profile::Put>(&request).await.map(|_| ())
    }

    /// Stop keeping the profile `name`.
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::UnknownProfile`]; and fails as any rpc
    /// can.
    pub async fn delete_profile(&self, name: &str) -> Result<()> {
        let request = rpc::ProfileRequest {
            name: name.to_string(),
        };
        self.call::<profile::Delete>(&request).await.map(|_| ())
    }

    /// The setups that exist, in the order of their names: a mirror read,
    /// no rpc.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::VersionMismatch`] when the daemon is known to
    /// run another release.
    pub fn list_setups(&self) -> Result<Vec<Setup>> {
        self.refuse_known_mismatch()?;
        Ok(self
            .mirror
            .read(|mirror| mirror.setups.values().cloned().collect()))
    }

    /// The setup called `name`, if there is one: a mirror read, no rpc.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::VersionMismatch`] when the daemon is known to
    /// run another release.
    pub fn setup(&self, name: &str) -> Result<Option<Setup>> {
        self.refuse_known_mismatch()?;
        Ok(self.mirror.read(|mirror| mirror.setups.get(name).cloned()))
    }

    /// Run the kept profile `name` as a setup of the same name. Returns
    /// once its virtual pads exist; it is then running, or held if a pad
    /// it needs isn't connected, which its state says.
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::UnknownProfile`],
    /// [`ErrorCode::InvalidProfile`], [`ErrorCode::SetupExists`],
    /// [`ErrorCode::PadInUse`], or [`ErrorCode::SetupFailed`] with why;
    /// and fails as any rpc can.
    pub async fn start_setup(&self, name: &str) -> Result<()> {
        let request = rpc::SetupRequest {
            name: name.to_string(),
        };
        self.call::<setup::Start>(&request).await.map(|_| ())
    }

    /// Stop the setup `name`. Returns once its virtual pads are gone.
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::UnknownSetup`], or with
    /// [`ErrorCode::SetupBusy`] while it is starting or stopping; and
    /// fails as any rpc can.
    pub async fn stop_setup(&self, name: &str) -> Result<()> {
        let request = rpc::SetupRequest {
            name: name.to_string(),
        };
        self.call::<setup::Stop>(&request).await.map(|_| ())
    }

    /// Have the setup `name` take up its profile as it is kept now, after
    /// a [`Self::put_profile`]; with `apply` false, only ask what that
    /// would cost. Returns whether it is disruptive: whether the setup's
    /// virtual pads are made anew, which a game sees as its pad
    /// disconnecting. Every update is, for now. With `apply`, returns
    /// once the setup runs again.
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::UnknownSetup`]; with
    /// [`ErrorCode::InvalidProfile`] or [`ErrorCode::UnknownProfile`] if
    /// what is kept now can't be run, the setup then carrying on as it
    /// was; with [`ErrorCode::SetupBusy`] while it is starting or
    /// stopping; or as [`Self::start_setup`] is; and fails as any rpc can.
    pub async fn update_setup(&self, name: &str, apply: bool) -> Result<bool> {
        let request = rpc::UpdateRequest {
            name: name.to_string(),
            apply,
        };
        Ok(self.call::<setup::Update>(&request).await?.disruptive)
    }

    /// Have a node of the setup `setup` select another input, for a mux
    /// that follows its switch, or another output, for a demux that
    /// sends to one at a time: `choice` by its name among the node's
    /// [`SetupNode::choices`] or by its place among them, or `None` to
    /// move to the next, as the node's switch button would. Returns once
    /// the daemon has passed the request on; the setup's state says when
    /// the node has moved, and selecting what is selected already changes
    /// nothing.
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::UnknownSetup`], with
    /// [`ErrorCode::SetupBusy`] for a setup that isn't running, or with
    /// [`ErrorCode::BadRequest`] for a node that isn't one of the setup's
    /// that selects or a choice that isn't one of the node's, saying
    /// which there are; and fails as any rpc can.
    pub async fn select(&self, setup: &str, node: &str, choice: Option<&str>) -> Result<()> {
        let request = rpc::SelectRequest {
            name: setup.to_string(),
            node: node.to_string(),
            choice: choice.map(str::to_string),
        };
        self.call::<setup::Select>(&request).await.map(|_| ())
    }

    /// Buzz a pad for `duration`, so a person can tell which one it is.
    /// `pad` is its id, a bare address, or `model:<vendor>:<product>`
    /// when one known pad is of that model. Returns once the buzz has
    /// started.
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::UnknownPad`], [`ErrorCode::AmbiguousPad`],
    /// [`ErrorCode::PadDisconnected`], [`ErrorCode::PadWithoutRumble`] or
    /// [`ErrorCode::PadRefused`], or with [`ErrorCode::BadRequest`] for a
    /// duration of nothing or of more than five seconds; and fails as any
    /// rpc can.
    pub async fn identify_pad(&self, pad: &str, duration: Duration) -> Result<()> {
        let request = rpc::IdentifyRequest {
            pad: pad.to_string(),
            // Longer than the daemon accepts either way.
            millis: u32::try_from(duration.as_millis()).unwrap_or(u32::MAX),
        };
        self.call::<pad::Identify>(&request).await.map(|_| ())
    }

    /// Stop remembering a pad that isn't connected. `pad` names it as for
    /// [`Self::identify_pad`].
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::UnknownPad`], [`ErrorCode::AmbiguousPad`]
    /// or [`ErrorCode::PadConnected`]; and fails as any rpc can.
    pub async fn forget_pad(&self, pad: &str) -> Result<()> {
        let request = rpc::PadRequest {
            pad: pad.to_string(),
        };
        self.call::<pad::Forget>(&request).await.map(|_| ())
    }

    /// A preset's fields, as the daemon describes them: what a form
    /// shows, and with [`form::to_toml`] and [`form::from_toml`] how its
    /// values become a profile and come back from one.
    ///
    /// # Errors
    ///
    /// Fails as any rpc can.
    pub async fn preset_form(&self) -> Result<Vec<form::Field>> {
        Ok(self.call::<profile::Schema>(&()).await?.fields)
    }

    /// Has the daemon read its files again, for edits made some other
    /// way than through it, and bring what runs up to them. Says what
    /// it found changed and which setups it set about updating; those
    /// are still changing when this returns, and their state says how
    /// they fare.
    ///
    /// # Errors
    ///
    /// Fails as any rpc can.
    pub async fn reload(&self) -> Result<Reloaded> {
        self.call::<daemon::Reload>(&()).await
    }

    /// The daemon's release. Ungated, so it reports what's running even
    /// across a mismatch.
    ///
    /// # Errors
    ///
    /// Fails as any rpc can, except with [`Error::VersionMismatch`].
    pub async fn version(&self) -> Result<String> {
        Ok(self.info().await?.version)
    }

    /// Check that the daemon is there and runs a release this client can
    /// talk to: the gate every other rpc passes first, on its own.
    ///
    /// # Errors
    ///
    /// Fails as any rpc can, with [`Error::VersionMismatch`] for a daemon
    /// on another release line.
    pub async fn check_release(&self) -> Result<()> {
        self.handshake.ensure(&self.events, || self.info()).await
    }

    async fn info(&self) -> Result<ServiceInfo> {
        self.caller
            .call::<daemon::Info>(&(), &())
            .await
            .map_err(|e| Error::from_rpc(Service::Daemon, e))
    }

    fn subscribe_events(&self) -> broadcast::Receiver<StateEvent> {
        self.events.subscribe()
    }

    /// Call a daemon rpc, once the daemon's release is known to match.
    async fn call<R: Unary<Target = ()>>(&self, request: &R::Request) -> Result<R::Response> {
        self.check_release().await?;
        self.caller
            .call::<R>(&(), request)
            .await
            .map_err(|e| Error::from_rpc(Service::Daemon, e))
    }

    /// Fail when the daemon is already known to run another release: the
    /// gate the mirror reads share, which doesn't ask.
    fn refuse_known_mismatch(&self) -> Result<()> {
        match self.handshake.known_mismatch() {
            Some(mismatch) => Err(mismatch),
            None => Ok(()),
        }
    }
}

/// Client for the tray's settings service: availability from its
/// liveliness token, its notification switches from what it publishes,
/// writes by rpc. The tray serves on the daemon's socket, through the
/// daemon, so a tray is only ever seen while a daemon runs.
#[derive(Debug, Clone)]
pub struct TrayClient {
    caller: Arc<Caller>,
    mirror: LifetimeMirror<TrayMirror>,
    events: broadcast::Sender<StateEvent>,
    handshake: Arc<Handshake>,
}

impl TrayClient {
    /// Connect to the daemon's socket and start mirroring the tray. Like
    /// [`DaemonClient::connect`], this needs neither the daemon nor the
    /// tray running.
    ///
    /// # Errors
    ///
    /// Fails only with [`Error::Transport`].
    pub async fn connect(socket: &Path) -> Result<Self> {
        let session = zenoh::open(
            bootstrap::client_config_resilient(socket)
                .map_err(|e| Error::transport("build the client config", e))?,
        )
        .await
        .map_err(|e| Error::transport("connect to the daemon socket", e))?;
        Self::with_session(session).await
    }

    /// Start mirroring the tray over the session `daemon` holds, rather
    /// than opening another on the same socket.
    ///
    /// # Errors
    ///
    /// Fails only with [`Error::Transport`].
    pub async fn beside(daemon: &DaemonClient) -> Result<Self> {
        Self::with_session(daemon.session.clone()).await
    }

    async fn with_session(session: zenoh::Session) -> Result<Self> {
        let caller = Arc::new(Caller::new(session.clone()));
        let mirror = LifetimeMirror::<TrayMirror>::start(&session, &caller).await?;
        Ok(Self {
            caller,
            events: mirror.events().clone(),
            handshake: Arc::clone(mirror.handshake()),
            mirror,
        })
    }

    /// Whether the tray's liveliness token is present, with the same
    /// caveat as [`DaemonClient::is_available`]. No rpc.
    #[must_use]
    pub fn is_available(&self) -> bool {
        self.mirror.is_available()
    }

    /// Whether the tray's switches have arrived in the mirror for its
    /// current lifetime ([`StateEvent::Hydrated`]). A mirror read.
    #[must_use]
    pub fn is_hydrated(&self) -> bool {
        self.mirror.is_hydrated()
    }

    /// What this lifetime's release check learned: the tray's release
    /// and its settings file. A mirror read, like
    /// [`DaemonClient::service_info`].
    #[must_use]
    pub fn service_info(&self) -> Option<ServiceInfo> {
        self.handshake.info()
    }

    /// The tray's notification switches as it last published them: a
    /// mirror read, updated on each
    /// [`StateEvent::TrayNotificationSettingsChanged`]. `None` until the
    /// tray first publishes them, while it's down, and against a tray on
    /// another release.
    #[must_use]
    pub fn notification_settings(&self) -> Option<NotificationSettings> {
        if self.handshake.known_mismatch().is_some() {
            return None;
        }
        self.mirror.read(|mirror| mirror.settings)
    }

    /// The tray's notification switches, asked of the tray: the active
    /// counterpart to [`Self::notification_settings`].
    ///
    /// # Errors
    ///
    /// Fails as any rpc can.
    pub async fn notification_settings_status(&self) -> Result<NotificationSettings> {
        self.call::<tray::Get>(&()).await
    }

    /// Change the switches `patch` sets, leaving the rest as they are.
    /// The tray saves them to its settings file and publishes them
    /// again.
    ///
    /// # Errors
    ///
    /// Fails as any rpc can.
    pub async fn update_notification_settings(
        &self,
        patch: NotificationSettingsPatch,
    ) -> Result<()> {
        self.call::<tray::Update>(&patch).await.map(|_| ())
    }

    /// Have the tray show a test notification, whatever the switches
    /// say.
    ///
    /// # Errors
    ///
    /// Fails as any rpc can.
    pub async fn send_test_notification(&self) -> Result<()> {
        self.call::<tray::Test>(&()).await.map(|_| ())
    }

    /// Have the tray take up a hand edit of its settings file.
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::SettingsUnreadable`] for a file that is
    /// missing or isn't settings, whose message says why; and fails as
    /// any rpc can.
    pub async fn reload_config(&self) -> Result<ReloadReport> {
        self.call::<tray::Reload>(&()).await
    }

    fn subscribe_events(&self) -> broadcast::Receiver<StateEvent> {
        self.events.subscribe()
    }

    async fn call<R: Unary<Target = ()>>(&self, request: &R::Request) -> Result<R::Response> {
        self.handshake.ensure(&self.events, || self.info()).await?;
        self.caller
            .call::<R>(&(), request)
            .await
            .map_err(|e| Error::from_rpc(Service::Tray, e))
    }

    async fn info(&self) -> Result<ServiceInfo> {
        self.caller
            .call::<tray::Info>(&(), &())
            .await
            .map_err(|e| Error::from_rpc(Service::Tray, e))
    }
}

/// Announce the daemon's liveliness edge (`announce`) and keep the
/// client's release check in step: a daemon that went away loses its
/// release before the announcement, so a reader acting on it sees none,
/// and its next lifetime gets a fresh check. One that appeared is checked
/// at once, after the announcement, so its release (and a mismatch)
/// reaches the watcher without waiting for a call. A failed check is
/// retried, backing off, for as long as that lifetime lasts. The check
/// runs on its own task, so a slow `info` never holds up the next
/// liveliness edge.
fn on_liveliness<F, Fut>(
    alive: bool,
    announce: StateEvent,
    handshake: &Arc<Handshake>,
    events: &broadcast::Sender<StateEvent>,
    caller: &Arc<Caller>,
    service: Service,
    info: F,
) where
    F: Fn(Arc<Caller>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = std::result::Result<ServiceInfo, ctrlassist_protocol::caller::RpcError>>
        + Send,
{
    if !alive {
        handshake.reset();
        let _ = events.send(announce);
        return;
    }
    // Read before spawning, on the liveliness task: a `reset` for this
    // service going away can only come after it.
    let lifetime = handshake.lifetime();
    let _ = events.send(announce);
    let handshake = Arc::clone(handshake);
    let events = events.clone();
    let caller = Arc::clone(caller);
    tokio::spawn(async move {
        let mut backoff = CHECK_RETRY_FIRST;
        loop {
            let caller = Arc::clone(&caller);
            let checked = handshake
                .ensure(&events, || async {
                    info(caller).await.map_err(|e| Error::from_rpc(service, e))
                })
                .await;
            // Settled, a mismatch included; or the service went away, and
            // its next lifetime gets its own check.
            if handshake.info().is_some() || handshake.lifetime() != lifetime {
                return;
            }
            if let Err(e) = checked {
                debug!(%service, error = %e, ?backoff, "release check failed; retrying");
            }
            tokio::time::sleep(backoff).await;
            backoff = (backoff * 2).min(CHECK_RETRY_MAX);
        }
    });
}

/// The first wait before a failed release check is retried, doubling up
/// to [`CHECK_RETRY_MAX`].
const CHECK_RETRY_FIRST: Duration = Duration::from_secs(1);

/// The longest wait between release-check retries.
const CHECK_RETRY_MAX: Duration = Duration::from_secs(30);

/// Watcher tuning: mirror deltas forward directly, so the one knob is
/// the fallback poll interval.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct StateWatcherConfig {
    /// How often the watcher emits [`StateEvent::FallbackPoll`], a
    /// safety net for a missed change. Zero turns the poll off.
    pub fallback_poll_interval: Duration,
}

impl Default for StateWatcherConfig {
    fn default() -> Self {
        Self {
            fallback_poll_interval: Duration::from_secs(300),
        }
    }
}

/// The merged change stream: the daemon's mirror deltas, the tray's when
/// asked for, liveliness edges and a fallback poll tick.
#[derive(Debug)]
pub struct StateWatcher {
    rx: mpsc::UnboundedReceiver<StateEvent>,
}

impl StateWatcher {
    /// Merge the events of `daemon` and of the optional `tray` client
    /// into one stream. Each service already alive when the watcher is
    /// built gets an `*Appeared` event, so a consumer driven by events
    /// alone still learns of it. Must run inside the tokio runtime.
    #[must_use]
    pub fn new(
        daemon: &DaemonClient,
        tray: Option<&TrayClient>,
        config: StateWatcherConfig,
    ) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        forward(daemon.subscribe_events(), tx.clone());
        if let Some(tray) = tray {
            forward(tray.subscribe_events(), tx.clone());
        }
        // Synthesize `DaemonAppeared` for a daemon already alive, and the
        // `ReleaseChecked` / `VersionMismatch` of a check already
        // settled: all fired into the client's broadcast before this
        // watcher subscribed (a tokio broadcast drops events sent with no
        // receivers), so consumers relying on edges alone would never
        // learn of them. A level-triggered emit after subscription closes
        // the window; a check finishing in between can announce twice,
        // and consumers treat repeats idempotently.
        {
            let tx = tx.clone();
            let daemon = daemon.clone();
            let tray = tray.cloned();
            tokio::spawn(async move {
                if daemon.is_available() {
                    let _ = tx.send(StateEvent::DaemonAppeared);
                }
                for event in daemon.handshake.settled_events() {
                    let _ = tx.send(event);
                }
                if let Some(tray) = tray {
                    if tray.is_available() {
                        let _ = tx.send(StateEvent::TrayAppeared);
                    }
                    for event in tray.handshake.settled_events() {
                        let _ = tx.send(event);
                    }
                }
            });
        }
        if config.fallback_poll_interval.is_zero() {
            return Self { rx };
        }
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(config.fallback_poll_interval);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            interval.tick().await; // immediate first tick is not a poll
            loop {
                interval.tick().await;
                if tx.send(StateEvent::FallbackPoll).is_err() {
                    return;
                }
            }
        });
        Self { rx }
    }

    /// The next event, waiting for one. `None` only once the watcher's
    /// tasks have stopped, which happens when the runtime shuts down.
    pub async fn next(&mut self) -> Option<StateEvent> {
        self.rx.recv().await
    }

    /// The next event if one is already queued, without waiting. Draining
    /// these after [`Self::next`] lets a consumer act once on the state a
    /// burst of events leaves, rather than once per event.
    pub fn try_next(&mut self) -> Option<StateEvent> {
        self.rx.try_recv().ok()
    }
}

fn forward(mut rx: broadcast::Receiver<StateEvent>, tx: mpsc::UnboundedSender<StateEvent>) {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => {
                    if tx.send(event).is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    warn!(missed, "state events lagged; forcing a refresh");
                    if tx.send(StateEvent::FallbackPoll).is_err() {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return,
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A watcher that fell behind is told to re-read everything, and its
    /// forwarding ends with either side of it.
    #[tokio::test]
    async fn a_lagging_watcher_is_told_to_refresh() {
        let (events, behind) = broadcast::channel(2);
        for _ in 0..5 {
            events.send(StateEvent::DaemonStateChanged).unwrap();
        }
        let (tx, mut rx) = mpsc::unbounded_channel();
        forward(behind, tx);
        assert_eq!(rx.recv().await, Some(StateEvent::FallbackPoll));
        assert_eq!(rx.recv().await, Some(StateEvent::DaemonStateChanged));
        assert_eq!(rx.recv().await, Some(StateEvent::DaemonStateChanged));
        // The client gone, the stream ends.
        drop(events);
        assert_eq!(rx.recv().await, None);

        // The watcher gone, the forwarding stops at the next event, lagged
        // or not.
        for lagged in [false, true] {
            let (events, behind) = broadcast::channel(2);
            let (tx, rx) = mpsc::unbounded_channel();
            drop(rx);
            for _ in 0..if lagged { 5 } else { 1 } {
                events.send(StateEvent::DaemonAppeared).unwrap();
            }
            forward(behind, tx.clone());
            tx.closed().await;
            while events.receiver_count() != 0 {
                tokio::task::yield_now().await;
            }
        }
    }
}
