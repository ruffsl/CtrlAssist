//! The ksni tray: it renders the menu model and funnels every click into
//! an [`Action`] over a channel. The async side owns the rpcs. Strings
//! are English literals: the tray isn't translated yet.

use ctrlassist::Phase;
use ctrlassist::form;
use ctrlassist_icons::Variant;
use ksni::menu::{MenuItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{ToolTip, Tray};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tracing::info;

use crate::badge::{Badge, Icons};
use crate::menu::{self, Command, Entry};

use crate::state::{Asked, View};
use crate::tip;

/// What a click asks of the daemon, or of the tray itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    /// Run a kept profile.
    Start(String),
    /// Stop a setup.
    Stop(String),
    /// Have a setup's node select a choice.
    Select {
        /// The setup.
        setup: String,
        /// The node.
        node: String,
        /// The choice.
        choice: String,
    },
    /// Buzz a pad.
    Identify(String),
    /// Stop remembering a pad.
    Forget(String),
    /// Keep this TOML as the profile `name`. A setup that runs it takes
    /// the change at once: all but another identity as it runs, and that
    /// one by a restart, which a game sees as its pad going and another
    /// coming.
    Put {
        /// The profile.
        name: String,
        /// Its new text.
        toml: String,
    },

    /// Open the settings window, or raise it if it is open.
    Settings,
    /// Keep this profile as the one last used, for the next start.
    Aim(String),
    /// End the tray.
    Quit,
}

impl Action {
    /// The action in a few words, for the log.
    #[must_use]
    pub fn named(&self) -> String {
        match self {
            Self::Start(name) => format!("start {name}"),
            Self::Stop(name) => format!("stop {name}"),
            Self::Select {
                setup,
                node,
                choice,
            } => format!("select {choice} at {node} of {setup}"),
            Self::Identify(pad) => format!("identify {pad}"),
            Self::Forget(pad) => format!("forget {pad}"),
            Self::Put { name, .. } => format!("keep {name} as picked"),
            Self::Settings => "open the settings window".to_string(),
            Self::Aim(name) => format!("aim at {name}"),
            Self::Quit => "quit".to_string(),
        }
    }
}

/// The tray's state: what it shows, and where its clicks go.
#[derive(Debug)]
pub struct PadTray {
    /// What the daemon publishes, as last seen.
    pub view: View,

    /// The icon with each state drawn into it, where they were made.
    icons: Icons,
    /// Whether a view has been taken yet: until one has, nothing is
    /// known of the daemon, and nothing is said of it.
    heard: bool,

    /// The profile last started, stopped or scrolled to: what a middle
    /// click on the icon acts on, while it can be.
    pub target: Option<String>,
    /// The profiles picked at whose picks the daemon hasn't all
    /// published yet.
    picked: BTreeMap<String, Picks>,
    /// What was asked of each setup from here and hasn't been seen
    /// done yet: it isn't news to the person who asked.
    pub asked: BTreeMap<String, Asked>,
    /// How far the wheel has turned toward its next notch, in the
    /// wheel's units, signed by direction.
    wheel: i32,
    /// When the wheel last turned.
    turned: Option<Instant>,
    actions: mpsc::UnboundedSender<Action>,
}

/// The picks made at a profile here that the daemon hasn't all published.
#[derive(Debug)]
struct Picks {
    /// The profile's text as it was kept before them.
    was: String,
    /// The text each of them sent, oldest first: the last is the one
    /// shown.
    sent: Vec<String>,
}

/// How long the wheel is still before what it had turned toward its
/// next notch is forgotten. A notch comes as a burst of scrolls that
/// add up to more than [`NOTCH`] by an amount that varies (168 to 204
/// was measured on one wheel, its scrolls up to 0.2 s apart), so what
/// is left over after the step belongs to the notch that made it, not
/// to the next. Only the time between scrolls tells two notches apart:
/// the maintainer's choice, for there being no event that does.
const WHEEL_PAUSE: Duration = Duration::from_millis(300);

/// One notch of a wheel, in the units a scroll is reported in. A wheel
/// with finer steps, or a touchpad, reports many small scrolls that add
/// up to this.
const NOTCH: i32 = 120;

impl PadTray {
    /// A tray whose clicks go to `actions`.
    #[must_use]
    pub fn new(actions: mpsc::UnboundedSender<Action>) -> Self {
        Self {
            view: View::default(),
            icons: Icons::default(),
            heard: false,

            target: None,
            picked: BTreeMap::new(),
            asked: BTreeMap::new(),
            wheel: 0,
            turned: None,
            actions,
        }
    }

