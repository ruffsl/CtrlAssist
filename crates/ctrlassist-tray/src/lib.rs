//! CtrlAssist's system tray. The menu is rendered from the SDK client's
//! mirror of the daemon, every click is an rpc, and what changes in the
//! mirror that a person should know of becomes a notification.

mod desktop;
pub mod menu;
pub mod notify;
pub mod quick;
pub mod service;
pub mod state;
pub mod tray;

use std::sync::Arc;
use std::time::Duration;

use ctrlassist::{DaemonClient, StateWatcher, StateWatcherConfig};
use tokio::sync::{Notify, mpsc};
use tracing::warn;

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
    }
}

/// Keeps the tray showing what the client mirrors, rebuilt on each of
/// the client's events, and says what changed that is worth saying.
pub(crate) async fn follow(daemon: DaemonClient, handle: Handle, notifier: Notifier) {
    let mut watcher = StateWatcher::new(&daemon, StateWatcherConfig::default());
    // Whether the daemon was there at the last look. Not known at the
    // first, so a tray started without a daemon has nothing to say of it.
    let mut was_alive: Option<bool> = None;
    let mut started = false;
    loop {
        let view = snapshot(&daemon);
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
                // `quick` profile was written from.
                if view.hydrated && !tray.quick_settled {
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
    while let Some(action) = clicked.recv().await {
        let (what, outcome) = match action {
            Action::Quit => {
                quit.notify_one();
                return;
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
            Action::QuickApply(toml) => (
                format!("Couldn't change `{}`", quick::NAME),
                quick_apply(&daemon, &toml).await,
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

/// Keeps `toml` as the profile `quick` and has the running `quick` take
/// it, if that needs no restart: a restart waits for a click.
async fn quick_apply(daemon: &DaemonClient, toml: &str) -> ctrlassist::Result<()> {
    daemon.put_profile(quick::NAME, toml).await?;
    if !daemon.update_setup(quick::NAME, false).await? {
        daemon.update_setup(quick::NAME, true).await?;
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
        let dir = tempfile::Builder::new()
            .prefix("ca")
            .tempdir_in("/tmp")
            .unwrap();
        let at = |name: &str| dir.path().join(name);
        let paths = Paths {
            socket: at("zenoh.sock"),
            state: at("state"),
            config: at("config"),
            hider: at("hider.sock"),
        };
        let daemon = start_daemon(&paths).await;
        let client = DaemonClient::connect(&paths.socket).await.unwrap();
        let (actions, clicked) = mpsc::unbounded_channel();
        let (notifier, toasts) = Notifier::capturing();
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
        rig.click(Command::Quick(Edit::Primary(id("Tray Primary", 0x8701))))
            .await;
        rig.click(Command::Quick(Edit::Assist(id("Tray Assist", 0x8702))))
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
        rig.click(Command::Quick(Edit::Mode(
            ctrlassist_protocol::profile::MuxMode::Toggle,
        )))
        .await;
        let menu = rig.shows("Has control >").await;
        assert!(menu.contains("- Running as `quick`"), "{menu}");
        assert_eq!(node(&*rig.tray.lock().await), before);
        // So is another pad: the two change parts, and back.
        rig.click(Command::Quick(Edit::Primary(id("Tray Assist", 0x8702))))
            .await;
        assert!(
            rig.tray.lock().await.quick.profile().is_none(),
            "one pad as both parts is no profile, and nothing is asked"
        );
        rig.click(Command::Quick(Edit::Assist(id("Tray Primary", 0x8701))))
            .await;
        rig.shows("primary: Tray Assist").await;
        rig.click(Command::Quick(Edit::Assist(id("Tray Assist", 0x8702))))
            .await;
        rig.click(Command::Quick(Edit::Primary(id("Tray Primary", 0x8701))))
            .await;
        rig.shows("primary: Tray Primary").await;
        assert_eq!(node(&*rig.tray.lock().await), before);
        // Another identity is another virtual pad, which waits.
        let identity = |identity| Command::Quick(Edit::Identity(identity));
        rig.click(identity(ctrlassist_protocol::profile::Identity::Xbox))
            .await;
        let menu = outline(&rig.tray.lock().await.entries());
        assert!(menu.contains("\n  Restart as `quick`"), "{menu}");
        rig.click(identity(ctrlassist_protocol::profile::Identity::Generic))
            .await;

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
        rig.click(Command::Quick(Edit::Primary(id(
            "Tray Kept Primary",
            0x8711,
        ))))
        .await;
        rig.click(Command::Quick(Edit::Assist(id("Tray Kept Assist", 0x8712))))
            .await;
        rig.click(Command::Quick(Edit::Rumble(quick::Rumble::Primary)))
            .await;
        rig.click(Command::Quick(Edit::AutoStart(true))).await;
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
        let (notifier, _toasts) = Notifier::capturing();
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
        assert!(menu.contains("Rumble goes to: primary only >"), "{menu}");
        assert!(menu.contains("[x] Start by itself"), "{menu}");
    }
}
