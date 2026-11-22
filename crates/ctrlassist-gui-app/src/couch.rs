//! Couch mode: the GUI's layout for a TV across the room, driven by a
//! pad or a remote. Maximized, scaled up on top of the display's own scale, and
//! with the keys adapted for a 10-foot session.
//!
//! The preference is the GUI's own file, `gui.toml` in the user's
//! config directory, the first state the GUI keeps for itself; nothing
//! on the wire. `Auto` follows the session's own signal, so Plasma
//! Bigscreen gets the layout without a setting, and the file lets a
//! plain desktop driving a TV ask for it. The override for testing is
//! an environment variable rather than a flag, since a window started
//! by its desktop entry is given no arguments.

use std::cell::Cell;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::warn;

/// The environment override: `auto`, `on` or `off`. Anything else is
/// ignored with a warning.
pub(crate) const ENV_OVERRIDE: &str = "CTRLASSIST_COUCH_MODE";

/// The scale on top of the compositor's when nothing else is set:
/// Plasma Bigscreen's body font over the GUI's.
pub(crate) const DEFAULT_SCALE: f32 = 1.4;

/// Whether couch mode follows the session or is forced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CouchMode {
    #[default]
    Auto,
    On,
    Off,
}

impl CouchMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::On => "on",
            Self::Off => "off",
        }
    }

    pub(crate) fn parse(text: &str) -> Option<Self> {
        match text {
            "auto" => Some(Self::Auto),
            "on" => Some(Self::On),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

/// The couch-mode preference.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct CouchPrefs {
    pub(crate) mode: CouchMode,
    /// The scale on top of the compositor's.
    pub(crate) scale: f32,
}

impl Default for CouchPrefs {
    fn default() -> Self {
        Self {
            mode: CouchMode::Auto,
            scale: DEFAULT_SCALE,
        }
    }
}

impl CouchPrefs {
    /// The scale within the Settings control's bounds (in percent):
    /// the file is hand-editable, so a value outside them is clamped,
    /// and one that isn't a number (TOML's `nan`, `inf`) is the default.
    pub(crate) fn sanitized(self, min_percent: i32, max_percent: i32) -> Self {
        debug_assert!(
            0 < min_percent && min_percent <= max_percent,
            "the scale's bounds are a positive, ordered range (the Couch global's)"
        );
        let scale = if self.scale.is_finite() {
            self.scale
                .clamp(min_percent as f32 / 100.0, max_percent as f32 / 100.0)
        } else {
            DEFAULT_SCALE
        };
        // Whole percents, as the control shows and the hook applies.
        let scale = (scale * 100.0).round() / 100.0;
        Self { scale, ..self }
    }
}

/// `gui.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct GuiConfig {
    version: u32,
    #[serde(default)]
    couch: CouchPrefs,
}

const SCHEMA_VERSION: u32 = 1;

/// Where `gui.toml` lives: the user's config directory, beside the
/// daemon's files.
pub(crate) fn default_config_path() -> PathBuf {
    ctrlassist_daemon::Paths::default_for_user()
        .config
        .join("gui.toml")
}

/// The file as it parses.
fn read(path: &Path) -> io::Result<GuiConfig> {
    toml::from_str(&std::fs::read_to_string(path)?).map_err(io::Error::other)
}

/// Moves a file that can't be read out of the next save's way, to the
/// same name with `.edited` after it, and says where it went.
fn set_aside(path: &Path) -> io::Result<PathBuf> {
    let mut aside = path.as_os_str().to_owned();
    aside.push(".edited");
    let aside = PathBuf::from(aside);
    std::fs::rename(path, &aside)?;
    Ok(aside)
}

/// The preference on disk, or the default when the file is missing. An
/// unreadable file is reported and read as the default, since the GUI
/// must still open, and set aside: the next save would overwrite it.
pub(crate) fn load(path: &Path) -> CouchPrefs {
    if matches!(path.try_exists(), Ok(false)) {
        return CouchPrefs::default();
    }
    match read(path) {
        Ok(config) => config.couch,
        Err(e) => {
            match set_aside(path) {
                Ok(aside) => warn!(
                    error = %e,
                    file = %path.display(),
                    aside = %aside.display(),
                    "couch preference file unreadable; set aside"
                ),
                Err(aside_error) => warn!(
                    error = %e,
                    file = %path.display(),
                    %aside_error,
                    "couch preference file unreadable, and couldn't be set aside"
                ),
            }
            CouchPrefs::default()
        }
    }
}

/// Write the preference durably.
pub(crate) fn save(path: &Path, prefs: CouchPrefs) -> io::Result<()> {
    let config = GuiConfig {
        version: SCHEMA_VERSION,
        couch: prefs,
    };
    let text = toml::to_string(&config).map_err(io::Error::other)?;
    ctrlassist_daemon::durable::write(path, &text)
}

