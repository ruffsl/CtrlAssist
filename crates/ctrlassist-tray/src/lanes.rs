//! The lanes the menu's clicks are taken up on: what a click asks of one
//! profile or one pad waits for what was asked of it before, and for
//! nothing else, so that an answer that takes long holds up nothing but
//! what has to come after it.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, VecDeque};
use std::ops::ControlFlow;

use tokio::sync::mpsc;
use tokio::task::JoinSet;
use tracing::warn;

use crate::tray::Action;

/// Where a click is taken up beside the others: on a lane, in turn with
/// what was asked on it before. What waits on an answer holds its own
/// lane and no other, so a start that takes long holds neither another
/// profile's nor a click that asks nothing of the daemon.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Lane {
    /// A profile's, and its setup's: started, stopped, changed and
    /// handed over in the order asked.
    Profile(String),
    /// A pad's: buzzed and forgotten in the order asked.
    Pad(String),
    /// The profile last used, kept in the order it was aimed at.
    Target,
}

impl Action {
    /// The lane the action waits its turn on, or none for one taken up
    /// at once: quitting and opening the window.
    pub(crate) fn lane(&self) -> Option<Lane> {
        match self {
            Self::Start(name) | Self::Stop(name) | Self::Put { name, .. } => {
                Some(Lane::Profile(name.clone()))
            }
            Self::Select { setup, .. } => Some(Lane::Profile(setup.clone())),
            Self::Identify(pad) | Self::Forget(pad) => Some(Lane::Pad(pad.clone())),
            Self::Aim(_) => Some(Lane::Target),
            Self::Settings | Self::Quit => None,
        }
    }
}

/// The lanes with something under way, each with what waits behind it.
#[derive(Debug, Default)]
pub(crate) struct Lanes {
    busy: BTreeMap<Lane, VecDeque<Action>>,
}

impl Lanes {
    /// Takes `action` onto `lane`: handed back to be started now when
    /// nothing is under way there, and queued behind what is otherwise.
    pub(crate) fn take(&mut self, lane: Lane, action: Action) -> Option<Action> {
        match self.busy.entry(lane) {
            Entry::Occupied(mut waiting) => {
                waiting.get_mut().push_back(action);
                None
            }
            Entry::Vacant(idle) => {
                idle.insert(VecDeque::new());
                Some(action)
            }
        }
    }

    /// What was under way on `lane` is done: the next to start there, or
    /// none, the lane then being idle.
    pub(crate) fn done(&mut self, lane: &Lane) -> Option<Action> {
        let waiting = self.busy.get_mut(lane)?;
        let next = waiting.pop_front();
        if next.is_none() {
            self.busy.remove(lane);
        }
        next
    }
}

