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
    /// How many requests to the hiding service about it are under way:
    /// expected before each is asked, until what it did is counted.
    asking: u32,
    /// How many of those show it again: it is published as hidden until
    /// each is counted.
    showing: u32,
    /// Whether a count was taken that the service didn't say, as for a
    /// request hung up: what is still expected may be right or not, until
    /// a count the service says with a re-bind in it.
    unsure: bool,
    /// How many requests about it have been expected: what a check of
    /// the count is of.
    generation: u64,
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
            asking: 0,
            showing: 0,
            unsure: false,
            generation: 0,
        }
    }

    /// Whether the pad is there, as it is published: one that has gone
    /// only to come back never left.
    fn connected(&self) -> bool {
        !self.nodes.is_empty() || self.away
    }

    /// Notes that no setup uses the pad any more, nor hides it. Showing
    /// it again, if it was hidden, is expected on its own account.
    fn let_go(&mut self) {
        self.setup = None;
        self.hidden = false;
    }

    /// Notes a request about it about to be asked, which may have it go
    /// and come back as many as `most` times.
    fn expect(&mut self, most: u8) {
        self.comebacks += u32::from(most);
        self.asking += 1;
        self.generation += 1;
    }
}

/// What a request to the hiding service came to: how many times it made
/// the pad go and come back, and whether the service said so or the
/// daemon guessed, as for a request hung up or a service that doesn't
/// count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Count {
    pub(crate) rebinds: u8,
    pub(crate) said: bool,
}

impl Count {
    /// As the service said.
    pub(crate) const fn said(rebinds: u8) -> Self {
        Self {
            rebinds,
            said: true,
        }
    }

    /// As the daemon guessed, the service having said nothing.
    pub(crate) const fn guessed(rebinds: u8) -> Self {
        Self {
            rebinds,
            said: false,
        }
    }
}

