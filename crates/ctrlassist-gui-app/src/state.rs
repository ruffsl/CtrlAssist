//! The daemon's state as the window shows it: read from the SDK's
//! mirror into one [`Snapshot`], and put on the window from that.
//!
//! The window renders level-triggered. The watch loop drains each burst
//! of the mirror's events and reads everything once, so it never needs
//! to know what an event changed, and an event that was dropped costs
//! nothing: the next one reads the same whole.

use ctrlassist::form::Field;
use ctrlassist::{
    DaemonClient, Hiding, NotificationSettings, Pad, Profile, Setup, StateWatcher,
    StateWatcherConfig, TrayClient,
};
use ctrlassist_protocol::{VERSION, compatible};
use slint::{ComponentHandle, SharedString};
use tracing::{debug, warn};

use crate::toast::show_toast;
use crate::{AppWindow, ErrorState, Errors, ServiceHealth, on_ui};

/// What the window shows of the daemon, read at one moment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// The daemon is there.
    pub available: bool,
    /// All it publishes has arrived, so an empty list means empty.
    pub hydrated: bool,
    /// Its release, once it has said.
    pub version: Option<String>,
    /// Whether the hiding service hides setups' pads, or they are only
    /// grabbed; unknown until the daemon's own state has arrived.
    pub hidden: Option<bool>,
    /// Where it keeps the user's files, when it says.
    pub config: String,
    /// Whether its input thread has real-time scheduling.
    pub realtime: bool,
    /// A tray is there, serving its settings through the daemon.
    pub tray_available: bool,
    /// The tray's release, once it has said.
    pub tray: Option<String>,
    /// The file the tray keeps its settings in, once it has said.
    pub tray_config: String,
    /// The tray's notification switches, once it has published them.
    pub notifications: Option<NotificationSettings>,
    /// The pads it knows.
    pub pads: Vec<Pad>,
    /// The setups that exist.
    pub setups: Vec<Setup>,
    /// The profiles it keeps.
    pub profiles: Vec<Profile>,
    /// A preset's fields, as it serves them; empty until it has.
    pub fields: Vec<Field>,
}

impl Snapshot {
    /// Reads the mirrors: the daemon's, and the tray's when the window
    /// follows one.
    #[must_use]
    pub fn read(daemon: &DaemonClient, tray: Option<&TrayClient>) -> Self {
        let state = daemon.daemon_state().ok().flatten();
        Self {
            available: daemon.is_available(),
            hydrated: daemon.is_hydrated(),
            version: daemon.service_info().map(|info| info.version),
            hidden: state.as_ref().map(|state| state.hiding == Hiding::Service),
            config: state
                .as_ref()
                .map(|state| state.config.clone())
                .unwrap_or_default(),
            realtime: state.as_ref().is_some_and(|state| state.realtime),
            tray_available: tray.is_some_and(TrayClient::is_available),
            tray: tray
                .and_then(TrayClient::service_info)
                .map(|info| info.version),
            tray_config: tray
                .and_then(TrayClient::service_info)
                .map(|info| info.config)
                .unwrap_or_default(),
            notifications: tray.and_then(TrayClient::notification_settings),
            pads: daemon.list_pads().unwrap_or_default(),
            setups: daemon.list_setups().unwrap_or_default(),
            profiles: daemon.list_profiles().unwrap_or_default(),
            fields: Vec::new(),
        }
    }

    /// The daemon's health: whether it runs, and whether this window
    /// can talk to it.
    #[must_use]
    pub fn daemon_health(&self) -> ServiceHealth {
        health(self.available, self.version.as_deref())
    }

    /// The hiding service's health, as the daemon finds it: absent with
    /// no daemon to ask, and while the daemon hasn't said.
    #[must_use]
    pub fn hider_health(&self) -> ServiceHealth {
        match (self.available, self.hidden) {
            (true, Some(true)) => ServiceHealth::Ok,
            _ => ServiceHealth::Offline,
        }
    }

