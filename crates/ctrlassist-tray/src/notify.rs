//! Where the tray's notifications go, and the service that lets a
//! person say which kinds they want.
//!
//! Every notification goes through one queue, which the desktop side
//! shows one at a time and tests read instead; a kind that is switched
//! off never enters it. The switches are kept in `tray.toml` and served
//! to the daemon's other clients: published from a cache, with rpcs to
//! read, change and reload them, and a liveliness token for the tray
//! itself.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use ctrlassist::{DaemonClient, StateEvent, StateWatcher, StateWatcherConfig};
use ctrlassist_protocol::endpoint::tray;
use ctrlassist_protocol::error::{ErrorCode, WireError};
pub use ctrlassist_protocol::rpc::NotificationSettings;
use ctrlassist_protocol::{bootstrap, keys, rpc};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{info, warn};
use zenoh::Session;
use zenoh::bytes::Encoding;
use zenoh::query::Query;
use zenoh_ext::{AdvancedPublisherBuilderExt, CacheConfig, MissDetectionConfig};

use crate::state::Toast;

/// How often the settings' publisher says it is still there to a
/// subscriber that may have missed a sample: the daemon's figure.
const MISS_HEARTBEAT: Duration = Duration::from_millis(500);

/// The kinds of thing the tray says, one switch each: their places in
/// the notifier's gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    /// A setup started or stopped.
    SetupStarted = 0,
    /// Something is wrong: a setup held or failed, pads only grabbed or
    /// without rumble, the daemon stopped or back.
    SetupTrouble = 1,
    /// A pad connected or left.
    PadConnection = 2,
    /// The daemon refused something asked from the menu.
    Refused = 3,
}

/// The settings as the notifier's gate: a bit for each [`Category`].
fn bits(settings: NotificationSettings) -> u8 {
    u8::from(settings.setup_started)
        | u8::from(settings.setup_trouble) << 1
        | u8::from(settings.pad_connection) << 2
        | u8::from(settings.refused) << 3
}

/// Where the tray's notifications go.
#[derive(Clone, Debug)]
pub struct Notifier {
    enabled: Arc<AtomicU8>,
    tx: mpsc::UnboundedSender<Toast>,
}

impl Notifier {
    /// A notifier whose toasts, of the kinds `initial` has on, arrive on
    /// the returned receiver instead of the desktop: what tests assert
    /// on, with nothing popping up.
    #[must_use]
    pub fn capturing(initial: NotificationSettings) -> (Self, mpsc::UnboundedReceiver<Toast>) {
        let enabled = Arc::new(AtomicU8::new(bits(initial)));
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { enabled, tx }, rx)
    }

    /// Takes up the switches: from now on only the kinds `settings` has
    /// on are shown.
    pub fn set_enabled(&self, settings: NotificationSettings) {
        self.enabled.store(bits(settings), Ordering::Release);
    }

    /// Shows a toast, if its kind is switched on.
    pub fn notify(&self, toast: Toast) {
        if self.enabled.load(Ordering::Acquire) & (1 << toast.category as u8) == 0 {
            return;
        }
        let _ = self.tx.send(toast);
    }

    /// Shows a toast whatever the switches say: what the settings' test
    /// button asks for.
    fn notify_test(&self) {
        let _ = self.tx.send(Toast {
            category: Category::Refused,
            summary: "Test notification".to_string(),
            body: "CtrlAssist's notifications are working.".to_string(),
            icon: "input-gaming",
        });
    }
}