/// What taking a request's count changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Answered {
    /// Whether the pad is now published otherwise: one that went
    /// meanwhile has gone after all.
    pub(crate) gone: bool,
    /// Whether what is still expected is to be checked once every event
    /// udev has sent so far has been taken in, and for which of the
    /// pad's requests: see [`Book::flushed`].
    pub(crate) check: Option<u64>,
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

    /// Notes that the hiding service is about to be asked to hide the
    /// pad, which may re-bind its driver as many as `most` times: it is
    /// expected to go and come back that many times more, whatever it is
    /// doing now, until what the service did is
    /// [counted](Self::answered). Expected before it is asked, its going
    /// and coming back is never published, whenever its events come. The
    /// pad is there, as one is that is asked to be hidden.
    pub(crate) fn expect(&mut self, id: &str, most: u8) {
        if let Some(row) = self.rows.get_mut(id) {
            row.expect(most);
        }
    }

    /// Notes that the hiding service is about to be asked to show the
    /// pad again: it is published as hidden until that is
    /// [counted](Self::shown), and, if it is there and was hidden by
    /// re-binding its driver, as `rebinds` says, is expected to go and
    /// come back once more. Returns how many times it is expected to.
    pub(crate) fn showing(&mut self, id: &str, rebinds: bool) -> u8 {
        let Some(row) = self.rows.get_mut(id) else {
            return 0;
        };
        let expected = u8::from(rebinds && row.connected());
        row.expect(expected);
        row.showing += 1;
        expected
    }

    /// Sets right what was [expected](Self::expect) of a request about
    /// the pad, `expected` goings and comings back, by what it came to.
    pub(crate) fn answered(&mut self, id: &str, expected: u8, count: Count) -> Answered {
        let Some(row) = self.rows.get_mut(id) else {
            return Answered::default();
        };
        row.asking = row.asking.saturating_sub(1);
        let mut gone = false;
        for _ in count.rebinds..expected {
            row.comebacks = row.comebacks.saturating_sub(1);
            if row.comebacks == 0 && row.away {
                row.away = false;
                gone = true;
            }
        }
        // More than the most a request makes: expected late, and left for
        // the check to find.
        row.comebacks += u32::from(count.rebinds.saturating_sub(expected));
        if !count.said {
            row.unsure = true;
        } else if count.rebinds > 0 {
            // The service re-binds a device once every re-bind it asked of
            // it before has been asked: what was guessed of those is in
            // udev's hands before this one's, and is checked with it.
            row.unsure = false;
        }
        let check = (row.asking == 0 && !row.unsure && row.comebacks > 0).then_some(row.generation);
        Answered { gone, check }
    }

    /// Sets right what was expected of showing the pad again, as
    /// [`answered`](Self::answered) does, and publishes it as hidden no
    /// longer, unless something else hides it.
    pub(crate) fn shown(&mut self, id: &str, expected: u8, count: Count) -> Answered {
        if let Some(row) = self.rows.get_mut(id) {
            row.showing = row.showing.saturating_sub(1);
        }
        self.answered(id, expected, count)
    }

    /// The check [`answered`](Self::answered) asks for, once every event
    /// udev had sent by the time it was asked has been taken in: a pad
    /// that is there, with nothing asked about it since, and every count
    /// of what was asked said by the service, is expected to go and come
    /// back no more. If it still is, the count was wrong somewhere, and
    /// would swallow its next unplugging: what is expected is cleared,
    /// and returned for the caller to log.
    ///
    /// It can't clear what a right count expects. The service says a
    /// re-bind is done only once udev has finished with the pad's new
    /// nodes, and udev finishes a device's events one after another,
    /// with its children's: the old nodes' removal is sent to every
    /// listener before the new nodes' addition is begun, and so before
    /// the reply is. So by the check, the removal of every re-bind the
    /// counts describe has been taken in, and a pad that is there has
    /// come back from each, consuming what was expected of it; only a
    /// last addition can still be on its way, and then the pad is away,
    /// not there. A request hung up, or a service that doesn't count,
    /// leaves the count guessed, and there is no check until a count the
    /// service says with a re-bind in it, which it makes only after any
    /// it was asked before. Nor once another request has been expected:
    /// its own count asks for its own check.
    pub(crate) fn flushed(&mut self, id: &str, generation: u64) -> Option<u32> {
        let row = self.rows.get_mut(id)?;
        let stale = row.generation == generation
            && row.asking == 0
            && !row.unsure
            && row.comebacks > 0
            && !row.nodes.is_empty()
            && !row.away;
        stale.then(|| std::mem::take(&mut row.comebacks))
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
    /// it again, if it was hidden, is [expected](Self::showing) on its
    /// own account.
    pub(crate) fn let_go(&mut self, id: &str) {
        if let Some(row) = self.rows.get_mut(id) {
            row.let_go();
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
            // Until the service has shown it again, as far as it said.
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

    /// A pad hidden by `couch` by re-binding its driver, connected on
    /// `node`.
    fn hidden_by_couch(node: &str) -> (Book, String) {
        let mut book = Book::new(Vec::new());
        let id = book.arrived(&dualsense(node), None).pop().unwrap().id;
        book.claim(&id, "couch");
        book.set_hidden(&id, true);
        (book, id)
    }

    /// The most a hide expects, as the reactor expects it.
    const HIDE: u8 = 2;

    /// Shown again by the hiding service, a pad goes and comes back by
    /// itself: that arrival is told from a pad plugged in, once. It is
    /// published as hidden until the service says it has shown it.
    #[test]
    fn a_pad_shown_again_is_told_from_one_plugged_in() {
        let node = Path::new("/dev/input/event5");
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        assert!(!book.was_restored(&id), "plugged in");
        assert_eq!(book.release("couch"), std::slice::from_ref(&id));
        assert_eq!(book.showing(&id, true), 1);
        assert!(book.pad(&id).unwrap().hidden, "until it is shown");
        assert!(!book.was_restored(&id), "not back yet");
        assert_eq!(book.left(node), None);
        book.arrived(&dualsense("/dev/input/event5"), None);
        assert!(book.was_restored(&id));
        assert!(!book.was_restored(&id), "asked once");
        assert_eq!(book.shown(&id, 1, Count::said(1)), Answered::default());
        assert!(!book.pad(&id).unwrap().hidden);

        // Given up by a setup that goes on running.
        book.claim(&id, "couch");
        book.set_hidden(&id, true);
        book.let_go(&id);
        let expected = book.showing(&id, true);
        assert_eq!(book.left(node), None);
        book.arrived(&dualsense("/dev/input/event5"), None);
        assert!(book.was_restored(&id));
        book.shown(&id, expected, Count::said(1));

        // A pad whose driver wasn't re-bound to hide it isn't to show
        // it: what comes next is a pad unplugged.
        book.claim(&id, "couch");
        book.set_hidden(&id, true);
        book.let_go(&id);
        assert_eq!(book.showing(&id, false), 0);
        book.shown(&id, 0, Count::said(0));
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
        book.release("couch");
        let expected = book.showing(&id, true);
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
        book.shown(&id, expected, Count::said(1));
        assert!(book.pad(&id).unwrap().connected);
        assert_eq!(book.left(Path::new("/dev/input/event7")), Some(id.clone()));
        assert!(!book.pad(&id).unwrap().connected);
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
        book.release("couch");
        assert_eq!(book.showing(&id, true), 0);
        book.arrived(&dualsense("/dev/input/event5"), None);
        assert!(!book.was_restored(&id), "turned on again");
        assert_eq!(book.left(node), Some(id), "and off");
    }

    /// The hiding service may not show a pad again as asked: it refused,
    /// or said it re-bound nothing. The pad then doesn't go and come back,
    /// and its next going is a going.
    #[test]
    fn a_pad_the_service_does_not_show_again_is_not_expected_back() {
        let node = Path::new("/dev/input/event5");
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        book.release("couch");
        let expected = book.showing(&id, true);
        let answered = book.shown(&id, expected, Count::said(0));
        assert!(!answered.gone, "nothing to publish");
        assert_eq!(book.left(node), Some(id.clone()), "unplugged");
        assert!(!book.pad(&id).unwrap().connected);
        book.arrived(&dualsense("/dev/input/event5"), None);
        assert!(!book.was_restored(&id), "plugged in again");

        // Unplugged while the service was asked: taken for going only to
        // come back, until the service says it didn't re-bind it.
        book.claim(&id, "couch");
        book.set_hidden(&id, true);
        book.release("couch");
        let expected = book.showing(&id, true);
        assert_eq!(book.left(node), None);
        assert!(
            book.shown(&id, expected, Count::said(0)).gone,
            "it has gone after all"
        );
        assert!(!book.pad(&id).unwrap().connected);
        assert_eq!(
            book.answered("address:nothing", 1, Count::said(0)),
            Answered::default()
        );
    }

    /// A pad expected back before the hiding service is asked to hide it
    /// goes and comes back as it is hidden, which the setup that hid it
    /// lives through: nothing is published, the arrival isn't a pad
    /// plugged in, and showing it again later is expected on its own
    /// account, whichever order the goings come in.
    #[test]
    fn a_pad_expected_back_before_it_is_hidden_never_left() {
        let node = Path::new("/dev/input/event5");
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        book.expect(&id, HIDE);
        assert_eq!(book.left(node), None);
        assert!(book.pad(&id).unwrap().connected);
        book.arrived(&dualsense("/dev/input/event5"), None);
        assert!(book.was_restored(&id));
        book.answered(&id, HIDE, Count::said(1));
        assert_eq!(book.left(node), Some(id.clone()), "then unplugged");

        // Stopped before the hiding's own going and coming back is seen:
        // that is one, and showing it again is another.
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        book.expect(&id, HIDE);
        book.answered(&id, HIDE, Count::said(1));
        book.release("couch");
        let expected = book.showing(&id, true);
        for went in ["as it was hidden", "as it is shown again"] {
            assert_eq!(book.left(node), None, "{went}");
            book.arrived(&dualsense("/dev/input/event5"), None);
            assert!(book.was_restored(&id), "{went}");
        }
        book.shown(&id, expected, Count::said(1));
        assert_eq!(book.left(node), Some(id), "then unplugged");

        // Gone as it was hidden, and let go before it is back.
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        book.expect(&id, HIDE);
        assert_eq!(book.left(node), None);
        book.answered(&id, HIDE, Count::said(1));
        book.release("couch");
        let expected = book.showing(&id, true);
        for went in ["back from being hidden", "back from being shown again"] {
            book.arrived(&dualsense("/dev/input/event5"), None);
            assert!(book.was_restored(&id), "{went}");
            book.left(node);
        }
        book.shown(&id, expected, Count::said(1));
        assert!(!book.pad(&id).unwrap().connected, "then unplugged");
        book.expect("address:nothing", HIDE);
    }

    /// What the service did sets right what was expected before it was
    /// asked: a hide it refused before acting, or that it gave up before
    /// re-binding, takes the goings and comings back away; one given up
    /// as it re-bound, and the pad shown again, keeps both; and one that
    /// showed the pad again itself after its re-bind, both, though they
    /// came before it said so.
    #[test]
    fn what_the_service_did_sets_right_what_was_expected() {
        let node = Path::new("/dev/input/event5");
        // Refused before acting: nothing goes, and an unplugging is told.
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        book.expect(&id, HIDE);
        assert!(!book.answered(&id, HIDE, Count::said(0)).gone);
        assert_eq!(book.left(node), Some(id.clone()));

        // Unplugged as it was being hidden: taken for going only to come
        // back, until the service says it didn't come back.
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        book.expect(&id, HIDE);
        assert_eq!(book.left(node), None);
        assert!(
            book.answered(&id, HIDE, Count::said(0)).gone,
            "it has gone after all"
        );
        assert!(!book.pad(&id).unwrap().connected);

        // Both re-binds of one request, come before it said so: neither
        // is published, nor left expected.
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        book.expect(&id, HIDE);
        book.release("couch");
        for _ in 0..2 {
            assert_eq!(book.left(node), None);
            book.arrived(&dualsense("/dev/input/event5"), None);
            assert!(book.was_restored(&id));
        }
        assert_eq!(
            book.answered(&id, HIDE, Count::said(2)),
            Answered::default()
        );
        assert_eq!(book.left(node), Some(id.clone()), "then unplugged");

        // Given up before anything was re-bound: both taken back.
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        book.expect(&id, HIDE);
        assert!(!book.answered(&id, HIDE, Count::said(0)).gone);
        assert_eq!(book.left(node), Some(id), "unplugged, and told");
    }

    /// Once nothing is asked about a pad, and every count was said, a
    /// pad that is there and still expected to go and come back was
    /// miscounted: the check clears it, and the pad's next unplugging is
    /// told. What a right count still expects is never cleared: not
    /// while the pad is away on a re-bind, not while another request is
    /// under way, nor after a count guessed, nor for a check asked before
    /// a later request.
    #[test]
    fn a_count_left_over_once_nothing_is_asked_is_cleared() {
        let node = Path::new("/dev/input/event5");
        // A hide said to re-bind once, its going and coming back seen:
        // nothing is left, and nothing to check.
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        book.expect(&id, HIDE);
        book.left(node);
        book.arrived(&dualsense("/dev/input/event5"), None);
        assert_eq!(book.answered(&id, HIDE, Count::said(1)).check, None);

        // Said before its going was seen: checked, and the check, made
        // while the pad is away, clears nothing.
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        book.expect(&id, HIDE);
        let generation = book.answered(&id, HIDE, Count::said(1)).check.unwrap();
        assert_eq!(book.left(node), None);
        assert_eq!(book.flushed(&id, generation), None, "away");
        book.arrived(&dualsense("/dev/input/event5"), None);
        assert_eq!(book.flushed(&id, generation), None, "nothing left");

        // A count wrong by one: one going and coming back is left over
        // with the pad there, and the check clears it.
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        book.expect(&id, HIDE);
        book.expect(&id, 1);
        book.answered(&id, HIDE, Count::said(1));
        let generation = book.answered(&id, 0, Count::said(0)).check.unwrap();
        book.left(node);
        book.arrived(&dualsense("/dev/input/event5"), None);
        assert_eq!(book.flushed(&id, generation), Some(1));
        assert_eq!(book.left(node), Some(id.clone()), "unplugged, and told");

        // Not while another request is under way, nor for a check asked
        // before one.
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        book.expect(&id, HIDE);
        let generation = book.answered(&id, HIDE, Count::said(1)).check.unwrap();
        book.expect(&id, HIDE);
        assert_eq!(book.flushed(&id, generation), None, "asking");
        assert_eq!(book.answered(&id, HIDE, Count::said(2)).check, Some(2));
        assert_eq!(book.flushed(&id, generation), None, "an earlier check");

        // Nor after a count guessed, until one said with a re-bind in it.
        let (mut book, id) = hidden_by_couch("/dev/input/event5");
        let expected = book.showing(&id, true);
        assert_eq!(book.shown(&id, expected, Count::guessed(1)).check, None);
        book.expect(&id, HIDE);
        assert_eq!(book.answered(&id, HIDE, Count::said(0)).check, None);
        book.expect(&id, HIDE);
        let generation = book.answered(&id, HIDE, Count::said(1)).check.unwrap();
        assert_eq!(book.flushed(&id, generation), Some(2), "both left over");
        assert_eq!(book.flushed("address:nothing", 1), None);
    }
}
