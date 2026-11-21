//! The release check the client runs against the daemon. The SDK talks
//! only to a daemon of its own release line (the same `major.minor`, see
//! [`ctrlassist_protocol::compatible`]); the frozen `info` rpc tells it
//! the daemon's release before anything else is decoded.
//!
//! The answer holds for one lifetime of the daemon: it's cleared when
//! the daemon's liveliness token goes away, so a daemon restarted on
//! another release (a package upgrade under a running client) is checked
//! again. Within a lifetime the client keeps the whole `info` reply, so
//! the daemon's release is a mirror read.

use std::future::Future;
use std::sync::Mutex;

use ctrlassist_protocol::{VERSION, rpc};
use tokio::sync::broadcast;

use crate::{Error, Result, StateEvent};

/// What a client knows about the daemon's release.
#[derive(Debug, Default)]
pub(crate) struct Handshake {
    /// Serializes checks, so calls wait for one in flight instead of each
    /// asking. Never held while `known` is locked.
    checking: tokio::sync::Mutex<()>,
    /// The settled answer for the current lifetime. A plain mutex, never
    /// held across an await, so a reader always sees a settled answer.
    known: Mutex<Known>,
}

#[derive(Debug, Default)]
struct Known {
    /// Bumped each time the daemon goes away, so a check started in one
    /// lifetime can't settle the next one's answer.
    lifetime: u64,
    settled: Option<Settled>,
}

#[derive(Debug, Clone)]
struct Settled {
    info: rpc::ServiceInfo,
    compatible: bool,
}

impl Handshake {
    /// Check the daemon's release unless this lifetime already has an
    /// answer, then pass a compatible one and fail a mismatch. Calls wait
    /// for a check in flight, so nothing is sent to a daemon whose
    /// release isn't known yet. Settling announces
    /// [`StateEvent::ReleaseChecked`] on `events`, then
    /// [`StateEvent::VersionMismatch`] for a mismatch. A failure to ask
    /// (the daemon down, a timeout) isn't remembered: the next call asks
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
        let lifetime = {
            let known = self
                .known
                .lock()
                .expect("nothing panics while the handshake lock is held");
            if let Some(settled) = known.settled.clone() {
                return self.outcome(&settled);
            }
            known.lifetime
        };
        let info = info().await?;
        let settled = Settled {
            compatible: ctrlassist_protocol::compatible(VERSION, &info.version),
            info,
        };
        // Settle it only if the daemon hasn't gone away meanwhile; the
        // answer still stands for this call either way.
        let mut known = self
            .known
            .lock()
            .expect("nothing panics while the handshake lock is held");
        if known.lifetime == lifetime {
            known.settled = Some(settled.clone());
            for event in self.events(&settled) {
                let _ = events.send(event);
            }
        }
        drop(known);
        self.outcome(&settled)
    }

    /// The daemon's `info` reply for this lifetime, once a check settled.
    pub(crate) fn info(&self) -> Option<rpc::ServiceInfo> {
        self.settled().map(|settled| settled.info)
    }

    /// The current lifetime, which changes each time the daemon goes
    /// away.
    pub(crate) fn lifetime(&self) -> u64 {
        self.known
            .lock()
            .expect("nothing panics while the handshake lock is held")
            .lifetime
    }

    /// A mismatch already known, without asking: what the mirror reads
    /// respect. A check in flight counts as not known yet.
    pub(crate) fn known_mismatch(&self) -> Option<Error> {
        let settled = self.settled()?;
        (!settled.compatible).then(|| self.mismatch(&settled.info.version))
    }

    /// The announcements of a check already settled: what a watcher
    /// subscribing late replays, since they went out before it listened.
    pub(crate) fn settled_events(&self) -> Vec<StateEvent> {
        self.settled()
            .map(|settled| self.events(&settled))
            .unwrap_or_default()
    }

    /// The daemon went away: its next lifetime gets a fresh check. Doesn't
    /// wait for a check in flight; that check's answer is dropped.
    pub(crate) fn reset(&self) {
        let mut known = self
            .known
            .lock()
            .expect("nothing panics while the handshake lock is held");
        known.lifetime += 1;
        known.settled = None;
    }

    fn settled(&self) -> Option<Settled> {
        self.known
            .lock()
            .expect("nothing panics while the handshake lock is held")
            .settled
            .clone()
    }

    fn outcome(&self, settled: &Settled) -> Result<()> {
        if settled.compatible {
            Ok(())
        } else {
            Err(self.mismatch(&settled.info.version))
        }
    }

    fn events(&self, settled: &Settled) -> Vec<StateEvent> {
        let mut events = vec![StateEvent::ReleaseChecked];
        if !settled.compatible {
            events.push(StateEvent::VersionMismatch {
                ours: VERSION.to_owned(),
                theirs: settled.info.version.clone(),
            });
        }
        events
    }

    fn mismatch(&self, theirs: &str) -> Error {
        Error::VersionMismatch {
            ours: VERSION.to_owned(),
            theirs: theirs.to_owned(),
        }
    }
}
