//! The client-side mirror: every cached key a client renders from, kept
//! current by subscriber tasks and readable as a snapshot. Deltas surface
//! as [`StateEvent`](crate::StateEvent)s on a broadcast channel.

use ctrlassist_protocol::{keys, payload};
use zenoh::sample::{Sample, SampleKind};

use crate::StateEvent;
use crate::lifetime::Mirror;

/// What the daemon publishes.
#[derive(Debug, Clone, Default)]
pub(crate) struct DaemonMirror {
    /// The daemon's own state, as last published. Absent until it first
    /// arrives, and while the daemon is down.
    pub daemon: Option<payload::Daemon>,
}

impl Mirror for DaemonMirror {
    fn keys() -> Vec<String> {
        vec![keys::daemon()]
    }

    fn clear(&mut self) -> Vec<StateEvent> {
        self.daemon
            .take()
            .map(|_| StateEvent::DaemonStateChanged)
            .into_iter()
            .collect()
    }

    fn apply(&mut self, sample: &Sample) -> Option<StateEvent> {
        self.daemon = match sample.kind() {
            SampleKind::Put => Some(serde_json::from_slice(&sample.payload().to_bytes()).ok()?),
            SampleKind::Delete => None,
        };
        Some(StateEvent::DaemonStateChanged)
    }
}

#[cfg(test)]
mod tests {
    use zenoh::key_expr::KeyExpr;
    use zenoh::sample::SampleBuilder;

    use super::*;

    fn key() -> KeyExpr<'static> {
        KeyExpr::try_from(keys::daemon()).expect("the daemon's key is a valid key expression")
    }

    fn put(body: &str) -> Sample {
        SampleBuilder::put(key(), body.as_bytes().to_vec()).into()
    }

    #[test]
    fn the_daemon_s_state_is_kept_as_last_published() {
        let mut mirror = DaemonMirror::default();
        assert!(mirror.clear().is_empty(), "nothing was showing");

        let event = mirror.apply(&put(r#"{"version":"0.5.0","realtime":true}"#));
        assert!(matches!(event, Some(StateEvent::DaemonStateChanged)));
        assert!(mirror.daemon.as_ref().is_some_and(|state| state.realtime));

        // A body this release can't read changes nothing.
        assert!(mirror.apply(&put("[]")).is_none());
        assert!(mirror.daemon.is_some());

        let delete: Sample = SampleBuilder::delete(key()).into();
        assert!(mirror.apply(&delete).is_some());
        assert!(mirror.daemon.is_none());
    }

    #[test]
    fn clearing_reports_what_was_showing() {
        let mut mirror = DaemonMirror::default();
        mirror.apply(&put(r#"{"version":"0.5.0","realtime":false}"#));
        let removed = mirror.clear();
        assert!(matches!(removed[..], [StateEvent::DaemonStateChanged]));
        assert!(mirror.daemon.is_none());
    }
}
