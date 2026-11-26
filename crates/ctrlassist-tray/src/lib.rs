//! CtrlAssist's system tray. The menu is rendered from the SDK client's
//! mirror of the daemon, every click is an rpc, and what changes in the
//! mirror that a person should know of becomes a notification.

mod desktop;
mod lanes;
pub mod menu;
pub mod notify;
pub mod service;
mod settings;
pub mod state;
pub mod tip;
pub mod tray;

use std::ops::ControlFlow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ctrlassist::{DaemonClient, StateEvent, StateWatcher, StateWatcherConfig};
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

/// What the client mirrors right now, in the daemon's `lifetime` as the
/// loop that follows it counts them.
fn snapshot(daemon: &DaemonClient, lifetime: u64) -> View {
    View {
        alive: daemon.is_available(),
        hydrated: daemon.is_hydrated(),
        lifetime,
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
    // The daemon's lifetimes, counted by its coming: one that goes and
    // comes back between two looks is another all the same. A watcher
    // that falls more than its queue behind loses the events, and with
    // them a restart in that gap.
    let mut lifetime = 0;
    // A preset's fields, asked once of each daemon that comes.
    let mut fields = Vec::new();
    loop {
        if !daemon.is_available() {
            fields.clear();
        } else if fields.is_empty() {
            match daemon.preset_form().await {
                Ok(served) => fields = served,
                Err(error) => warn!(%error, "the daemon didn't say what a preset's fields are"),
            }
        }
        // Read under the tray's lock, as the answers to its picks read
        // it too: the views it takes are in the order they were read.
        let (news, alive) = handle
            .update(|tray| {
                let mut view = snapshot(&daemon, lifetime);
                view.fields.clone_from(&fields);
                let alive = view.alive;
                let mut news = view.news_for(&tray.view, &mut tray.asked);
                news.extend(state::daemon_news(
                    was_alive,
                    alive,
                    !tray.view.setups.is_empty(),
                ));
                tray.see(view);
                (news, alive)
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
        match watcher.next().await {
            None => return,
            Some(StateEvent::DaemonAppeared) => lifetime += 1,
            Some(_) => {}
        }
    }
}

/// Brings the tray's view up to what the client mirrors now, and says
/// what changed: as the loop that follows the daemon does, for a daemon
/// that is there and whose state has arrived. That loop says the rest.
fn catch_up(tray: &mut PadTray, daemon: &DaemonClient) -> Vec<Toast> {
    let mut view = snapshot(daemon, tray.view.lifetime);
    if !(view.alive && view.hydrated) {
        return Vec::new();
    }
    view.fields.clone_from(&tray.view.fields);
    let news = view.news_for(&tray.view, &mut tray.asked);
    tray.see(view);
    news
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

/// Does what the menu's clicks ask, as rpcs: each profile's and each
/// pad's in turn, and beside the others'. One that fails is said as a
/// notification: a menu has nowhere else to say it.
pub(crate) async fn act(
    daemon: DaemonClient,
    clicked: mpsc::UnboundedReceiver<Action>,
    notifier: Notifier,
    quit: Arc<Notify>,
    config: Option<std::path::PathBuf>,
    handle: Handle,
) {
    let opening = Arc::new(AtomicBool::new(false));
    let now = {
        let notifier = notifier.clone();
        move |action: Action| {
            info!(action = action.named(), "an action from the menu");
            match action {
                Action::Quit => {
                    quit.notify_one();
                    return ControlFlow::Break(());
                }
                // A window that is starting takes a while to answer, and
                // the menu's other clicks don't wait for it. One opening
                // at a time: clicks while one is under way are that one's.
                Action::Settings if !opening.swap(true, Ordering::AcqRel) => {
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
                        opening.store(false, Ordering::Release);
                    });
                }
                _ => {}
            }
            ControlFlow::Continue(())
        }
    };
    let job = move |action: Action| {
        let (daemon, notifier, config, handle) = (
            daemon.clone(),
            notifier.clone(),
            config.clone(),
            handle.clone(),
        );
        async move {
            // Said as it is taken up, so that a click that led nowhere
            // can be told from one that was never made. An aim isn't a
            // click, and comes with every notch of the wheel.
            if !matches!(action, Action::Aim(_)) {
                info!(action = action.named(), "an action from the menu");
            }
            if let Some((what, Err(error))) =
                perform(&daemon, &notifier, config, &handle, action).await
            {
                warn!(%error, what, "an action from the menu failed");
                notifier.notify(refused(what, &error));
            }
        }
    };
    lanes::dispatch(clicked, now, job).await;
}

/// Does one action from a lane, and says what came of it that is news.
/// What it asked and how that went, for one whose failure is said.
async fn perform(
    daemon: &DaemonClient,
    notifier: &Notifier,
    config: Option<std::path::PathBuf>,
    handle: &Handle,
    action: Action,
) -> Option<(String, ctrlassist::Result<()>)> {
    Some(match action {
        // Kept, and nothing said of it.
        Action::Aim(name) => {
            if let Some(config) = &config {
                notify::save_target(config, name).await;
            }
            return None;
        }
        Action::Start(name) => {
            let outcome = daemon.start_setup(&name).await;
            answered_mark(handle, daemon, notifier, &name, state::Mark::Start).await;
            (format!("Couldn't start {name}"), outcome)
        }
        Action::Stop(name) => {
            let outcome = daemon.stop_setup(&name).await;
            answered_mark(handle, daemon, notifier, &name, state::Mark::Stop).await;
            (format!("Couldn't stop {name}"), outcome)
        }
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
            // The daemon the change is asked of: one that comes after
            // has nothing of it to answer.
            let asked_in = handle.update(|tray| tray.view.lifetime).await;
            let outcome = put_and_apply(daemon, &name, &toml).await;
            let restarted = match outcome {
                Ok(Applied::Restarted(start)) => Some(start),
                _ => None,
            };
            let refused = matches!(outcome, Err(Unapplied::Refused(_)));
            // The answer is taken with the view caught up with the
            // daemon as it gave it: a restart is said once, and what
            // was seen meanwhile that wasn't the restart is said too.
            let (news, running) = handle
                .update(|tray| {
                    // The tray showed the pick as made. Refused, it
                    // shows what is kept again.
                    if refused {
                        tray.unpick(&name, daemon.profile(&name).ok().flatten());
                    }
                    let mut news = catch_up(tray, daemon);
                    if asked_in == Some(tray.view.lifetime) {
                        news.extend(state::answered(
                            &mut tray.asked,
                            &tray.view,
                            &name,
                            restarted,
                        ));
                    }
                    (news, tray.view.setup(&name).is_some())
                })
                .await
                .unwrap_or_default();
            for toast in news {
                notifier.notify(toast);
            }
            match outcome {
                Ok(_) => return None,
                Err(Unapplied::Refused(error)) => (format!("Couldn't change {name}"), Err(error)),
                Err(Unapplied::Untaken(error)) => {
                    warn!(%error, setup = name, "a change was kept but not taken");
                    notifier.notify(state::untaken(&name, &error, running));
                    return None;
                }
            }
        }
        // Taken up at once, never on a lane.
        Action::Settings | Action::Quit => return None,
    })
}

/// What came of a pick that was kept.
#[derive(Debug)]
enum Applied {
    /// Kept: a setup that runs it took it as it ran, or none runs it.
    Kept,
    /// Kept, and the setup that runs it restarted with it, as the start
    /// numbered so (0 from a daemon too old to say).
    Restarted(u32),
}

/// Why a pick didn't come to all it asked.
#[derive(Debug)]
enum Unapplied {
    /// It wasn't kept: nothing changed.
    Refused(ctrlassist::Error),
    /// It was kept, but the setup that runs it didn't take it.
    Untaken(ctrlassist::Error),
}

/// Keeps `toml` as the profile `name`, and has a setup that runs it
/// take the change: as it runs where it can, and by a restart where it
/// can't. One that doesn't run is only kept.
async fn put_and_apply(
    daemon: &DaemonClient,
    name: &str,
    toml: &str,
) -> Result<Applied, Unapplied> {
    daemon
        .put_profile(name, toml)
        .await
        .map_err(Unapplied::Refused)?;
    let taken = async {
        if daemon.setup(name)?.is_some() {
            return daemon.update_setup(name, true).await.map(Some);
        }
        Ok(None)
    };
    match taken.await {
        Ok(Some(updated)) if updated.disruptive => Ok(Applied::Restarted(updated.start)),
        Ok(_) => Ok(Applied::Kept),
        Err(error) => Err(Unapplied::Untaken(error)),
    }
}

/// Takes the answer to a start or a stop of `name` asked from here,
/// whatever it was: the view caught up with the daemon as it answered,
/// and what that says, said; the start or the stop no longer marked as
/// asked, as what was to be seen of it has been or never will be.
async fn answered_mark(
    handle: &Handle,
    daemon: &DaemonClient,
    notifier: &Notifier,
    name: &str,
    mark: state::Mark,
) {
    let news = handle
        .update(|tray| {
            let news = catch_up(tray, daemon);
            state::unmark(&mut tray.asked, name, mark);
            news
        })
        .await
        .unwrap_or_default();
    for toast in news {
        notifier.notify(toast);
    }
}

#[cfg(test)]
mod tests {
    //! The tray's loop against a live daemon and fake pads, with no
    //! desktop: the tray's state is held here, and its notifications are
    //! caught instead of shown.

    use std::path::Path;

    use ctrlassist_daemon::{Daemon, Paths};
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
        let (input, reports) = Daemon::input_thread().unwrap();
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

        /// The summaries of the notifications up to the one that has
        /// `wanted` in it.
        async fn heard_until(&mut self, wanted: &str) -> Vec<String> {
            let mut heard = Vec::new();
            tokio::time::timeout(Duration::from_secs(20), async {
                loop {
                    let toast = self.toasts.recv().await.expect("the notifier is open");
                    let found = toast.summary.contains(wanted);
                    heard.push(toast.summary);
                    if found {
                        return;
                    }
                }
            })
            .await
            .unwrap_or_else(|_| unreachable!("never told {wanted:?}"));
            heard
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
        // The pad the other part has can be picked: the two change
        // places in one pick, as the setup runs, and back.
        let menu = rig.shows("Mode: Toggle >").await;
        assert!(
            menu.contains("    (o) Tray Primary\n    ( ) Tray Assist (swap with Assist)\n"),
            "{menu}"
        );
        rig.click(pick("primary", &id("Tray Assist", 0x8702))).await;
        let menu = rig.shows("Primary: Tray Assist >").await;
        assert!(menu.contains("Assist: Tray Primary >"), "{menu}");
        rig.click(pick("assist", &id("Tray Assist", 0x8702))).await;
        let menu = rig.shows("Primary: Tray Primary >").await;
        assert!(menu.contains("Assist: Tray Assist >"), "{menu}");
        assert_eq!(made(&rig).await, before);
        // What the daemon refuses is said, and the menu goes back to
        // what is kept.
        rig.click(pick("identity", "nobody")).await;
        rig.told("Couldn't change default").await;
        rig.shows("Identity: Generic >").await;
        assert_eq!(made(&rig).await, before);
        // Another identity is another virtual pad: picking it restarts
        // the setup at once, with nothing more to click.
        rig.click(pick("identity", "xbox")).await;
        rig.shows("Identity: Xbox >").await;
        eventually_pad(&rig, "Microsoft Xbox Series S|X Controller (in default)").await;
        rig.shows("default ✓ (Running) >").await;
        // Said once, as a restart: not as a stop and a start, and
        // nothing was said of the start and the picks before it either.
        let heard = rig.heard_until("default restarted").await;
        assert!(
            !heard
                .iter()
                .any(|said| said.ends_with(" started") || said.ends_with(" stopped")),
            "{heard:?}"
        );
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
        rig.shows("default ✓ (Held) >").await;
        // The pad's own leaving is published apart from its setup's
        // being held, and may be seen a moment after.
        rig.shows("Assist: Tray Assist (disconnected) >").await;
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
        assert!(menu.contains("Managed: On >"), "{menu}");
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
