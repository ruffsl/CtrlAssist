//! The pads the daemon knows: those connected now, and those it
//! remembers, so a profile can name a pad that isn't plugged in.
//!
//! This is the bookkeeping alone, with no I/O, so every rule about what
//! a pad is called and when it counts as connected is tested without a
//! pad.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ctrlassist_device::Found;
use ctrlassist_protocol::error::{ErrorCode, WireError};
use ctrlassist_protocol::{keys, payload};

/// What is remembered of a pad once it is unplugged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Known {
    /// What it is known by across reconnects.
    pub(crate) id: String,
    /// The pad's own name.
    pub(crate) name: String,
    pub(crate) vendor: u16,
    pub(crate) product: u16,
    /// Whether it could rumble when last asked.
    pub(crate) rumble: Option<bool>,
}

#[derive(Debug)]
struct Row {
    known: Known,
    /// The event nodes it is connected on now. Two pads that report
    /// nothing to tell them apart share a row and have a node each.
    nodes: Vec<PathBuf>,
}

/// What a change to the book touched.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Changed {
    /// The pad whose published state is now stale.
    pub(crate) id: String,
    /// Whether what is remembered changed, so the store is stale too.
    pub(crate) remembered: bool,
}

/// The known pads, by id.
#[derive(Debug, Default)]
pub(crate) struct Book {
    rows: BTreeMap<String, Row>,
}

impl Book {
    /// A book of remembered pads, none connected.
    pub(crate) fn new(known: Vec<Known>) -> Self {
        let rows = known
            .into_iter()
            .map(|known| {
                (
                    known.id.clone(),
                    Row {
                        known,
                        nodes: Vec::new(),
                    },
                )
            })
            .collect();
        Self { rows }
    }

    /// What to remember: every pad, connected or not.
    pub(crate) fn known(&self) -> Vec<Known> {
        self.rows.values().map(|row| row.known.clone()).collect()
    }

    /// Every pad's id.
    pub(crate) fn ids(&self) -> impl Iterator<Item = &str> {
        self.rows.keys().map(String::as_str)
    }

    /// A pad connected, or one already connected changed. `rumble` is
    /// whether it can, when that could be asked. The daemon's own virtual
    /// pads are never taken in. Returns each pad whose state changed.
    pub(crate) fn arrived(&mut self, found: &Found, rumble: Option<bool>) -> Vec<Changed> {
        if found.is_own_output() {
            return Vec::new();
        }
        let id = found.identifiers.id().to_string();
        // A node is one pad's at a time: one that now reports another
        // name has left the pad it was.
        let mut changed: Vec<Changed> = self
            .rows
            .iter_mut()
            .filter(|(held, _)| **held != id)
            .filter_map(|(held, row)| {
                let at = row.nodes.iter().position(|node| *node == found.node)?;
                row.nodes.remove(at);
                Some(Changed {
                    id: held.clone(),
                    remembered: false,
                })
            })
            .collect();
        let was = self.rows.get(&id).map(|row| &row.known);
        let known = Known {
            id: id.clone(),
            name: found.identifiers.name.clone(),
            vendor: found.identifiers.vendor,
            product: found.identifiers.product,
            // A pad that couldn't be asked this time keeps its last answer.
            rumble: rumble.or(was.and_then(|was| was.rumble)),
        };
        let remembered = was != Some(&known);
        let row = self.rows.entry(id.clone()).or_insert_with(|| Row {
            known: known.clone(),
            nodes: Vec::new(),
        });
        row.known = known;
        let connected = !row.nodes.contains(&found.node);
        if connected {
            row.nodes.push(found.node.clone());
        }
        if remembered || connected {
            changed.push(Changed { id, remembered });
        }
        changed
    }

    /// The pad on `node` has gone. Returns its id; it stays remembered.
    pub(crate) fn left(&mut self, node: &Path) -> Option<String> {
        self.rows.iter_mut().find_map(|(id, row)| {
            let at = row.nodes.iter().position(|held| held == node)?;
            row.nodes.remove(at);
            Some(id.clone())
        })
    }