/// Takes the clicks up as they come: one with no lane at once, by `now`,
/// which says whether to go on; any other by `job`, on its lane, after
/// what was asked there before it and beside what is under way on the
/// others. Ends when `now` says to or the clicks end, dropping what is
/// under way.
pub(crate) async fn dispatch<F>(
    mut clicked: mpsc::UnboundedReceiver<Action>,
    mut now: impl FnMut(Action) -> ControlFlow<()>,
    job: impl Fn(Action) -> F,
) where
    F: Future<Output = ()> + Send + 'static,
{
    let mut lanes = Lanes::default();
    let mut running = JoinSet::new();
    let mut on: BTreeMap<tokio::task::Id, Lane> = BTreeMap::new();
    loop {
        tokio::select! {
            action = clicked.recv() => {
                let Some(action) = action else {
                    return;
                };
                let Some(lane) = action.lane() else {
                    if now(action).is_break() {
                        return;
                    }
                    continue;
                };
                if let Some(action) = lanes.take(lane.clone(), action) {
                    on.insert(running.spawn(job(action)).id(), lane);
                }
            }
            Some(finished) = running.join_next_with_id() => {
                let id = match finished {
                    Ok((id, ())) => id,
                    Err(error) => {
                        warn!(%error, "an action from the menu ended early");
                        error.id()
                    }
                };
                if let Some(lane) = on.remove(&id)
                    && let Some(next) = lanes.done(&lane)
                {
                    on.insert(running.spawn(job(next)).id(), lane);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tokio::sync::oneshot;

    use super::*;

    fn start(name: &str) -> Action {
        Action::Start(name.to_string())
    }

    fn put(name: &str) -> Action {
        Action::Put {
            name: name.to_string(),
            toml: String::new(),
        }
    }

    /// What a profile is asked, a pad is asked and the target kept each
    /// have a lane of their own; quitting and opening the window none.
    #[test]
    fn each_profile_and_each_pad_has_a_lane_and_quit_and_settings_none() {
        let couch = Some(Lane::Profile("couch".to_string()));
        for action in [
            start("couch"),
            Action::Stop("couch".to_string()),
            put("couch"),
            Action::Select {
                setup: "couch".to_string(),
                node: "mux".to_string(),
                choice: "assist".to_string(),
            },
        ] {
            assert_eq!(action.lane(), couch, "{action:?}");
        }
        assert_eq!(start("desk").lane(), Some(Lane::Profile("desk".into())));
        let pad = Some(Lane::Pad("couch".to_string()));
        assert_eq!(Action::Identify("couch".into()).lane(), pad);
        assert_eq!(Action::Forget("couch".into()).lane(), pad);
        assert_eq!(Action::Aim("couch".into()).lane(), Some(Lane::Target));
        assert_eq!(Action::Quit.lane(), None);
        assert_eq!(Action::Settings.lane(), None);
    }

    /// On one lane each action waits for the one before; the others
    /// don't wait for it.
    #[test]
    fn a_lane_keeps_its_order_and_holds_no_other() {
        let mut lanes = Lanes::default();
        let couch = Lane::Profile("couch".to_string());
        let desk = Lane::Profile("desk".to_string());
        assert_eq!(lanes.take(couch.clone(), put("couch")), Some(put("couch")));
        assert_eq!(lanes.take(couch.clone(), start("couch")), None);
        assert_eq!(lanes.take(couch.clone(), put("couch")), None);
        assert_eq!(lanes.take(desk.clone(), start("desk")), Some(start("desk")));
        assert_eq!(lanes.done(&couch), Some(start("couch")));
        assert_eq!(lanes.done(&couch), Some(put("couch")));
        assert_eq!(lanes.done(&couch), None);
        // Idle again: the next is started at once.
        assert_eq!(
            lanes.take(couch.clone(), start("couch")),
            Some(start("couch"))
        );
        assert_eq!(lanes.done(&desk), None);
        assert_eq!(lanes.done(&desk), None, "nothing under way there");
    }

    /// A start that is never answered holds the picks and the stop after
    /// it on its profile, and nothing else: another profile's start,
    /// aiming, opening the window and quitting all go on.
    #[tokio::test]
    async fn a_start_never_answered_holds_only_what_comes_after_it_on_its_profile() {
        let (clicks, clicked) = mpsc::unbounded_channel();
        let done: Arc<Mutex<Vec<String>>> = Arc::default();
        let now_seen: Arc<Mutex<Vec<String>>> = Arc::default();
        // The answer to couch's first start, held until the test sends.
        let (answer, answered) = oneshot::channel::<()>();
        let answered = Arc::new(Mutex::new(Some(answered)));
        let (all_done, mut finished) = mpsc::unbounded_channel::<()>();
        let job = {
            let done = Arc::clone(&done);
            move |action: Action| {
                let wait = matches!(&action, Action::Start(name) if name == "couch")
                    .then(|| answered.lock().unwrap().take())
                    .flatten();
                let (done, all_done) = (Arc::clone(&done), all_done.clone());
                async move {
                    if let Some(wait) = wait {
                        let _ = wait.await;
                    }
                    done.lock().unwrap().push(action.named());
                    let _ = all_done.send(());
                }
            }
        };
        let now = {
            let now_seen = Arc::clone(&now_seen);
            move |action: Action| {
                now_seen.lock().unwrap().push(action.named());
                if action == Action::Quit {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            }
        };
        let dispatcher = tokio::spawn(dispatch(clicked, now, job));
        for action in [
            start("couch"),
            put("couch"),
            Action::Stop("couch".into()),
            start("desk"),
            Action::Aim("desk".into()),
            Action::Settings,
            Action::Identify("pad".into()),
        ] {
            clicks.send(action).unwrap();
        }
        // Three are done while couch's start waits.
        for _ in 0..3 {
            finished.recv().await.unwrap();
        }
        let mut seen = done.lock().unwrap().clone();
        seen.sort();
        assert_eq!(seen, ["aim at desk", "identify pad", "start desk"]);
        assert_eq!(*now_seen.lock().unwrap(), ["open the settings window"]);
        // Answered: what came after it on couch, in order.
        answer.send(()).unwrap();
        for _ in 0..3 {
            finished.recv().await.unwrap();
        }
        assert_eq!(
            done.lock().unwrap()[3..],
            ["start couch", "keep couch as picked", "stop couch"]
        );
        // Quit ends it, with nothing under way.
        clicks.send(Action::Quit).unwrap();
        dispatcher.await.unwrap();
        assert_eq!(now_seen.lock().unwrap().last().unwrap(), "quit");
    }

    /// Quit is taken up while a start is still unanswered, which is
    /// dropped with the rest.
    #[tokio::test]
    async fn quit_does_not_wait_for_a_start() {
        let (clicks, clicked) = mpsc::unbounded_channel();
        let job = |_action: Action| std::future::pending::<()>();
        let now = |action: Action| {
            if action == Action::Quit {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        };
        let dispatcher = tokio::spawn(dispatch(clicked, now, job));
        clicks.send(start("couch")).unwrap();
        clicks.send(Action::Quit).unwrap();
        dispatcher.await.unwrap();
    }
}