    /// Takes a new view. A setup that came or went since the last is the
    /// profile last used, whoever started or stopped it.
    pub fn see(&mut self, mut view: View) {
        self.heard = true;
        // A profile picked at here is shown as picked until the daemon
        // publishes the last pick: a view read before it did would undo
        // the picks, and the next would be built on the profile without
        // them.
        self.picked.retain(|name, picks| {
            let Some(kept) = view.profiles.iter_mut().find(|kept| kept.name == *name) else {
                return false;
            };
            let Some(last) = picks.sent.last() else {
                return false;
            };
            // As the last pick left it, all has arrived. As it was, or
            // as an earlier pick left it, the last is still to come.
            // Anything else is someone else's change, which stands.
            if kept.toml == *last || (kept.toml != picks.was && !picks.sent.contains(&kept.toml)) {
                return false;
            }
            kept.toml.clone_from(last);
            kept.unset = ctrlassist_protocol::profile::Profile::unchosen(last);
            true
        });
        if self.view.hydrated && view.hydrated {
            let changed = view
                .setups
                .iter()
                .find(|setup| self.view.setup(&setup.name).is_none())
                .or_else(|| {
                    self.view
                        .setups
                        .iter()
                        .find(|setup| view.setup(&setup.name).is_none())
                });
            if let Some(name) = changed.map(|setup| setup.name.clone()) {
                self.aim(name);
            }
        }
        self.view = view;
    }

    /// Shows the profile `name` as the daemon keeps it, `kept`, after it
    /// refused a pick there. The picks at other profiles stay shown.
    pub fn unpick(&mut self, name: &str, kept: Option<ctrlassist::Profile>) {
        self.picked.remove(name);
        let profiles = &mut self.view.profiles;
        match (
            profiles.iter().position(|profile| profile.name == name),
            kept,
        ) {
            (Some(at), Some(kept)) => profiles[at] = kept,
            (Some(at), None) => {
                profiles.remove(at);
            }
            (None, Some(kept)) => {
                let at = profiles.partition_point(|profile| profile.name.as_str() < name);
                profiles.insert(at, kept);
            }
            (None, None) => {}
        }
    }

    /// Makes `name` the profile last used, and has it kept if it
    /// wasn't already.
    fn aim(&mut self, name: String) {
        if self.target.as_deref() != Some(name.as_str()) {
            self.target = Some(name.clone());
            let _ = self.actions.send(Action::Aim(name));
        }
    }

    /// Takes a turn of the wheel, reported at `now`, and moves the mark
    /// over the profiles a middle click could act on, around from the
    /// last to the first: one profile a notch. Scrolls smaller than a
    /// notch add up to one, so a finer wheel or a touchpad moves the
    /// mark as far as a plain wheel does. What was turned is dropped
    /// when the wheel turns the other way, and when it has been still
    /// for 0.3 s.
    pub fn turn(&mut self, delta: i32, now: Instant) {
        let paused = self
            .turned
            .is_some_and(|last| now.duration_since(last) >= WHEEL_PAUSE);
        self.turned = Some(now);
        if paused || self.wheel.signum() != delta.signum() {
            self.wheel = 0;
        }
        self.wheel = self.wheel.saturating_add(delta);
        let steps = self.wheel / NOTCH;
        self.wheel %= NOTCH;
        let candidates = tip::candidates(&self.view);
        if steps == 0 || candidates.is_empty() {
            return;
        }
        let count = i64::try_from(candidates.len()).unwrap_or(i64::MAX);
        let at = self
            .aimed()
            .and_then(|aimed| candidates.iter().position(|name| *name == aimed))
            .and_then(|at| i64::try_from(at).ok());
        // With no mark yet, the first notch puts it on the first
        // profile going down the list, or the last going up.
        let from = at.unwrap_or(if steps > 0 { -1 } else { count });
        let next = (from + i64::from(steps)).rem_euclid(count);
        if let Some(name) = usize::try_from(next).ok().and_then(|at| candidates.get(at)) {
            let name = (*name).to_string();
            self.aim(name);
        }
    }

    /// The profile a middle click acts on now.
    #[must_use]
    pub fn aimed(&self) -> Option<&str> {
        tip::target(&self.view, self.target.as_deref())
    }

    /// Does what a menu entry asks: a pick is worked into its profile
    /// here, and the async side is asked to keep it and to do anything
    /// else.
    pub fn activate(&mut self, command: Command) {
        let action = match command {
            Command::Set {
                profile,
                key,
                value,
            } => {
                // Only a profile in the preset form has fields to set.
                let Some((was, mut values)) = self
                    .view
                    .profiles
                    .iter()
                    .find(|kept| kept.name == profile)
                    .and_then(|kept| {
                        let values = form::from_toml(&self.view.fields, &kept.toml)?;
                        Some((kept.toml.clone(), values))
                    })
                else {
                    return;
                };
                menu::pick(&self.view, &profile, &mut values, &key, &value);
                let toml = form::to_toml(&self.view.fields, &values);
                let Some(kept) = self
                    .view
                    .profiles
                    .iter_mut()
                    .find(|kept| kept.name == profile)
                else {
                    return;
                };
                // Shown as picked until the daemon publishes it, so that
                // a second pick made at once builds on the first.
                self.picked
                    .entry(profile.clone())
                    .or_insert_with(|| Picks {
                        was,
                        sent: Vec::new(),
                    })
                    .sent
                    .push(toml.clone());
                kept.toml.clone_from(&toml);
                kept.unset = ctrlassist_protocol::profile::Profile::unchosen(&toml);
                // A setup that runs it, or comes to, may be restarted by
                // this.
                self.asked.entry(profile.clone()).or_default().changes += 1;
                Action::Put {
                    name: profile,
                    toml,
                }
            }
            Command::Start(name) => {
                self.asked.entry(name.clone()).or_default().start = true;
                Action::Start(name)
            }
            Command::Stop(name) => {
                self.asked.entry(name.clone()).or_default().stop = true;
                Action::Stop(name)
            }
            Command::Select {
                setup,
                node,
                choice,
            } => Action::Select {
                setup,
                node,
                choice,
            },
            Command::Identify(pad) => Action::Identify(pad),
            Command::Forget(pad) => Action::Forget(pad),
            Command::Settings => Action::Settings,
            Command::Quit => Action::Quit,
        };
        let _ = self.actions.send(action);
    }

