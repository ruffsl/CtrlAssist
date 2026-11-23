#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

mod error;
mod handshake;
mod lifetime;
mod mirror;

use std::collections::VecDeque;
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
    NotificationSettings, NotificationSettingsPatch, ReloadReport, Reloaded, ServiceInfo, Updated,
};
pub use error::{BoxError, Error, Result, Service};
use tokio::sync::broadcast;
use tracing::warn;

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
    /// rpcs, and the client is subscribed to its state. Sent once per
    /// lifetime; a watcher built while the daemon is up gets it on start.
    DaemonAppeared,
    /// The daemon's liveliness token went away (stopped, crashed, or the
    /// link dropped). Its state has already left the mirror, announced
    /// before this event, and [`DaemonClient::service_info`] has cleared.
    /// Can come with no [`Self::DaemonAppeared`] before it, for a token
    /// that went while the client was still subscribing.
    DaemonDisappeared,
    /// The tray's liveliness token appeared: its settings service is
    /// running. Sent once per lifetime; a watcher built while the tray is
    /// up gets it on start.
    TrayAppeared,
    /// The tray's liveliness token went away: the tray stopped, or the
    /// daemon it serves through did. Its switches have already left the
    /// mirror, announced before this event, and
    /// [`TrayClient::service_info`] has cleared. Can come with no
    /// [`Self::TrayAppeared`] before it, as for the daemon.
    TrayDisappeared,
    /// The tray's notification switches changed in the mirror, or left
    /// it because the tray went away. Re-read
    /// [`TrayClient::notification_settings`].
    TrayNotificationSettingsChanged,
    /// Re-read everything: the watcher's periodic tick
    /// ([`StateWatcherConfig::fallback_poll_interval`]), also sent in
    /// place of the events a watcher lost by going unread for more than
    /// 256 of them (see [`StateWatcher`]).
    FallbackPoll,
    /// A service's release check settled for its current lifetime, so
    /// its client's `service_info` now answers. Sent once per service
    /// lifetime, whatever the verdict, after its `*Appeared` and before
    /// its `*Disappeared`: the check runs as soon as the service appears,
    /// and only a check made while its token is present settles. A
    /// watcher built after the check settled gets it on start.
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
    /// start.
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

    /// Whether the daemon's liveliness token is present, as last delivered
    /// to this client: a mirror read, which unlike [`Self::is_available`]
    /// doesn't wait for the client to have subscribed to the daemon's
    /// state. A daemon that is stopping withdraws its token first and
    /// then answers what is in flight; over a session in client mode,
    /// as `ctrlassist_protocol::bootstrap::client_config` opens, the
    /// daemon's token reaches the client in order with its replies, so
    /// once a call has been answered this tells such a daemon from one
    /// that is up.
    #[must_use]
    pub fn token_present(&self) -> bool {
        self.mirror.token_present()
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
    /// one that has the name. The daemon writes it to disk before it
    /// answers, which is waited for as long as the daemon is there, with
    /// no time limit.
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::InvalidProfile`], whose message says
    /// where the profile is wrong and what, or with
    /// [`ErrorCode::BadRequest`] for a name that isn't 1 to 32 lowercase
    /// letters, digits and hyphens; fails with [`Error::Gone`] if the
    /// daemon goes before it answers; and fails as any rpc can.
    pub async fn put_profile(&self, name: &str, toml: &str) -> Result<()> {
        let request = rpc::PutProfileRequest {
            name: name.to_string(),
            toml: toml.to_string(),
        };
        self.call::<profile::Put>(&request).await.map(|_| ())
    }

    /// Stop keeping the profile `name`. Its file is removed before the
    /// daemon answers, which is waited for as long as the daemon is
    /// there, with no time limit.
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::UnknownProfile`]; fails with
    /// [`Error::Gone`] if the daemon goes before it answers; and fails as
    /// any rpc can.
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
    /// it needs isn't connected, which its state says. Hiding its pads
    /// and making its virtual pads can take a while (the hiding service
    /// waits on udev, a DualSense on its driver), so the answer is waited
    /// for as long as the daemon is there, with no time limit.
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::UnknownProfile`],
    /// [`ErrorCode::InvalidProfile`], [`ErrorCode::SetupExists`],
    /// [`ErrorCode::PadInUse`], or [`ErrorCode::SetupFailed`] with why;
    /// fails with [`Error::Gone`] if the daemon goes before it answers,
    /// the start's outcome then unknown; and fails as any rpc can.
    pub async fn start_setup(&self, name: &str) -> Result<()> {
        let request = rpc::SetupRequest {
            name: name.to_string(),
        };
        self.call::<setup::Start>(&request).await.map(|_| ())
    }

    /// Stop the setup `name`. Returns once its virtual pads are gone and
    /// its pads shown again, which is waited for as long as the daemon is
    /// there, with no time limit.
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::UnknownSetup`], or with
    /// [`ErrorCode::SetupBusy`] while it is starting or stopping; fails
    /// with [`Error::Gone`] if the daemon goes before it answers; and
    /// fails as any rpc can.
    pub async fn stop_setup(&self, name: &str) -> Result<()> {
        let request = rpc::SetupRequest {
            name: name.to_string(),
        };
        self.call::<setup::Stop>(&request).await.map(|_| ())
    }

    /// Have the setup `name` take up its profile as it is kept now, after
    /// a [`Self::put_profile`]; with `apply` false, only ask what that
    /// would cost. Returns whether it is [`Updated::disruptive`]: whether
    /// the setup's virtual pads are made anew, which a game sees as its
    /// pad disconnecting. A profile that keeps the setup's virtual pads
    /// and its way of hiding pads is taken up live, without; any other
    /// change, or a setup that failed, is a restart. With `apply`,
    /// returns once the setup runs again, with the [`Updated::start`] it
    /// runs as: after a restart, the number its new start is published
    /// under, which tells that start from any other of the name whatever
    /// this client's mirror has seen of it. Like a start, the answer is
    /// waited for as long as the daemon is there, with no time limit.
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::UnknownSetup`]; with
    /// [`ErrorCode::InvalidProfile`] or [`ErrorCode::UnknownProfile`] if
    /// what is kept now can't be run, the setup then carrying on as it
    /// was; with [`ErrorCode::SetupBusy`] while it is starting or
    /// stopping; or as [`Self::start_setup`] is, [`Error::Gone`]
    /// included; and fails as any rpc can.
    pub async fn update_setup(&self, name: &str, apply: bool) -> Result<Updated> {
        let request = rpc::UpdateRequest {
            name: name.to_string(),
            apply,
        };
        self.call::<setup::Update>(&request).await
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
    /// `pad` is its id, its id's key segment, a bare address, or
    /// `model:<vendor>:<product>` when one known pad is of that model.
    /// Or a running setup's virtual pad, as `virtual:<setup>/<name>`,
    /// which is rumbled as a game would rumble it: the pads behind it
    /// buzz as the setup's profile sends it on. Returns once the buzz has
    /// started.
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::UnknownPad`] (for a virtual pad, when no
    /// setup has it now), [`ErrorCode::AmbiguousPad`],
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
    /// [`Self::identify_pad`], a virtual pad aside. The daemon writes the
    /// pads it remembers to disk before it answers, which is waited for
    /// as long as the daemon is there, with no time limit.
    ///
    /// # Errors
    ///
    /// Refused with [`ErrorCode::UnknownPad`], [`ErrorCode::AmbiguousPad`]
    /// or [`ErrorCode::PadConnected`]; fails with [`Error::Gone`] if the
    /// daemon goes before it answers; and fails as any rpc can.
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
    /// they fare. The files are read before the daemon answers, which is
    /// waited for as long as the daemon is there, with no time limit.
    ///
    /// # Errors
    ///
    /// Fails with [`Error::Gone`] if the daemon goes before it answers;
    /// and fails as any rpc can.
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

    /// Call a daemon rpc, once the daemon's release is known to match.
    async fn call<R: Unary<Target = ()>>(&self, request: &R::Request) -> Result<R::Response> {
        self.check_release().await?;
        call_while_there::<R>(&self.caller, &self.mirror, request).await
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

    async fn call<R: Unary<Target = ()>>(&self, request: &R::Request) -> Result<R::Response> {
        self.handshake.ensure(&self.events, || self.info()).await?;
        call_while_there::<R>(&self.caller, &self.mirror, request).await
    }

    async fn info(&self) -> Result<ServiceInfo> {
        self.caller
            .call::<tray::Info>(&(), &())
            .await
            .map_err(|e| Error::from_rpc(Service::Tray, e))
    }
}

/// Calls `R` on `mirror`'s service. An [`untimed`](Unary::UNTIMED) rpc
/// waits for its answer for as long as the service's liveliness token is
/// there, and fails with [`Error::Gone`] once it isn't, at once and
/// without asking if it isn't now: the service answers every rpc once,
/// or drops it as it ends, which ends the call too; a stopping service
/// withdraws its token before anything else, and a service whose link
/// drops is seen without its token, so nothing else ends the wait. Every
/// other rpc waits as long as the protocol's caller does.
async fn call_while_there<R: Unary<Target = ()>>(
    caller: &Caller,
    mirror: &LifetimeMirror<impl lifetime::Mirror>,
    request: &R::Request,
) -> Result<R::Response> {
    let service = mirror.service();
    let call = async {
        caller
            .call::<R>(&(), request)
            .await
            .map_err(|e| Error::from_rpc(service, e))
    };
    if !R::UNTIMED {
        return call.await;
    }
    // A service already going isn't asked what it may not finish.
    if !mirror.token_present() {
        return Err(Error::Gone { service });
    }
    tokio::select! {
        // An answer that is there with the token's going is the answer.
        biased;
        outcome = call => outcome,
        () = mirror.gone() => Err(Error::Gone { service }),
    }
}

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
///
/// It reads each client's own queue of events, which holds the last 256
/// for each watcher: one that isn't read for
/// longer than that loses the oldest, and is sent a
/// [`StateEvent::FallbackPoll`] in their place, rather than holding on to
/// them. A slow consumer costs events it then re-reads as state, never
/// memory.
#[derive(Debug)]
pub struct StateWatcher {
    /// What was announced before the watcher was built that still holds,
    /// handed over first.
    replay: VecDeque<StateEvent>,
    /// The daemon client's events; `None` once every copy of it is gone.
    daemon: Option<broadcast::Receiver<StateEvent>>,
    /// The tray client's, when asked for; `None` once it is gone.
    tray: Option<broadcast::Receiver<StateEvent>>,
    /// The fallback poll, when on.
    poll: Option<Poll>,
}

#[derive(Debug)]
struct Poll {
    every: Duration,
    next: tokio::time::Instant,
}

/// What woke [`StateWatcher::next`].
enum Woke {
    Daemon(std::result::Result<StateEvent, broadcast::error::RecvError>),
    Tray(std::result::Result<StateEvent, broadcast::error::RecvError>),
    Poll,
}

impl StateWatcher {
    /// Merge the events of `daemon` and of the optional `tray` client
    /// into one stream. Each service already alive when the watcher is
    /// built gets its `*Appeared` event first, followed by the
    /// [`StateEvent::ReleaseChecked`] (and [`StateEvent::VersionMismatch`])
    /// of a check already settled, so a consumer driven by events alone
    /// still learns of them, each once and in order.
    #[must_use]
    pub fn new(
        daemon: &DaemonClient,
        tray: Option<&TrayClient>,
        config: StateWatcherConfig,
    ) -> Self {
        let (daemon, mut replay) = daemon.handshake.subscribe(&daemon.events);
        let tray = tray.map(|tray| {
            let (events, announced) = tray.handshake.subscribe(&tray.events);
            replay.extend(announced);
            events
        });
        Self::merging(replay, daemon, tray, &config)
    }

    fn merging(
        replay: Vec<StateEvent>,
        daemon: broadcast::Receiver<StateEvent>,
        tray: Option<broadcast::Receiver<StateEvent>>,
        config: &StateWatcherConfig,
    ) -> Self {
        let every = config.fallback_poll_interval;
        Self {
            replay: replay.into(),
            daemon: Some(daemon),
            tray,
            poll: (!every.is_zero()).then(|| Poll {
                every,
                next: tokio::time::Instant::now() + every,
            }),
        }
    }

    /// The next event, waiting for one. `None` once every client the
    /// watcher follows has been dropped, every copy of it, and its tasks
    /// have stopped; until then, with the fallback poll off and nothing
    /// changing, it waits. Cancel-safe: dropping the future loses no
    /// event.
    pub async fn next(&mut self) -> Option<StateEvent> {
        if let Some(event) = self.replay.pop_front() {
            return Some(event);
        }
        loop {
            if self.daemon.is_none() && self.tray.is_none() {
                return None;
            }
            let woke = tokio::select! {
                got = recv(self.daemon.as_mut()) => Woke::Daemon(got),
                got = recv(self.tray.as_mut()) => Woke::Tray(got),
                () = due(self.poll.as_ref()) => Woke::Poll,
            };
            let (got, source) = match woke {
                Woke::Daemon(got) => (got, &mut self.daemon),
                Woke::Tray(got) => (got, &mut self.tray),
                Woke::Poll => {
                    self.polled();
                    return Some(StateEvent::FallbackPoll);
                }
            };
            match got {
                Ok(event) => return Some(event),
                Err(broadcast::error::RecvError::Lagged(missed)) => {
                    warn!(missed, "state events lagged; forcing a refresh");
                    return Some(StateEvent::FallbackPoll);
                }
                Err(broadcast::error::RecvError::Closed) => *source = None,
            }
        }
    }

    /// The next event if one is already queued, without waiting. Draining
    /// these after [`Self::next`] lets a consumer act once on the state a
    /// burst of events leaves, rather than once per event.
    pub fn try_next(&mut self) -> Option<StateEvent> {
        if let Some(event) = self.replay.pop_front() {
            return Some(event);
        }
        for source in [&mut self.daemon, &mut self.tray] {
            let Some(events) = source else {
                continue;
            };
            match events.try_recv() {
                Ok(event) => return Some(event),
                Err(broadcast::error::TryRecvError::Lagged(missed)) => {
                    warn!(missed, "state events lagged; forcing a refresh");
                    return Some(StateEvent::FallbackPoll);
                }
                Err(broadcast::error::TryRecvError::Empty) => {}
                Err(broadcast::error::TryRecvError::Closed) => *source = None,
            }
        }
        if self
            .poll
            .as_ref()
            .is_some_and(|poll| poll.next <= tokio::time::Instant::now())
        {
            self.polled();
            return Some(StateEvent::FallbackPoll);
        }
        None
    }

    /// The poll fired: the next one is a whole interval from now, so a
    /// watcher that wasn't read for a while gets one, not a backlog.
    fn polled(&mut self) {
        if let Some(poll) = &mut self.poll {
            poll.next = tokio::time::Instant::now() + poll.every;
        }
    }
}

/// The next of a client's events, or never for a client that is gone.
async fn recv(
    events: Option<&mut broadcast::Receiver<StateEvent>>,
) -> std::result::Result<StateEvent, broadcast::error::RecvError> {
    match events {
        Some(events) => events.recv().await,
        None => std::future::pending().await,
    }
}

/// When the fallback poll is due, or never with it off.
async fn due(poll: Option<&Poll>) {
    match poll {
        Some(poll) => tokio::time::sleep_until(poll.next).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quiet() -> StateWatcherConfig {
        StateWatcherConfig {
            fallback_poll_interval: Duration::ZERO,
        }
    }

    /// A watcher that fell behind is told to re-read everything, rather
    /// than queuing what it hasn't read, and its stream ends with the
    /// client.
    #[tokio::test]
    async fn a_lagging_watcher_is_told_to_refresh() {
        let (events, behind) = broadcast::channel(2);
        let mut watcher = StateWatcher::merging(Vec::new(), behind, None, &quiet());
        for _ in 0..5 {
            events.send(StateEvent::DaemonStateChanged).unwrap();
        }
        assert_eq!(watcher.next().await, Some(StateEvent::FallbackPoll));
        assert_eq!(watcher.next().await, Some(StateEvent::DaemonStateChanged));
        assert_eq!(watcher.try_next(), Some(StateEvent::DaemonStateChanged));
        assert_eq!(watcher.try_next(), None);
        for _ in 0..5 {
            events.send(StateEvent::DaemonStateChanged).unwrap();
        }
        assert_eq!(watcher.try_next(), Some(StateEvent::FallbackPoll));
        // The client gone, the stream ends once drained.
        drop(events);
        assert_eq!(watcher.next().await, Some(StateEvent::DaemonStateChanged));
        assert_eq!(watcher.next().await, Some(StateEvent::DaemonStateChanged));
        assert_eq!(watcher.next().await, None);
    }

    /// What a watcher is handed on start comes before anything it then
    /// hears; with the poll on, it still ends with its clients.
    #[tokio::test]
    async fn a_watcher_hands_over_what_already_holds_first() {
        let (events, heard) = broadcast::channel(4);
        let (tray_events, tray_heard) = broadcast::channel(4);
        let mut watcher = StateWatcher::merging(
            vec![StateEvent::DaemonAppeared],
            heard,
            Some(tray_heard),
            &StateWatcherConfig::default(),
        );
        events.send(StateEvent::DaemonDisappeared).unwrap();
        assert_eq!(watcher.next().await, Some(StateEvent::DaemonAppeared));
        assert_eq!(watcher.next().await, Some(StateEvent::DaemonDisappeared));
        drop(events);
        tray_events.send(StateEvent::TrayAppeared).unwrap();
        assert_eq!(watcher.next().await, Some(StateEvent::TrayAppeared));
        drop(tray_events);
        assert_eq!(watcher.next().await, None);
    }

    /// The poll fires once for a stretch the watcher wasn't read, and is
    /// seen by `try_next` too.
    #[tokio::test]
    async fn the_poll_fires_once_however_long_it_waited() {
        let (_events, heard) = broadcast::channel(4);
        let config = StateWatcherConfig {
            fallback_poll_interval: Duration::from_millis(50),
        };
        let mut watcher = StateWatcher::merging(Vec::new(), heard, None, &config);
        assert_eq!(watcher.try_next(), None);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(watcher.try_next(), Some(StateEvent::FallbackPoll));
        assert_eq!(watcher.try_next(), None);
        assert_eq!(watcher.next().await, Some(StateEvent::FallbackPoll));
    }
}
