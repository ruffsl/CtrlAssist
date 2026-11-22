//! CtrlAssist's system tray. The menu is rendered from the SDK client's
//! mirror of the daemon, every click is an rpc, and what changes in the
//! mirror that a person should know of becomes a notification.

mod desktop;
pub mod menu;
pub mod notify;
pub mod quick;
pub mod service;
mod settings;
pub mod state;
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
                // The quick-start choices start out as whatever the kept
                // `quick` profile was written from, which takes the
                // fields to read.
                if view.hydrated && !view.fields.is_empty() && !tray.quick_settled {
                    tray.quick_settled = true;
                    if let Some(kept) = view.kept_quick() {
                        tray.quick = kept;
                    }
                }
                tray.view = view;
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
) {
    let opening = Arc::new(std::sync::atomic::AtomicBool::new(false));
    while let Some(action) = clicked.recv().await {
        // Said as it is taken up, so that a click that led nowhere can
        // be told from one that was never made.
        info!(action = action.named(), "an action from the menu");
        let (what, outcome) = match action {
            Action::Quit => {
                quit.notify_one();
                return;
            }
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
            Action::Quick(toml) => (
                format!("Couldn't start `{}`", quick::NAME),
                quick_start(&daemon, &toml).await,
            ),
        };
        if let Err(error) = outcome {
            warn!(%error, what, "an action from the menu failed");
            notifier.notify(refused(what, &error));
        }
    }
}

