//! The ksni tray: it renders the menu model and funnels every click into
//! an [`Action`] over a channel. The async side owns the rpcs. Strings
//! are English literals: the tray isn't translated yet.

use ctrlassist::Phase;
use ctrlassist_icons::Variant;
use ksni::menu::{CheckmarkItem, MenuItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{ToolTip, Tray};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tracing::info;

use crate::menu::{self, Command, Entry};
use crate::quick::{self, Quick};
use crate::state::View;
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
    /// Keep this TOML as the profile `quick`, and run it: started if it
    /// isn't running, and if it is, changed as it runs or restarted.
    Quick(String),
    /// Open the settings window, or raise it if it is open.
    Settings,
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
            Self::Quick(_) => format!("keep and run `{}` as picked", quick::NAME),
            Self::Settings => "open the settings window".to_string(),
            Self::Quit => "quit".to_string(),
        }
    }
}

/// The tray's state: what it shows, and where its clicks go.
#[derive(Debug)]
pub struct PadTray {
    /// What the daemon publishes, as last seen.
    pub view: View,
    /// The quick-start choices.
    pub quick: Quick,
    /// Whether the choices have been read from a kept `quick` profile, or
    /// touched, so that they aren't overwritten by a later look.
    pub quick_settled: bool,
    /// The profile last started, stopped or scrolled to: what a middle
    /// click on the icon acts on, while it can be.
    pub target: Option<String>,
    /// How far the wheel has turned toward its next notch, in the
    /// wheel's units, signed by direction.
    wheel: i32,
    /// When the wheel last turned.
    turned: Option<Instant>,
    actions: mpsc::UnboundedSender<Action>,
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
            quick: Quick::default(),
            quick_settled: false,
            target: None,
            wheel: 0,
            turned: None,
            actions,
        }
    }

    /// Takes a new view. A setup that came or went since the last is the
    /// profile last used, whoever started or stopped it.
    pub fn see(&mut self, view: View) {
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
            if let Some(setup) = changed {
                self.target = Some(setup.name.clone());
            }
        }
        self.view = view;
    }

    /// Takes a turn of the wheel, reported at `now`, and moves the mark
    /// over the profiles a middle click could act on, around from the
    /// last to the first: one profile a notch. Scrolls smaller than a
    /// notch add up to one, so a finer wheel or a touchpad moves the
    /// mark as far as a plain wheel does. What was turned is dropped
    /// when the wheel turns the other way, and when it has been still
    /// for [`WHEEL_PAUSE`].
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
            self.target = Some((*name).to_string());
        }
    }

    /// The profile a middle click acts on now.
    #[must_use]
    pub fn aimed(&self) -> Option<&str> {
        tip::target(&self.view, self.target.as_deref())
    }

    /// Does what a menu entry asks: a quick-start choice changes here,
    /// and anything else goes to the async side.
    pub fn activate(&mut self, command: Command) {
        let action = match command {
            Command::Quick(edit) => {
                self.quick_settled = true;
                self.quick.set(&edit.key, &edit.value);
                // A pick is applied to a running `quick` as it is made.
                // All but another identity it takes as it runs; that
                // one restarts it, which a game sees as its pad going
                // and another coming.
                let running = self.view.setup(quick::NAME).is_some();
                match self.quick.profile(&self.view.fields).filter(|_| running) {
                    Some(toml) => Action::Quick(toml),
                    None => return,
                }
            }
            Command::QuickStart => match self.quick.profile(&self.view.fields) {
                Some(toml) => Action::Quick(toml),
                None => return,
            },
            // Starting `quick` starts what is picked, not what was last
            // kept, where the picks make a profile.
            Command::Start(name) if name == quick::NAME => {
                match self.quick.profile(&self.view.fields) {
                    Some(toml) => Action::Quick(toml),
                    None => Action::Start(name),
                }
            }
            Command::Start(name) => Action::Start(name),
            Command::Stop(name) => Action::Stop(name),
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

    /// The menu as it stands, as plain data.
    #[must_use]
    pub fn entries(&self) -> Vec<Entry> {
        menu::menu(&self.view, &self.quick)
    }

    /// The tray's own icon, and which form of it: the colored pad glyph
    /// whether or not something runs, with a warning or an error in its
    /// place for a setup held or failed. The menu's rows are the
    /// symbolic ones.
    fn glyph(&self) -> (menu::Icon, Variant) {
        match self.worst() {
            Some(Phase::Failed) => (menu::icon::FAILED, Variant::Color),
            Some(Phase::Held) => (menu::icon::WRONG, Variant::Color),
            Some(_) | None => (menu::icon::PAD, Variant::Color),
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
        Entry::Check {
            label: text,
            checked,
            command,
        } => CheckmarkItem {
            label: label(&text),
            checked,
            activate: Box::new(move |tray: &mut PadTray| tray.activate(command.clone())),
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

    fn icon_name(&self) -> String {
        let (name, variant) = self.glyph();
        themed(name, variant)
    }

    fn status(&self) -> ksni::Status {
        match self.worst() {
            Some(Phase::Failed | Phase::Held) => ksni::Status::NeedsAttention,
            Some(_) => ksni::Status::Active,
            None => ksni::Status::Passive,
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
    use crate::menu::Edit;
    use crate::state::tests::view;

    fn tray(phase: Option<Phase>) -> (PadTray, mpsc::UnboundedReceiver<Action>) {
        let (actions, clicked) = mpsc::unbounded_channel();
        let mut tray = PadTray::new(actions);
        tray.view = view(phase, Hiding::Service);
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
            // The glyph and its form, before the desktop's theme is
            // asked which name it has for it.
            let (name, variant) = tray.glyph();
            let flat = matches!(variant, Variant::Symbolic);
            (tray.title(), (name[0], flat), tray.tool_tip().description)
        };
        assert_eq!(
            said(None),
            (
                "CtrlAssist".to_string(),
                ("input-gaming", false),
                "Nothing is running\n3 of 3 pads connected".to_string()
            )
        );
        assert_eq!(said(Some(Phase::Running)).1, ("input-gaming", false));
        assert_eq!(said(Some(Phase::Held)).0, "CtrlAssist [held]");
        assert_eq!(said(Some(Phase::Held)).1, ("dialog-warning", false));
        assert_eq!(said(Some(Phase::Failed)).1, ("dialog-error", false));
        assert_eq!(
            said(Some(Phase::Failed)).2,
            "► couch: Failed\n    a virtual pad refused a write"
        );

        let (mut down, _clicked) = tray(Some(Phase::Running));
        down.view.alive = false;
        assert_eq!(down.title(), "CtrlAssist [daemon not running]");
        assert_eq!(down.tool_tip().description, "The daemon isn't running");
        let (mut other, _clicked) = tray(None);
        other.view.mismatch = Some("9.0.0".to_string());
        assert_eq!(other.title(), "CtrlAssist [version mismatch]");
        assert!(other.tool_tip().description.starts_with("Version mismatch"));
        assert!(matches!(other.status(), ksni::Status::Passive));
        assert_eq!(other.id(), "ctrlassist");
    }

    #[test]
    fn every_entry_of_the_menu_becomes_an_item_of_the_tray_s() {
        let (tray, _clicked) = tray(Some(Phase::Held));
        assert_eq!(tray.menu().len(), tray.entries().len());
        assert_eq!(label("a_b"), "a__b");
    }

    #[test]
    fn a_click_is_an_action_and_a_quick_choice_stays_in_the_tray() {
        let (mut tray, mut clicked) = tray(None);
        tray.activate(Command::Quick(Edit::set("primary", "address:dualsense")));
        tray.activate(Command::QuickStart);
        assert!(clicked.try_recv().is_err(), "one pad isn't a profile yet");
        assert!(tray.quick_settled);
        tray.activate(Command::Quick(Edit::set("assist", "address:8bitdo")));
        tray.activate(Command::Quick(Edit::set("identity", "xbox")));
        tray.activate(Command::QuickStart);
        let Ok(Action::Quick(toml)) = clicked.try_recv() else {
            unreachable!("two pads are a profile")
        };
        assert!(toml.contains("identity = \"xbox\""), "{toml}");

        // Starting `quick` itself starts what is picked.
        tray.activate(Command::Start(quick::NAME.into()));
        assert_eq!(clicked.try_recv().unwrap(), Action::Quick(toml));
        assert_eq!(
            Action::Quick(String::new()).named(),
            "keep and run `quick` as picked"
        );
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
                MenuItem::Checkmark(item) => (item.activate)(tray),
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
        // The quick-start choices changed where they are kept.
        assert!(tray.quick_settled);
        assert_eq!(tray.quick.values["managed"], "true");
        assert_eq!(tray.quick.values["assist"], "serial:xbox");

        // With no StatusNotifier host the tray stays, for its notifications.
        assert!(tray.watcher_offline(ksni::OfflineReason::No));
        tray.watcher_online();
        assert!(matches!(tray.category(), ksni::Category::Hardware));
    }
}
