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
    /// The event nodes it is connected on now. Two pads that report
    /// nothing to tell them apart share a row and have a node each.
    nodes: Vec<PathBuf>,
    /// When it last connected, as a count of connections: lower is
    /// earlier.
    connected: u64,
    /// The bus it last connected on, as the kernel numbers them.
    bus: u16,
    /// The setup that uses it.
    setup: Option<String>,
    /// Whether the hiding service has it hidden.
    hidden: bool,
    /// How many times it is still to go and come back because the
    /// hiding service re-bound its driver, to hide it or to show it
    /// again. Nobody touched it: that isn't a pad leaving and one
    /// connecting, and isn't published as such.
    comebacks: u32,
    /// Whether it has gone on one of those, and is on its way back.
    away: bool,
    /// Whether it has just come back from one, until asked.
    back: bool,
}

impl Row {
    fn remembered(known: Known) -> Self {
        Self {
            known,
            nodes: Vec::new(),
            connected: 0,
            bus: 0,
            setup: None,
            hidden: false,
            comebacks: 0,
            away: false,
            back: false,
        }
    }

    /// Whether the pad is there, as it is published: one that has gone
    /// only to come back never left.
    fn connected(&self) -> bool {
        !self.nodes.is_empty() || self.away
    }

    /// Notes that no setup uses the pad any more, nor hides it. A pad
    /// that was hidden by re-binding its driver, as `rebinds` says, is
    /// shown again the same way, and is expected to go and come back.
    fn let_go(&mut self, rebinds: bool) {
        self.setup = None;
        if self.hidden && rebinds {
            self.expect_back();
        }
        self.hidden = false;
    }

    /// Expects the pad to go and come back once more, if it is there to.
    fn expect_back(&mut self) {
        if self.connected() {
            self.comebacks += 1;
        }
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
        let row = self
            .rows
            .entry(id.clone())
            .or_insert_with(|| Row::remembered(known.clone()));
        row.known = known;
        // Back from going only to come back, if that is where it was.
        row.back = std::mem::take(&mut row.away);
        if row.back {
            row.comebacks = row.comebacks.saturating_sub(1);
        }
        let connected = !row.nodes.contains(&found.node);
        if connected {
            if row.nodes.is_empty() {
                self.connections += 1;
                row.connected = self.connections;
            }
            row.nodes.push(found.node.clone());
            row.bus = found.identifiers.bus;
        }
        if remembered || connected {
            changed.push(Changed { id, remembered });
        }
        changed
    }

    /// The pad on `node` has gone. Returns its id; it stays remembered.
    /// Nothing for a pad that has gone only to come back, as one does
    /// whose driver the hiding service re-binds: it is still there, as
    /// far as anyone is told.
    pub(crate) fn left(&mut self, node: &Path) -> Option<String> {
        self.rows.iter_mut().find_map(|(id, row)| {
            let at = row.nodes.iter().position(|held| held == node)?;
            row.nodes.remove(at);
            if row.nodes.is_empty() && row.comebacks > 0 {
                row.away = true;
                return None;
            }
            Some(id.clone())
        })
    }

    /// Notes that the hiding service is about to re-bind the pad's
    /// driver to show it again, for no setup: it goes and comes back,
    /// once, if it is there.
    pub(crate) fn expect_back(&mut self, id: &str) {
        if let Some(row) = self.rows.get_mut(id) {
            row.expect_back();
        }
    }

    /// When the pad last connected, as a count of connections, if it is
    /// known: going and coming back counts as connecting.
    pub(crate) fn arrival(&self, id: &str) -> Option<u64> {
        Some(self.rows.get(id)?.connected)
    }

    /// Notes that the hiding service hid the pad by re-binding its
    /// driver after `arrival`, the pad's [`arrival`](Self::arrival) then:
    /// that it goes and comes back is expected too, if it hasn't been
    /// seen to. Returns whether the pad is now published otherwise: one
    /// already seen to go is there after all.
    pub(crate) fn expect_back_since(&mut self, id: &str, arrival: u64) -> bool {
        let Some(row) = self.rows.get_mut(id) else {
            return false;
        };
        // Back already, or unplugged and plugged in since.
        if row.connected != arrival || row.away {
            return false;
        }
        row.comebacks += 1;
        // Gone, and on its way back.
        let gone = row.nodes.is_empty();
        row.away = gone;
        gone
    }