    /// The tray's health. A tray is seen through the daemon, so it is
    /// absent with no daemon running.
    #[must_use]
    pub fn tray_health(&self) -> ServiceHealth {
        health(self.tray_available, self.tray.as_deref())
    }

    /// Puts the snapshot on the window.
    pub fn show(&self, app: &AppWindow) {
        app.set_daemon_available(self.available);
        app.set_daemon_loading(self.available && !self.hydrated);
        app.set_daemon_health(self.daemon_health());
        app.set_hider_health(self.hider_health());
        app.set_daemon_version(SharedString::from(self.version.clone().unwrap_or_default()));
        app.set_daemon_config(SharedString::from(self.config.as_str()));
        app.set_daemon_realtime(self.realtime);
        app.set_tray_health(self.tray_health());
        app.set_tray_version(SharedString::from(self.tray.clone().unwrap_or_default()));
        let tray = app.global::<crate::Tray>();
        tray.set_available(self.tray_available);
        // With no tray the pane goes on showing what was last known.
        if let Some(settings) = self.notifications {
            tray.set_notifications(crate::settings::shown(settings));
        }
        if !self.tray_config.is_empty() {
            tray.set_config_path(SharedString::from(self.tray_config.as_str()));
        }
        crate::about::refresh(app);
        crate::pads::show(app, crate::pads::rows(&self.pads, &self.setups));
        crate::setups::show(
            app,
            crate::setups::rows(
                &self.setups,
                &self.pads,
                &self
                    .profiles
                    .iter()
                    .map(|profile| profile.name.clone())
                    .collect::<Vec<_>>(),
            ),
            crate::setups::trouble(&self.setups),
        );
        crate::profiles::show(
            app,
            crate::profiles::Known {
                profiles: self.profiles.clone(),
                setups: self.setups.clone(),
                pads: self.pads.clone(),
                fields: self.fields.clone(),
            },
        );
        let banner = match (&self.version, self.daemon_health()) {
            (Some(theirs), ServiceHealth::Incompatible) => ErrorState {
                kind: SharedString::from("version"),
                tone: SharedString::from("error"),
                title: SharedString::from("The daemon is on another release"),
                message: SharedString::from(format!(
                    "This window is {VERSION} and the daemon is {theirs}, so nothing here can reach it. Restart the daemon, or this window, after an upgrade."
                )),
                action_label: SharedString::default(),
            },
            _ => ErrorState::default(),
        };
        app.global::<Errors>().set_version(banner);
    }
}

/// A service's health from whether it runs and the release it reports:
/// unusable on another release line, usable with a note on another
/// patch release, and taken as fine until it has said.
#[must_use]
pub fn health(available: bool, version: Option<&str>) -> ServiceHealth {
    match version {
        _ if !available => ServiceHealth::Offline,
        None => ServiceHealth::Ok,
        Some(theirs) if !compatible(VERSION, theirs) => ServiceHealth::Incompatible,
        Some(theirs) if theirs != VERSION => ServiceHealth::Skew,
        Some(_) => ServiceHealth::Ok,
    }
}

/// Follows the daemon, and the tray when there is a client for it, for
/// as long as the window lives, showing each change.
pub(crate) async fn follow(
    daemon: DaemonClient,
    tray: Option<TrayClient>,
    app: slint::Weak<AppWindow>,
) {
    let mut watcher = StateWatcher::new(&daemon, tray.as_ref(), StateWatcherConfig::default());
    let mut shown = None;
    // A preset's fields, asked once of each daemon that comes.
    let mut fields: Vec<Field> = Vec::new();
    loop {
        // One read for a burst of events.
        while watcher.try_next().is_some() {}
        let mut snapshot = Snapshot::read(&daemon, tray.as_ref());
        if !snapshot.available {
            fields.clear();
        } else if fields.is_empty() {
            match daemon.preset_form().await {
                Ok(served) => fields = served,
                Err(error) => warn!(%error, "the daemon didn't say what a preset's fields are"),
            }
        }
        snapshot.fields.clone_from(&fields);
        if shown.as_ref() != Some(&snapshot) {
            debug!(?snapshot, "the daemon's state changed");
            shown = Some(snapshot.clone());
            on_ui(&app, move |app| snapshot.show(&app));
        }
        if watcher.next().await.is_none() {
            return;
        }
    }
}