    /// Composes the tray's icons into `dir` from the desktop theme's
    /// files: the pad with each state drawn into it. Without this, or
    /// for a state whose icon couldn't be made, the icon is the theme's
    /// pad alone.
    pub fn compose_icons(&mut self, dir: &std::path::Path) {
        self.icons = Icons::make(dir);
    }

    /// The menu as it stands, as plain data.
    #[must_use]
    pub fn entries(&self) -> Vec<Entry> {
        menu::menu(&self.view)
    }

    /// The theme's own icon for the tray: the colored pad glyph. The
    /// menu's rows are the symbolic ones.
    const fn glyph() -> (menu::Icon, Variant) {
        (menu::icon::PAD, Variant::Color)
    }

    /// Whether the daemon is known to be away, or another release: not
    /// before the first view is taken, when nothing is known of it yet.
    fn unavailable(&self) -> bool {
        self.heard && (!self.view.alive || self.view.mismatch.is_some())
    }

    /// What is drawn into the tray's icon for the worst of what the
    /// setups are in: nothing while none runs or one is still starting.
    /// A daemon that isn't there, or is another release, comes before
    /// any of it.
    fn badge(&self) -> Option<Badge> {
        if self.unavailable() {
            return Some(Badge::Unavailable);
        }
        match self.worst() {
            Some(Phase::Failed) => Some(Badge::Failed),
            Some(Phase::Held) => Some(Badge::Held),
            Some(Phase::Running) => Some(Badge::Running),
            Some(_) | None => None,
        }
    }

    /// The worst phase among the setups: failed, then held, then running.
    fn worst(&self) -> Option<Phase> {
        [Phase::Failed, Phase::Held, Phase::Starting, Phase::Running]
            .into_iter()
            .find(|phase| self.view.setups.iter().any(|setup| setup.phase == *phase))
    }
}

/// Resolves `names` to the one the desktop's icon theme has, so the
/// tray's host loads a glyph it ships and, for a symbolic one, colors it
/// as the panel's text. The first name where the theme can't be read.
/// As TisVlay's tray does.
fn themed(names: &[&str], prefer: Variant) -> String {
    ctrlassist_icons::resolve_name(names, prefer)
        .or_else(|| names.first().map(ToString::to_string))
        .unwrap_or_default()
}

/// An underscore in a label is the toolkit's mark for an access key;
/// doubled, it is shown as itself.
fn label(text: &str) -> String {
    text.replace('_', "__")
}

fn item(entry: Entry) -> MenuItem<PadTray> {
    match entry {
        Entry::Separator => MenuItem::Separator,
        Entry::Item {
            icon,
            label: text,
            command,
        } => StandardItem {
            label: label(&text),
            icon_name: themed(icon, Variant::Symbolic),
            enabled: command.is_some(),
            activate: Box::new(move |tray: &mut PadTray| {
                if let Some(command) = command.clone() {
                    tray.activate(command);
                }
            }),
            ..Default::default()
        }
        .into(),

        Entry::Radio { choices, selected } => {
            let commands: Vec<Command> = choices
                .iter()
                .map(|choice| choice.command.clone())
                .collect();
            RadioGroup {
                // Past the end: nothing is picked.
                selected: selected.unwrap_or(usize::MAX),
                select: Box::new(move |tray: &mut PadTray, index| {
                    if let Some(command) = commands.get(index) {
                        tray.activate(command.clone());
                    }
                }),
                options: choices
                    .into_iter()
                    .map(|choice| RadioItem {
                        label: label(&choice.label),
                        enabled: choice.enabled,
                        ..Default::default()
                    })
                    .collect(),
            }
            .into()
        }
        Entry::Sub {
            icon,
            label: text,
            entries,
        } => SubMenu {
            label: label(&text),
            icon_name: themed(icon, Variant::Symbolic),
            submenu: entries.into_iter().map(item).collect(),
            ..Default::default()
        }
        .into(),
    }
}