/// The preference's writer: one task writing in order, the latest
/// value last. The scale spin box commits a value per step of a held
/// arrow, and concurrent atomic writes would unlink each other's temp
/// files. Each change carries a sequence number and the task reports
/// the last one it attempted, written or not, so [`flush`](Self::flush)
/// waits for exactly the changes made and never for a write that
/// failed.
pub(crate) struct PrefsWriter {
    changes: tokio::sync::watch::Sender<(u64, CouchPrefs)>,
    /// The sequence number of the last change sent.
    sent: Cell<u64>,
    /// The sequence number of the last change the task attempted.
    settled: tokio::sync::watch::Receiver<u64>,
}

impl PrefsWriter {
    /// Start the writer for `path` on the current runtime. `initial` is
    /// what the file holds now; nothing is written until a change.
    pub(crate) fn spawn(path: PathBuf, initial: CouchPrefs) -> Self {
        let (changes, mut changes_rx) = tokio::sync::watch::channel((0, initial));
        let (settled_tx, settled) = tokio::sync::watch::channel(0);
        tokio::spawn(async move {
            while changes_rx.changed().await.is_ok() {
                let (seq, prefs) = *changes_rx.borrow_and_update();
                let target = path.clone();
                match tokio::task::spawn_blocking(move || save(&target, prefs)).await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        warn!(%error, file = %path.display(), "couch preference write failed");
                    }
                    Err(error) => warn!(%error, "couch preference writer stopped"),
                }
                // The receiver lives as long as the writer.
                let _ = settled_tx.send(seq);
            }
        });
        Self {
            changes,
            sent: Cell::new(0),
            settled,
        }
    }

    /// Queue a change. One queued over a change not yet written
    /// replaces it.
    pub(crate) fn send(&self, prefs: CouchPrefs) {
        let seq = self.sent.get() + 1;
        self.sent.set(seq);
        // Only a closed channel refuses, and the task runs as long as
        // the runtime.
        let _ = self.changes.send((seq, prefs));
    }

    /// Wait until the task has attempted every change sent so far, at
    /// most `timeout`; `false` when it hadn't by then.
    pub(crate) async fn flush(&self, timeout: Duration) -> bool {
        let last = self.sent.get();
        let mut settled = self.settled.clone();
        let drained = async move {
            while *settled.borrow_and_update() < last {
                if settled.changed().await.is_err() {
                    break;
                }
            }
        };
        tokio::time::timeout(timeout, drained).await.is_ok()
    }
}

/// The window's scale: the compositor's times couch mode's factor, or
/// the compositor's own when couch mode is off (`None`).
pub(crate) fn window_scale(compositor: f32, factor: Option<f32>) -> f32 {
    compositor * factor.unwrap_or(1.0)
}

/// Whether the window's scale is another than `wanted`, beyond float
/// noise: a resize that finds the scale in place leaves it alone.
pub(crate) fn scale_is_stale(applied: f32, wanted: f32) -> bool {
    (applied - wanted).abs() > 1e-3
}

/// The 10-foot-session signals a desktop exposes to its apps.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct SessionSignals {
    /// Plasma's platform list, `PLASMA_PLATFORM`: Bigscreen sets it to
    /// `mediacenter`, the key its own apps pick their TV layouts by.
    pub(crate) plasma_platform: Option<String>,
}

impl SessionSignals {
    pub(crate) fn from_env() -> Self {
        Self {
            plasma_platform: std::env::var("PLASMA_PLATFORM").ok(),
        }
    }
}

/// Whether the session is a 10-foot one. `PLASMA_PLATFORM` is a
/// colon-separated list, so a component is matched, not the whole.
pub(crate) fn is_ten_foot_session(signals: &SessionSignals) -> bool {
    signals
        .plasma_platform
        .as_deref()
        .is_some_and(|list| list.split(':').any(|item| item == "mediacenter"))
}