    /// The id of the pad `reference` names: an id, an id's key segment, a
    /// bare address, or `model:<vendor>:<product>` when one pad fits.
    pub(crate) fn resolve(&self, reference: &str) -> Result<&str, WireError> {
        if let Some((id, _)) = self.rows.get_key_value(reference) {
            return Ok(id);
        }
        let address = format!("address:{reference}");
        if let Some((id, _)) = self.rows.get_key_value(&address) {
            return Ok(id);
        }
        let mut fits = self.rows.iter().filter(|(id, row)| {
            keys::pad_id(id) == reference
                || format!("model:{:04x}:{:04x}", row.known.vendor, row.known.product)
                    == reference.to_ascii_lowercase()
        });
        match (fits.next(), fits.next()) {
            (Some((id, _)), None) => Ok(id),
            (None, _) => Err(WireError::pad(
                ErrorCode::UnknownPad,
                format!("no known pad is called {reference}"),
            )),
            (Some(_), Some(_)) => Err(WireError::pad(
                ErrorCode::AmbiguousPad,
                format!("more than one known pad is {reference}; name one by its id"),
            )),
        }
    }

    /// Stop remembering the pad `reference` names. A connected pad can't
    /// be forgotten: it would be taken in again at once.
    pub(crate) fn forget(&mut self, reference: &str) -> Result<String, WireError> {
        let id = self.resolve(reference)?.to_string();
        if self.node(&id).is_some() {
            return Err(WireError::pad(
                ErrorCode::PadConnected,
                format!("{id} is connected; unplug it to forget it"),
            ));
        }
        self.rows.remove(&id);
        Ok(id)
    }

    /// A node the pad is connected on, if it is.
    pub(crate) fn node(&self, id: &str) -> Option<&Path> {
        self.rows.get(id)?.nodes.first().map(PathBuf::as_path)
    }

    /// Whether the pad could rumble when last asked.
    pub(crate) fn rumble(&self, id: &str) -> Option<bool> {
        self.rows.get(id)?.known.rumble
    }

    /// The pad's published state.
    pub(crate) fn pad(&self, id: &str) -> Option<payload::Pad> {
        let row = self.rows.get(id)?;
        Some(payload::Pad {
            id: row.known.id.clone(),
            name: row.known.name.clone(),
            vendor: row.known.vendor,
            product: row.known.product,
            connected: !row.nodes.is_empty(),
            // Nothing hides a pad or runs a setup yet.
            hidden: false,
            setup: None,
            rumble: row.known.rumble,
        })
    }
}

#[cfg(test)]
mod tests {
    use ctrlassist_device::Identifiers;

    use super::*;

    fn found(node: &str, name: &str, address: Option<&str>, port: Option<&str>) -> Found {
        Found {
            node: PathBuf::from(node),
            identifiers: Identifiers {
                name: name.to_string(),
                bus: 3,
                vendor: 0x054c,
                product: 0x0ce6,
                version: 1,
                address: address.map(str::to_string),
                serial: None,
                port: port.map(str::to_string),
            },
        }
    }

    const ADDRESS: &str = "0c:27:56:4f:bd:14";

    fn dualsense(node: &str) -> Found {
        found(node, "DualSense", Some(ADDRESS), Some("usb-1/input0"))
    }

    #[test]
    fn a_pad_is_remembered_once_seen_and_stays_when_it_leaves() {
        let mut book = Book::default();
        let id = format!("address:{ADDRESS}");
        assert_eq!(
            book.arrived(&dualsense("/dev/input/event5"), Some(true)),
            [Changed {
                id: id.clone(),
                remembered: true
            }]
        );
        let pad = book.pad(&id).unwrap();
        assert!(pad.connected);
        assert_eq!(
            (pad.name.as_str(), pad.vendor, pad.product),
            ("DualSense", 0x054c, 0x0ce6)
        );
        assert_eq!(pad.rumble, Some(true));
        assert!(!pad.hidden && pad.setup.is_none());
        assert_eq!(book.node(&id), Some(Path::new("/dev/input/event5")));
        assert_eq!(book.ids().collect::<Vec<_>>(), [id.as_str()]);

        // The same report again changes nothing.
        assert_eq!(
            book.arrived(&dualsense("/dev/input/event5"), Some(true)),
            []
        );

        assert_eq!(book.left(Path::new("/dev/input/event5")), Some(id.clone()));
        assert_eq!(book.left(Path::new("/dev/input/event5")), None);
        assert!(!book.pad(&id).unwrap().connected);
        assert_eq!(book.node(&id), None);
        assert_eq!(book.known().len(), 1);

        // Back on another node: connected again, nothing new to remember,
        // and a failed rumble question keeps the last answer.
        assert_eq!(
            book.arrived(&dualsense("/dev/input/event9"), None),
            [Changed {
                id: id.clone(),
                remembered: false
            }]
        );
        assert_eq!(book.rumble(&id), Some(true));
        assert_eq!(book.pad("address:none"), None);
        assert_eq!(book.rumble("address:none"), None);
    }

