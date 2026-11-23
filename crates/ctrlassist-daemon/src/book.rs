//! The pads the daemon knows: those connected now, and those it
//! remembers, so a profile can name a pad that isn't plugged in.
//!
//! This is the bookkeeping alone, with no I/O, so every rule about what
//! a pad is called and when it counts as connected is tested without a
//! pad.
//!
//! **A pad is unplugged when its [anchor](Anchor) goes, not its node.**
//! Hiding a pad and showing it again re-bind its driver, which removes
//! its nodes and makes new ones and never removes its anchor, whoever
//! asked for the re-bind. So a node gone while its anchor is still there
//! is the pad away: nothing is published, and it is still connected. A
//! node come on that anchor is the pad back, which isn't a pad plugged
//! in. The anchor's own removal, which udev sends after its nodes', is
//! the pad unplugged; and a node on an anchor the book hasn't seen is a
//! pad plugged in, as is a pad away as the daemon starts, which has none
//! yet. A pad whose anchor couldn't be read is its own anchor: its node
//! going is its unplugging. A re-bind no driver comes back from leaves
//! the pad away, and so connected, until its anchor goes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ctrlassist_device::{Anchor, Found};
use ctrlassist_protocol::error::{ErrorCode, WireError};
use ctrlassist_protocol::profile::PadRef;
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
    /// The event nodes it is connected on now, each with its anchor. Two
    /// pads that report nothing to tell them apart share a row and have
    /// a node each, and an anchor each.
    nodes: Vec<(PathBuf, Option<Anchor>)>,
    /// The anchor of each of its nodes gone while that anchor is still
    /// there: it is away, and comes back on one.
    away: Vec<Anchor>,
    /// Whether it has just come back on one, until asked.
    back: bool,
    /// When it last connected, as a count of connections: lower is
    /// earlier.
    connected: u64,
    /// The bus it last connected on, as the kernel numbers them.
    bus: u16,
    /// The setup that uses it.
    setup: Option<String>,
    /// Whether the hiding service has it hidden.
    hidden: bool,
    /// How many showings of it again are under way: it is published as
    /// hidden until each is done.
    showing: u32,
}

impl Row {
    fn remembered(known: Known) -> Self {
        Self {
            known,
            nodes: Vec::new(),
            away: Vec::new(),
            back: false,
            connected: 0,
            bus: 0,
            setup: None,
            hidden: false,
            showing: 0,
        }
    }

    /// Whether the pad is there, as it is published: one away hasn't
    /// left.
    fn connected(&self) -> bool {
        !self.nodes.is_empty() || !self.away.is_empty()
    }

    /// Notes that no setup uses the pad any more, nor hides it.
    fn let_go(&mut self) {
        self.setup = None;
        self.hidden = false;
    }
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
    /// How many connections have been seen.
    connections: u64,
}

impl Book {
    /// A book of remembered pads, none connected.
    pub(crate) fn new(known: Vec<Known>) -> Self {
        let rows = known
            .into_iter()
            .map(|known| (known.id.clone(), Row::remembered(known)))
            .collect();
        Self {
            rows,
            connections: 0,
        }
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
        let mut changed = Vec::new();
        let mut back = false;
        for (held, row) in &mut self.rows {
            let was = row.connected();
            // A node is one pad's at a time: one that now reports another
            // name has left the pad it was.
            if *held != id {
                row.nodes.retain(|(node, _)| *node != found.node);
            }
            // Its anchor, if a node of it is away: back, under whatever
            // name it now reports. One at the same place, enumerated
            // afresh, is another device, whose going wasn't heard.
            if let Some(anchor) = &found.anchor {
                row.away.retain(|away| {
                    back |= away == anchor;
                    away.path != anchor.path
                });
            }
            if *held != id && was != row.connected() {
                changed.push(Changed {
                    id: held.clone(),
                    remembered: false,
                });
            }
        }
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
        let row = self
            .rows
            .entry(id.clone())
            .or_insert_with(|| Row::remembered(known.clone()));
        row.known = known;
        row.back = back;
        let connected = !row.nodes.iter().any(|(node, _)| *node == found.node);
        if connected {
            if row.nodes.is_empty() && !back {
                self.connections += 1;
                row.connected = self.connections;
            }
            row.nodes.push((found.node.clone(), found.anchor.clone()));
            row.bus = found.identifiers.bus;
        }
        if remembered || connected {
            changed.push(Changed { id, remembered });
        }
        changed
    }