impl Tray for PadTray {
    // Left-click opens the menu, not just right-click.
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        "ctrlassist".into()
    }

    /// The desktop has no StatusNotifier host: vanilla GNOME with no
    /// AppIndicator extension, or a host that hasn't come up yet. The
    /// tray keeps running, for its notifications; only the icon is
    /// hidden until a host appears.
    fn watcher_offline(&self, reason: ksni::OfflineReason) -> bool {
        info!(
            ?reason,
            "no StatusNotifier host; the tray icon is hidden (on GNOME, install and enable gnome-shell-extension-appindicator), and notifications still work"
        );
        true
    }

    fn watcher_online(&self) {
        info!("a StatusNotifier host is there; the tray icon is registered");
    }

    /// A left click on the icon opens the settings window; the menu is
    /// the right click's.
    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.actions.send(Action::Settings);
    }

    /// A middle click starts or stops the marked profile.
    fn secondary_activate(&mut self, _x: i32, _y: i32) {
        let Some(name) = self.aimed().map(str::to_string) else {
            return;
        };
        let command = if self.view.setup(&name).is_some() {
            Command::Stop(name)
        } else {
            Command::Start(name)
        };
        self.activate(command);
    }

    /// The wheel moves the mark; see [`PadTray::turn`].
    fn scroll(&mut self, delta: i32, orientation: ksni::Orientation) {
        if orientation == ksni::Orientation::Vertical && delta != 0 {
            self.turn(delta, Instant::now());
        }
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::Hardware
    }

    fn title(&self) -> String {
        if !self.view.alive {
            "CtrlAssist [daemon not running]".into()
        } else if self.view.mismatch.is_some() {
            "CtrlAssist [version mismatch]".into()
        } else {
            match self.worst() {
                Some(phase) => format!("CtrlAssist [{}]", phase.as_str()),
                None => "CtrlAssist".into(),
            }
        }
    }

    /// The pad with the state drawn into it, or alone with nothing to
    /// say, where that icon was made, and the theme's pad otherwise.
    fn icon_name(&self) -> String {
        let made = match self.badge() {
            Some(badge) => self.icons.name(badge),
            None => self.icons.plain(),
        };
        if let Some(name) = made {
            return name.to_string();
        }
        let (name, variant) = Self::glyph();
        themed(name, variant)
    }

    fn icon_theme_path(&self) -> String {
        self.icons.dir().to_string_lossy().into_owned()
    }

    /// Active while something runs, and while the daemon is away or
    /// another release, so that a desktop that tucks passive items out
    /// of sight keeps the icon and its mark in view then. Never one that
    /// asks for attention, which a desktop may animate, as Plasma does
    /// by pulsing the icon: the icon itself says a setup is held or
    /// failed, and stays still.
    fn status(&self) -> ksni::Status {
        if self.unavailable() || self.worst().is_some() {
            ksni::Status::Active
        } else {
            ksni::Status::Passive
        }
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "CtrlAssist".into(),
            description: tip::description(&self.view, self.aimed()),
            icon_name: themed(menu::icon::PAD, Variant::Color),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        self.entries().into_iter().map(item).collect()
    }
}

#[cfg(test)]
mod tests {
    use ctrlassist::Hiding;

    use super::*;

    use crate::state::tests::view;

    fn tray(phase: Option<Phase>) -> (PadTray, mpsc::UnboundedReceiver<Action>) {
        let (actions, clicked) = mpsc::unbounded_channel();
        let mut tray = PadTray::new(actions);
        tray.view = view(phase, Hiding::Service);
        tray.heard = true;
        (tray, clicked)
    }

    /// The wheel moves the mark over what could be started or stopped,
    /// a middle click does that to the marked one, and a profile started
    /// or stopped from anywhere becomes the marked one.
    #[test]
    fn the_wheel_picks_the_profile_a_middle_click_starts_or_stops() {
        const ONE: &str = "mode = \"unicast\"\npad = \"model:2dc8:6003\"\n";
        let (mut tray, mut clicked) = tray(Some(Phase::Running));
        tray.view.profiles[1].toml = ONE.to_string();
        (tray.view.pads[1].vendor, tray.view.pads[1].product) = (0x2dc8, 0x6003);
        // What runs is aimed at, and a middle click stops it.
        assert_eq!(tray.aimed(), Some("couch"));
        tray.secondary_activate(0, 0);
        assert_eq!(clicked.try_recv().unwrap(), Action::Stop("couch".into()));
        // The wheel goes on to the one that could start, and around.
        tray.scroll(120, ksni::Orientation::Vertical);
        assert_eq!(tray.aimed(), Some("desk"));
        assert!(tray.tool_tip().description.ends_with("► desk: Ready"));
        // What is aimed at is kept for the next start, as it changes.
        assert_eq!(clicked.try_recv().unwrap(), Action::Aim("desk".into()));
        tray.secondary_activate(0, 0);
        assert_eq!(clicked.try_recv().unwrap(), Action::Start("desk".into()));
        tray.scroll(120, ksni::Orientation::Vertical);
        assert_eq!(tray.aimed(), Some("couch"));
        tray.scroll(-120, ksni::Orientation::Vertical);
        assert_eq!(tray.aimed(), Some("desk"));
        tray.scroll(120, ksni::Orientation::Horizontal);
        assert_eq!(tray.aimed(), Some("desk"));
        // A finer wheel's small scrolls add up to a notch before the
        // mark moves, and turning back drops what was turned.
        for _ in 0..7 {
            tray.scroll(15, ksni::Orientation::Vertical);
        }
        assert_eq!(tray.aimed(), Some("desk"));
        tray.scroll(-15, ksni::Orientation::Vertical);
        for _ in 0..7 {
            tray.scroll(15, ksni::Orientation::Vertical);
        }
        assert_eq!(tray.aimed(), Some("desk"));
        tray.scroll(15, ksni::Orientation::Vertical);
        assert_eq!(tray.aimed(), Some("couch"));
        // A flick of three notches moves three, around.
        tray.scroll(360, ksni::Orientation::Vertical);
        assert_eq!(tray.aimed(), Some("desk"));

        // `couch` stopped, from the menu or anywhere: it is the last
        // used. With nothing to act on, a click does nothing.
        let mut stopped = view(None, Hiding::Service);
        stopped.profiles[1].toml = ONE.to_string();
        (stopped.pads[1].vendor, stopped.pads[1].product) = (0x2dc8, 0x6003);
        tray.see(stopped);
        assert_eq!(tray.target.as_deref(), Some("couch"));
        let mut kept = Vec::new();
        while let Ok(action) = clicked.try_recv() {
            kept.push(action);
        }
        assert_eq!(kept.last(), Some(&Action::Aim("couch".into())), "{kept:?}");
        assert!(
            kept.iter().all(|action| matches!(action, Action::Aim(_))),
            "{kept:?}"
        );
        tray.view.profiles.clear();
        assert_eq!(tray.aimed(), None);
        tray.secondary_activate(0, 0);
        tray.scroll(120, ksni::Orientation::Vertical);
        assert!(clicked.try_recv().is_err());
    }

