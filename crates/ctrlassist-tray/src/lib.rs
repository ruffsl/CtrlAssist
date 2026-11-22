//! CtrlAssist's system tray. The menu is rendered from the SDK client's
//! mirror of the daemon, every click is an rpc, and what changes in the
//! mirror that a person should know of becomes a notification.

mod desktop;
pub mod menu;
pub mod notify;
pub mod service;
mod settings;
pub mod state;
pub mod tip;
pub mod tray;

use std::sync::Arc;
use std::time::Duration;

use ctrlassist::{DaemonClient, StateWatcher, StateWatcherConfig};
use tokio::sync::{Notify, mpsc};
use tracing::{info, warn};

pub use desktop::{TrayOptions, run};

use notify::Notifier;
use state::{Toast, View};
use tray::{Action, PadTray};

/// How long `Identify` in the menu buzzes a pad.
const BUZZ: Duration = Duration::from_millis(600);

/// Where the tray's state lives: in the desktop's tray, or for tests in
/// none.
#[derive(Clone)]
pub(crate) enum Handle {
    Desktop(ksni::Handle<PadTray>),
    #[cfg(test)]
    Headless(Arc<tokio::sync::Mutex<PadTray>>),
}

impl Handle {
    async fn update<R>(&self, update: impl FnOnce(&mut PadTray) -> R) -> Option<R> {
        match self {
            Self::Desktop(handle) => handle.update(update).await,
            #[cfg(test)]
            Self::Headless(tray) => Some(update(&mut *tray.lock().await)),
        }
    }
}

/// What the client mirrors right now.
fn snapshot(daemon: &DaemonClient) -> View {
    View {
        alive: daemon.is_available(),
        hydrated: daemon.is_hydrated(),
        mismatch: daemon
            .service_info()
            .map(|info| info.version)
            .filter(|theirs| {
                !ctrlassist_protocol::compatible(ctrlassist_protocol::VERSION, theirs)
            }),
        daemon: daemon.daemon_state().ok().flatten(),
        pads: daemon.list_pads().unwrap_or_default(),
        profiles: daemon.list_profiles().unwrap_or_default(),
        setups: daemon.list_setups().unwrap_or_default(),
        identities: daemon.list_identities().unwrap_or_default(),
        // Asked for by the loop that follows the daemon, not mirrored.
        fields: Vec::new(),
    }
}

/// Keeps the tray showing what the client mirrors, rebuilt on each of
/// the client's events, and says what changed that is worth saying.
pub(crate) async fn follow(daemon: DaemonClient, handle: Handle, notifier: Notifier) {
    let mut watcher = StateWatcher::new(&daemon, None, StateWatcherConfig::default());
    // Whether the daemon was there at the last look. Not known at the
    // first, so a tray started without a daemon has nothing to say of it.
    let mut was_alive: Option<bool> = None;
    let mut started = false;
    // A preset's fields, asked once of each daemon that comes.
    let mut fields = Vec::new();
    loop {
        let mut view = snapshot(&daemon);
        if !view.alive {
            fields.clear();
        } else if fields.is_empty() {
            match daemon.preset_form().await {
                Ok(served) => fields = served,
                Err(error) => warn!(%error, "the daemon didn't say what a preset's fields are"),
            }
        }
        view.fields.clone_from(&fields);
        let alive = view.alive;
        let news = handle
            .update(move |tray| {
                let mut news = view.news(&tray.view);
                news.extend(state::daemon_news(
                    was_alive,
                    alive,
                    !tray.view.setups.is_empty(),
                ));
                tray.see(view);
                news
            })
            .await
            .unwrap_or_default();
        for toast in news {
            notifier.notify(toast);
        }
        if started {
            was_alive = Some(alive);
        }
        started = true;
        if watcher.next().await.is_none() {
            return;
        }
    }
}

/// What to say when something asked from the menu didn't happen.
fn refused(what: String, error: &ctrlassist::Error) -> Toast {
    Toast {
        category: notify::Category::Refused,
        summary: what,
        body: error.to_string(),
        icon: "dialog-error",
    }
}