    /// The pad on `node` has gone. Returns its id if it is unplugged; it
    /// stays remembered. Nothing for a pad whose anchor is still there,
    /// which is away.
    pub(crate) fn left(&mut self, node: &Path) -> Option<String> {
        self.rows.iter_mut().find_map(|(id, row)| {
            let at = row.nodes.iter().position(|(held, _)| held == node)?;
            match row.nodes.remove(at).1 {
                Some(anchor) => {
                    row.away.push(anchor);
                    None
                }
                None => Some(id.clone()),
            }
        })
    }

    /// The device at `path` has gone, which unplugs each pad anchored to
    /// it or to a device under it: its nodes have gone before it. Returns
    /// each pad unplugged, once: what was anchored there is no more.
    pub(crate) fn unplugged(&mut self, path: &Path) -> Vec<String> {
        let under = |anchor: &Anchor| anchor.path.starts_with(path);
        self.rows
            .iter_mut()
            .filter_map(|(id, row)| {
                let (nodes, away) = (row.nodes.len(), row.away.len());
                row.away.retain(|anchor| !under(anchor));
                row.nodes
                    .retain(|(_, anchor)| !anchor.as_ref().is_some_and(under));
                let gone = (nodes, away) != (row.nodes.len(), row.away.len());
                gone.then(|| id.clone())
            })
            .collect()
    }

    /// Notes that the hiding service is about to be asked to show the
    /// pad again: it is published as hidden until that is
    /// [done](Self::shown).
    pub(crate) fn showing(&mut self, id: &str) {
        if let Some(row) = self.rows.get_mut(id) {
            row.showing += 1;
        }
    }

