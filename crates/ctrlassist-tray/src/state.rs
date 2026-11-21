//! What the tray shows: a snapshot of the SDK's mirror, and what changed
//! between two of them that a person should be told.

use ctrlassist::{DaemonState, Hiding, Identity, Pad, Phase, Profile, Setup};

use crate::quick::{self, Quick};

/// One snapshot of what the daemon publishes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    /// Whether the daemon is there.
    pub alive: bool,
    /// Whether all it publishes has arrived, so that empty means empty.
    pub hydrated: bool,
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
}

/// Something to tell the person at the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toast {
    /// The line that says it.
    pub summary: String,
    /// What else there is to say.
    pub body: String,
    /// A themed icon's name.
    pub icon: &'static str,
}

fn toast(summary: String, body: impl Into<String>, icon: &'static str) -> Toast {
    Toast {
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

    /// The quick-start choices the kept `quick` profile was written
    /// from, if one is kept that quick start wrote.
    #[must_use]
    pub fn kept_quick(&self) -> Option<Quick> {
        self.profiles
            .iter()
            .find(|profile| profile.name == quick::NAME)
            .and_then(|profile| Quick::from_profile(&profile.toml))
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
    /// notification: a setup held, running again or failed, and a setup
    /// started whose pads are only grabbed or can't rumble.
    #[must_use]
    pub fn news(&self, before: &Self) -> Vec<Toast> {
        let mut news = Vec::new();
        // Nothing is compared until both are complete: a state still
        // arriving looks like things appearing one by one.
        if !(before.hydrated && self.hydrated) {
            return news;
        }
        for setup in &self.setups {
            let was = before.setup(&setup.name).map(|setup| setup.phase);
            let name = &setup.name;
            let started = matches!(was, None | Some(Phase::Starting))
                && matches!(setup.phase, Phase::Running | Phase::Held);
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
                        format!("{name} is held"),
                        format!(
                            "{} disconnected. The virtual pad stays, and the rest plays on.",
                            missing.join(" and ")
                        ),
                        WARNING,
                    ));
                }
                (Some(Phase::Held), Phase::Running) => news.push(toast(
                    format!("{name} is running again"),
                    "Its pad is back and has taken its place.",
                    GOOD,
                )),
                (Some(was), Phase::Failed) if was != Phase::Failed => news.push(toast(
                    format!("{name} failed"),
                    format!(
                        "{}. Its pads are visible to games again; stop it to clear it.",
                        setup.reason.as_deref().unwrap_or("It stopped working")
                    ),
                    ERROR,
                )),
                _ => {}
            }
            if !started {
                continue;
            }
            if self
                .daemon
                .as_ref()
                .is_some_and(|daemon| daemon.hiding == Hiding::Grab)
            {
                news.push(toast(
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
                    format!("{name}: no rumble on {}", silent.join(" or ")),
                    "What a game rumbles won't be felt there.",
                    WARNING,
                ));
            }
        }
        news
    }
}

/// What to say of the daemon going away or coming back. `was` is whether
/// it was there at the last look, or nothing at the first: a tray that
/// starts without a daemon has no news of one.
#[must_use]
pub fn daemon_news(was: Option<bool>, alive: bool, had_setups: bool) -> Option<Toast> {
    match (was?, alive) {
        (true, false) => Some(toast(
            "The CtrlAssist daemon stopped".to_string(),
            if had_setups {
                "Its virtual pads are gone and the pads are visible to games again."
            } else {
                ""
            },
            ERROR,
        )),
        (false, true) => Some(toast(
            "The CtrlAssist daemon is back".to_string(),
            "Profiles that start by themselves will.",
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
        }
    }

    pub(crate) fn profile(name: &str, problem: Option<&str>) -> Profile {
        Profile {
            name: name.to_string(),
            toml: String::new(),
            problem: problem.map(|message| Problem {
                at: "preset".to_string(),
                message: message.to_string(),
            }),
        }
    }

    /// A toggle over the DualSense and the Xbox pad, as `phase` has it.
    pub(crate) fn couch(phase: Phase) -> Setup {
        let held = phase == Phase::Held;
        Setup {
            name: "couch".to_string(),
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
        }
    }

    /// A daemon with three pads, two profiles and `couch` as given.
    pub(crate) fn view(couch_is: Option<Phase>, hiding: Hiding) -> View {
        let in_couch = couch_is.map(|_| "couch");
        let held = couch_is == Some(Phase::Held);
        View {
            alive: true,
            hydrated: true,
            mismatch: None,
            daemon: Some(DaemonState {
                version: "0.5.0".to_string(),
                realtime: true,
                hiding,
            }),
            pads: vec![
                pad("address:dualsense", "DualSense", true, in_couch),
                pad("address:8bitdo", "8BitDo Pro 2", true, None),
                pad("serial:xbox", "Xbox pad", !held, in_couch),
            ],
            profiles: vec![profile("couch", None), profile("desk", None)],
            setups: couch_is.map(couch).into_iter().collect(),
            identities: Vec::new(),
        }
    }

    fn summaries(news: &[Toast]) -> Vec<&str> {
        news.iter().map(|toast| toast.summary.as_str()).collect()
    }

    #[test]
    fn a_setup_s_phases_are_news_once_each() {
        let running = view(Some(Phase::Running), Hiding::Service);
        let held = view(Some(Phase::Held), Hiding::Service);
        let failed = view(Some(Phase::Failed), Hiding::Service);

        let news = held.news(&running);
        assert_eq!(summaries(&news), ["couch is held"]);
        assert!(
            news[0].body.starts_with("Xbox pad disconnected"),
            "{news:?}"
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
        // Nor is a setup stopped by whoever asked for that.
        assert!(view(None, Hiding::Service).news(&running).is_empty());
    }

    #[test]
    fn a_setup_starting_says_what_it_could_not_do_for_its_pads() {
        let none = view(None, Hiding::Grab);
        let started = view(Some(Phase::Running), Hiding::Grab);
        assert_eq!(
            summaries(&started.news(&none)),
            ["couch: its pads are grabbed, not hidden"]
        );
        // Through its starting phase, it is said once.
        let starting = view(Some(Phase::Starting), Hiding::Grab);
        assert!(starting.news(&none).is_empty());
        assert_eq!(started.news(&starting).len(), 1);
        // With the hiding service there is nothing to warn of.
        assert!(
            view(Some(Phase::Running), Hiding::Service)
                .news(&view(None, Hiding::Service))
                .is_empty()
        );

        // A pad that can't rumble is named.
        let mut silent = view(Some(Phase::Running), Hiding::Service);
        silent.setups[0].sources[0].pad = Some("address:8bitdo".to_string());
        let news = silent.news(&view(None, Hiding::Service));
        assert_eq!(summaries(&news), ["couch: no rumble on 8BitDo Pro 2"]);
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