/// What a reload found, in a line for the toast.
#[must_use]
pub fn reload_summary(reloaded: &ctrlassist::Reloaded) -> String {
    let read = reloaded.changed.len();
    let gone = reloaded.removed.len();
    if read == 0 && gone == 0 {
        return "Nothing changed on disk".to_string();
    }
    let mut parts = Vec::new();
    if read > 0 {
        parts.push(format!("{read} read again"));
    }
    if gone > 0 {
        parts.push(format!("{gone} gone"));
    }
    if !reloaded.updating.is_empty() {
        parts.push(format!("{} running updated", reloaded.updating.len()));
    }
    format!("Profiles: {}", parts.join(", "))
}

/// The reload banner for profiles a reload found wrong, one per line as
/// the daemon reports them; empty when none is.
#[must_use]
pub fn reload_banner(reloaded: &ctrlassist::Reloaded) -> ErrorState {
    let lines: Vec<String> = reloaded
        .wrong
        .iter()
        .filter_map(|profile| {
            let problem = profile.problem.as_ref()?;
            Some(format!("{}.toml: {problem}", profile.name))
        })
        .collect();
    if lines.is_empty() {
        return ErrorState::default();
    }
    ErrorState {
        kind: SharedString::from("reload"),
        tone: SharedString::from("error"),
        title: SharedString::from("Some profiles can't be run as they are"),
        message: SharedString::from(lines.join("\n")),
        action_label: SharedString::default(),
    }
}

/// The sidebar's Reload: asks the daemon, then says what it found.
pub(crate) fn wire_reload(app: &AppWindow, daemon: &DaemonClient, tray: Option<&TrayClient>) {
    let weak = app.as_weak();
    let daemon = daemon.clone();
    let tray = tray.cloned();
    app.global::<crate::Daemon>().on_reload_config(move || {
        let (weak, daemon, tray) = (weak.clone(), daemon.clone(), tray.clone());
        tokio::spawn(async move {
            // The tray's file too, where a tray is there to reread it.
            let tray = match tray.filter(TrayClient::is_available) {
                Some(tray) => Some(tray.reload_config().await),
                None => None,
            };
            match daemon.reload().await {
                Ok(reloaded) => {
                    let mut summary = reload_summary(&reloaded);
                    if matches!(tray, Some(Ok(report)) if report.changed) {
                        summary.push_str("; the tray's settings read again");
                    }
                    show_toast(weak.clone(), &summary);
                    let mut banner = reload_banner(&reloaded);
                    // A profile that is wrong is said first; the tray's
                    // file is said when nothing else is.
                    if banner == ErrorState::default()
                        && let Some(Err(error)) = &tray
                    {
                        warn!(%error, "the tray couldn't reread its settings");
                        banner = ErrorState {
                            kind: SharedString::from("reload"),
                            tone: SharedString::from("warn"),
                            title: SharedString::from("The tray couldn't reread its settings"),
                            message: SharedString::from(error.to_string()),
                            action_label: SharedString::default(),
                        };
                    }
                    on_ui(&weak, move |app| app.global::<Errors>().set_reload(banner));
                }
                Err(error) => {
                    warn!(%error, "the reload failed");
                    let banner = ErrorState {
                        kind: SharedString::from("reload"),
                        tone: SharedString::from("error"),
                        title: SharedString::from("The daemon couldn't reload"),
                        message: SharedString::from(error.to_string()),
                        action_label: SharedString::default(),
                    };
                    on_ui(&weak, move |app| app.global::<Errors>().set_reload(banner));
                }
            }
        });
    });
}