    /// Notes that showing the pad again is done: it is published as
    /// hidden no longer, unless something else hides it.
    pub(crate) fn shown(&mut self, id: &str) {
        if let Some(row) = self.rows.get_mut(id) {
            row.showing = row.showing.saturating_sub(1);
        }
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
            (None, _) => Err(WireError::refusal(
                ErrorCode::UnknownPad,
                format!("no known pad is called {reference}"),
            )),
            (Some(_), Some(_)) => Err(WireError::refusal(
                ErrorCode::AmbiguousPad,
                format!("more than one known pad is {reference}; name one by its id"),
            )),
        }
    }

    /// Stop remembering the pad `reference` names. A connected pad can't
    /// be forgotten: it would be taken in again at once. Nor can one a
    /// setup is waiting for.
    pub(crate) fn forget(&mut self, reference: &str) -> Result<String, WireError> {
        let id = self.resolve(reference)?.to_string();
        if let Some(setup) = self.user(&id) {
            return Err(WireError::refusal(
                ErrorCode::PadInUse,
                format!("{id} is part of the setup {setup}; stop that first"),
            ));
        }
        if self.node(&id).is_some() {
            return Err(WireError::refusal(
                ErrorCode::PadConnected,
                format!("{id} is connected; unplug it to forget it"),
            ));
        }
        self.rows.remove(&id);
        Ok(id)
    }

    /// The connected pads that `wanted` could mean and that no setup
    /// uses, in the order they connected.
    pub(crate) fn fitting(&self, wanted: &PadRef) -> Vec<&str> {
        let mut fitting: Vec<&Row> = self
            .rows
            .values()
            .filter(|row| !row.nodes.is_empty() && row.setup.is_none())
            .filter(|row| wanted.matches(&row.known.id, row.known.vendor, row.known.product))
            .collect();
        fitting.sort_by_key(|row| row.connected);
        fitting
            .into_iter()
            .map(|row| row.known.id.as_str())
            .collect()
    }

    /// Whether `wanted` could mean the pad `id`.
    pub(crate) fn fits(&self, id: &str, wanted: &PadRef) -> bool {
        self.rows
            .get(id)
            .is_some_and(|row| wanted.matches(id, row.known.vendor, row.known.product))
    }

    /// The setup that uses the pad, if one does.
    pub(crate) fn user(&self, id: &str) -> Option<&str> {
        self.rows.get(id)?.setup.as_deref()
    }

    /// Notes that `setup` uses the pad. A pad the book doesn't know (one
    /// a profile names that has never been seen) has nothing to note.
    pub(crate) fn claim(&mut self, id: &str, setup: &str) {
        if let Some(row) = self.rows.get_mut(id) {
            row.setup = Some(setup.to_string());
        }
    }

    /// Notes whether the hiding service has the pad hidden.
    pub(crate) fn set_hidden(&mut self, id: &str, hidden: bool) {
        if let Some(row) = self.rows.get_mut(id) {
            row.hidden = hidden;
        }
    }

    /// Notes that no setup uses the pad any more, nor hides it. Showing
    /// it again, if it was hidden, is [noted](Self::showing) on its own
    /// account.
    pub(crate) fn let_go(&mut self, id: &str) {
        if let Some(row) = self.rows.get_mut(id) {
            row.let_go();
        }
    }

    /// Whether the pad has just come back on an anchor it was away from,
    /// as from a re-bind of its driver, and not been plugged in; asked
    /// once, as it arrives.
    pub(crate) fn was_restored(&mut self, id: &str) -> bool {
        self.rows
            .get_mut(id)
            .is_some_and(|row| std::mem::take(&mut row.back))
    }

    /// Notes that `setup` no longer uses any pad, nor hides one, as
    /// [`let_go`](Self::let_go) does for each. Returns the pads it used.
    pub(crate) fn release(&mut self, setup: &str) -> Vec<String> {
        self.rows
            .iter_mut()
            .filter(|(_, row)| row.setup.as_deref() == Some(setup))
            .map(|(id, row)| {
                row.let_go();
                id.clone()
            })
            .collect()
    }

    /// A node the pad is connected on, if it is.
    pub(crate) fn node(&self, id: &str) -> Option<&Path> {
        self.rows
            .get(id)?
            .nodes
            .first()
            .map(|(node, _)| node.as_path())
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
            connected: row.connected(),
            // Until the service has shown it again.
            hidden: row.hidden || row.showing > 0,
            setup: row.setup.clone(),
            rumble: row.known.rumble,
            bus: row.connected().then_some(row.bus),
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
            anchor: None,
        }
    }

    const ADDRESS: &str = "0a:1b:2c:3d:4e:14";

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
    fn a_setup_takes_the_pads_that_fit_in_the_order_they_connected() {
        let mut book = Book::default();
        let second = found(
            "/dev/input/event6",
            "DualSense",
            Some("aa:bb:cc:dd:ee:02"),
            None,
        );
        let first = found(
            "/dev/input/event5",
            "DualSense",
            Some("aa:bb:cc:dd:ee:01"),
            None,
        );
        // The later address connects first: the order is of connecting,
        // not of names.
        book.arrived(&second, None);
        book.arrived(&first, None);
        let model = PadRef::parse("model:054c:0ce6").unwrap();
        let exact = PadRef::parse("aa:bb:cc:dd:ee:01").unwrap();
        assert_eq!(
            book.fitting(&model),
            ["address:aa:bb:cc:dd:ee:02", "address:aa:bb:cc:dd:ee:01"]
        );
        assert_eq!(book.fitting(&exact), ["address:aa:bb:cc:dd:ee:01"]);
        assert!(book.fits("address:aa:bb:cc:dd:ee:01", &model));
        assert!(!book.fits("address:aa:bb:cc:dd:ee:02", &exact));
        assert!(!book.fits("address:none", &model));

        // One in use fits no other setup, and says whose it is.
        book.claim("address:aa:bb:cc:dd:ee:02", "couch");
        book.claim("address:never-seen", "couch");
        assert_eq!(book.fitting(&model), ["address:aa:bb:cc:dd:ee:01"]);
        assert_eq!(book.user("address:aa:bb:cc:dd:ee:02"), Some("couch"));
        assert_eq!(book.user("address:aa:bb:cc:dd:ee:01"), None);
        assert_eq!(
            book.pad("address:aa:bb:cc:dd:ee:02")
                .unwrap()
                .setup
                .as_deref(),
            Some("couch")
        );
        book.set_hidden("address:aa:bb:cc:dd:ee:02", true);
        book.set_hidden("address:never-seen", true);
        assert!(book.pad("address:aa:bb:cc:dd:ee:02").unwrap().hidden);
        book.left(Path::new("/dev/input/event6"));
        let refused = book.forget("aa:bb:cc:dd:ee:02").unwrap_err();
        assert_eq!(refused.code, ErrorCode::PadInUse);

        // One that left and returned is the latest to connect.
        assert_eq!(book.release("couch"), ["address:aa:bb:cc:dd:ee:02"]);
        assert!(!book.pad("address:aa:bb:cc:dd:ee:02").unwrap().hidden);
        assert_eq!(book.release("couch"), Vec::<String>::new());
        book.arrived(&second, None);
        assert_eq!(
            book.fitting(&model),
            ["address:aa:bb:cc:dd:ee:01", "address:aa:bb:cc:dd:ee:02"]
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

    /// The HID device of a pad on USB, enumerated as `enumeration`.
    fn anchor(path: &str, enumeration: u64) -> Anchor {
        Anchor {
            path: PathBuf::from(path),
            enumeration: Some(enumeration),
        }
    }

    const HID: &str = "/sys/devices/usb1/1-1/1-1:1.3/0003:054C:0CE6.0005";

    /// [`dualsense`] on `node`, hanging from `anchor`.
    fn on(node: &str, anchor: &Anchor) -> Found {
        Found {
            anchor: Some(anchor.clone()),
            ..dualsense(node)
        }
    }

    /// A pad whose node goes while its anchor stays, as a re-bind of its
    /// driver has it, whoever asked for it and however many times, is
    /// away: still connected, nothing published, and no node to open.
    /// Back on the same anchor, under another node as it may be, it isn't
    /// a pad plugged in. Its anchor's removal, after its node's, is its
    /// unplugging, told once.
    #[test]
    fn a_pad_whose_anchor_stays_never_left() {
        let mut book = Book::default();
        let hid = anchor(HID, 1);
        let id = book
            .arrived(&on("/dev/input/event5", &hid), None)
            .remove(0)
            .id;
        assert!(!book.was_restored(&id), "plugged in");
        for (went, back) in [("event5", "event7"), ("event7", "event5")] {
            assert_eq!(book.left(&Path::new("/dev/input").join(went)), None);
            let away = book.pad(&id).unwrap();
            assert!(away.connected && away.bus.is_some());
            assert_eq!(book.node(&id), None, "not there to be used meanwhile");
            let node = format!("/dev/input/{back}");
            assert_eq!(book.arrived(&on(&node, &hid), None).len(), 1);
            assert!(book.was_restored(&id), "back");
            assert!(!book.was_restored(&id), "asked once");
        }

        // Unplugged: its node goes first, and then its anchor.
        assert_eq!(book.left(Path::new("/dev/input/event5")), None);
        assert_eq!(book.unplugged(&hid.path), std::slice::from_ref(&id));
        assert!(!book.pad(&id).unwrap().connected);
        assert!(book.unplugged(&hid.path).is_empty(), "once");
        assert!(
            book.unplugged(Path::new("/sys/devices/usb1/1-1"))
                .is_empty()
        );

        // Plugged in again: another anchor, as a HID device is named
        // afresh at each enumeration.
        let again = anchor("/sys/devices/usb1/1-1/1-1:1.3/0003:054C:0CE6.0006", 2);
        book.arrived(&on("/dev/input/event5", &again), None);
        assert!(!book.was_restored(&id), "plugged in");
    }

    /// The device above an anchor going unplugs the pad too, its own
    /// removal unheard of or not; one at the anchor's place enumerated
    /// afresh is another device, its going unheard of.
    #[test]
    fn an_anchor_gone_another_way_unplugs_the_pad() {
        let mut book = Book::default();
        let interface = anchor("/sys/devices/usb1/1-1/1-1:1.0", 1);
        let id = book
            .arrived(&on("/dev/input/event5", &interface), None)
            .remove(0)
            .id;
        book.left(Path::new("/dev/input/event5"));
        assert_eq!(
            book.unplugged(Path::new("/sys/devices/usb1/1-1")),
            std::slice::from_ref(&id)
        );
        assert!(!book.pad(&id).unwrap().connected);

        // Away, and then a pad at its port that is another enumeration.
        book.arrived(&on("/dev/input/event5", &interface), None);
        book.left(Path::new("/dev/input/event5"));
        book.arrived(
            &on(
                "/dev/input/event6",
                &anchor(&interface.path.to_string_lossy(), 2),
            ),
            None,
        );
        assert!(!book.was_restored(&id), "plugged in");
        assert_eq!(book.left(Path::new("/dev/input/event6")), None);
        assert_eq!(book.unplugged(&interface.path), [id]);
    }

    /// A pad whose anchor couldn't be read is its own: its node going is
    /// its unplugging, and its node coming its plugging in.
    #[test]
    fn a_pad_without_an_anchor_is_unplugged_with_its_node() {
        let mut book = Book::default();
        let id = book
            .arrived(&dualsense("/dev/input/event5"), None)
            .remove(0)
            .id;
        assert_eq!(book.left(Path::new("/dev/input/event5")), Some(id.clone()));
        assert!(!book.pad(&id).unwrap().connected);
        book.arrived(&dualsense("/dev/input/event5"), None);
        assert!(!book.was_restored(&id));
    }

    /// Two pads that share a row are told apart by their anchors: one
    /// away and back while the other is unplugged, each is seen for what
    /// it does, and the row is connected while either is.
    #[test]
    fn two_pads_of_one_row_are_told_apart_by_their_anchors() {
        let mut book = Book::default();
        let (one, other) = (anchor(HID, 1), anchor("/sys/devices/usb1/1-2/1-2:1.0", 1));
        let pad = |node: &str, anchor: &Anchor| Found {
            anchor: Some(anchor.clone()),
            ..found(node, "Pad", None, None)
        };
        let id = book
            .arrived(&pad("/dev/input/event5", &one), None)
            .remove(0)
            .id;
        book.arrived(&pad("/dev/input/event6", &other), None);
        assert_eq!(book.left(Path::new("/dev/input/event5")), None, "away");
        assert_eq!(book.left(Path::new("/dev/input/event6")), None, "away");
        assert_eq!(book.unplugged(&other.path), std::slice::from_ref(&id));
        assert!(book.pad(&id).unwrap().connected, "the one away is there");
        book.arrived(&pad("/dev/input/event7", &one), None);
        assert!(book.was_restored(&id), "back, not plugged in");
        book.left(Path::new("/dev/input/event7"));
        assert_eq!(book.unplugged(&one.path), std::slice::from_ref(&id));
        assert!(!book.pad(&id).unwrap().connected);
    }

    /// A pad away that comes back reporting another name, as one whose
    /// address comes late does, is back all the same, and the row it
    /// was away from is left.
    #[test]
    fn a_pad_back_under_another_name_is_back() {
        let mut book = Book::default();
        let hid = anchor(HID, 1);
        let id = book
            .arrived(&on("/dev/input/event5", &hid), None)
            .remove(0)
            .id;
        book.left(Path::new("/dev/input/event5"));
        let early = Found {
            anchor: Some(hid),
            ..found("/dev/input/event5", "DualSense", None, Some("usb-1/input0"))
        };
        let changed = book.arrived(&early, None);
        assert_eq!(changed[0].id, id, "left");
        assert!(!book.pad(&id).unwrap().connected);
        assert!(book.was_restored(&changed[1].id), "back");
    }

    /// A pad being shown again is published as hidden until that is
    /// done, as many times as it is under way.
    #[test]
    fn a_pad_is_hidden_until_it_is_shown_again() {
        let mut book = Book::default();
        let id = book
            .arrived(&dualsense("/dev/input/event5"), None)
            .remove(0)
            .id;
        book.claim(&id, "couch");
        book.set_hidden(&id, true);
        assert_eq!(book.release("couch"), std::slice::from_ref(&id));
        book.showing(&id);
        book.showing(&id);
        assert!(book.pad(&id).unwrap().hidden, "until it is shown");
        book.shown(&id);
        assert!(book.pad(&id).unwrap().hidden, "once more");
        book.shown(&id);
        assert!(!book.pad(&id).unwrap().hidden);
        book.showing("address:nothing");
        book.shown("address:nothing");
    }
}
