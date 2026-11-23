//! What the tray shows: a snapshot of the SDK's mirror, and what changed
//! between two of them that a person should be told.

use std::collections::BTreeMap;

use ctrlassist::form::Field;
use ctrlassist::{DaemonState, Hiding, Identity, Pad, Phase, Profile, Setup};

use crate::notify::Category;

/// One snapshot of what the daemon publishes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    /// Whether the daemon is there.
    pub alive: bool,
    /// Whether all it publishes has arrived, so that empty means empty.
    pub hydrated: bool,
    /// Which of the daemon's lifetimes this is, as the tray counts them:
    /// one more each time the daemon was seen to come. A daemon that went
    /// and came between two views is told by this, its being down never
    /// having been seen.
    pub lifetime: u64,
    /// The daemon's release, when this tray can't talk to it.
    pub mismatch: Option<String>,
    /// The daemon's own state.
    pub daemon: Option<DaemonState>,
    /// The pads it knows, in the order of their ids.
    pub pads: Vec<Pad>,
    /// The profiles it keeps, in the order of their names.
    pub profiles: Vec<Profile>,
    /// The setups that exist, in the order of their names.
    pub setups: Vec<Setup>,
    /// What a virtual pad can present itself as.
    pub identities: Vec<Identity>,
    /// A preset's fields, as the daemon serves them: what a profile's menu
    /// has to choose from. Empty until the daemon has said.
    pub fields: Vec<Field>,
}

/// Something to tell the person at the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    /// The kind of thing it says, which a person can switch off.
    pub category: Category,
    /// The line that says it.
    pub summary: String,
    /// What else there is to say.
    pub body: String,
    /// A themed icon's name.
    pub icon: &'static str,
}

fn toast(
    category: Category,
    summary: String,
    body: impl Into<String>,
    icon: &'static str,
) -> Toast {
    Toast {
        category,
        summary,
        body: body.into(),
        icon,
    }
}

const GOOD: &str = "input-gaming";
const WARNING: &str = "dialog-warning";
const ERROR: &str = "dialog-error";

impl View {
    /// The setup called `name`, if there is one.
    #[must_use]
    pub fn setup(&self, name: &str) -> Option<&Setup> {
        self.setups.iter().find(|setup| setup.name == name)
    }

    /// The pad known by `id`, if the daemon knows one.
    #[must_use]
    pub fn pad(&self, id: &str) -> Option<&Pad> {
        self.pads.iter().find(|pad| pad.id == id)
    }

    /// What a pad is called when shown: its own name, or its id for one
    /// the daemon has never seen.
    #[must_use]
    pub fn pad_name<'a>(&'a self, id: &'a str) -> &'a str {
        self.pad(id).map_or(id, |pad| pad.name.as_str())
    }

    /// What happened between `before` and this view that is worth a
    /// notification: a setup started or stopped, held, running again or
    /// failed; a setup started whose pads are only grabbed or can't
    /// rumble; a setup restarted; and a pad that no setup uses
    /// connecting or leaving. A restart seen over more than two views,
    /// which takes [`news_for`](Self::news_for)'s bookkeeping to tell, is
    /// said as a stop and a start.
    #[must_use]
    pub fn news(&self, before: &Self) -> Vec<Toast> {
        self.news_for(before, &mut BTreeMap::new())
    }

    /// As [`news`](Self::news), less what was asked for from this tray,
    /// and with what takes more than two views to tell followed on
    /// `asked`. A start or a stop a person has just clicked is no news to
    /// them, nor are the stop and the start a change they picked restarts
    /// the setup with: a stop or a start seen while a change is
    /// unanswered is held on `asked`, for [`answered`] to say or not. A
    /// setup seen under another start number than before was started
    /// afresh in between, and is said to have restarted, once, whoever
    /// asked.
    #[must_use]
    pub fn news_for(&self, before: &Self, asked: &mut BTreeMap<String, Asked>) -> Vec<Toast> {
        let mut news = Vec::new();
        if !self.alive || self.lifetime != before.lifetime {
            // Nothing asked of a daemon that went is still to be seen: a
            // start or a stop it answered or never will, a change it
            // answered or never will, and a restart it numbered, a
            // daemon that comes numbering its starts afresh.
            asked.clear();
        }
        // Nothing is compared until both are complete: a state still
        // arriving looks like things appearing one by one.
        if !(before.hydrated && self.hydrated) {
            return news;
        }
        for setup in &self.setups {
            let name = &setup.name;
            let prior = before.setup(name);
            // What it was, if it is the same start.
            let same = prior.filter(|was| was.start == setup.start);
            let was = same.map(|setup| setup.phase);
            match (was, setup.phase) {
                (Some(Phase::Running), Phase::Held) => {
                    let missing: Vec<&str> = setup
                        .sources
                        .iter()
                        .filter(|source| !source.present)
                        .map(|source| {
                            source
                                .pad
                                .as_deref()
                                .map_or(source.name.as_str(), |pad| self.pad_name(pad))
                        })
                        .collect();
                    news.push(toast(
                        Category::SetupTrouble,
                        format!("{name} is held"),
                        held_body(setup, &missing),
                        WARNING,
                    ));
                }
                (Some(Phase::Held), Phase::Running) => news.push(toast(
                    Category::SetupTrouble,
                    format!("{name} is running again"),
                    "Its pad is back and has taken its place.",
                    GOOD,
                )),
                // Failed, where it was there before: as the same start
                // in another phase, or as one started in its place.
                (Some(was), Phase::Failed) if was != Phase::Failed => {
                    news.push(failed_news(setup));
                }
                (None, Phase::Failed) if prior.is_some() => news.push(failed_news(setup)),
                _ => {}
            }
            let entry = asked.entry(name.clone()).or_default();
            // Another start in its place: the one before stopped, which
            // the start that replaced it says.
            let replaced = prior.filter(|was| was.start != setup.start);
            if let Some(was) = replaced {
                entry.stopped(was.start, Vec::new(), &mut news);
                if setup.phase == Phase::Starting {
                    entry.replacing = Some(setup.start);
                }
            }
            let started = matches!(setup.phase, Phase::Running | Phase::Held)
                && matches!(was, None | Some(Phase::Starting));
            if started {
                let restart = replaced.is_some() || entry.replacing == Some(setup.start);
                let said = self.start_news(setup, restart);
                entry.started(setup.start, said, &mut news);
            }
            if setup.phase != Phase::Starting || entry.replacing != Some(setup.start) {
                entry.replacing = None;
            }
        }
        for setup in &before.setups {
            if self.setup(&setup.name).is_some() {
                continue;
            }
            let said = vec![toast(
                Category::SetupStarted,
                format!("{} stopped", setup.name),
                "Its virtual pad is gone and its pads are visible to games again.",
                GOOD,
            )];
            let entry = asked.entry(setup.name.clone()).or_default();
            entry.replacing = None;
            entry.stopped(setup.start, said, &mut news);
        }
        asked.retain(|_, asked| !asked.is_empty());
        for pad in &self.pads {
            let was = before.pad(&pad.id);
            // A setup speaks for its pads: held, and running again.
            if pad.setup.is_some() || was.is_some_and(|was| was.setup.is_some()) {
                continue;
            }
            if pad.connected == was.is_some_and(|was| was.connected) {
                continue;
            }
            news.push(if pad.connected {
                toast(
                    Category::PadConnection,
                    format!("{} connected", pad.name),
                    "",
                    GOOD,
                )
            } else {
                toast(
                    Category::PadConnection,
                    format!("{} disconnected", pad.name),
                    "",
                    WARNING,
                )
            });
        }
        news
    }