/// Does what the menu's clicks ask, one at a time, as rpcs. One that
/// fails is said as a notification: a menu has nowhere else to say it.
pub(crate) async fn act(
    daemon: DaemonClient,
    mut clicked: mpsc::UnboundedReceiver<Action>,
    notifier: Notifier,
    quit: Arc<Notify>,
    config: Option<std::path::PathBuf>,
    handle: Handle,
) {
    let opening = Arc::new(std::sync::atomic::AtomicBool::new(false));
    while let Some(action) = clicked.recv().await {
        // Not a click, and one for every notch of the wheel: kept, and
        // nothing said of it.
        let action = match action {
            Action::Aim(name) => {
                if let Some(config) = &config {
                    notify::save_target(config, name).await;
                }
                continue;
            }
            other => other,
        };
        // Said as it is taken up, so that a click that led nowhere can
        // be told from one that was never made.
        info!(action = action.named(), "an action from the menu");
        let (what, outcome) = match action {
            Action::Quit => {
                quit.notify_one();
                return;
            }
            // Taken above.
            Action::Aim(_) => continue,
            Action::Settings => {
                // On a task of its own: a window that is starting takes
                // a while to answer, and the menu's other clicks don't
                // wait for it. One opening at a time: clicks while one
                // is under way are that one's.
                if !opening.swap(true, std::sync::atomic::Ordering::AcqRel) {
                    let (opening, notifier) = (Arc::clone(&opening), notifier.clone());
                    tokio::spawn(async move {
                        if let Err(error) = settings::open().await {
                            warn!(
                                error = format!("{error:#}"),
                                "the settings window didn't open"
                            );
                            notifier.notify(Toast {
                                category: notify::Category::Refused,
                                summary: "Couldn't open CtrlAssist's settings".to_string(),
                                body: format!("{error:#}"),
                                icon: "dialog-error",
                            });
                        }
                        opening.store(false, std::sync::atomic::Ordering::Release);
                    });
                }
                continue;
            }
            Action::Start(name) => (
                format!("Couldn't start {name}"),
                daemon.start_setup(&name).await,
            ),
            Action::Stop(name) => (
                format!("Couldn't stop {name}"),
                daemon.stop_setup(&name).await,
            ),
            Action::Select {
                setup,
                node,
                choice,
            } => (
                format!("Couldn't hand {setup} over to {choice}"),
                daemon.select(&setup, &node, Some(&choice)).await,
            ),
            Action::Identify(pad) => (
                "Couldn't buzz the pad".to_string(),
                daemon.identify_pad(&pad, BUZZ).await,
            ),
            Action::Forget(pad) => (
                "Couldn't forget the pad".to_string(),
                daemon.forget_pad(&pad).await,
            ),
            Action::Put { name, toml } => {
                let outcome = put_and_apply(&daemon, &name, &toml).await;
                // The tray showed the pick as made. Refused, it shows
                // what is kept again.
                if outcome.is_err() {
                    let kept = daemon.list_profiles().unwrap_or_default();
                    let name = name.clone();
                    handle.update(move |tray| tray.unpick(&name, kept)).await;
                }
                (format!("Couldn't change {name}"), outcome)
            }
        };
        if let Err(error) = outcome {
            warn!(%error, what, "an action from the menu failed");
            notifier.notify(refused(what, &error));
        }
    }
}

