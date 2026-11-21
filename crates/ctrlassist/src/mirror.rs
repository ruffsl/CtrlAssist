//! The client-side mirror: every cached key a client renders from, kept
//! current by subscriber tasks and readable as a snapshot. Deltas surface
//! as [`StateEvent`](crate::StateEvent)s on a broadcast channel.

use std::collections::BTreeMap;

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
    /// The pads the daemon knows, by key segment.
    pub pads: BTreeMap<String, payload::Pad>,
    /// What a virtual pad can present itself as, by name.
    pub identities: BTreeMap<String, payload::Identity>,
    /// The profiles the daemon keeps, by name.
    pub profiles: BTreeMap<String, payload::Profile>,
    /// The setups that exist, by name.
    pub setups: BTreeMap<String, payload::Setup>,
}

/// Apply a sample to the row it is for; whether anything changed.
fn assign<T: serde::de::DeserializeOwned>(
    rows: &mut BTreeMap<String, T>,
    id: &str,
    sample: &Sample,
) -> bool {
    match sample.kind() {
        SampleKind::Put => match serde_json::from_slice(&sample.payload().to_bytes()) {
            Ok(row) => {
                rows.insert(id.to_string(), row);
                true
            }
            Err(_) => false,
        },
        SampleKind::Delete => rows.remove(id).is_some(),
    }
}

impl Mirror for DaemonMirror {
    fn keys() -> Vec<String> {
        vec![
            keys::daemon(),
            keys::pad_wild(),
            keys::identity_wild(),
            keys::profile_wild(),
            keys::setup_wild(),
        ]
    }

    fn clear(&mut self) -> Vec<StateEvent> {
        let mut removed: Vec<StateEvent> = self
            .daemon
            .take()
            .map(|_| StateEvent::DaemonStateChanged)
            .into_iter()
            .collect();
        removed.extend(
            std::mem::take(&mut self.pads)
                .into_values()
                .map(|pad| StateEvent::PadChanged { pad: pad.id }),
        );
        if !std::mem::take(&mut self.identities).is_empty() {
            removed.push(StateEvent::IdentitiesChanged);
        }
        removed.extend(
            std::mem::take(&mut self.profiles)
                .into_keys()
                .map(|profile| StateEvent::ProfileChanged { profile }),
        );
        removed.extend(
            std::mem::take(&mut self.setups)
                .into_keys()
                .map(|setup| StateEvent::SetupChanged { setup }),
        );
        removed
    }