    /// What to say of `setup` starting: that it did, then what it
    /// couldn't do for its pads. Started in place of another start, a
    /// `restart`, it finds what the start before it found, said then.
    fn start_news(&self, setup: &Setup, restart: bool) -> Vec<Toast> {
        let name = &setup.name;
        if restart {
            return vec![toast(
                Category::SetupStarted,
                format!("{name} restarted"),
                "It was started again with a new virtual pad: a game saw its controller disconnect and another connect.",
                GOOD,
            )];
        }
        let mut news = vec![toast(
            Category::SetupStarted,
            format!("{name} started"),
            if setup.phase == Phase::Held {
                "Its virtual pad is there; a pad it needs isn't connected yet."
            } else {
                "Games see its virtual pad in place of the pads it uses."
            },
            GOOD,
        )];
        if self
            .daemon
            .as_ref()
            .is_some_and(|daemon| daemon.hiding == Hiding::Grab)
        {
            news.push(toast(
                Category::SetupTrouble,
                format!("{name}: its pads are grabbed, not hidden"),
                "The hiding service isn't there. A program that reads a pad some other way, as Steam reads a PlayStation pad, still sees it beside the virtual pad.",
                WARNING,
            ));
        }
        let silent: Vec<&str> = setup
            .sources
            .iter()
            .filter_map(|source| self.pad(source.pad.as_deref()?))
            .filter(|pad| pad.rumble == Some(false))
            .map(|pad| pad.name.as_str())
            .collect();
        if !silent.is_empty() {
            news.push(toast(
                Category::SetupTrouble,
                format!("{name}: no rumble on {}", silent.join(" or ")),
                "What a game rumbles won't be felt there.",
                WARNING,
            ));
        }
        news
    }
}

/// What a held setup's notification says: the pads that left, by name,
/// that its virtual pads stay, and whether the pads still there play on
/// or the setup waits for the ones that left.
fn held_body(setup: &Setup, missing: &[&str]) -> String {
    let stay = if setup.sinks.len() == 1 {
        "The virtual pad stays"
    } else {
        "The virtual pads stay"
    };
    let present = setup.sources.iter().filter(|source| source.present).count();
    let rest = match (present, missing.len()) {
        (0, 1) => " until it is back",
        (0, _) => " until they are back",
        (1, _) => ", and the other pad plays on",
        _ => ", and the other pads play on",
    };
    format!("{} disconnected. {stay}{rest}.", missing.join(" and "))
}

/// What a failed setup's notification says.
fn failed_news(setup: &Setup) -> Toast {
    toast(
        Category::SetupTrouble,
        format!("{} failed", setup.name),
        format!(
            "{}. Its pads are visible to games again; stop it to clear it.",
            setup.reason.as_deref().unwrap_or("It stopped working")
        ),
        ERROR,
    )
}

/// What was asked of a setup from this tray and isn't news when seen,
/// and what was seen of it that waits: on an answer, or on more of a
/// restart than two views show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Asked {
    /// A start, not seen yet.
    pub start: bool,
    /// A stop, not seen yet.
    pub stop: bool,
    /// Changes to its profile picked here and not answered yet, each of
    /// which may restart it.
    pub changes: usize,
    /// The setup seen to stop or to start while a change was unanswered:
    /// news held until the answer says whether that was its restart.
    held: Vec<Seen>,
    /// A change answered here with a restart whose start is still to be
    /// seen, by the number the answer gave that start. Its start, and
    /// what is seen of the starts numbered before it, the stop of the
    /// start it replaced among them, were the restart, said already. The
    /// daemon numbers its starts upward, so a start or a stop of a later
    /// number, or that start stopping, is past it, and ends the wait.
    restarted_as: Option<u32>,
    /// A start seen in place of another and still starting, by its
    /// number: once it has started, it is said to have restarted.
    replacing: Option<u32>,
}