/// Keeps `toml` as the profile `name`, and has a setup that runs it
/// take the change: as it runs where it can, and by a restart where it
/// can't. One that doesn't run is only kept.
async fn put_and_apply(daemon: &DaemonClient, name: &str, toml: &str) -> ctrlassist::Result<()> {
    daemon.put_profile(name, toml).await?;
    if daemon.setup(name)?.is_some() {
        daemon.update_setup(name, true).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! The tray's loop against a live daemon and fake pads, with no
    //! desktop: the tray's state is held here, and its notifications are
    //! caught instead of shown.

    use std::path::Path;

    use ctrlassist_daemon::{Daemon, Paths};
    use ctrlassist_device::InputThread;
    use ctrlassist_test::{FakePad, can_make_pads};
    use tokio::sync::Mutex;

    use super::*;
    use crate::menu::{Command, outline};

    struct Rig {
        _dir: tempfile::TempDir,
        daemon: Option<Daemon>,
        paths: Paths,
        tray: Arc<Mutex<PadTray>>,
        toasts: mpsc::UnboundedReceiver<Toast>,
    }

    async fn start_daemon(paths: &Paths) -> Daemon {
        let (input, reports) = InputThread::spawn().unwrap();
        Daemon::start(paths, input, reports).await.unwrap()
    }

    async fn rig() -> Rig {
        rig_remembering(0).await
    }

    /// A rig whose daemon starts out remembering this many pads.
    async fn rig_remembering(pads: usize) -> Rig {
        let dir = tempfile::Builder::new()
            .prefix("ca")
            .tempdir_in("/tmp")
            .unwrap();
        let at = |name: &str| dir.path().join(name);
        if pads > 0 {
            use std::fmt::Write as _;
            let mut known = "schema = 1\n".to_string();
            for pad in 0..pads {
                let _ = write!(
                    known,
                    "\n[[pad]]\nid = \"port:1209:5eed:somewhere/{pad}\"\nname = \"Remembered Pad {pad}\"\nmodel = \"1209:5eed\"\n"
                );
            }
            std::fs::create_dir_all(at("state")).unwrap();
            std::fs::write(at("state").join("pads.toml"), known).unwrap();
        }
        let paths = Paths {
            socket: at("zenoh.sock"),
            state: at("state"),
            config: at("config"),
            hider: at("hider.sock"),
            test_pads: true,
            steam: Vec::new(),
        };
        let daemon = start_daemon(&paths).await;
        let client = DaemonClient::connect(&paths.socket).await.unwrap();
        let (actions, clicked) = mpsc::unbounded_channel();
        let (notifier, toasts) = Notifier::capturing(notify::NotificationSettings::default());
        let tray = Arc::new(Mutex::new(PadTray::new(actions)));
        let handle = Handle::Headless(Arc::clone(&tray));
        tokio::spawn(follow(client.clone(), handle.clone(), notifier.clone()));
        tokio::spawn(act(
            client,
            clicked,
            notifier,
            Arc::new(Notify::new()),
            None,
            handle,
        ));
        Rig {
            _dir: dir,
            daemon: Some(daemon),
            paths,
            tray,
            toasts,
        }
    }

    impl Rig {
        /// Waits until the tray's menu, as an outline, has `wanted` in it.
        async fn shows(&self, wanted: &str) -> String {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
            loop {
                let menu = outline(&self.tray.lock().await.entries());
                if menu.contains(wanted) {
                    return menu;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the menu never showed {wanted:?}:\n{menu}"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }

        async fn click(&self, command: Command) {
            self.tray.lock().await.activate(command);
        }

        /// The next notification whose summary has `wanted` in it.
        async fn told(&mut self, wanted: &str) -> Toast {
            tokio::time::timeout(Duration::from_secs(20), async {
                loop {
                    let toast = self.toasts.recv().await.expect("the notifier is open");
                    if toast.summary.contains(wanted) {
                        return toast;
                    }
                }
            })
            .await
            .unwrap_or_else(|_| unreachable!("never told {wanted:?}"))
        }
    }

    fn id(name: &str, product: u16) -> String {
        format!("model:1209:{product:04x}:{name}")
    }

    /// A pick in the default profile's menu.
    fn pick(key: &str, value: &str) -> Command {
        Command::set("default", key, value)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_default_profile_runs_two_pads_and_the_menu_follows_what_happens_to_them() {
        const TEST: &str =
            "the_default_profile_runs_two_pads_and_the_menu_follows_what_happens_to_them";
        if !can_make_pads(TEST) {
            return;
        }
        let _primary = FakePad::new("Tray Primary", 0x1209, 0x8701).unwrap();
        let assist = FakePad::new("Tray Assist", 0x1209, 0x8702).unwrap();
        let mut rig = rig().await;
        // The default profile is there from the start, its pads to
        // choose, and nothing to start yet.
        let menu = rig.shows("Primary: not chosen >").await;
        assert!(menu.starts_with("default ✓ (Not set up) >\n"), "{menu}");
        assert!(menu.contains("  - Choose its pads to start\n"), "{menu}");
        assert!(menu.contains("Hiding: grab"), "{menu}");

        // Two pads, picked from its menu one after the other, then
        // started.
        rig.click(pick("primary", &id("Tray Primary", 0x8701)))
            .await;
        rig.click(pick("assist", &id("Tray Assist", 0x8702))).await;
        let menu = rig.shows("default ✓ >\n  Start\n").await;
        assert!(menu.contains("Primary: Tray Primary >"), "{menu}");
        assert!(menu.contains("Assist: Tray Assist >"), "{menu}");
        rig.click(Command::Start("default".into())).await;
        rig.shows("default ✓ (Running) >").await;
        // Without the hiding service the pads are grabbed, which is said
        // once, as the setup starts.
        let grabbed = rig.told("grabbed, not hidden").await;
        assert!(grabbed.summary.starts_with("default:"), "{grabbed:?}");
        rig.told("no rumble on Tray").await;

        // A mode picked while it runs is taken at once, by the virtual
        // pad it has: nothing waits for a click.
        let node = |tray: &PadTray| tray.view.setup("default").unwrap().sinks[0].node.clone();
        let made = |rig: &Rig| {
            let tray = Arc::clone(&rig.tray);
            async move { node(&*tray.lock().await) }
        };
        let before = made(&rig).await;
        assert!(before.is_some());
        rig.click(pick("mode", "toggle")).await;
        rig.shows("Has control >").await;
        assert_eq!(made(&rig).await, before);
        // A pad one part has can't be picked for the other.
        let menu = rig.shows("Mode: Toggle >").await;
        assert!(
            menu.contains("    (o) Tray Primary\n    ( ) - Tray Assist\n"),
            "{menu}"
        );
        // What the daemon refuses is said, and the menu goes back to
        // what is kept.
        rig.click(pick("primary", &id("Tray Assist", 0x8702))).await;
        rig.told("Couldn't change default").await;
        rig.shows("Primary: Tray Primary >").await;
        assert_eq!(made(&rig).await, before);
        // Another identity is another virtual pad: picking it restarts
        // the setup at once, with nothing more to click.
        rig.click(pick("identity", "xbox")).await;
        rig.shows("Identity: Xbox >").await;
        eventually_pad(&rig, "Microsoft Xbox Series S|X Controller (in default)").await;
        rig.shows("default ✓ (Running) >").await;
        rig.click(pick("identity", "generic")).await;
        eventually_pad(&rig, "CtrlAssist Virtual Pad (in default)").await;
        // Stopped, a pick is kept, and Start runs the profile as kept.
        rig.click(Command::Stop("default".into())).await;
        rig.shows("default ✓ >\n  Start\n").await;
        rig.click(pick("identity", "xbox")).await;
        rig.shows("Identity: Xbox >").await;
        rig.click(Command::Start("default".into())).await;
        eventually_pad(&rig, "Microsoft Xbox Series S|X Controller (in default)").await;
        rig.click(pick("identity", "generic")).await;
        eventually_pad(&rig, "CtrlAssist Virtual Pad (in default)").await;

        // The toggle's holder is in the menu, and a click hands over.
        let menu = rig.shows("(o) primary").await;
        assert!(menu.contains("Has control >"), "{menu}");
        rig.click(Command::Select {
            setup: "default".into(),
            node: "mux".into(),
            choice: "assist".into(),
        })
        .await;
        rig.shows("(o) assist").await;

        // The assist leaves: the setup is held, the menu and a
        // notification say so, and it can't be handed to a pad that is gone.
        drop(assist);
        let menu = rig.shows("default ✓ (Held) >").await;
        assert!(
            menu.contains("Assist: Tray Assist (disconnected) >"),
            "{menu}"
        );
        let held = rig.told("default is held").await;
        assert!(
            held.body.starts_with("Tray Assist disconnected"),
            "{held:?}"
        );
        rig.shows("( ) - assist").await;

        // It returns: running again, and said.
        let _assist = FakePad::new("Tray Assist", 0x1209, 0x8702).unwrap();
        rig.shows("default ✓ (Running) >").await;
        rig.told("default is running again").await;

        // Stopped from the menu; the profile stays, to start again.
        rig.click(Command::Stop("default".into())).await;
        rig.shows("default ✓ >\n  Start\n").await;

        // What can't be done is said, since a menu has nowhere to say it.
        rig.click(Command::Stop("default".into())).await;
        let refused = rig.told("Couldn't stop default").await;
        assert!(refused.body.contains("unknown-setup"), "{refused:?}");
        rig.click(Command::Start("nothing".into())).await;
        rig.told("Couldn't start nothing").await;
        rig.click(Command::Identify(id("Tray Primary", 0x8701)))
            .await;
        let silent = rig.told("Couldn't buzz the pad").await;
        assert!(silent.body.contains("pad-without-rumble"), "{silent:?}");
        rig.click(Command::Forget(id("Tray Primary", 0x8701))).await;
        rig.told("Couldn't forget the pad").await;
        rig.click(Command::Select {
            setup: "default".into(),
            node: "mux".into(),
            choice: "assist".into(),
        })
        .await;
        rig.told("Couldn't hand default over to assist").await;
    }

    /// Waits for a virtual pad of this name among the menu's pads.
    async fn eventually_pad(rig: &Rig, wanted: &str) {
        rig.shows(&format!("  {wanted} >\n")).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_tray_outlives_the_daemon_and_a_later_tray_reads_what_was_picked() {
        const TEST: &str = "the_tray_outlives_the_daemon_and_a_later_tray_reads_what_was_picked";
        if !can_make_pads(TEST) {
            return;
        }
        let _primary = FakePad::new("Tray Kept Primary", 0x1209, 0x8711).unwrap();
        let _assist = FakePad::new("Tray Kept Assist", 0x1209, 0x8712).unwrap();
        let mut rig = rig().await;
        rig.shows("Primary: not chosen >").await;
        rig.click(pick("primary", &id("Tray Kept Primary", 0x8711)))
            .await;
        rig.click(pick("assist", &id("Tray Kept Assist", 0x8712)))
            .await;
        rig.click(pick("rumble", "primary")).await;
        // A managed profile starts by itself once its pads are there.
        rig.click(pick("managed", "true")).await;
        rig.shows("default ✓ (Running) >").await;

        // The daemon stops: the menu says so, and a notification says
        // what went with it.
        rig.daemon.take().unwrap().stop().await;
        let menu = rig.shows("The CtrlAssist daemon isn't running").await;
        assert!(!menu.contains("default"), "{menu}");
        let stopped = rig.told("daemon stopped").await;
        assert!(
            stopped.body.contains("virtual pads are gone"),
            "{stopped:?}"
        );

        // It comes back, and the profile, which asks to, starts by itself.
        rig.daemon = Some(start_daemon(&rig.paths).await);
        rig.told("daemon is back").await;
        rig.shows("default ✓ (Running) >").await;

        // A tray that starts later shows what was picked: it is the
        // profile, and nothing of it is the tray's to remember.
        let client = DaemonClient::connect(Path::new(&rig.paths.socket))
            .await
            .unwrap();
        let (actions, _clicked) = mpsc::unbounded_channel();
        let (notifier, _toasts) = Notifier::capturing(notify::NotificationSettings::default());
        let later = Arc::new(Mutex::new(PadTray::new(actions)));
        tokio::spawn(follow(
            client,
            Handle::Headless(Arc::clone(&later)),
            notifier,
        ));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        let menu = loop {
            let menu = outline(&later.lock().await.entries());
            if menu.contains("Primary: Tray Kept Primary >") {
                break menu;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "it never read the daemon:\n{menu}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        };
        assert!(menu.contains("Rumble goes to: Primary only >"), "{menu}");
        assert!(menu.contains("[x] Managed"), "{menu}");
    }

    /// A daemon that knows hundreds of pads is followed like any other:
    /// the tray reads all of its state and shows it.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_daemon_that_knows_hundreds_of_pads_is_still_followed() {
        let rig = rig_remembering(450).await;
        // More than the 256 samples zenoh queues for a subscription
        // that isn't being read yet, all under the one key.
        let menu = rig.shows("Remembered Pad 449").await;
        assert!(menu.contains("Pads ("), "the daemon is followed");
    }
}