    /// A wheel as one was measured: each notch a burst of scrolls that
    /// add up to between 168 and 204, with a second or so between
    /// notches. Each notch moves the mark once, however many there are.
    #[test]
    fn a_notch_that_comes_as_a_burst_of_more_than_a_notch_moves_the_mark_once() {
        const NOTCHES: [&[i32]; 7] = [
            &[48, 84, 36, 24],
            &[24, 24, 36, 24, 36, 24],
            &[24, 36, 36, 48, 36],
            &[24, 36, 24, 48, 24, 24],
            &[24, 24, 24, 36, 24, 36],
            // One that ends with the wheel settling back.
            &[24, 36, 36, 60, 48, -24],
            &[24, 24, 36, 48, 36],
        ];
        let (mut tray, _clicked) = tray(None);
        // Seven profiles that could start, each a pad of its own.
        tray.view.pads.clear();
        tray.view.profiles.clear();
        for at in 0..7_u16 {
            let mut pad = tray
                .view
                .pads
                .first()
                .cloned()
                .unwrap_or_else(|| view(None, Hiding::Service).pads[1].clone());
            pad.id = format!("address:{at}");
            (pad.vendor, pad.product) = (0x1209, at);
            tray.view.pads.push(pad);
            let mut profile = view(None, Hiding::Service).profiles[0].clone();
            profile.name = format!("p{at}");
            profile.toml = format!("mode = \"unicast\"\npad = \"model:1209:{at:04x}\"\n");
            tray.view.profiles.push(profile);
        }
        assert_eq!(tip::candidates(&tray.view).len(), 7);
        let mut now = Instant::now();
        let mut reached = Vec::new();
        for notch in NOTCHES {
            now += Duration::from_secs(1);
            for delta in notch {
                now += Duration::from_millis(20);
                tray.turn(*delta, now);
            }
            reached.push(tray.aimed().unwrap_or_default().to_string());
        }
        assert_eq!(reached, ["p0", "p1", "p2", "p3", "p4", "p5", "p6"]);
        // Without a pause, the wheel spun: what is left over counts on.
        for delta in [120, 60, 60] {
            now += Duration::from_millis(20);
            tray.turn(delta, now);
        }
        assert_eq!(tray.aimed(), Some("p1"));
    }

