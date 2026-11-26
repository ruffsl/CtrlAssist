//! The release check the client runs against the daemon. The SDK talks
//! only to a daemon of its own release line (the same `major.minor`, see
//! [`ctrlassist_protocol::compatible`]); the frozen `info` rpc tells it
//! the daemon's release before anything else is decoded.
//!
//! The answer holds for one lifetime of the daemon, from its liveliness
//! token appearing to its going, and is kept only while the token is
//! present: a daemon that has withdrawn its token may still answer while
//! it drains, and what it says mustn't stand for the daemon that comes
//! next (a package upgrade under a running client). A call made while no
//! token is present still gets its answer, but the lifetime's is settled
//! only by a check made within it. Within a lifetime the client keeps
//! the whole `info` reply, so the daemon's release is a mirror read.
//!
//! The service's liveliness edges are announced here too, under the
//! same lock as a settled check, so a watcher hears them in the order
//! they happened: a check's [`StateEvent::ReleaseChecked`] always comes
//! after its lifetime's appearance and before its going.

use std::future::Future;
use std::sync::Mutex;

use ctrlassist_protocol::{VERSION, rpc};
use tokio::sync::broadcast;

use crate::{Error, Result, Service, StateEvent};

/// What a client knows about its service's release.
#[derive(Debug)]
pub(crate) struct Handshake {
    service: Service,
    /// Serializes checks, so calls wait for one in flight instead of each
    /// asking. Never held while `known` is locked.
    checking: tokio::sync::Mutex<()>,
    /// The settled answer for the current lifetime. A plain mutex, never
    /// held across an await, so a reader always sees a settled answer.
    known: Mutex<Known>,
}

#[derive(Debug, Default)]
struct Known {
    /// Bumped each time the service's token appears or goes, so a check
    /// started in one lifetime, or between two, can't settle the next
    /// one's answer.
    lifetime: u64,
    /// Whether the service's token is present: the only time an answer
    /// is kept.
    alive: bool,
    settled: Option<Settled>,
}

#[derive(Debug, Clone)]
struct Settled {
    info: rpc::ServiceInfo,
    compatible: bool,
}

impl Handshake {
    pub(crate) fn new(service: Service) -> Self {
        Self {
            service,
            checking: tokio::sync::Mutex::new(()),
            known: Mutex::new(Known::default()),
        }
    }

    /// Check the service's release unless this lifetime already has an
    /// answer, then pass a compatible one and fail a mismatch. Calls wait
    /// for a check in flight, so nothing is sent to a daemon whose
    /// release isn't known yet. Settling announces
    /// [`StateEvent::ReleaseChecked`] on `events`, then
    /// [`StateEvent::VersionMismatch`] for a mismatch. A failure to ask
    /// (the daemon down, a timeout) isn't remembered, nor is an answer
    /// got while the service's token isn't present: the next call asks
    /// again.
    pub(crate) async fn ensure<F, Fut>(
        &self,
        events: &broadcast::Sender<StateEvent>,
        info: F,
    ) -> Result<()>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<rpc::ServiceInfo>>,
    {
        if let Some(settled) = self.settled() {
            return self.outcome(&settled);
        }
        let _checking = self.checking.lock().await;
        let (lifetime, alive) = {
            let known = self.lock();
            if let Some(settled) = known.settled.clone() {
                return self.outcome(&settled);
            }
            (known.lifetime, known.alive)
        };
        let info = info().await?;
        let settled = Settled {
            compatible: ctrlassist_protocol::compatible(VERSION, &info.version),
            info,
        };
        // Settle it only if the token was present when the check began
        // and hasn't gone or come since; the answer still stands for this
        // call either way.
        let mut known = self.lock();
        if alive && known.lifetime == lifetime {
            known.settled = Some(settled.clone());
            for event in self.events(&settled) {
                let _ = events.send(event);
            }
        }
        drop(known);
        self.outcome(&settled)
    }

    /// The service's token appeared: a lifetime begins, with no answer
    /// yet, and is announced. Returns the lifetime, at whose change a
    /// check that retries stops.
    pub(crate) fn appeared(&self, events: &broadcast::Sender<StateEvent>) -> u64 {
        let mut known = self.lock();
        known.lifetime += 1;
        known.alive = true;
        known.settled = None;
        let _ = events.send(appeared(self.service));
        known.lifetime
    }

    /// The service's token went: its answer goes before the going is
    /// announced, so a reader acting on the announcement sees none.
    /// Doesn't wait for a check in flight; that check's answer is
    /// dropped.
    pub(crate) fn disappeared(&self, events: &broadcast::Sender<StateEvent>) {
        let mut known = self.lock();
        known.lifetime += 1;
        known.alive = false;
        known.settled = None;
        let _ = events.send(disappeared(self.service));
    }

    /// The daemon's `info` reply for this lifetime, once a check settled.
    pub(crate) fn info(&self) -> Option<rpc::ServiceInfo> {
        self.settled().map(|settled| settled.info)
    }

    /// The current lifetime, which changes each time the service's token
    /// appears or goes.
    pub(crate) fn lifetime(&self) -> u64 {
        self.lock().lifetime
    }

    /// A mismatch already known, without asking: what the mirror reads
    /// respect. A check in flight counts as not known yet.
    pub(crate) fn known_mismatch(&self) -> Option<Error> {
        let settled = self.settled()?;
        (!settled.compatible).then(|| self.mismatch(&settled.info.version))
    }