/// Keeps `toml` as the profile `quick` and runs it: started if it isn't
/// running, restarted on the new profile if it is.
async fn quick_start(daemon: &DaemonClient, toml: &str) -> ctrlassist::Result<()> {
    daemon.put_profile(quick::NAME, toml).await?;
    let running = daemon.setup(quick::NAME)?.is_some();
    if running {
        daemon.update_setup(quick::NAME, true).await.map(|_| ())
    } else {
        match daemon.start_setup(quick::NAME).await {
            // A profile that starts by itself may have, at the put.
            Err(error) if error.rpc_code() == Some(ctrlassist::ErrorCode::SetupExists) => Ok(()),
            outcome => outcome,
        }
    }
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
    use crate::menu::{Command, Edit, outline};

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
        tokio::spawn(follow(client.clone(), handle, notifier.clone()));
        tokio::spawn(act(client, clicked, notifier, Arc::new(Notify::new())));
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

    #[tokio::test(flavor = "multi_thread")]
    async fn quick_start_runs_two_pads_and_the_menu_follows_what_happens_to_them() {
        const TEST: &str = "quick_start_runs_two_pads_and_the_menu_follows_what_happens_to_them";
        if !can_make_pads(TEST) {
            return;
        }
        let _primary = FakePad::new("Tray Primary", 0x1209, 0x8701).unwrap();
        let assist = FakePad::new("Tray Assist", 0x1209, 0x8702).unwrap();
        let mut rig = rig().await;
        let menu = rig.shows("Pads (").await;
        assert!(menu.contains("Quick start >"), "{menu}");
        assert!(menu.contains("Primary: not chosen >"), "{menu}");
        assert!(
            menu.contains("- Start as `quick`"),
            "nothing to start yet:\n{menu}"
        );
        assert!(menu.contains("Hiding: grab"), "{menu}");

        // Two pads, picked from the menu, then started.
        rig.click(Command::Quick(Edit::set(
            "primary",
            &id("Tray Primary", 0x8701),
        )))
        .await;
        rig.click(Command::Quick(Edit::set(
            "assist",
            &id("Tray Assist", 0x8702),
        )))
        .await;
        let menu = outline(&rig.tray.lock().await.entries());
        assert!(menu.contains("Primary: Tray Primary >"), "{menu}");
        assert!(menu.contains("\n  Start as `quick`"), "{menu}");
        rig.click(Command::QuickStart).await;
        let menu = rig.shows("quick (running) >").await;
        assert!(menu.contains("- Running as `quick`"), "{menu}");
        // Without the hiding service the pads are grabbed, which is said
        // once, as the setup starts.
        let grabbed = rig.told("grabbed, not hidden").await;
        assert!(grabbed.summary.starts_with("quick:"), "{grabbed:?}");
        rig.told("no rumble on Tray").await;

        // A mode picked while it runs is taken at once, by the virtual
        // pad it has: nothing waits for a click.
        let node = |tray: &PadTray| tray.view.setup("quick").unwrap().sinks[0].node.clone();
        let before = node(&*rig.tray.lock().await);
        rig.click(Command::Quick(Edit::set("mode", "toggle"))).await;
        let menu = rig.shows("Has control >").await;
        assert!(menu.contains("- Running as `quick`"), "{menu}");
        assert_eq!(node(&*rig.tray.lock().await), before);
        // So is another pad: the two change parts, and back.
        rig.click(Command::Quick(Edit::set(
            "primary",
            &id("Tray Assist", 0x8702),
        )))
        .await;
        {
            let tray = rig.tray.lock().await;
            assert!(
                tray.quick.profile(&tray.view.fields).is_none(),
                "one pad as both parts is no profile, and nothing is asked"
            );
        }
        rig.click(Command::Quick(Edit::set(
            "assist",
            &id("Tray Primary", 0x8701),
        )))
        .await;
        rig.shows("primary: Tray Assist").await;
        rig.click(Command::Quick(Edit::set(
            "assist",
            &id("Tray Assist", 0x8702),
        )))
        .await;
        rig.click(Command::Quick(Edit::set(
            "primary",
            &id("Tray Primary", 0x8701),
        )))
        .await;
        rig.shows("primary: Tray Primary").await;
        assert_eq!(node(&*rig.tray.lock().await), before);
        // Another identity is another virtual pad: picking it restarts
        // the setup at once, with nothing more to click.
        let identity = |identity: ctrlassist_protocol::profile::Identity| {
            Command::Quick(Edit::set("identity", identity.as_str()))
        };
        rig.click(identity(ctrlassist_protocol::profile::Identity::Xbox))
            .await;
        // Another pad, though the kernel may give it the node the old
        // one had.
        rig.shows("virtual: xbox at event").await;
        let menu = rig.shows("- Running as `quick`").await;
        assert!(menu.contains("quick (running) >"), "{menu}");
        rig.click(identity(ctrlassist_protocol::profile::Identity::Generic))
            .await;
        rig.shows("virtual: generic at event").await;
        // Stopped, the profile's own Start runs what is picked now, not
        // what was last kept.
        rig.click(Command::Stop("quick".into())).await;
        rig.shows("quick >\n  Start\n").await;
        rig.click(identity(ctrlassist_protocol::profile::Identity::Xbox))
            .await;
        rig.click(Command::Start("quick".into())).await;
        rig.shows("virtual: xbox at event").await;
        rig.click(identity(ctrlassist_protocol::profile::Identity::Generic))
            .await;
        rig.shows("virtual: generic at event").await;

        // The toggle's holder is in the menu, and a click hands over.
        let menu = rig.shows("(o) primary").await;
        assert!(menu.contains("Has control >"), "{menu}");
        rig.click(Command::Select {
            setup: "quick".into(),
            node: "mux".into(),
            choice: "assist".into(),
        })
        .await;
        rig.shows("(o) assist").await;

        // The assist leaves: the setup is held, the menu and a
        // notification say so, and it can't be handed to a pad that is gone.
        drop(assist);
        let menu = rig.shows("quick (held) >").await;
        assert!(menu.contains("assist: Tray Assist (missing)"), "{menu}");
        let held = rig.told("quick is held").await;
        assert!(
            held.body.starts_with("Tray Assist disconnected"),
            "{held:?}"
        );
        rig.shows("( ) - assist").await;

        // It returns: running again, and said.
        let _assist = FakePad::new("Tray Assist", 0x1209, 0x8702).unwrap();
        rig.shows("quick (running) >").await;
        rig.told("quick is running again").await;

        // Stopped from the menu; the profile stays, to start again.
        rig.click(Command::Stop("quick".into())).await;
        let menu = rig.shows("quick >\n  Start\n").await;
        assert!(menu.contains("Start as `quick`"), "{menu}");

        // What can't be done is said, since a menu has nowhere to say it.
        rig.click(Command::Stop("quick".into())).await;
        let refused = rig.told("Couldn't stop quick").await;
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
            setup: "quick".into(),
            node: "mux".into(),
            choice: "assist".into(),
        })
        .await;
        rig.told("Couldn't hand quick over to assist").await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn the_tray_outlives_the_daemon_and_remembers_the_quick_choices() {
        const TEST: &str = "the_tray_outlives_the_daemon_and_remembers_the_quick_choices";
        if !can_make_pads(TEST) {
            return;
        }
        let _primary = FakePad::new("Tray Kept Primary", 0x1209, 0x8711).unwrap();
        let _assist = FakePad::new("Tray Kept Assist", 0x1209, 0x8712).unwrap();
        let mut rig = rig().await;
        rig.shows("Pads (").await;
        rig.click(Command::Quick(Edit::set(
            "primary",
            &id("Tray Kept Primary", 0x8711),
        )))
        .await;
        rig.click(Command::Quick(Edit::set(
            "assist",
            &id("Tray Kept Assist", 0x8712),
        )))
        .await;
        rig.click(Command::Quick(Edit::set("rumble", "primary")))
            .await;
        rig.click(Command::Quick(Edit::set("managed", "true")))
            .await;
        rig.click(Command::QuickStart).await;
        rig.shows("quick (running) >").await;

        // The daemon stops: the menu says so, and a notification says
        // what went with it.
        rig.daemon.take().unwrap().stop().await;
        let menu = rig.shows("The CtrlAssist daemon isn't running").await;
        assert!(!menu.contains("Quick start"), "{menu}");
        let stopped = rig.told("daemon stopped").await;
        assert!(
            stopped.body.contains("virtual pads are gone"),
            "{stopped:?}"
        );

        // It comes back, and the profile, which asks to, starts by itself.
        rig.daemon = Some(start_daemon(&rig.paths).await);
        rig.told("daemon is back").await;
        rig.shows("quick (running) >").await;

        // A tray that starts later reads its choices from the kept profile.
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
        while !later.lock().await.quick_settled {
            assert!(
                tokio::time::Instant::now() < deadline,
                "it never read the daemon"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let menu = outline(&later.lock().await.entries());
        assert!(menu.contains("Primary: Tray Kept Primary >"), "{menu}");
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
