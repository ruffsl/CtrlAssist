//! The ksni tray: it renders the menu model and funnels every click into
//! an [`Action`] over a channel. The async side owns the rpcs. Strings
//! are English literals: the tray isn't translated yet.

use ctrlassist::Phase;
use ksni::menu::{CheckmarkItem, MenuItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{ToolTip, Tray};
use tokio::sync::mpsc;
use tracing::info;

use crate::menu::{self, Command, Edit, Entry};
use crate::quick::{self, Quick};
use crate::state::View;

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
    /// Keep this TOML as the profile `quick`, and run it.
    Quick(String),
    /// Keep this TOML as the profile `quick`, and have the running
    /// `quick` take it if it can as it runs.
    QuickApply(String),
    /// End the tray.
    Quit,
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
    actions: mpsc::UnboundedSender<Action>,
}

impl PadTray {
    /// A tray whose clicks go to `actions`.
    #[must_use]
    pub fn new(actions: mpsc::UnboundedSender<Action>) -> Self {
        Self {
            view: View::default(),
            quick: Quick::default(),
            quick_settled: false,
            actions,
        }
    }

    /// Does what a menu entry asks: a quick-start choice changes here,
    /// and anything else goes to the async side.
    pub fn activate(&mut self, command: Command) {
        let action = match command {
            Command::Quick(edit) => {
                self.quick_settled = true;
                let waits = matches!(edit, Edit::Identity(_));
                match edit {
                    Edit::Primary(pad) => self.quick.primary = Some(pad),
                    Edit::Assist(pad) => self.quick.assist = Some(pad),
                    Edit::Mode(mode) => self.quick.mode = mode,
                    Edit::Identity(identity) => self.quick.identity = identity,
                    Edit::AutoStart(on) => self.quick.auto_start = on,
                    Edit::Rumble(rumble) => self.quick.rumble = rumble,
                }
                // A running `quick` takes everything but another
                // identity as it runs, so a pick is applied as it is
                // made, on the identity it runs with: one picked and
                // still waiting isn't slipped in with it.
                let running = self.view.setup(quick::NAME).is_some();
                let live = self
                    .view
                    .kept_quick()
                    .filter(|_| running && !waits)
                    .and_then(|kept| self.quick.onto(&kept).profile());
                if let Some(toml) = live {
                    let _ = self.actions.send(Action::QuickApply(toml));
                }
                return;
            }
            Command::QuickStart => match self.quick.profile() {
                Some(toml) => Action::Quick(toml),
                None => return,
            },
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
            Command::Quit => Action::Quit,
        };
        let _ = self.actions.send(action);
    }

    /// The menu as it stands, as plain data.
    #[must_use]
    pub fn entries(&self) -> Vec<Entry> {
        menu::menu(&self.view, &self.quick)
    }

    /// The worst phase among the setups: failed, then held, then running.
    fn worst(&self) -> Option<Phase> {
        [Phase::Failed, Phase::Held, Phase::Starting, Phase::Running]
            .into_iter()
            .find(|phase| self.view.setups.iter().any(|setup| setup.phase == *phase))
    }
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
            label: text,
            command,
        } => StandardItem {
            label: label(&text),
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
            label: text,
            entries,
        } => SubMenu {
            label: label(&text),
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
        match self.worst() {
            Some(Phase::Failed) => "dialog-error".into(),
            Some(Phase::Held) => "dialog-warning".into(),
            // Something runs: the colored glyph. Nothing does: the flat one.
            Some(_) => "input-gaming".into(),
            None => "input-gaming-symbolic".into(),
        }
    }

    fn status(&self) -> ksni::Status {
        match self.worst() {
            Some(Phase::Failed | Phase::Held) => ksni::Status::NeedsAttention,
            Some(_) => ksni::Status::Active,
            None => ksni::Status::Passive,
        }
    }

    fn tool_tip(&self) -> ToolTip {
        let description = if !self.view.alive {
            "The daemon isn't running".to_string()
        } else if let Some(theirs) = &self.view.mismatch {
            menu::mismatch_text(theirs)
        } else if self.view.setups.is_empty() {
            "Nothing is running".to_string()
        } else {
            self.view
                .setups
                .iter()
                .map(|setup| format!("{}: {}", setup.name, setup.phase.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        };
        ToolTip {
            title: "CtrlAssist".into(),
            description,
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
        (tray, clicked)
    }

    #[test]
    fn the_icon_title_and_tooltip_say_the_worst_of_what_runs() {
        let said = |phase| {
            let (tray, _clicked) = tray(phase);
            (tray.title(), tray.icon_name(), tray.tool_tip().description)
        };
        assert_eq!(
            said(None),
            (
                "CtrlAssist".to_string(),
                "input-gaming-symbolic".to_string(),
                "Nothing is running".to_string()
            )
        );
        assert_eq!(said(Some(Phase::Running)).1, "input-gaming");
        assert_eq!(said(Some(Phase::Held)).0, "CtrlAssist [held]");
        assert_eq!(said(Some(Phase::Held)).1, "dialog-warning");
        assert_eq!(said(Some(Phase::Failed)).1, "dialog-error");
        assert_eq!(said(Some(Phase::Failed)).2, "couch: failed");

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
        tray.activate(Command::Quick(Edit::Primary("address:dualsense".into())));
        tray.activate(Command::QuickStart);
        assert!(clicked.try_recv().is_err(), "one pad isn't a profile yet");
        assert!(tray.quick_settled);
        tray.activate(Command::Quick(Edit::Assist("address:8bitdo".into())));
        tray.activate(Command::Quick(Edit::Identity(
            ctrlassist_protocol::profile::Identity::Xbox,
        )));
        tray.activate(Command::QuickStart);
        let Ok(Action::Quick(toml)) = clicked.try_recv() else {
            unreachable!("two pads are a profile")
        };
        assert!(toml.contains("identity = \"xbox\""), "{toml}");

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
        assert!(
            asked.iter().any(|action| matches!(
                action,
                Action::Select { setup, choice, .. } if setup == "couch" && choice == "assist"
            )),
            "{asked:?}"
        );
        // The quick-start choices changed where they are kept.
        assert!(tray.quick_settled && tray.quick.auto_start);
        assert_eq!(tray.quick.assist.as_deref(), Some("serial:xbox"));

        // With no StatusNotifier host the tray stays, for its notifications.
        assert!(tray.watcher_offline(ksni::OfflineReason::No));
        tray.watcher_online();
        assert!(matches!(tray.category(), ksni::Category::Hardware));
    }
}