impl Asked {
    /// Whether nothing is asked, held or awaited: an entry that has
    /// served what it was for.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !(self.start || self.stop)
            && self.changes == 0
            && self.held.is_empty()
            && self.restarted_as.is_none()
            && self.replacing.is_none()
    }

    /// Takes the setup's start numbered `start` seen to stop, which
    /// `said` says: into `news`, or not when it was asked for here or was
    /// a restart's, or held while a change is unanswered.
    fn stopped(&mut self, start: u32, said: Vec<Toast>, news: &mut Vec<Toast>) {
        match self.restarted_as {
            // A start before the restart's: the one it replaced.
            Some(own) if start < own => return,
            // The restart's own start, or a later one, stopped: what is
            // seen from here isn't the restart.
            Some(_) => self.restarted_as = None,
            None => {}
        }
        if self.stop {
            self.stop = false;
            return;
        }
        if self.changes > 0 {
            self.held.push(Seen::Stopped { start, said });
        } else {
            news.extend(said);
        }
    }

    /// Takes the setup's start numbered `start` seen to have started,
    /// which `said` says: into `news`, or not when it was asked for here
    /// or was a restart's, or held while a change is unanswered.
    fn started(&mut self, start: u32, mut said: Vec<Toast>, news: &mut Vec<Toast>) {
        match self.restarted_as {
            // The restart's own start, or one before it: said already.
            Some(own) if start <= own => {
                if start == own {
                    self.restarted_as = None;
                }
                return;
            }
            // Past it: its start came and went unseen.
            Some(_) => self.restarted_as = None,
            None => {}
        }
        if self.start {
            // Asked for here: seen, and not said. What a start finds
            // wrong is said all the same.
            self.start = false;
            said.remove(0);
            news.extend(said);
            return;
        }
        if self.changes > 0 {
            self.held.push(Seen::Started { start, said });
        } else {
            news.extend(said);
        }
    }
}

/// A setup's start seen to stop or to have started, by its number, with
/// what that says.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Seen {
    Stopped { start: u32, said: Vec<Toast> },
    Started { start: u32, said: Vec<Toast> },
}

/// Takes the answer to a change picked at `name`, with `view` caught up
/// with the daemon as it answered: `restart` is the number of the start
/// the change restarted the setup as, from the update's answer, or
/// nothing for a change taken as it ran. Says what is news now: the
/// restart, once, and anything held that wasn't it. The restart's own
/// is its start of that number and the stop of the last start before it;
/// what of them is still to be seen is awaited on `asked`, and nothing
/// else is silenced by it.
///
/// A daemon too old to number the start in its answer answers 0. The
/// tray asks for one change at a time, so what was held is then guessed
/// to be this change's restart or no change's at all: its stop the first
/// stop held, and its start the first start after it of another number.
/// What is still to be seen of it is said as a stop and a start.
#[must_use]
pub fn answered(
    asked: &mut BTreeMap<String, Asked>,
    view: &View,
    name: &str,
    restart: Option<u32>,
) -> Vec<Toast> {
    let mut news: Vec<Toast> = restart.map(|_| restarted(name)).into_iter().collect();
    let Some(entry) = asked.get_mut(name) else {
        return news;
    };
    entry.changes = entry.changes.saturating_sub(1);
    let held = std::mem::take(&mut entry.held);
    let (stop, start) = match restart {
        None => (None, None),
        Some(0) => guessed(&held),
        Some(own) => (
            held.iter()
                .rposition(|seen| matches!(seen, Seen::Stopped { start, .. } if *start < own)),
            held.iter()
                .position(|seen| matches!(seen, Seen::Started { start, .. } if *start == own)),
        ),
    };
    if let Some(own) = restart.filter(|own| *own != 0)
        && start.is_none()
    {
        // Its start is still to be seen, unless what is seen is past it
        // already: it stopped, or a later start is there.
        let stopped = held
            .iter()
            .any(|seen| matches!(seen, Seen::Stopped { start, .. } if *start >= own));
        let later = view.setup(name).is_some_and(|setup| setup.start > own);
        if !(stopped || later) {
            entry.restarted_as = Some(entry.restarted_as.map_or(own, |was| was.max(own)));
        }
    }
    for (at, seen) in held.into_iter().enumerate() {
        if stop == Some(at) || start == Some(at) {
            continue;
        }
        let (Seen::Stopped { said, .. } | Seen::Started { said, .. }) = seen;
        news.extend(said);
    }
    if entry.is_empty() {
        asked.remove(name);
    }
    news
}

/// A start or a stop asked of a setup from here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// A start.
    Start,
    /// A stop.
    Stop,
}

/// Takes the answer to a start or a stop asked of `name` from here, with
/// the view caught up with the daemon as it answered. What of it the
/// views were to show they have shown by now, or never will: the setup
/// came and went between two of them, or failed before it was seen
/// running, or the rpc failed. The mark goes either way, so that it
/// can't silence a later start or stop of the name that is someone
/// else's.
pub fn unmark(asked: &mut BTreeMap<String, Asked>, name: &str, mark: Mark) {
    let Some(entry) = asked.get_mut(name) else {
        return;
    };
    match mark {
        Mark::Start => entry.start = false,
        Mark::Stop => entry.stop = false,
    }
    if entry.is_empty() {
        asked.remove(name);
    }
}

/// Of what was held while a change was unanswered, where the restart's
/// stop and start are, by a guess for a daemon that doesn't number the
/// restart's start in its answer: the first stop, and the first start
/// after it of another number.
fn guessed(held: &[Seen]) -> (Option<usize>, Option<usize>) {
    let stop = held.iter().enumerate().find_map(|(at, seen)| match seen {
        Seen::Stopped { start, .. } => Some((at, *start)),
        Seen::Started { .. } => None,
    });
    let start = held.iter().enumerate().position(|(at, seen)| match seen {
        Seen::Started { start, .. } => {
            stop.is_none_or(|(stopped_at, from)| at > stopped_at && *start != from)
        }
        Seen::Stopped { .. } => false,
    });
    (stop.map(|(at, _)| at), start)
}

/// What to say of a setup that a change picked here restarted: one
/// notification, where a stop and a start would be two.
#[must_use]
pub fn restarted(name: &str) -> Toast {
    toast(
        Category::SetupStarted,
        format!("{name} restarted"),
        "That change needed a new virtual pad: a game saw its controller disconnect and another connect.",
        GOOD,
    )
}