    #[test]
    fn a_remembered_pad_that_returns_changed_is_remembered_anew() {
        let known = Known {
            id: format!("address:{ADDRESS}"),
            name: "Wireless Controller".into(),
            vendor: 0x054c,
            product: 0x0ce6,
            rumble: None,
        };
        let mut book = Book::new(vec![known.clone()]);
        assert!(!book.pad(&known.id).unwrap().connected);
        let changed = book.arrived(&dualsense("/dev/input/event5"), Some(false));
        assert!(changed[0].remembered, "its name and rumble are news");
        assert_eq!(book.known()[0].name, "DualSense");
        assert_eq!(book.known()[0].rumble, Some(false));
    }

    #[test]
    fn our_own_virtual_pads_are_never_taken_in() {
        let mut book = Book::default();
        let own = found(
            "/dev/input/event7",
            "CtrlAssist Virtual Pad",
            None,
            Some("ctrlassist/virtual/1/0"),
        );
        assert_eq!(book.arrived(&own, Some(true)), []);
        assert!(book.known().is_empty());
    }

    #[test]
    fn two_pads_that_report_nothing_apart_share_a_row() {
        let mut book = Book::default();
        let first = found("/dev/input/event5", "Pad", None, None);
        let second = found("/dev/input/event6", "Pad", None, None);
        let id = book.arrived(&first, None).remove(0).id;
        assert_eq!(id, "model:054c:0ce6:Pad");
        assert_eq!(
            book.arrived(&second, None),
            [Changed {
                id: id.clone(),
                remembered: false
            }]
        );
        assert_eq!(book.known().len(), 1);
        book.left(Path::new("/dev/input/event5"));
        assert!(book.pad(&id).unwrap().connected, "one of the two is left");
        assert_eq!(book.node(&id), Some(Path::new("/dev/input/event6")));
        book.left(Path::new("/dev/input/event6"));
        assert!(!book.pad(&id).unwrap().connected);
    }

    /// A pad's address can arrive after the pad does: the node then
    /// belongs to the pad it turned out to be.
    #[test]
    fn a_node_that_reports_another_name_leaves_the_pad_it_was() {
        let mut book = Book::default();
        let early = found("/dev/input/event5", "DualSense", None, Some("usb-1/input0"));
        let was = book.arrived(&early, None).remove(0).id;
        let changed = book.arrived(&dualsense("/dev/input/event5"), None);
        assert_eq!(
            changed,
            [
                Changed {
                    id: was.clone(),
                    remembered: false
                },
                Changed {
                    id: format!("address:{ADDRESS}"),
                    remembered: true
                }
            ]
        );
        assert!(!book.pad(&was).unwrap().connected);
        assert!(book.pad(&format!("address:{ADDRESS}")).unwrap().connected);
    }

    #[test]
    fn a_pad_is_named_by_id_key_segment_address_or_model() {
        let mut book = Book::default();
        book.arrived(&dualsense("/dev/input/event5"), None);
        let id = format!("address:{ADDRESS}");
        for reference in [
            id.clone(),
            keys::pad_id(&id),
            ADDRESS.to_string(),
            "model:054c:0ce6".to_string(),
            "model:054C:0CE6".to_string(),
        ] {
            assert_eq!(book.resolve(&reference), Ok(id.as_str()), "{reference}");
        }
        let unknown = book.resolve("model:045e:0b12").unwrap_err();
        assert_eq!(unknown.code, ErrorCode::UnknownPad);
        assert!(unknown.message.contains("model:045e:0b12"));

        // A second pad of the model: the model alone no longer says which.
        let other = found(
            "/dev/input/event6",
            "DualSense",
            Some("aa:bb:cc:dd:ee:ff"),
            None,
        );
        book.arrived(&other, None);
        let ambiguous = book.resolve("model:054c:0ce6").unwrap_err();
        assert_eq!(ambiguous.code, ErrorCode::AmbiguousPad);
        assert_eq!(
            book.resolve("aa:bb:cc:dd:ee:ff"),
            Ok("address:aa:bb:cc:dd:ee:ff")
        );
    }

    #[test]
    fn only_a_pad_that_is_not_connected_can_be_forgotten() {
        let mut book = Book::default();
        book.arrived(&dualsense("/dev/input/event5"), None);
        let refused = book.forget(ADDRESS).unwrap_err();
        assert_eq!(refused.code, ErrorCode::PadConnected);
        book.left(Path::new("/dev/input/event5"));
        assert_eq!(book.forget(ADDRESS), Ok(format!("address:{ADDRESS}")));
        assert!(book.known().is_empty());
        assert_eq!(
            book.forget(ADDRESS).unwrap_err().code,
            ErrorCode::UnknownPad
        );
    }
}