    #[test]
    fn the_icon_title_and_tooltip_say_the_worst_of_what_runs() {
        let said = |phase| {
            let (tray, _clicked) = tray(phase);
            // The pad stays the icon whatever runs.
            let (name, variant) = PadTray::glyph();
            assert!(matches!(variant, Variant::Color));
            assert_eq!(name[0], "applications-games");
            (tray.title(), tray.tool_tip().description)
        };
        assert_eq!(
            said(None),
            (
                "CtrlAssist".to_string(),
                "Nothing is running\n3 of 3 pads connected".to_string()
            )
        );
        assert_eq!(said(Some(Phase::Running)).0, "CtrlAssist [running]");
        assert_eq!(said(Some(Phase::Held)).0, "CtrlAssist [held]");
        assert_eq!(
            said(Some(Phase::Failed)).1,
            "► couch: Failed · Toggle\n    a virtual pad refused a write"
        );
        // Nothing asks for attention, which a desktop may animate.
        for phase in [Phase::Starting, Phase::Running, Phase::Held, Phase::Failed] {
            let (tray, _clicked) = tray(Some(phase));
            assert!(matches!(tray.status(), ksni::Status::Active), "{phase:?}");
        }
        let (idle, _clicked) = tray(None);
        assert!(matches!(idle.status(), ksni::Status::Passive));
        // The state that is drawn into the icon.
        let badge = |phase| tray(phase).0.badge();
        assert_eq!(badge(None), None);
        assert_eq!(badge(Some(Phase::Starting)), None);
        assert_eq!(badge(Some(Phase::Running)), Some(Badge::Running));
        assert_eq!(badge(Some(Phase::Held)), Some(Badge::Held));
        assert_eq!(badge(Some(Phase::Failed)), Some(Badge::Failed));
        // Until its icons are made, the tray names the theme's pad and
        // no path of its own.
        let (mut held, _clicked) = tray(Some(Phase::Held));
        assert_eq!(held.icon_theme_path(), "");
        let plain = held.icon_name();
        assert!(!plain.starts_with("ctrlassist-"));
        // Made, it names the pad with the state in it wherever the
        // theme had what that takes, and the pad alone for no state.
        let dir = tempfile::tempdir().unwrap();
        held.compose_icons(dir.path());
        if dir.path().join("ctrlassist-held.svg").exists() {
            assert_eq!(held.icon_name(), "ctrlassist-held");
            assert_eq!(held.icon_theme_path(), dir.path().to_string_lossy());
        } else {
            assert_eq!(held.icon_name(), plain);
        }
        // With nothing running it names the pad alone, made from the
        // same file as the others, so the pad looks the same in every
        // state.
        let (mut idle, _clicked) = tray(None);
        idle.compose_icons(dir.path());
        if dir.path().join("ctrlassist-pad.svg").exists() {
            assert_eq!(idle.icon_name(), "ctrlassist-pad");
            assert_eq!(idle.icon_theme_path(), dir.path().to_string_lossy());
        } else {
            assert_eq!(idle.icon_name(), plain);
        }

        let (mut down, _clicked) = tray(Some(Phase::Running));
        down.view.alive = false;
        assert_eq!(down.title(), "CtrlAssist [daemon not running]");
        assert_eq!(down.tool_tip().description, "The daemon isn't running");
        assert_eq!(down.badge(), Some(Badge::Unavailable));
        let (mut other, _clicked) = tray(None);
        other.view.mismatch = Some("9.0.0".to_string());
        assert_eq!(other.title(), "CtrlAssist [version mismatch]");
        assert!(other.tool_tip().description.starts_with("Version mismatch"));
        assert_eq!(other.badge(), Some(Badge::Unavailable));
        // Either is in view with its mark though nothing runs, where a
        // desktop keeps passive items out of sight.
        assert!(matches!(other.status(), ksni::Status::Active));
        let (mut gone, _clicked) = tray(None);
        gone.view.alive = false;
        assert_eq!(gone.badge(), Some(Badge::Unavailable));
        assert!(matches!(gone.status(), ksni::Status::Active));
        // Before the first view is taken nothing is known of the daemon,
        // and nothing is said of it: no mark comes and goes as the tray
        // starts.
        let (actions, _clicked) = mpsc::unbounded_channel();
        let mut fresh = PadTray::new(actions);
        assert_eq!(fresh.badge(), None);
        assert!(matches!(fresh.status(), ksni::Status::Passive));
        assert!(!fresh.icon_name().starts_with("ctrlassist-"));
        fresh.see(View::default());
        assert_eq!(fresh.badge(), Some(Badge::Unavailable));
        assert!(matches!(fresh.status(), ksni::Status::Active));
        assert_eq!(other.id(), "ctrlassist");
    }

    #[test]
    fn every_entry_of_the_menu_becomes_an_item_of_the_tray_s() {
        let (tray, _clicked) = tray(Some(Phase::Held));
        assert_eq!(tray.menu().len(), tray.entries().len());
        assert_eq!(label("a_b"), "a__b");
    }

