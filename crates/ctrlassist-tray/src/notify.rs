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
use ctrlassist::{DaemonClient, StateWatcher, StateWatcherConfig};
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

/// `tray.toml`: the switches, under a schema number.
#[derive(Serialize, Deserialize)]
struct Persisted {
    #[serde(default = "schema")]
    schema: u32,
    #[serde(flatten)]
    settings: NotificationSettings,
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

/// Writes the switches to `path`, off the runtime's threads since the
/// write waits for the disk. Says whether the file now holds them.
async fn save_settings(path: &Path, settings: NotificationSettings) -> bool {
    let persisted = Persisted {
        schema: schema(),
        settings,
    };
    let path = path.to_path_buf();
    let written = tokio::task::spawn_blocking(move || {
        let text = toml::to_string(&persisted).map_err(std::io::Error::other)?;
        ctrlassist_daemon::durable::write(&path, &text)
    })
    .await;
    match written {
        Ok(Ok(())) => true,
        Ok(Err(error)) => {
            warn!(%error, "the tray's settings couldn't be written");
            false
        }
        Err(error) => {
            warn!(%error, "writing the tray's settings failed");
            false
        }
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
                settings = settings.merged_with(&patch);
                notifier.set_enabled(settings);
                save_settings(&config, settings).await;
                publish(settings).await;
                responder.ok(&settings).await;
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
    loop {
        let wanted = daemon.is_available();
        // One that ended by itself is done with, whatever the daemon.
        let ended = serving
            .as_ref()
            .is_some_and(tokio::task::JoinHandle::is_finished);
        if (ended || !wanted)
            && let Some(served) = serving.take()
        {
            // Its session goes with it, and the token with that.
            served.abort();
        }
        if wanted && serving.is_none() {
            serving = open_and_serve(&socket, &config, &notifier).await;
        }
        if watcher.next().await.is_none() {
            return;
        }
    }
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