/// What to say of a change that was kept and that the setup running it
/// didn't take: why, and whether the setup is still `running`, since a
/// restart that couldn't start again leaves it stopped.
#[must_use]
pub fn untaken(name: &str, error: &dyn std::fmt::Display, running: bool) -> Toast {
    let stopped = if running {
        String::new()
    } else {
        format!("\n{name} is stopped; starting it runs the profile as kept.")
    };
    toast(
        Category::Refused,
        format!("Couldn't apply the change to {name}"),
        format!("The change is kept, but couldn't be applied: {error}{stopped}"),
        ERROR,
    )
}

/// What to say of the daemon going away or coming back. `was` is whether
/// it was there at the last look, or nothing at the first: a tray that
/// starts without a daemon has no news of one.
#[must_use]
pub fn daemon_news(was: Option<bool>, alive: bool, had_setups: bool) -> Option<Toast> {
    match (was?, alive) {
        (true, false) => Some(toast(
            Category::SetupTrouble,
            "The CtrlAssist daemon stopped".to_string(),
            if had_setups {
                "Its virtual pads are gone and the pads are visible to games again."
            } else {
                ""
            },
            ERROR,
        )),
        (false, true) => Some(toast(
            Category::SetupTrouble,
            "The CtrlAssist daemon is back".to_string(),
            "Managed profiles start again by themselves.",
            GOOD,
        )),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use ctrlassist::{Problem, SetupNode, SetupSink, SetupSource};

    use super::*;

    pub(crate) fn pad(id: &str, name: &str, connected: bool, setup: Option<&str>) -> Pad {
        Pad {
            id: id.to_string(),
            name: name.to_string(),
            vendor: 0x054c,
            product: 0x0ce6,
            connected,
            hidden: false,
            setup: setup.map(str::to_string),
            rumble: Some(name != "8BitDo Pro 2"),
            bus: None,
        }
    }

    pub(crate) fn profile(name: &str, problem: Option<&str>) -> Profile {
        Profile {
            name: name.to_string(),
            toml: String::new(),
            unset: Vec::new(),
            problem: problem.map(|message| Problem {
                at: "mode".to_string(),
                message: message.to_string(),
            }),
        }
    }

    /// A toggle over the DualSense and the Xbox pad, as `phase` has it.
    pub(crate) fn couch(phase: Phase) -> Setup {
        let held = phase == Phase::Held;
        Setup {
            name: "couch".to_string(),
            start: 1,
            phase,
            reason: (phase == Phase::Failed).then(|| "a virtual pad refused a write".to_string()),
            sources: vec![
                SetupSource {
                    name: "assist".to_string(),
                    pad: Some("serial:xbox".to_string()),
                    present: !held,
                },
                SetupSource {
                    name: "primary".to_string(),
                    pad: Some("address:dualsense".to_string()),
                    present: true,
                },
            ],
            sinks: vec![SetupSink {
                name: "virtual".to_string(),
                identity: "generic".to_string(),
                node: Some("/dev/input/event31".to_string()),
            }],
            nodes: vec![SetupNode {
                name: "mux".to_string(),
                choices: vec!["primary".to_string(), "assist".to_string()],
                selected: 0,
            }],
            stale: false,
        }
    }

    /// `couch` as kept: a toggle over the DualSense and the Xbox pad.
    pub(crate) const COUCH: &str =
        "mode = \"toggle\"\nprimary = \"address:dualsense\"\nassist = \"serial:xbox\"\n";

    /// `desk` as kept: the 8BitDo pad as two virtual pads.
    pub(crate) const DESK: &str = "mode = \"unicast\"\npad = \"address:8bitdo\"\n";

    /// A daemon with three pads, two profiles and `couch` as given.
    pub(crate) fn view(couch_is: Option<Phase>, hiding: Hiding) -> View {
        let in_couch = couch_is.map(|_| "couch");
        let held = couch_is == Some(Phase::Held);
        View {
            alive: true,
            hydrated: true,
            lifetime: 1,
            mismatch: None,
            daemon: Some(DaemonState {
                version: "0.5.0".to_string(),
                realtime: true,
                hiding,
                config: String::new(),
                hider_version: None,
            }),
            pads: vec![
                pad("address:dualsense", "DualSense", true, in_couch),
                pad("address:8bitdo", "8BitDo Pro 2", true, None),
                pad("serial:xbox", "Xbox pad", !held, in_couch),
            ],
            profiles: vec![
                Profile {
                    toml: COUCH.to_string(),
                    ..profile("couch", None)
                },
                Profile {
                    toml: DESK.to_string(),
                    ..profile("desk", None)
                },
            ],
            setups: couch_is.map(couch).into_iter().collect(),
            identities: Vec::new(),
            fields: ctrlassist::form::fields(),
        }
    }

    fn summaries(news: &[Toast]) -> Vec<&str> {
        news.iter().map(|toast| toast.summary.as_str()).collect()
    }

    /// `couch` asked for from the tray as `asked` says.
    fn asked(asked: Asked) -> BTreeMap<String, Asked> {
        BTreeMap::from([("couch".to_string(), asked)])
    }

    fn start() -> Asked {
        Asked {
            start: true,
            ..Asked::default()
        }
    }

    /// `n` changes picked at `couch` and not answered yet.
    fn changes(n: usize) -> BTreeMap<String, Asked> {
        asked(Asked {
            changes: n,
            ..Asked::default()
        })
    }

    /// What a person asked for from the tray isn't news to them: a
    /// start or a stop. From anywhere else, each is.
    #[test]
    fn what_was_asked_from_the_tray_is_not_news() {
        let running = view(Some(Phase::Running), Hiding::Service);
        let stopped = view(None, Hiding::Service);

        let mut started = asked(start());
        assert!(running.news_for(&stopped, &mut started).is_empty());
        assert!(started.is_empty(), "seen, and taken off");
        let mut stop = asked(Asked {
            stop: true,
            ..Asked::default()
        });
        assert!(stopped.news_for(&running, &mut stop).is_empty());
        assert!(stop.is_empty());

        // Asked of another setup, or the other way about, it is news.
        let mut other = BTreeMap::from([("desk".to_string(), start())]);
        assert_eq!(
            summaries(&running.news_for(&stopped, &mut other)),
            ["couch started"]
        );
        let mut wrong = asked(start());
        assert_eq!(
            summaries(&stopped.news_for(&running, &mut wrong)),
            ["couch stopped"]
        );
        // What a start finds wrong is said, asked for or not.
        let grabbed = view(Some(Phase::Running), Hiding::Grab);
        let mut started = asked(start());
        assert_eq!(
            summaries(&grabbed.news_for(&view(None, Hiding::Grab), &mut started)),
            ["couch: its pads are grabbed, not hidden"]
        );
    }

    /// A start asked from here and answered leaves nothing marked to
    /// silence a later one from elsewhere: not when its setup is first
    /// seen failed, nor when it came and went between two views.
    #[test]
    fn a_start_answered_leaves_no_mark_whatever_the_views_showed_of_it() {
        let stopped = view(None, Hiding::Service);
        let running = view(Some(Phase::Running), Hiding::Service);
        let failed = view(Some(Phase::Failed), Hiding::Service);

        // First seen failed: no start is seen, and nothing said.
        let mut marks = asked(start());
        assert!(failed.news_for(&stopped, &mut marks).is_empty());
        unmark(&mut marks, "couch", Mark::Start);
        assert!(marks.is_empty());
        // Cleared away, then started from elsewhere: news.
        let _ = stopped.news_for(&failed, &mut marks);
        assert_eq!(
            summaries(&numbered(&running, 2).news_for(&stopped, &mut marks)),
            ["couch started"]
        );

        // Started and stopped again between two views: nothing seen.
        let mut marks = asked(start());
        assert!(stopped.news_for(&stopped, &mut marks).is_empty());
        unmark(&mut marks, "couch", Mark::Start);
        assert!(marks.is_empty());
        assert_eq!(
            summaries(&running.news_for(&stopped, &mut marks)),
            ["couch started"]
        );

        // The same of a stop whose setup came back between two views:
        // the start after it is someone else's, and news.
        let mut marks = asked(Asked {
            stop: true,
            ..Asked::default()
        });
        unmark(&mut marks, "couch", Mark::Stop);
        assert!(marks.is_empty());
        assert_eq!(
            summaries(&stopped.news_for(&running, &mut marks)),
            ["couch stopped"]
        );
        // Marks of another kind, or of another setup, stay.
        let mut both = asked(Asked {
            start: true,
            stop: true,
            ..Asked::default()
        });
        unmark(&mut both, "couch", Mark::Stop);
        unmark(&mut both, "desk", Mark::Start);
        assert_eq!(both, asked(start()));
    }

    /// A daemon that went and came between two views, its being down
    /// never seen, takes with it all that was asked of it: neither a
    /// start it never answered nor a restart it numbered silences what
    /// the daemon that came does.
    #[test]
    fn a_daemon_restart_never_seen_as_down_leaves_nothing_asked() {
        let running = numbered(&view(Some(Phase::Running), Hiding::Service), 4);
        let stopped = view(None, Hiding::Service);
        let next = |view: &View| View {
            lifetime: view.lifetime + 1,
            ..view.clone()
        };

        // A restart answered as start 5, whose start the old daemon never
        // published; the new daemon starts the profile as its first.
        let mut marks = changes(1);
        let _ = answered(&mut marks, &running, "couch", Some(5));
        assert!(!marks.is_empty());
        let first = numbered(&next(&running), 1);
        assert_eq!(
            summaries(&first.news_for(&running, &mut marks)),
            ["couch restarted"]
        );
        assert!(marks.is_empty());

        // A start asked of the daemon that went, never seen.
        let mut marks = asked(start());
        let again = next(&stopped);
        assert!(again.news_for(&stopped, &mut marks).is_empty());
        assert!(marks.is_empty());
        assert_eq!(
            summaries(&numbered(&next(&running), 1).news_for(&again, &mut marks)),
            ["couch started"]
        );
    }

    /// `view` with its setup started afresh as the daemon's `start`th.
    fn numbered(view: &View, start: u32) -> View {
        let mut view = view.clone();
        for setup in &mut view.setups {
            setup.start = start;
        }
        view
    }

    /// A change picked here that restarts a setup is said once, as a
    /// restart, however much of its stop and start the views show and
    /// whichever comes first, the views or the answer. Nothing is left
    /// to silence what comes after.
    #[test]
    fn a_restart_is_said_once_however_much_of_it_is_seen() {
        let running = view(Some(Phase::Running), Hiding::Service);
        let again = numbered(&running, 2);
        let starting = numbered(&view(Some(Phase::Starting), Hiding::Service), 2);
        let stopped = view(None, Hiding::Service);

        // All of it seen before the answer, and nothing after.
        let mut asked = changes(1);
        assert!(stopped.news_for(&running, &mut asked).is_empty());
        assert!(again.news_for(&stopped, &mut asked).is_empty());
        assert_eq!(
            summaries(&answered(&mut asked, &again, "couch", Some(2))),
            ["couch restarted"]
        );
        assert!(asked.is_empty());

        // Its stop and its start between two views, before the answer.
        // What is done elsewhere afterwards is news.
        let mut asked = changes(1);
        assert!(again.news_for(&running, &mut asked).is_empty());
        assert_eq!(
            summaries(&answered(&mut asked, &again, "couch", Some(2))),
            ["couch restarted"]
        );
        assert!(asked.is_empty(), "nothing is left to wait for");
        assert_eq!(
            summaries(&stopped.news_for(&again, &mut asked)),
            ["couch stopped"]
        );
        assert_eq!(
            summaries(&numbered(&running, 3).news_for(&stopped, &mut asked)),
            ["couch started"]
        );

        // None of it before the answer: the views catch up after, all of
        // it at once or a part at a time.
        let mut asked = changes(1);
        assert!(running.news_for(&running, &mut asked).is_empty());
        assert_eq!(
            summaries(&answered(&mut asked, &running, "couch", Some(2))),
            ["couch restarted"]
        );
        let mut at_once = asked.clone();
        assert!(again.news_for(&running, &mut at_once).is_empty());
        assert!(at_once.is_empty());
        assert!(stopped.news_for(&running, &mut asked).is_empty());
        assert!(starting.news_for(&stopped, &mut asked).is_empty());
        assert!(again.news_for(&starting, &mut asked).is_empty());
        assert!(asked.is_empty());

        // Its stop seen before the answer, its start after.
        let mut asked = changes(1);
        assert!(stopped.news_for(&running, &mut asked).is_empty());
        assert_eq!(
            summaries(&answered(&mut asked, &stopped, "couch", Some(2))),
            ["couch restarted"]
        );
        assert!(again.news_for(&stopped, &mut asked).is_empty());
        assert!(asked.is_empty());
        // Or seen starting at the answer, its stop not seen at all.
        let mut asked = changes(1);
        assert!(starting.news_for(&running, &mut asked).is_empty());
        assert_eq!(
            summaries(&answered(&mut asked, &starting, "couch", Some(2))),
            ["couch restarted"]
        );
        assert!(again.news_for(&starting, &mut asked).is_empty());
        assert!(asked.is_empty());
        // Its start went by unseen, and what stops next is someone
        // else's doing.
        let mut asked = changes(1);
        assert!(stopped.news_for(&running, &mut asked).is_empty());
        let _ = answered(&mut asked, &stopped, "couch", Some(2));
        assert!(starting.news_for(&stopped, &mut asked).is_empty());
        assert_eq!(
            summaries(&stopped.news_for(&starting, &mut asked)),
            ["couch stopped"]
        );
        assert!(asked.is_empty());
        // Nor is a restart awaited from a daemon that has gone.
        let mut asked = changes(1);
        let _ = answered(&mut asked, &running, "couch", Some(2));
        assert!(!asked.is_empty());
        assert!(View::default().news_for(&running, &mut asked).is_empty());
        assert!(asked.is_empty());

        // Nor is what the restart finds wrong said, which its first
        // start said.
        let mut asked = changes(1);
        let grabbed = view(Some(Phase::Running), Hiding::Grab);
        let none = view(None, Hiding::Grab);
        let regrabbed = numbered(&grabbed, 2);
        assert!(none.news_for(&grabbed, &mut asked).is_empty());
        assert!(regrabbed.news_for(&none, &mut asked).is_empty());
        assert_eq!(
            summaries(&answered(&mut asked, &regrabbed, "couch", Some(2))),
            ["couch restarted"]
        );
    }

    /// A setup restarted from elsewhere is said to have, once, whether
    /// its new start is seen running at once or starting first. Its
    /// stop, seen on its own, is said as a stop.
    #[test]
    fn a_restart_from_elsewhere_is_said_as_one() {
        let running = view(Some(Phase::Running), Hiding::Grab);
        let again = numbered(&running, 2);
        let starting = numbered(&view(Some(Phase::Starting), Hiding::Grab), 2);
        let mut asked = BTreeMap::new();

        let news = again.news_for(&running, &mut asked);
        assert_eq!(summaries(&news), ["couch restarted"]);
        assert_eq!(news[0].category, Category::SetupStarted);
        assert!(news[0].body.contains("new virtual pad"), "{news:?}");
        assert!(asked.is_empty());

        assert!(starting.news_for(&running, &mut asked).is_empty());
        assert!(!asked.is_empty(), "its start is still to come");
        assert_eq!(
            summaries(&again.news_for(&starting, &mut asked)),
            ["couch restarted"]
        );
        assert!(asked.is_empty());
        // One that never started is said for what became of it.
        assert!(starting.news_for(&running, &mut asked).is_empty());
        let failed = numbered(&view(Some(Phase::Failed), Hiding::Grab), 2);
        assert_eq!(
            summaries(&failed.news_for(&starting, &mut asked)),
            ["couch failed"]
        );
        assert!(asked.is_empty());
        assert_eq!(
            summaries(&failed.news_for(&running, &mut asked)),
            ["couch failed"]
        );
        assert!(asked.is_empty());

        // The same start taking a change as it runs is no news.
        let mut changed = running.clone();
        changed.setups[0].nodes.clear();
        assert!(changed.news_for(&running, &mut asked).is_empty());
        assert!(asked.is_empty());
    }

    /// Two changes picked in a row are two answers: each restart is said
    /// as one, and a change taken as the setup ran has nothing to say.
    #[test]
    fn changes_picked_in_a_row_are_answered_each_on_its_own() {
        let stopped = view(None, Hiding::Service);
        let mut asked = changes(2);
        for start in 1..3 {
            let running = numbered(&view(Some(Phase::Running), Hiding::Service), start);
            let again = numbered(&running, start + 1);
            assert!(stopped.news_for(&running, &mut asked).is_empty());
            assert!(again.news_for(&stopped, &mut asked).is_empty());
            assert_eq!(
                summaries(&answered(&mut asked, &again, "couch", Some(start + 1))),
                ["couch restarted"]
            );
        }
        assert!(asked.is_empty());
        // A pick taken as it ran, then one that restarts it.
        let running = view(Some(Phase::Running), Hiding::Service);
        let again = numbered(&running, 2);
        let mut asked = changes(2);
        assert!(answered(&mut asked, &running, "couch", None).is_empty());
        assert!(stopped.news_for(&running, &mut asked).is_empty());
        assert!(again.news_for(&stopped, &mut asked).is_empty());
        assert_eq!(
            summaries(&answered(&mut asked, &again, "couch", Some(2))),
            ["couch restarted"]
        );
        assert!(asked.is_empty());
        // A restart answered before the views show it, then another
        // whose stop and start are seen before its answer.
        let mut asked = changes(2);
        let _ = answered(&mut asked, &running, "couch", Some(2));
        assert!(again.news_for(&running, &mut asked).is_empty());
        let third = numbered(&running, 3);
        assert!(stopped.news_for(&again, &mut asked).is_empty());
        assert!(third.news_for(&stopped, &mut asked).is_empty());
        assert_eq!(
            summaries(&answered(&mut asked, &third, "couch", Some(3))),
            ["couch restarted"]
        );
        assert!(asked.is_empty());
        // Two restarts answered before the views show either: all that
        // is seen of both is theirs, however much of it is seen.
        let mut asked = changes(2);
        let _ = answered(&mut asked, &running, "couch", Some(2));
        let _ = answered(&mut asked, &running, "couch", Some(3));
        let mut partly = asked.clone();
        assert!(again.news_for(&running, &mut asked).is_empty());
        assert!(stopped.news_for(&again, &mut asked).is_empty());
        assert!(third.news_for(&stopped, &mut asked).is_empty());
        assert!(asked.is_empty());
        assert!(third.news_for(&running, &mut partly).is_empty());
        assert!(partly.is_empty());
    }

    /// The answer to a change that restarts a setup names the restart's
    /// start, and only that start, and the stop of the one it replaced,
    /// are silenced: a restart whose start the views never show leaves
    /// nothing waiting to silence the start after it, whoever asks for
    /// it.
    #[test]
    fn only_the_start_the_answer_names_is_the_restart_s() {
        let running = view(Some(Phase::Running), Hiding::Service);
        let stopped = view(None, Hiding::Service);
        let third = numbered(&running, 3);

        // Its stop is seen; its start, 2, is stopped by someone else
        // before the views show it. Start 3 is news, and so is its stop.
        let mut asked = changes(1);
        assert!(stopped.news_for(&running, &mut asked).is_empty());
        assert_eq!(
            summaries(&answered(&mut asked, &stopped, "couch", Some(2))),
            ["couch restarted"]
        );
        assert_eq!(
            summaries(&third.news_for(&stopped, &mut asked)),
            ["couch started"]
        );
        assert!(asked.is_empty(), "nothing waits past the restart's start");
        assert_eq!(
            summaries(&stopped.news_for(&third, &mut asked)),
            ["couch stopped"]
        );

        // The same, with start 3 seen before the answer: it is said at
        // the answer, and nothing is left to wait for.
        let mut asked = changes(1);
        assert!(stopped.news_for(&running, &mut asked).is_empty());
        assert!(third.news_for(&stopped, &mut asked).is_empty());
        assert_eq!(
            summaries(&answered(&mut asked, &third, "couch", Some(2))),
            ["couch restarted", "couch started"]
        );
        assert!(asked.is_empty());

        // Seen starting, then gone, before the answer: its stop is
        // someone else's, said at the answer, and nothing waits.
        let starting = numbered(&view(Some(Phase::Starting), Hiding::Service), 2);
        let mut asked = changes(1);
        assert!(starting.news_for(&running, &mut asked).is_empty());
        assert!(stopped.news_for(&starting, &mut asked).is_empty());
        assert_eq!(
            summaries(&answered(&mut asked, &stopped, "couch", Some(2))),
            ["couch restarted", "couch stopped"]
        );
        assert!(asked.is_empty());

        // Stopped and started from elsewhere while the change was
        // unanswered, then restarted by it: only the restart's own stop
        // and start are silenced, the others said at the answer.
        let again = numbered(&running, 2);
        let mut asked = changes(1);
        assert!(stopped.news_for(&running, &mut asked).is_empty());
        assert!(again.news_for(&stopped, &mut asked).is_empty());
        assert!(stopped.news_for(&again, &mut asked).is_empty());
        assert!(third.news_for(&stopped, &mut asked).is_empty());
        assert_eq!(
            summaries(&answered(&mut asked, &third, "couch", Some(3))),
            ["couch restarted", "couch stopped", "couch started"]
        );
        assert!(asked.is_empty());
    }

    /// A daemon too old to number the restart's start in its answer
    /// answers 0: what was held is paired as the restart by a guess, and
    /// nothing waits for what is still to be seen of it, which is said.
    #[test]
    fn a_restart_unnumbered_by_an_older_daemon_waits_for_nothing() {
        let running = view(Some(Phase::Running), Hiding::Service);
        let again = numbered(&running, 2);
        let stopped = view(None, Hiding::Service);

        let mut asked = changes(1);
        assert!(stopped.news_for(&running, &mut asked).is_empty());
        assert!(again.news_for(&stopped, &mut asked).is_empty());
        assert_eq!(
            summaries(&answered(&mut asked, &again, "couch", Some(0))),
            ["couch restarted"]
        );
        assert!(asked.is_empty());

        let mut asked = changes(1);
        assert!(stopped.news_for(&running, &mut asked).is_empty());
        assert_eq!(
            summaries(&answered(&mut asked, &stopped, "couch", Some(0))),
            ["couch restarted"]
        );
        assert!(asked.is_empty(), "nothing waits");
        assert_eq!(
            summaries(&again.news_for(&stopped, &mut asked)),
            ["couch started"]
        );
    }

    /// What is seen while a change is unanswered and turns out not to be
    /// its restart is said once the answer comes: a stop from elsewhere,
    /// or the stop of a restart whose new start failed.
    #[test]
    fn a_stop_that_was_not_a_restart_is_said_at_the_answer() {
        let running = view(Some(Phase::Running), Hiding::Service);
        let stopped = view(None, Hiding::Service);
        let mut asked = changes(1);
        assert!(stopped.news_for(&running, &mut asked).is_empty());
        assert_eq!(
            summaries(&answered(&mut asked, &stopped, "couch", None)),
            ["couch stopped"]
        );
        assert!(asked.is_empty());
        // The change is kept and said so, with the setup now stopped.
        let said = untaken("couch", &"setup-failed: no pad", false);
        assert_eq!(said.summary, "Couldn't apply the change to couch");
        assert_eq!(
            said.body,
            "The change is kept, but couldn't be applied: setup-failed: no pad\ncouch is stopped; starting it runs the profile as kept."
        );
        assert_eq!(said.category, Category::Refused);
        let said = untaken("couch", &"setup-busy: starting", true);
        assert_eq!(
            said.body,
            "The change is kept, but couldn't be applied: setup-busy: starting"
        );
        // An answer for a profile nothing was asked of says only that.
        assert!(answered(&mut BTreeMap::new(), &running, "couch", None).is_empty());
    }

    #[test]
    fn a_setup_s_phases_are_news_once_each() {
        let running = view(Some(Phase::Running), Hiding::Service);
        let held = view(Some(Phase::Held), Hiding::Service);
        let failed = view(Some(Phase::Failed), Hiding::Service);

        let news = held.news(&running);
        assert_eq!(summaries(&news), ["couch is held"]);
        assert_eq!(
            news[0].body,
            "Xbox pad disconnected. The virtual pad stays, and the other pad plays on."
        );
        // A setup of one pad, with two virtual pads, waits for it.
        let (mut alone, mut was) = (held.clone(), running.clone());
        for view in [&mut alone, &mut was] {
            view.setups[0].sources.truncate(1);
            let sink = view.setups[0].sinks[0].clone();
            view.setups[0].sinks.push(sink);
        }
        assert_eq!(
            alone.news(&was)[0].body,
            "Xbox pad disconnected. The virtual pads stay until it is back."
        );
        assert_eq!(summaries(&running.news(&held)), ["couch is running again"]);
        let news = failed.news(&running);
        assert_eq!(summaries(&news), ["couch failed"]);
        assert!(
            news[0].body.starts_with("a virtual pad refused a write."),
            "{news:?}"
        );

        // Nothing changing is nothing to say, whatever the phase.
        for same in [&running, &held, &failed] {
            assert!(same.news(same).is_empty());
        }
        // A setup stopped is said, as one kind with its starting.
        let news = view(None, Hiding::Service).news(&running);
        assert_eq!(summaries(&news), ["couch stopped"]);
        assert_eq!(news[0].category, Category::SetupStarted);
    }

    #[test]
    fn a_pad_coming_or_going_is_news_unless_a_setup_speaks_for_it() {
        let idle = view(None, Hiding::Service);
        let mut without = idle.clone();
        without.pads[1].connected = false;
        let news = idle.news(&without);
        assert_eq!(summaries(&news), ["8BitDo Pro 2 connected"]);
        assert_eq!(news[0].category, Category::PadConnection);
        assert_eq!(
            summaries(&without.news(&idle)),
            ["8BitDo Pro 2 disconnected"]
        );
        // A pad first heard of, connected.
        let mut fewer = idle.clone();
        fewer.pads.remove(1);
        assert_eq!(summaries(&idle.news(&fewer)), ["8BitDo Pro 2 connected"]);
        // One that is remembered and then forgotten was never there.
        assert!(fewer.news(&without).is_empty());
        // A setup's pad leaving is the setup held, said once.
        let running = view(Some(Phase::Running), Hiding::Service);
        let held = view(Some(Phase::Held), Hiding::Service);
        assert_eq!(summaries(&held.news(&running)), ["couch is held"]);
    }

    #[test]
    fn a_setup_starting_says_what_it_could_not_do_for_its_pads() {
        let none = view(None, Hiding::Grab);
        let started = view(Some(Phase::Running), Hiding::Grab);
        let news = started.news(&none);
        assert_eq!(
            summaries(&news),
            ["couch started", "couch: its pads are grabbed, not hidden"]
        );
        assert_eq!(news[0].category, Category::SetupStarted);
        assert_eq!(news[1].category, Category::SetupTrouble);
        // Through its starting phase, it is said once.
        let starting = view(Some(Phase::Starting), Hiding::Grab);
        assert!(starting.news(&none).is_empty());
        assert_eq!(started.news(&starting).len(), 2);
        // With the hiding service there is nothing to warn of.
        assert_eq!(
            summaries(
                &view(Some(Phase::Running), Hiding::Service).news(&view(None, Hiding::Service))
            ),
            ["couch started"]
        );
        // A setup that starts short of a pad says so.
        let news = view(Some(Phase::Held), Hiding::Service).news(&view(None, Hiding::Service));
        assert_eq!(summaries(&news), ["couch started"]);
        assert!(news[0].body.contains("isn't connected yet"), "{news:?}");

        // A pad that can't rumble is named.
        let mut silent = view(Some(Phase::Running), Hiding::Service);
        silent.setups[0].sources[0].pad = Some("address:8bitdo".to_string());
        let news = silent.news(&view(None, Hiding::Service));
        assert_eq!(
            summaries(&news),
            ["couch started", "couch: no rumble on 8BitDo Pro 2"]
        );
    }

    #[test]
    fn a_state_still_arriving_is_not_news() {
        let mut arriving = view(None, Hiding::Grab);
        arriving.hydrated = false;
        let arrived = view(Some(Phase::Held), Hiding::Grab);
        assert!(arrived.news(&arriving).is_empty());
        assert!(arrived.news(&View::default()).is_empty());
    }

    #[test]
    fn the_daemon_going_and_coming_back_is_news_but_its_absence_at_start_is_not() {
        assert_eq!(daemon_news(None, false, false), None);
        assert_eq!(daemon_news(None, true, false), None);
        assert_eq!(daemon_news(Some(true), true, true), None);
        assert_eq!(daemon_news(Some(false), false, false), None);
        let stopped = daemon_news(Some(true), false, true).unwrap();
        assert!(stopped.body.contains("virtual pads are gone"));
        assert_eq!(daemon_news(Some(true), false, false).unwrap().body, "");
        assert!(
            daemon_news(Some(false), true, false)
                .unwrap()
                .summary
                .ends_with("is back")
        );
    }

    #[test]
    fn a_pad_the_daemon_never_saw_goes_by_its_id() {
        let view = view(None, Hiding::Service);
        assert_eq!(view.pad_name("address:dualsense"), "DualSense");
        assert_eq!(view.pad_name("address:unseen"), "address:unseen");
    }
}