    fn apply(&mut self, sample: &Sample) -> Option<StateEvent> {
        let key = sample.key_expr().as_str();
        if let Some(id) = keys::parse_pad(key) {
            // The event names the pad as a person would, which a delete
            // can only take from the row it removes.
            let was = self.pads.get(id).map(|pad| pad.id.clone());
            let changed = assign(&mut self.pads, id, sample);
            let pad = self.pads.get(id).map(|pad| pad.id.clone()).or(was)?;
            return changed.then_some(StateEvent::PadChanged { pad });
        }
        if let Some(id) = keys::parse_identity(key) {
            return assign(&mut self.identities, id, sample)
                .then_some(StateEvent::IdentitiesChanged);
        }
        if let Some(name) = keys::parse_profile(key) {
            return assign(&mut self.profiles, name, sample).then(|| StateEvent::ProfileChanged {
                profile: name.to_string(),
            });
        }
        if let Some(name) = keys::parse_setup(key) {
            return assign(&mut self.setups, name, sample).then(|| StateEvent::SetupChanged {
                setup: name.to_string(),
            });
        }
        if key != keys::daemon() {
            return None;
        }
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

    fn put_on(key: String, body: &impl serde::Serialize) -> Sample {
        let key = KeyExpr::try_from(key).expect("a state key is a valid key expression");
        SampleBuilder::put(key, serde_json::to_vec(body).expect("a payload serializes")).into()
    }

    fn delete_on(key: String) -> Sample {
        let key = KeyExpr::try_from(key).expect("a state key is a valid key expression");
        SampleBuilder::delete(key).into()
    }

    fn pad(id: &str, connected: bool) -> payload::Pad {
        payload::Pad {
            id: id.to_string(),
            name: "Pad".to_string(),
            vendor: 1,
            product: 2,
            connected,
            hidden: false,
            setup: None,
            rumble: None,
        }
    }

    #[test]
    fn pads_and_identities_are_kept_by_key_and_dropped_with_the_daemon() {
        let mut mirror = DaemonMirror::default();
        let key = keys::pad(&keys::pad_id("address:aa"));
        let changed = |pad: &str| {
            Some(StateEvent::PadChanged {
                pad: pad.to_string(),
            })
        };

        assert_eq!(
            mirror.apply(&put_on(key.clone(), &pad("address:aa", true))),
            changed("address:aa")
        );
        assert_eq!(
            mirror.apply(&put_on(key.clone(), &pad("address:aa", false))),
            changed("address:aa")
        );
        assert!(!mirror.pads[&keys::pad_id("address:aa")].connected);
        // A body this release can't read, and a delete of a row it never
        // had, change nothing.
        assert_eq!(mirror.apply(&put_on(key.clone(), &"nonsense")), None);
        assert_eq!(
            mirror.apply(&delete_on(keys::pad("0000000000000000"))),
            None
        );
        assert_eq!(mirror.pads.len(), 1);
        assert_eq!(mirror.apply(&delete_on(key.clone())), changed("address:aa"));
        assert!(mirror.pads.is_empty());

        let xbox = payload::Identity {
            id: "xbox".to_string(),
            name: "Xbox".to_string(),
            vendor: 0x045e,
            product: 0x0b12,
            available: true,
        };
        assert_eq!(
            mirror.apply(&put_on(keys::identity("xbox"), &xbox)),
            Some(StateEvent::IdentitiesChanged)
        );
        assert_eq!(mirror.identities["xbox"], xbox);
        // A key the mirror doesn't know is not the daemon's own state.
        assert_eq!(
            mirror.apply(&put_on("ctrlassist/later/x".to_string(), &1)),
            None
        );

        let couch = payload::Profile {
            name: "couch".to_string(),
            toml: "preset = \"priority\"".to_string(),
            problem: None,
        };
        let profile_changed = Some(StateEvent::ProfileChanged {
            profile: "couch".to_string(),
        });
        assert_eq!(
            mirror.apply(&put_on(keys::profile("couch"), &couch)),
            profile_changed
        );
        assert_eq!(mirror.profiles["couch"], couch);
        assert_eq!(
            mirror.apply(&delete_on(keys::profile("couch"))),
            profile_changed
        );
        assert_eq!(mirror.apply(&delete_on(keys::profile("couch"))), None);
        mirror.apply(&put_on(keys::profile("couch"), &couch));

        let running = payload::Setup {
            name: "couch".to_string(),
            phase: payload::Phase::Running,
            reason: None,
            sources: Vec::new(),
            sinks: Vec::new(),
            nodes: Vec::new(),
        };
        let setup_changed = Some(StateEvent::SetupChanged {
            setup: "couch".to_string(),
        });
        assert_eq!(
            mirror.apply(&put_on(keys::setup("couch"), &running)),
            setup_changed
        );
        assert_eq!(mirror.setups["couch"], running);

        mirror.apply(&put_on(key, &pad("address:aa", true)));
        assert_eq!(
            mirror.clear(),
            [
                StateEvent::PadChanged {
                    pad: "address:aa".to_string()
                },
                StateEvent::IdentitiesChanged,
                StateEvent::ProfileChanged {
                    profile: "couch".to_string()
                },
                StateEvent::SetupChanged {
                    setup: "couch".to_string()
                },
            ]
        );
        assert!(mirror.setups.is_empty());
        assert!(mirror.pads.is_empty() && mirror.identities.is_empty());
        assert!(mirror.profiles.is_empty());
    }

    #[test]
    fn the_daemon_s_state_is_kept_as_last_published() {
        let mut mirror = DaemonMirror::default();
        assert!(mirror.clear().is_empty(), "nothing was showing");

        let event = mirror.apply(&put(
            r#"{"version":"0.5.0","realtime":true,"hiding":"service"}"#,
        ));
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
        mirror.apply(&put(
            r#"{"version":"0.5.0","realtime":false,"hiding":"grab"}"#,
        ));
        let removed = mirror.clear();
        assert!(matches!(removed[..], [StateEvent::DaemonStateChanged]));
        assert!(mirror.daemon.is_none());
    }
}