/// Whether couch mode is on: the override wins, then the preference,
/// and `Auto` follows the session.
pub(crate) fn active(
    mode: CouchMode,
    env_override: Option<&str>,
    signals: &SessionSignals,
) -> bool {
    let mode = match env_override {
        Some(text) => match CouchMode::parse(text) {
            Some(forced) => forced,
            None => {
                warn!(
                    value = text,
                    "{ENV_OVERRIDE} isn't auto, on or off; ignored"
                );
                mode
            }
        },
        None => mode,
    };
    match mode {
        CouchMode::On => true,
        CouchMode::Off => false,
        CouchMode::Auto => is_ten_foot_session(signals),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bigscreen() -> SessionSignals {
        SessionSignals {
            plasma_platform: Some("mediacenter".into()),
        }
    }

    #[test]
    fn auto_follows_the_session() {
        assert!(active(CouchMode::Auto, None, &bigscreen()));
        assert!(!active(CouchMode::Auto, None, &SessionSignals::default()));
        let desktop = SessionSignals {
            plasma_platform: Some("desktop".into()),
        };
        assert!(!active(CouchMode::Auto, None, &desktop));
    }

    #[test]
    fn a_platform_list_matches_by_component() {
        let listed = SessionSignals {
            plasma_platform: Some("phone:mediacenter".into()),
        };
        assert!(is_ten_foot_session(&listed));
        let prefix = SessionSignals {
            plasma_platform: Some("mediacenter-ish".into()),
        };
        assert!(!is_ten_foot_session(&prefix));
    }

    #[test]
    fn the_preference_forces_either_way() {
        assert!(active(CouchMode::On, None, &SessionSignals::default()));
        assert!(!active(CouchMode::Off, None, &bigscreen()));
    }

    #[test]
    fn the_override_wins_and_nonsense_is_ignored() {
        assert!(active(
            CouchMode::Off,
            Some("on"),
            &SessionSignals::default()
        ));
        assert!(!active(CouchMode::On, Some("off"), &bigscreen()));
        assert!(active(CouchMode::Off, Some("auto"), &bigscreen()));
        assert!(active(
            CouchMode::On,
            Some("sideways"),
            &SessionSignals::default()
        ));
    }

    #[test]
    fn the_preference_round_trips_and_a_missing_file_is_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("gui.toml");
        assert_eq!(load(&path), CouchPrefs::default());
        let prefs = CouchPrefs {
            mode: CouchMode::On,
            scale: 1.8,
        };
        save(&path, prefs).unwrap();
        assert_eq!(load(&path), prefs);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("mode = \"on\""), "{text}");
    }

    #[test]
    fn an_unreadable_file_reads_as_the_default_and_is_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gui.toml");
        std::fs::write(&path, "version = = 1\n").unwrap();
        assert_eq!(load(&path), CouchPrefs::default());
        assert!(
            !path.exists(),
            "the unreadable file is out of the next save's way"
        );
        assert!(dir.path().join("gui.toml.edited").exists());
    }

    #[test]
    fn a_scale_outside_the_controls_bounds_is_clamped_and_nonsense_is_the_default() {
        let percent = |scale: f32| {
            let prefs = CouchPrefs {
                mode: CouchMode::On,
                scale,
            }
            .sanitized(100, 250);
            assert_eq!(prefs.mode, CouchMode::On, "only the scale is touched");
            (prefs.scale * 100.0).round() as i32
        };
        assert_eq!(percent(0.0), 100);
        assert_eq!(percent(-1.0), 100);
        assert_eq!(percent(1e9), 250);
        assert_eq!(percent(1.8), 180);
        let default = (DEFAULT_SCALE * 100.0).round() as i32;
        assert_eq!(percent(f32::NAN), default);
        assert_eq!(percent(f32::INFINITY), default);
    }

    #[tokio::test]
    async fn the_writer_lands_the_last_change_and_the_flush_waits_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("gui.toml");
        let writer = PrefsWriter::spawn(path.clone(), CouchPrefs::default());
        assert!(
            writer.flush(Duration::from_secs(5)).await,
            "nothing sent, nothing to wait for"
        );
        assert!(!path.exists(), "the initial value isn't written back");
        for percent in [150, 160, 170] {
            writer.send(CouchPrefs {
                mode: CouchMode::On,
                scale: percent as f32 / 100.0,
            });
        }
        assert!(writer.flush(Duration::from_secs(5)).await);
        assert_eq!(
            load(&path),
            CouchPrefs {
                mode: CouchMode::On,
                scale: 1.7,
            }
        );
    }

    #[tokio::test]
    async fn the_flush_doesnt_wait_for_a_write_that_failed() {
        let dir = tempfile::tempdir().unwrap();
        let not_a_dir = dir.path().join("file");
        std::fs::write(&not_a_dir, "").unwrap();
        let writer = PrefsWriter::spawn(not_a_dir.join("gui.toml"), CouchPrefs::default());
        writer.send(CouchPrefs {
            mode: CouchMode::Off,
            scale: 1.2,
        });
        assert!(
            writer.flush(Duration::from_secs(5)).await,
            "an attempted write settles, written or not"
        );
    }

    #[test]
    fn the_window_scale_stacks_on_the_compositors_and_a_resize_finds_it_in_place() {
        assert_eq!(window_scale(2.0, None), 2.0);
        assert_eq!(window_scale(1.0, Some(1.5)), 1.5);
        let wanted = window_scale(1.5, Some(1.4));
        assert!(
            scale_is_stale(1.5, wanted),
            "the window's first size carries the compositor's scale alone"
        );
        assert!(!scale_is_stale(wanted, wanted));
        assert!(!scale_is_stale(2.1, wanted), "float noise isn't a change");
    }
}