    /// Notes that the hiding service didn't show the pad again as was
    /// expected: it doesn't go and come back after all. Returns whether
    /// the pad is now published otherwise: one that went meanwhile has
    /// gone.
    pub(crate) fn not_shown(&mut self, id: &str) -> bool {
        let Some(row) = self.rows.get_mut(id) else {
            return false;
        };
        row.comebacks = row.comebacks.saturating_sub(1);
        let gone = row.comebacks == 0 && row.away;
        if gone {
            row.away = false;
        }
        gone
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

    /// Notes that no setup uses the pad any more, nor hides it. One that
    /// was hidden by re-binding its driver, as `rebinds` says, is shown
    /// again the same way: it is expected to go and come back.
    pub(crate) fn let_go(&mut self, id: &str, rebinds: bool) {
        if let Some(row) = self.rows.get_mut(id) {
            row.let_go(rebinds);
        }
    }

    /// Whether the pad has just come back from going because the hiding
    /// service re-bound its driver, and not from being plugged in;
    /// asked once, as it arrives.
    pub(crate) fn was_restored(&mut self, id: &str) -> bool {
        self.rows
            .get_mut(id)
            .is_some_and(|row| std::mem::take(&mut row.back))
    }

    /// Notes that `setup` no longer uses any pad, nor hides one, as
    /// [`let_go`](Self::let_go) does for each, with `rebinds` saying
    /// which pads' drivers were re-bound to hide them. Returns the pads
    /// it used.
    pub(crate) fn release(&mut self, setup: &str, rebinds: impl Fn(&str) -> bool) -> Vec<String> {
        self.rows
            .iter_mut()
            .filter(|(_, row)| row.setup.as_deref() == Some(setup))
            .map(|(id, row)| {
                row.let_go(rebinds(id));
                id.clone()
            })
            .collect()
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
            connected: row.connected(),
            hidden: row.hidden,
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
        assert_eq!(
            book.release("couch", |_| false),
            ["address:aa:bb:cc:dd:ee:02"]
        );
        assert!(!book.pad("address:aa:bb:cc:dd:ee:02").unwrap().hidden);
        assert_eq!(book.release("couch", |_| false), Vec::<String>::new());
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

    /// A pad hidden by `couch` by re-binding its driver, connected on
    /// `node`.
    fn hidden_by_couch(node: &str) -> (Book, String) {
        let mut book = Book::new(Vec::new());
        let id = book.arrived(&dualsense(node), None).pop().unwrap().id;
        book.claim(&id, "couch");
        book.set_hidden(&id, true);
        (book, id)
    }

    /// Shown again by the hiding service, a pad goes and comes back by
    /// itself: that arrival is told from a pad plugged in, once.
    #[test]
    fn a_pad_shown_again_is_told_from_one_plugged_in() {
        let node = Path::new("/dev/input/event5");
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        assert!(!book.was_restored(&id), "plugged in");
        assert_eq!(book.release("couch", |_| true), std::slice::from_ref(&id));
        assert!(!book.was_restored(&id), "not back yet");
        assert_eq!(book.left(node), None);
        book.arrived(&dualsense("/dev/input/event5"), None);
        assert!(book.was_restored(&id));
        assert!(!book.was_restored(&id), "asked once");

        // Given up by a setup that goes on running.
        book.claim(&id, "couch");
        book.set_hidden(&id, true);
        book.let_go(&id, true);
        assert_eq!(book.left(node), None);
        book.arrived(&dualsense("/dev/input/event5"), None);
        assert!(book.was_restored(&id));

        // A pad that was only grabbed never goes, nor does one whose
        // driver wasn't re-bound: what comes next is a pad unplugged.
        book.claim(&id, "couch");
        book.release("couch", |_| true);
        book.set_hidden(&id, true);
        book.let_go(&id, false);
        assert_eq!(book.left(node), Some(id.clone()));
        book.arrived(&dualsense("/dev/input/event5"), None);
        assert!(!book.was_restored(&id) && !book.was_restored("address:nothing"));
    }

    /// A pad whose driver is re-bound to show it again goes and comes
    /// back: it is published as there throughout, and nothing is said
    /// of its going. A pad unplugged is said to have gone, as ever.
    #[test]
    fn a_pad_that_goes_only_to_come_back_never_left() {
        let node = Path::new("/dev/input/event5");
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        book.release("couch", |_| true);
        assert_eq!(book.left(node), None, "nothing to publish");
        let gone = book.pad(&id).unwrap();
        assert!(gone.connected && gone.bus.is_some());
        // Not there to be used meanwhile, whatever is published.
        assert!(book.node(&id).is_none());
        // Back, under another node as it may be; and from then on a
        // going is a going.
        let back = dualsense("/dev/input/event7");
        assert_eq!(book.arrived(&back, None).len(), 1);
        assert!(
            book.was_restored(&id),
            "as the reactor asks of each arrival"
        );
        assert!(book.pad(&id).unwrap().connected);
        assert_eq!(book.left(Path::new("/dev/input/event7")), Some(id.clone()));
        assert!(!book.pad(&id).unwrap().connected);

        // Nothing is expected of a pad that wasn't hidden.
        book.arrived(&dualsense("/dev/input/event5"), None);
        book.claim(&id, "couch");
        book.release("couch", |_| true);
        assert_eq!(book.left(node), Some(id.clone()));
        // Expected for no setup, it is expected all the same.
        book.arrived(&dualsense("/dev/input/event5"), None);
        book.expect_back(&id);
        assert_eq!(book.left(node), None);
    }

    /// A pad let go while it isn't there has nothing to go and come back
    /// from: turned on later, it is a pad plugged in, and the setups that
    /// wait for it hear of it.
    #[test]
    fn a_pad_that_is_not_there_when_let_go_is_expected_back_from_nothing() {
        let node = Path::new("/dev/input/event5");
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        // Its battery dies, and then its setup is stopped.
        assert_eq!(book.left(node), Some(id.clone()));
        book.release("couch", |_| true);
        book.arrived(&dualsense("/dev/input/event5"), None);
        assert!(!book.was_restored(&id), "turned on again");
        assert_eq!(book.left(node), Some(id), "and off");
    }

    /// The hiding service may not show a pad again as asked: it refused,
    /// or wasn't there to ask. The pad then doesn't go and come back, and
    /// its next going is a going.
    #[test]
    fn a_pad_the_service_does_not_show_again_is_not_expected_back() {
        let node = Path::new("/dev/input/event5");
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        book.release("couch", |_| true);
        assert!(!book.not_shown(&id), "nothing to publish");
        assert_eq!(book.left(node), Some(id.clone()), "unplugged");
        assert!(!book.pad(&id).unwrap().connected);
        book.arrived(&dualsense("/dev/input/event5"), None);
        assert!(!book.was_restored(&id), "plugged in again");

        // Unplugged while the service was asked: taken for going only to
        // come back, until the service says it won't.
        book.claim(&id, "couch");
        book.set_hidden(&id, true);
        book.release("couch", |_| true);
        assert_eq!(book.left(node), None);
        assert!(book.not_shown(&id), "it has gone after all");
        assert!(!book.pad(&id).unwrap().connected);
        assert!(!book.not_shown(&id) && !book.not_shown("address:nothing"));
    }

    /// A pad hidden by re-binding its driver goes and comes back as it
    /// is hidden, which the setup that hid it lives through: nothing is
    /// published, the arrival isn't a pad plugged in, and showing it
    /// again later is expected on its own account, whichever order the
    /// goings come in.
    #[test]
    fn a_pad_hidden_by_re_binding_its_driver_never_left() {
        let node = Path::new("/dev/input/event5");
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        let arrival = book.arrival(&id).unwrap();
        assert!(!book.expect_back_since(&id, arrival), "nothing to publish");
        assert_eq!(book.left(node), None);
        assert!(book.pad(&id).unwrap().connected);
        book.arrived(&dualsense("/dev/input/event5"), None);
        assert!(book.was_restored(&id));
        assert_eq!(book.left(node), Some(id.clone()), "then unplugged");

        // Stopped before the hiding's own going and coming back is seen:
        // that is one, and showing it again is another.
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        let arrival = book.arrival(&id).unwrap();
        book.expect_back_since(&id, arrival);
        book.release("couch", |_| true);
        for went in ["as it was hidden", "as it is shown again"] {
            assert_eq!(book.left(node), None, "{went}");
            book.arrived(&dualsense("/dev/input/event5"), None);
            assert!(book.was_restored(&id), "{went}");
        }
        assert_eq!(book.left(node), Some(id), "then unplugged");
    }

    /// A pad hidden by re-binding its driver went and came back as it
    /// was hidden, which may not have been seen yet when it is shown
    /// again: then both goings and comings back are still to come, or
    /// the second of them, and neither is a pad plugged in.
    #[test]
    fn a_pad_hidden_and_shown_again_at_once_goes_and_comes_back_twice() {
        let node = Path::new("/dev/input/event5");
        // Neither seen yet.
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        let arrival = book.arrival(&id).unwrap();
        assert!(!book.expect_back_since(&id, arrival));
        book.release("couch", |_| true);
        for _ in 0..2 {
            assert_eq!(book.left(node), None);
            book.arrived(&dualsense("/dev/input/event5"), None);
            assert!(book.was_restored(&id));
        }
        assert_eq!(book.left(node), Some(id.clone()), "then unplugged");

        // Its going seen, and published, but not its coming back.
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        let arrival = book.arrival(&id).unwrap();
        assert_eq!(book.left(node), Some(id.clone()));
        assert!(book.expect_back_since(&id, arrival), "there after all");
        assert!(book.pad(&id).unwrap().connected);
        book.release("couch", |_| true);
        book.arrived(&dualsense("/dev/input/event5"), None);
        assert!(book.was_restored(&id), "back from being hidden");
        assert_eq!(book.left(node), None, "shown again");
        book.arrived(&dualsense("/dev/input/event5"), None);
        assert!(book.was_restored(&id));
        assert_eq!(book.left(node), Some(id.clone()), "then unplugged");

        // Both seen already.
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        let arrival = book.arrival(&id).unwrap();
        book.left(node);
        book.arrived(&dualsense("/dev/input/event5"), None);
        assert!(!book.expect_back_since(&id, arrival));
        book.release("couch", |_| true);
        assert_eq!(book.left(node), None);
        book.arrived(&dualsense("/dev/input/event5"), None);
        assert!(book.was_restored(&id));
        assert_eq!(book.left(node), Some(id.clone()));
        assert!(!book.expect_back_since("address:nothing", 0));
    }
}