/// `text` as a notification server that reads a body as markup shows it
/// as written: a pad's name or a daemon's reason with `&` or `<` in it
/// is otherwise taken for markup, and mangled or dropped.
#[must_use]
pub(crate) fn as_markup(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// `tray.toml`: the switches and the profile last used, under a schema
/// number.
#[derive(Serialize, Deserialize)]
struct Persisted {
    #[serde(default = "schema")]
    schema: u32,
    /// The profile a middle click on the icon acts on: the last one
    /// started, stopped or scrolled to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    #[serde(flatten)]
    settings: NotificationSettings,
}

/// One write of `tray.toml` at a time: the switches and the profile
/// last used are written from different tasks, each keeping what the
/// file holds of the other.
static WRITING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn read_persisted(path: &Path) -> Option<Persisted> {
    toml::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// The profile last used, as `path` keeps it; none where it keeps none
/// or can't be read.
#[must_use]
pub fn load_target(path: &Path) -> Option<String> {
    read_persisted(path)?.target
}

/// Keeps `target` as the profile last used, beside the switches the
/// file holds. A file that isn't settings is left as it is, for a
/// reload to say what is wrong with it: written over, its switches
/// would be lost.
pub(crate) async fn save_target(path: &Path, target: String) {
    let _one = WRITING.lock().await;
    let settings = match load_settings(path) {
        Ok(settings) => settings,
        Err(error) => {
            warn!(
                error = format!("{error:#}"),
                "the profile last used isn't kept, so as not to write over the file"
            );
            return;
        }
    };
    if let Err(error) = write_persisted(path, settings, Some(target)).await {
        warn!(error, "the profile last used couldn't be kept");
    }
}

fn schema() -> u32 {
    1
}

/// Where the tray keeps its settings when it isn't told: `tray.toml`
/// beside the daemon's files.
#[must_use]
pub fn default_config_path() -> PathBuf {
    ctrlassist_daemon::Paths::default_for_user()
        .config
        .join("tray.toml")
}

/// Reads the switches from `path`. A file that isn't there is the
/// defaults.
///
/// # Errors
///
/// Fails if the file can't be read, or isn't settings.
pub fn load_settings(path: &Path) -> Result<NotificationSettings> {
    match std::fs::read_to_string(path) {
        Ok(text) => toml::from_str::<Persisted>(&text)
            .map(|persisted| persisted.settings)
            .with_context(|| format!("reading {} as settings", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(NotificationSettings::default())
        }
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

/// `tray.toml` as a reload sees it. A file that is missing or isn't
/// settings is refused, where a start takes the defaults; unless what
/// the tray holds is still the defaults, when no file is a tray that
/// has never been told anything.
fn read_for_reload(
    path: &Path,
    held: NotificationSettings,
) -> Result<NotificationSettings, WireError> {
    if matches!(path.try_exists(), Ok(false)) && held == NotificationSettings::default() {
        return Ok(held);
    }
    let unreadable = |why: String| {
        WireError::refusal(
            ErrorCode::SettingsUnreadable,
            format!("{}: {why}", path.display()),
        )
    };
    let text = std::fs::read_to_string(path).map_err(|error| unreadable(error.to_string()))?;
    toml::from_str::<Persisted>(&text)
        .map(|persisted| persisted.settings)
        .map_err(|error| unreadable(error.to_string()))
}

/// Writes the switches to `path`, beside the profile last used that it
/// holds.
///
/// # Errors
///
/// Says why, where the file doesn't hold them now.
async fn save_settings(path: &Path, settings: NotificationSettings) -> Result<(), String> {
    let _one = WRITING.lock().await;
    let target = load_target(path);
    write_persisted(path, settings, target).await
}

/// Takes a change to the switches from another client: written to
/// `config` first, so that what is answered and published is what the
/// file holds. A write that fails changes nothing, and is answered so;
/// held in memory alone, the change would be undone when the file is
/// next read.
async fn update(
    config: &Path,
    held: NotificationSettings,
    patch: &rpc::NotificationSettingsPatch,
) -> Result<NotificationSettings, WireError> {
    let wanted = held.merged_with(patch);
    save_settings(config, wanted).await.map_err(|why| {
        warn!(error = why, "the tray's settings couldn't be written");
        WireError::internal(format!("writing {}: {why}", config.display()))
    })?;
    Ok(wanted)
}

/// Writes `tray.toml`, off the runtime's threads since the write waits
/// for the disk.
///
/// # Errors
///
/// Says why, where the file doesn't hold what was given now.
async fn write_persisted(
    path: &Path,
    settings: NotificationSettings,
    target: Option<String>,
) -> Result<(), String> {
    let persisted = Persisted {
        schema: schema(),
        target,
        settings,
    };
    let path = path.to_path_buf();
    let written = tokio::task::spawn_blocking(move || {
        let text = toml::to_string(&persisted).map_err(std::io::Error::other)?;
        ctrlassist_daemon::durable::write(&path, &text)
    })
    .await;
    match written {
        Ok(Ok(())) => Ok(()),
        Ok(Err(error)) => Err(error.to_string()),
        Err(error) => Err(format!("the write didn't finish: {error}")),
    }
}

/// Serves the settings on `link` for as long as it is polled: the
/// switches published from a cache, the rpcs, and the tray's liveliness
/// token. `link` is a session in client mode, whose declarations the
/// daemon relays to its other clients. Needs no desktop, so tests run
/// it as it is.
///
/// # Errors
///
/// Fails if something can't be declared on `link`.
pub async fn serve_settings(link: Session, config: PathBuf, notifier: Notifier) -> Result<()> {
    let mut settings = load_settings(&config).unwrap_or_else(|error| {
        warn!(
            error = format!("{error:#}"),
            "the defaults are used instead"
        );
        NotificationSettings::default()
    });
    notifier.set_enabled(settings);

    let publisher = link
        .declare_publisher(keys::tray_settings())
        .encoding(Encoding::APPLICATION_JSON)
        .cache(CacheConfig::default().max_samples(1))
        .sample_miss_detection(MissDetectionConfig::default().sporadic_heartbeat(MISS_HEARTBEAT))
        .publisher_detection()
        .await
        .map_err(|error| anyhow!("declaring the settings' publisher: {error}"))?;
    let publish = |settings: NotificationSettings| {
        let publisher = &publisher;
        async move {
            let bytes = serde_json::to_vec(&settings).unwrap_or_default();
            if let Err(error) = publisher.put(bytes).await {
                warn!(%error, "publishing the tray's settings failed");
            }
        }
    };
    publish(settings).await;

    let (asked, mut queries) = mpsc::unbounded_channel::<Query>();
    let _queryable = link
        .declare_queryable(tray::Incoming::wild())
        .complete(true)
        .callback(move |query| {
            let _ = asked.send(query);
        })
        .await
        .map_err(|error| anyhow!("declaring the tray's rpcs: {error}"))?;

    // After the rpcs: a client that sees the tray alive must find them
    // declared.
    let _alive = link
        .liveliness()
        .declare_token(keys::tray_alive())
        .await
        .map_err(|error| anyhow!("declaring the tray's liveliness: {error}"))?;

    info!(config = %config.display(), "the tray's settings are served");
    while let Some(query) = queries.recv().await {
        let Some(((), incoming)) = tray::Incoming::receive(query).await else {
            continue;
        };
        match incoming {
            tray::Incoming::Get((), responder) => responder.ok(&settings).await,
            tray::Incoming::Update(patch, responder) => {
                match update(&config, settings, &patch).await {
                    Ok(updated) => {
                        settings = updated;
                        notifier.set_enabled(settings);
                        publish(settings).await;
                        responder.ok(&settings).await;
                    }
                    Err(refusal) => responder.err(&refusal).await,
                }
            }
            tray::Incoming::Test((), responder) => {
                notifier.notify_test();
                responder.ok(&rpc::Ack {}).await;
            }
            tray::Incoming::Info((), responder) => {
                let info = rpc::ServiceInfo {
                    version: ctrlassist_protocol::VERSION.to_string(),
                    keys: vec![keys::tray_settings()],
                    config: config.display().to_string(),
                };
                responder.ok(&info).await;
            }
            tray::Incoming::Reload((), responder) => {
                // Every update is written before it is answered, so the
                // file differs from what is held only after a hand edit,
                // which is then taken whole.
                let reloaded = read_for_reload(&config, settings).map(|disk| {
                    let changed = disk != settings;
                    settings = disk;
                    changed
                });
                if reloaded == Ok(true) {
                    info!("took up a hand edit of the tray's settings");
                    notifier.set_enabled(settings);
                    publish(settings).await;
                }
                responder
                    .answer(reloaded.map(|changed| rpc::ReloadReport { changed }))
                    .await;
            }
        }
    }
    Ok(())
}

/// Serves the settings for as long as a daemon is there to serve them
/// through, and only then.
///
/// What the tray serves has to be declared on a link in client mode for
/// the daemon to relay it, and such a link can't be opened with nobody
/// listening. So it is opened when the daemon's liveliness token shows
/// on `daemon`'s own link, which waits for a daemon as long as it takes,
/// and dropped when the token goes: a daemon that comes back is served
/// through a new one. Nothing is retried on a timer; the token is the
/// event.
pub async fn serve_while_daemon(
    daemon: DaemonClient,
    socket: PathBuf,
    config: PathBuf,
    notifier: Notifier,
) {
    let mut quiet = StateWatcherConfig::default();
    quiet.fallback_poll_interval = Duration::ZERO;
    let mut watcher = StateWatcher::new(&daemon, None, quiet);
    let mut serving: Option<tokio::task::JoinHandle<()>> = None;
    let mut event = None;
    loop {
        let wanted = daemon.is_available();
        let ended = serving
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished);
        if link_goes(event.as_ref(), wanted, ended)
            && let Some(served) = serving.take()
        {
            // Its session goes with it, and the token with that.
            served.abort();
        }
        if wanted && serving.is_none() {
            serving = open_and_serve(&socket, &config, &notifier).await;
        }
        event = watcher.next().await;
        if event.is_none() {
            return;
        }
    }
}

/// Whether the link the settings are served on goes, woken by `event`
/// with the daemon `available` or not: when the daemon isn't there, and
/// when it went, though another may be there by now, since the link was
/// to the one that went. Events lost on the way may have been its going.
/// A link whose task `ended` by itself is done with, whatever the daemon.
fn link_goes(event: Option<&StateEvent>, available: bool, ended: bool) -> bool {
    ended
        || !available
        || matches!(
            event,
            Some(StateEvent::DaemonDisappeared | StateEvent::FallbackPoll)
        )
}

/// Opens a link in client mode to the daemon on `socket` and serves the
/// settings on it, on a task of its own.
async fn open_and_serve(
    socket: &Path,
    config: &Path,
    notifier: &Notifier,
) -> Option<tokio::task::JoinHandle<()>> {
    let link = match bootstrap::client_config(socket) {
        Ok(link) => zenoh::open(link).await,
        Err(error) => {
            warn!(%error, "the tray's settings can't be served on this socket");
            return None;
        }
    };
    let link = match link {
        Ok(link) => link,
        Err(error) => {
            // The daemon went between its token and this.
            warn!(%error, "the tray's settings aren't served: no link to the daemon");
            return None;
        }
    };
    let config = config.to_path_buf();
    let notifier = notifier.clone();
    Some(tokio::spawn(async move {
        if let Err(error) = serve_settings(link, config, notifier).await {
            warn!(
                error = format!("{error:#}"),
                "the tray's settings service stopped"
            );
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The switches and the profile last used share a file, written
    /// from different tasks: each write keeps what the other wrote.
    #[tokio::test]
    async fn the_switches_and_the_profile_last_used_keep_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tray.toml");
        assert_eq!(load_target(&path), None);

        save_target(&path, "couch".to_string()).await;
        assert_eq!(load_target(&path).as_deref(), Some("couch"));
        assert_eq!(
            load_settings(&path).unwrap(),
            NotificationSettings::default()
        );

        let quieter = NotificationSettings {
            pad_connection: false,
            ..NotificationSettings::default()
        };
        assert_eq!(save_settings(&path, quieter).await, Ok(()));
        assert_eq!(load_target(&path).as_deref(), Some("couch"));

        save_target(&path, "desk".to_string()).await;
        assert_eq!(load_target(&path).as_deref(), Some("desk"));
        assert_eq!(load_settings(&path).unwrap(), quieter);

        // A file from before the profile was kept has none.
        std::fs::write(&path, "schema = 1\nsetup_started = false\n").unwrap();
        assert_eq!(load_target(&path), None);
    }

    /// A daemon that went and came back between two looks is served
    /// through a new link: the old one was to the daemon that went.
    #[test]
    fn the_link_goes_with_the_daemon_it_was_to() {
        let gone = Some(&StateEvent::DaemonDisappeared);
        assert!(link_goes(gone, true, false), "another is there already");
        assert!(link_goes(gone, false, false));
        assert!(link_goes(None, false, false));
        assert!(link_goes(Some(&StateEvent::FallbackPoll), true, false));
        assert!(link_goes(Some(&StateEvent::DaemonAppeared), true, true));
        // The daemon it is to, still there.
        assert!(!link_goes(Some(&StateEvent::DaemonAppeared), true, false));
        assert!(!link_goes(
            Some(&StateEvent::DaemonStateChanged),
            true,
            false
        ));
        assert!(!link_goes(None, true, false));
    }

    #[test]
    fn a_body_read_as_markup_shows_what_was_written() {
        assert_eq!(
            as_markup("Mad Catz <Pro> & 8BitDo disconnected"),
            "Mad Catz &lt;Pro&gt; &amp; 8BitDo disconnected"
        );
        assert_eq!(as_markup("plain"), "plain");
    }

    /// A hand edit that isn't settings is left for a reload to refuse
    /// with a reason: keeping the profile last used doesn't write the
    /// defaults over it.
    #[tokio::test]
    async fn a_file_that_is_not_settings_is_not_written_over() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tray.toml");
        let broken = "schema = 1\nsetup_started = \"no\"\n";
        std::fs::write(&path, broken).unwrap();
        save_target(&path, "couch".to_string()).await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
        assert!(read_for_reload(&path, NotificationSettings::default()).is_err());
    }

    /// A switch set from another client is kept before it is taken:
    /// one that can't be written is answered as a failure.
    #[tokio::test]
    async fn a_switch_that_cannot_be_written_is_answered_as_not_set() {
        let dir = tempfile::tempdir().unwrap();
        let patch = rpc::NotificationSettingsPatch {
            refused: Some(false),
            ..rpc::NotificationSettingsPatch::default()
        };
        let held = NotificationSettings::default();
        let path = dir.path().join("tray.toml");
        let set = update(&path, held, &patch).await.unwrap();
        assert!(!set.refused);
        assert_eq!(load_settings(&path).unwrap(), set);
        // Where a file stands in for the directory, nothing is written.
        let blocked = dir.path().join("tray.toml").join("tray.toml");
        let refused = update(&blocked, held, &patch).await.unwrap_err();
        assert_eq!(refused.code, ErrorCode::Internal);
        assert!(refused.message.starts_with("writing "), "{refused:?}");
    }
}