    #[test]
    fn a_click_is_an_action_and_a_pick_is_worked_into_its_profile() {
        let (mut tray, mut clicked) = tray(None);
        // A pick asks for the profile to be kept with it, and the next
        // builds on it without waiting for the daemon to say so.
        tray.activate(Command::set("couch", "identity", "xbox"));
        tray.activate(Command::set("couch", "assist", "address:8bitdo"));
        let put = |action| match action {
            Ok(Action::Put { name, toml }) => (name, toml),
            other => unreachable!("a pick is a put, not {other:?}"),
        };
        let (name, first) = put(clicked.try_recv());
        assert_eq!(name, "couch");
        assert!(first.contains("identity = \"xbox\""), "{first}");
        assert!(first.contains("assist = \"serial:xbox\""), "{first}");
        let (_, second) = put(clicked.try_recv());
        assert!(second.contains("identity = \"xbox\""), "{second}");
        assert!(second.contains("assist = \"address:8bitdo\""), "{second}");
        assert_eq!(tray.view.profiles[0].toml, second);
        // A view read before the daemon published the pick doesn't undo
        // it; one that has it settles it; and a change from elsewhere
        // made meanwhile stands.
        let kept = view(None, Hiding::Service);
        tray.see(kept.clone());
        assert_eq!(tray.view.profiles[0].toml, second);
        let mut arrived = kept;
        arrived.profiles[0].toml.clone_from(&second);
        tray.see(arrived.clone());
        assert!(tray.picked.is_empty());
        tray.activate(Command::set("couch", "rumble", "assist"));
        let _ = clicked.try_recv();
        let mut elsewhere = arrived.clone();
        elsewhere.profiles[0].toml = crate::state::tests::COUCH.replace("toggle", "average");
        tray.see(elsewhere.clone());
        assert_eq!(tray.view.profiles[0].toml, elsewhere.profiles[0].toml);
        assert!(tray.picked.is_empty());
        // One the daemon refused is put back as kept.
        tray.activate(Command::set("couch", "rumble", "assist"));
        let _ = clicked.try_recv();
        tray.unpick("couch", Some(elsewhere.profiles[0].clone()));
        tray.see(elsewhere.clone());
        assert_eq!(tray.view.profiles[0].toml, elsewhere.profiles[0].toml);
        tray.see(arrived);
        // The pad the other part has: the two change places.
        tray.activate(Command::set("couch", "primary", "address:8bitdo"));
        let (_, swapped) = put(clicked.try_recv());
        assert!(
            swapped.contains("primary = \"address:8bitdo\""),
            "{swapped}"
        );
        assert!(
            swapped.contains("assist = \"address:dualsense\""),
            "{swapped}"
        );
        // Another mode keeps what it has a use for, and drops the rest.
        tray.activate(Command::set("couch", "mode", "unicast"));
        let (_, other) = put(clicked.try_recv());
        assert_eq!(other, "mode = \"unicast\"\nidentity = \"xbox\"\n");
        // A profile there is none of, or that no form can show, has
        // nothing to pick.
        tray.activate(Command::set("nobody", "identity", "xbox"));
        tray.view.profiles[1].toml = "[source.a]\npad = \"model:0001:0001\"\n".to_string();
        tray.activate(Command::set("desk", "identity", "xbox"));
        assert!(clicked.try_recv().is_err());

        tray.activate(Command::Start("couch".into()));
        assert_eq!(clicked.try_recv().unwrap(), Action::Start("couch".into()));
        let put = Action::Put {
            name: "couch".into(),
            toml: String::new(),
        };
        assert_eq!(put.named(), "keep couch as picked");
        let select = Action::Select {
            setup: "couch".into(),
            node: "mux".into(),
            choice: "assist".into(),
        };
        assert_eq!(select.named(), "select assist at mux of couch");
        let named: Vec<String> = [
            Action::Start("a".into()),
            Action::Stop("a".into()),
            Action::Identify("p".into()),
            Action::Forget("p".into()),
            Action::Quit,
        ]
        .iter()
        .map(Action::named)
        .collect();
        assert_eq!(
            named,
            ["start a", "stop a", "identify p", "forget p", "quit"]
        );

        for (command, action) in [
            (Command::Start("desk".into()), Action::Start("desk".into())),
            (Command::Identify("p".into()), Action::Identify("p".into())),
            (Command::Forget("p".into()), Action::Forget("p".into())),
            (Command::Quit, Action::Quit),
        ] {
            tray.activate(command);
            assert_eq!(clicked.try_recv().unwrap(), action);
        }
    }

    /// One notch moves the mark to the next line of the tooltip.
    #[test]
    fn a_notch_moves_the_mark_to_the_tooltip_s_next_line() {
        let (mut tray, _clicked) = tray(Some(Phase::Running));
        tray.view.profiles[1].toml = "mode = \"unicast\"\npad = \"model:2dc8:6003\"\n".to_string();
        (tray.view.pads[1].vendor, tray.view.pads[1].product) = (0x2dc8, 0x6003);
        let mut alpha = tray.view.profiles[1].clone();
        alpha.name = "alpha".to_string();
        tray.view.profiles.insert(0, alpha);
        assert_eq!(tray.aimed(), Some("couch"));
        tray.scroll(120, ksni::Orientation::Vertical);
        assert_eq!(tray.aimed(), Some("alpha"));
        assert!(tray.tool_tip().description.contains("\n► alpha: Ready\n"));
    }

    /// The text a pick asked to keep.
    fn sent(action: Result<Action, mpsc::error::TryRecvError>) -> String {
        match action {
            Ok(Action::Put { toml, .. }) => toml,
            other => unreachable!("a pick is a put, not {other:?}"),
        }
    }