    /// A receiver of `events` from now on, and what was announced before
    /// it that still holds: the appearance of a service that is up, and
    /// its settled check. Taken under the lock those announcements are
    /// sent under, so each reaches the receiver once, either here or as
    /// it is sent.
    pub(crate) fn subscribe(
        &self,
        events: &broadcast::Sender<StateEvent>,
    ) -> (broadcast::Receiver<StateEvent>, Vec<StateEvent>) {
        let known = self.lock();
        let receiver = events.subscribe();
        let mut replay = Vec::new();
        if known.alive {
            replay.push(appeared(self.service));
            if let Some(settled) = &known.settled {
                replay.extend(self.events(settled));
            }
        }
        (receiver, replay)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Known> {
        self.known
            .lock()
            .expect("nothing panics while the handshake lock is held")
    }

    fn settled(&self) -> Option<Settled> {
        self.lock().settled.clone()
    }

    fn outcome(&self, settled: &Settled) -> Result<()> {
        if settled.compatible {
            Ok(())
        } else {
            Err(self.mismatch(&settled.info.version))
        }
    }

    fn events(&self, settled: &Settled) -> Vec<StateEvent> {
        let mut events = vec![StateEvent::ReleaseChecked(self.service)];
        if !settled.compatible {
            events.push(StateEvent::VersionMismatch {
                service: self.service,
                ours: VERSION.to_owned(),
                theirs: settled.info.version.clone(),
            });
        }
        events
    }

    fn mismatch(&self, theirs: &str) -> Error {
        Error::VersionMismatch {
            service: self.service,
            ours: VERSION.to_owned(),
            theirs: theirs.to_owned(),
        }
    }
}

fn appeared(service: Service) -> StateEvent {
    match service {
        Service::Daemon => StateEvent::DaemonAppeared,
        Service::Tray => StateEvent::TrayAppeared,
    }
}

fn disappeared(service: Service) -> StateEvent {
    match service {
        Service::Daemon => StateEvent::DaemonDisappeared,
        Service::Tray => StateEvent::TrayDisappeared,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(version: &str) -> rpc::ServiceInfo {
        rpc::ServiceInfo {
            version: version.to_string(),
            keys: Vec::new(),
            config: String::new(),
        }
    }

    fn drain(events: &mut broadcast::Receiver<StateEvent>) -> Vec<StateEvent> {
        std::iter::from_fn(|| events.try_recv().ok()).collect()
    }

    /// A daemon that has withdrawn its token still answers while it
    /// drains: what it says passes for that call, and isn't kept for the
    /// daemon that comes next, which is checked afresh.
    #[tokio::test]
    async fn an_answer_got_with_no_token_is_not_kept_for_the_next_daemon() {
        let handshake = Handshake::new(Service::Daemon);
        let (events, mut heard) = broadcast::channel(16);
        handshake.appeared(&events);
        handshake.disappeared(&events);
        // The old daemon, draining, on another release.
        let old = handshake
            .ensure(&events, || async { Ok(info("99.0.0")) })
            .await;
        assert!(matches!(old, Err(Error::VersionMismatch { .. })));
        assert!(handshake.info().is_none(), "nothing settled with no token");
        assert!(handshake.known_mismatch().is_none());

        // The new daemon, on this release: its own check settles.
        handshake.appeared(&events);
        assert!(
            handshake.info().is_none(),
            "the new lifetime starts unchecked"
        );
        handshake
            .ensure(&events, || async { Ok(info(VERSION)) })
            .await
            .unwrap();
        assert_eq!(handshake.info().unwrap().version, VERSION);
        assert_eq!(
            drain(&mut heard),
            [
                StateEvent::DaemonAppeared,
                StateEvent::DaemonDisappeared,
                StateEvent::DaemonAppeared,
                StateEvent::ReleaseChecked(Service::Daemon),
            ],
            "a check settles only after its lifetime's appearance"
        );
    }

    /// A check that began before the token appeared doesn't settle the
    /// lifetime that began while it was asking.
    #[tokio::test]
    async fn a_check_begun_before_the_token_does_not_settle_its_lifetime() {
        let handshake = Handshake::new(Service::Daemon);
        let (events, _heard) = broadcast::channel(16);
        handshake
            .ensure(&events, || async {
                handshake.appeared(&events);
                Ok(info(VERSION))
            })
            .await
            .unwrap();
        assert!(handshake.info().is_none());
    }

    /// What a late subscriber is handed and what it then hears add up to
    /// each announcement once.
    #[tokio::test]
    async fn a_late_subscriber_hears_each_announcement_once() {
        let handshake = Handshake::new(Service::Daemon);
        let (events, _heard) = broadcast::channel(16);
        let (_, replay) = handshake.subscribe(&events);
        assert!(replay.is_empty(), "nothing is up");

        handshake.appeared(&events);
        handshake
            .ensure(&events, || async { Ok(info("99.0.0")) })
            .await
            .unwrap_err();
        let (mut late, replay) = handshake.subscribe(&events);
        assert_eq!(
            replay,
            [
                StateEvent::DaemonAppeared,
                StateEvent::ReleaseChecked(Service::Daemon),
                StateEvent::VersionMismatch {
                    service: Service::Daemon,
                    ours: VERSION.to_string(),
                    theirs: "99.0.0".to_string(),
                },
            ]
        );
        assert!(drain(&mut late).is_empty(), "not heard again");
        handshake.disappeared(&events);
        assert_eq!(drain(&mut late), [StateEvent::DaemonDisappeared]);
        let (_, replay) = handshake.subscribe(&events);
        assert!(replay.is_empty(), "nothing holds once it went");
    }
}