    /// Picks made one after another are shown until the daemon has
    /// published the last: one it publishes on the way, as it takes them
    /// in turn, neither undoes the later ones nor is built on in their
    /// place.
    #[test]
    fn picks_in_a_row_are_shown_until_the_last_is_published() {
        let (actions, mut clicked) = mpsc::unbounded_channel();
        let mut tray = PadTray::new(actions);
        let kept = view(Some(Phase::Running), Hiding::Service);
        tray.see(kept.clone());
        tray.activate(Command::set("couch", "identity", "xbox"));
        let second = sent(clicked.try_recv());
        tray.activate(Command::set("couch", "rumble", "assist"));
        let third = sent(clicked.try_recv());
        // The first published, the second still to come.
        let mut published = kept;
        published.profiles[0].toml.clone_from(&second);
        tray.see(published.clone());
        assert_eq!(tray.view.profiles[0].toml, third);
        // A pick now builds on all of them.
        tray.activate(Command::set("couch", "managed", "true"));
        let fourth = sent(clicked.try_recv());
        for kept in [
            "identity = \"xbox\"",
            "rumble = \"assist\"",
            "managed = true",
        ] {
            assert!(fourth.contains(kept), "{kept} in {fourth}");
        }
        // Published one by one, the last settles it.
        published.profiles[0].toml.clone_from(&third);
        tray.see(published.clone());
        assert_eq!(tray.view.profiles[0].toml, fourth);
        published.profiles[0].toml.clone_from(&fourth);
        tray.see(published.clone());
        assert!(tray.picked.is_empty());
        // A text the tray never sent is someone else's, and stands.
        tray.activate(Command::set("couch", "rumble", "none"));
        let _ = clicked.try_recv();
        let mut elsewhere = published;
        elsewhere.profiles[0].toml = crate::state::tests::COUCH.to_string();
        tray.see(elsewhere.clone());
        assert_eq!(tray.view.profiles[0].toml, elsewhere.profiles[0].toml);
        assert!(tray.picked.is_empty());
    }

    /// A pick the daemon refused puts its own profile back as kept, and
    /// leaves the picks at other profiles shown.
    #[test]
    fn a_refused_pick_leaves_the_other_profiles_picks() {
        let (actions, mut clicked) = mpsc::unbounded_channel();
        let mut tray = PadTray::new(actions);
        let kept = view(None, Hiding::Service);
        tray.see(kept.clone());
        tray.activate(Command::set("couch", "identity", "xbox"));
        let _ = clicked.try_recv();
        tray.activate(Command::set("desk", "outputs", "3"));
        let desk = sent(clicked.try_recv());
        tray.unpick("couch", Some(kept.profiles[0].clone()));
        assert_eq!(tray.view.profiles[0].toml, kept.profiles[0].toml);
        assert_eq!(tray.view.profiles[1].toml, desk);
        // And holds over a view read before it was published.
        tray.see(kept);
        assert_eq!(tray.view.profiles[1].toml, desk);
        // One the daemon no longer keeps goes; one it has come to keep
        // is shown, in its place by name.
        tray.unpick("couch", None);
        assert_eq!(tray.view.profiles.len(), 1);
        tray.unpick("alpha", Some(crate::state::tests::profile("alpha", None)));
        let names: Vec<&str> = tray.view.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["alpha", "desk"]);
    }
}

#[cfg(test)]
mod toolkit_tests {
    use ctrlassist::{Hiding, Phase};

    use super::*;
    use crate::state::tests::view;

    /// Activates every item of a menu that can be, as clicks would.
    fn click_all(items: &[MenuItem<PadTray>], tray: &mut PadTray) {
        for item in items {
            match item {
                MenuItem::Standard(item) if item.enabled => (item.activate)(tray),

                MenuItem::RadioGroup(group) => {
                    for index in 0..=group.options.len() {
                        (group.select)(tray, index);
                    }
                }
                MenuItem::SubMenu(sub) => click_all(&sub.submenu, tray),
                _ => {}
            }
        }
    }

    /// The toolkit's items carry the menu's commands: clicking through
    /// all of them asks for each thing the menu offers.
    #[test]
    fn the_tray_s_items_do_what_the_menu_s_entries_say() {
        let (actions, mut clicked) = mpsc::unbounded_channel();
        let mut tray = PadTray::new(actions);
        tray.view = view(Some(Phase::Running), Hiding::Service);
        let items = tray.menu();
        click_all(&items, &mut tray);
        let mut asked = Vec::new();
        while let Ok(action) = clicked.try_recv() {
            asked.push(action);
        }
        assert!(asked.contains(&Action::Stop("couch".into())), "{asked:?}");
        assert!(asked.contains(&Action::Start("desk".into())), "{asked:?}");
        assert!(asked.contains(&Action::Identify("address:dualsense".into())));
        assert!(asked.contains(&Action::Quit));
        assert!(asked.contains(&Action::Settings));
        // A left click on the icon asks for the same.
        Tray::activate(&mut tray, 0, 0);
        assert_eq!(clicked.try_recv(), Ok(Action::Settings));
        assert!(
            asked.iter().any(|action| matches!(
                action,
                Action::Select { setup, choice, .. } if setup == "couch" && choice == "assist"
            )),
            "{asked:?}"
        );
        // Every field of a preset was picked through.
        assert!(
            asked.iter().any(|action| matches!(
                action,
                Action::Put { name, toml } if name == "desk" && toml.contains("outputs = 8")
            )),
            "{asked:?}"
        );

        // With no StatusNotifier host the tray stays, for its notifications.
        assert!(tray.watcher_offline(ksni::OfflineReason::No));
        tray.watcher_online();
        assert!(matches!(tray.category(), ksni::Category::Hardware));
    }
}
