//! The pads the daemon knows: those connected now, and those it
//! remembers, so a profile can name a pad that isn't plugged in.
//!
//! This is the bookkeeping alone, with no I/O, so every rule about what
//! a pad is called and when it counts as connected is tested without a
//! pad.
//!
//! **A re-bind is known by its request.** Hiding a pad and showing it
//! again re-bind its driver, which removes its nodes and makes new ones
//! and never removes its [anchor](Anchor). The hiding service asks each
//! re-bind by a change event on the anchor that it marks as its own, and
//! udev runs the rule that re-binds the driver before it sends that event
//! on, sending the removals and arrivals that caused after it, those
//! being of the anchor's children. So as the request is heard, the nodes
//! under the anchor that are gone from sysfs are going for that re-bind,
//! and are marked so. A node gone so is the pad away: nothing is
//! published, and it is still connected. A node come on that anchor is
//! the pad back, which isn't a pad plugged in. Any other node going has
//! left, and is published, whatever became of its anchor: a pad on a
//! wireless receiver switched off, whose receiver is its anchor and
//! stays, has gone, and switched on again is a pad plugged in; so with a
//! pad re-bound by anyone but the service, which is what that is.
//!
//! The mark is on the nodes gone as the request is heard, and each goes
//! with its node, so a request the rule didn't act on marks nothing and
//! none outlasts what it was for. The anchor's own removal, which udev
//! sends after its nodes', unplugs the pad, away or not. A node on an
//! anchor the book hasn't seen is a pad plugged in, as is a pad away as
//! the daemon starts, which has none yet. A pad whose anchor couldn't be
//! read is its own anchor: its node going is its unplugging. A re-bind no
//! driver comes back from leaves the pad away, and so connected, until
//! its anchor goes.
//!
//! **So is a raw node held.** A Steam Controller pad is read through its
//! raw node, and its driver takes the pad's event node away for as long
//! as anything has that open, making it anew, under another input
//! device, once nothing has; its HID device, the anchor, stays
//! throughout. So the invariant: before the daemon opens a pad's raw
//! node, the node the pad is on is marked as going for that, and no
//! other node of its row is, the mark being heard ahead of every udev
//! event the opening causes (the work that opens it says so on the
//! hotplug watch's own queue first). A node so marked that goes is the
//! pad away, as for a re-bind: still connected, nothing published, and
//! its setup never told; a node come on its anchor is the pad back, not
//! plugged in, which starts no profile, so a profile stopped by hand
//! stays stopped as the node returns. The mark goes with its node; at
//! once if the raw node wasn't opened after all; and once the hold has
//! ended with no effect, the driver never having taken the node, which
//! the event node's descriptor, not hung up once the raw node is closed,
//! says. A pad that truly goes while held is said so by what reads it,
//! the input thread, and is then [lost](Book::lost): what of it is away
//! or marked for the hold goes, and its next node there is a pad plugged
//! in. Nothing waits on the node's return: another program holding the
//! raw node too, as Steam does, keeps the node away after the daemon
//! lets go, and the pad stays away, connected and with no node to open,
//! until it returns or its anchor goes.
//!
//! A node's going that the hotplug watch lost is learned from what udev
//! sends after it, as well as from a look at what is connected: a node
//! arriving under its name, or on the anchor it was marked as going from
//! for a re-bind, says it has gone, udev sending the one before the
//! other. It goes as it would have had its going been heard, away if it
//! was marked, so that the node come is the pad back.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ctrlassist_device::{Anchor, Found};
use ctrlassist_protocol::error::{ErrorCode, WireError};
use ctrlassist_protocol::profile::{PadRef, is_bare_mac};
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

/// An event node a pad is connected on.
#[derive(Debug)]
struct Held {
    node: PathBuf,
    /// Its place in sysfs, which no later node has.
    syspath: PathBuf,
    anchor: Option<Anchor>,
    /// Whether it is going for a re-bind the hiding service asked: it was
    /// gone from sysfs as the request was heard.
    rebinding: bool,
    /// Whether a setup is about to read the pad through its raw node, or
    /// does: its driver takes this node away while that is open.
    raw: bool,
}

impl Held {
    /// Whether this node's going is the pad away, not gone: for a re-bind
    /// the hiding service asked, or the pad's raw node held.
    fn goes_away(&self) -> bool {
        self.rebinding || self.raw
    }

    /// What is away of the pad once this node has gone, if its going
    /// leaves the pad away.
    fn away(self) -> Option<Away> {
        let raw = self.raw;
        self.goes_away()
            .then_some(self.anchor)
            .flatten()
            .map(|anchor| Away { anchor, raw })
    }
}

/// A node of a pad gone for something the daemon did, which comes back
/// on the same anchor.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Away {
    anchor: Anchor,
    /// Whether it went for the pad's raw node held, not for a re-bind.
    raw: bool,
}

#[derive(Debug)]
struct Row {
    known: Known,
    /// The event nodes it is connected on now. Two pads that report
    /// nothing to tell them apart share a row and have a node each, and
    /// an anchor each.
    nodes: Vec<Held>,
    /// Each of its nodes gone for a re-bind or a hold of its raw node: it
    /// is away, and comes back on that node's anchor.
    away: Vec<Away>,
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

    /// Whether the pad can be rumbled now ([`Book::rumble`]).
    fn rumble(&self) -> Option<bool> {
        let steam =
            ctrlassist_device::is_steam_controller_model(self.known.vendor, self.known.product);
        if steam && self.setup.is_some() {
            Some(true)
        } else {
            self.known.rumble
        }
    }

    /// Notes that no setup uses the pad any more, nor hides it.
    fn let_go(&mut self) {
        self.setup = None;
        self.hidden = false;
    }
}

/// Whether the node `found`, arriving, says that `held` has gone, its
/// going unheard; `come` if `found` isn't taken in yet. udev sends a
/// node's removal before it sends another node's arrival under its name,
/// the two being one device number, and a re-bind's removals before its
/// arrivals, its anchor's `unbind` coming between and being the parent
/// of both. So a node held under the name of one arriving at another
/// place has gone; and so has one marked as going for a re-bind of the
/// anchor a node comes on, a node come there after the request being
/// the re-bind's or later. Only when the hotplug watch lost events can
/// either still be held.
fn has_gone(held: &Held, found: &Found, come: bool) -> bool {
    held.syspath != found.syspath
        && (held.node == found.node
            || (come && held.goes_away() && held.anchor.is_some() && held.anchor == found.anchor))
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

    /// A pad connected, or one already connected changed. Whether it can
    /// rumble is [learned](Self::learned) after. The daemon's own virtual
    /// pads are never taken in. Nodes it says have gone, their going
    /// unheard, go first (see [`has_gone`]). Returns each pad whose state
    /// changed.
    pub(crate) fn arrived(&mut self, found: &Found) -> Vec<Changed> {
        if found.is_own_output() {
            return Vec::new();
        }
        let id = found.identifiers.id().to_string();
        // A node already taken in is a connected pad that changed; any
        // other is a node come. A node is told by its place, as it is
        // when it leaves: its name is handed on.
        let come = !self
            .rows
            .values()
            .any(|row| row.nodes.iter().any(|held| held.syspath == found.syspath));
        let mut changed = Vec::new();
        for (held, row) in &mut self.rows {
            let was = row.connected();
            // Nodes this one says are gone, their going unheard, as when
            // the hotplug watch lost it: each goes as it would have had
            // its going been heard.
            let (gone, kept): (Vec<Held>, Vec<Held>) = std::mem::take(&mut row.nodes)
                .into_iter()
                .partition(|held| has_gone(held, found, come));
            row.nodes = kept;
            row.away.extend(gone.into_iter().filter_map(Held::away));
            // A node is one pad's at a time: one that now reports another
            // name has left the pad it was.
            if *held != id {
                row.nodes.retain(|held| held.syspath != found.syspath);
            }
            // One at the place of a node away, enumerated afresh, is
            // another device, whose going wasn't heard: what was away
            // from the one before has gone.
            if let Some(anchor) = &found.anchor {
                row.away
                    .retain(|away| away.anchor.path != anchor.path || away.anchor == *anchor);
            }
            if *held != id && was != row.connected() {
                changed.push(Changed {
                    id: held.clone(),
                    remembered: false,
                });
            }
        }
        let back = come
            && found
                .anchor
                .as_ref()
                .is_some_and(|anchor| self.back(&id, anchor, &mut changed));
        let was = self.rows.get(&id).map(|row| &row.known);
        let known = Known {
            id: id.clone(),
            name: found.identifiers.name.clone(),
            vendor: found.identifiers.vendor,
            product: found.identifiers.product,
            // Its last answer, until it is asked again.
            rumble: was.and_then(|was| was.rumble),
        };
        let remembered = was != Some(&known);
        let row = self
            .rows
            .entry(id.clone())
            .or_insert_with(|| Row::remembered(known.clone()));
        row.known = known;
        row.back = back;
        let connected = !row.nodes.iter().any(|held| held.syspath == found.syspath);
        if connected {
            if row.nodes.is_empty() && !back {
                self.connections += 1;
                row.connected = self.connections;
            }
            row.nodes.push(Held {
                node: found.node.clone(),
                syspath: found.syspath.clone(),
                anchor: found.anchor.clone(),
                rebinding: false,
                raw: false,
            });
            row.bus = found.identifiers.bus;
        }
        if remembered || connected {
            changed.push(Changed { id, remembered });
        }
        changed
    }

    /// A node come on `anchor`, for the pad `id`: back, if a node of that
    /// anchor is away. Each node is away and back on its own, so one away
    /// entry is taken, the pad's own first, and failing that another
    /// row's: a pad back under another name than it went with. That row,
    /// left with nothing, is noted in `changed`.
    fn back(&mut self, id: &str, anchor: &Anchor, changed: &mut Vec<Changed>) -> bool {
        let take = |row: &mut Row| {
            let at = row.away.iter().position(|away| away.anchor == *anchor)?;
            row.away.remove(at);
            Some(())
        };
        if self.rows.get_mut(id).and_then(take).is_some() {
            return true;
        }
        let Some((other, row)) = self
            .rows
            .iter_mut()
            .find(|(_, row)| row.away.iter().any(|away| away.anchor == *anchor))
        else {
            return false;
        };
        take(row);
        if !row.connected() {
            changed.push(Changed {
                id: other.clone(),
                remembered: false,
            });
        }
        true
    }

    /// The hiding service has asked a re-bind of the device at `anchor`,
    /// and of the event nodes under it only those at `present` are still
    /// there. Each node under it that isn't is going for that re-bind,
    /// and is away when it goes. Nothing published changes.
    pub(crate) fn rebinding(&mut self, anchor: &Path, present: &[PathBuf]) {
        for held in self.rows.values_mut().flat_map(|row| &mut row.nodes) {
            if held.anchor.as_ref().is_some_and(|its| its.path == anchor)
                && !present.contains(&held.syspath)
            {
                held.rebinding = true;
            }
        }
    }

    /// The pad on the event node at `syspath` has gone. Returns its id if
    /// that leaves it no longer connected, as it is published: a pad of a
    /// row with another node, or another away, is still there. It stays
    /// remembered. Nothing for a node gone for a re-bind the hiding
    /// service asked, which leaves the pad away. The node is told by its
    /// place, not its name: a removal can be heard after another node has
    /// taken the name, at start, where what was queued as the list was
    /// made is heard after it.
    pub(crate) fn left(&mut self, syspath: &Path) -> Option<String> {
        let (id, row) = self
            .rows
            .iter_mut()
            .find(|(_, row)| row.nodes.iter().any(|held| held.syspath == syspath))?;
        let at = row.nodes.iter().position(|held| held.syspath == syspath)?;
        match row.nodes.remove(at).away() {
            Some(away) => {
                row.away.push(away);
                None
            }
            None => (!row.connected()).then(|| id.clone()),
        }
    }

    /// A setup is about to open the raw node of the pad on the event node
    /// at `syspath`, which has the pad's driver take that node away while
    /// it is open and make it anew after: the node's going is the pad
    /// away, and a node come on its anchor the pad back, as for a re-bind.
    /// Heard before any udev event the opening causes. A pad is held by
    /// one raw node at a time, so a mark left on another node of its row,
    /// as of the same pad on another transport, goes. Nothing published
    /// changes; nothing for a node the book doesn't hold.
    pub(crate) fn holding(&mut self, syspath: &Path) {
        let Some(row) = self
            .rows
            .values_mut()
            .find(|row| row.nodes.iter().any(|held| held.syspath == syspath))
        else {
            return;
        };
        for held in &mut row.nodes {
            held.raw = held.syspath == syspath;
        }
    }

    /// The raw node of the pad on the event node at `syspath` wasn't
    /// opened after all, or was let go with no effect, its driver never
    /// having taken the node away: a going of the node is again the
    /// pad's own.
    pub(crate) fn not_holding(&mut self, syspath: &Path) {
        if let Some(held) = self.held_at(syspath) {
            held.raw = false;
        }
    }

    fn held_at(&mut self, syspath: &Path) -> Option<&mut Held> {
        self.rows
            .values_mut()
            .flat_map(|row| &mut row.nodes)
            .find(|held| held.syspath == syspath)
    }

    /// The pad `id`, which a setup read through its raw node, has gone, as
    /// the input thread says: the node failed or hung up, or a receiver's
    /// slot said its pad left. What of it is away for the hold, or marked
    /// to go for it, goes with it, so that its next node there is a pad
    /// plugged in; a node of it on another transport, still there, stays.
    /// Returns whether that leaves it no longer connected.
    pub(crate) fn lost(&mut self, id: &str) -> bool {
        let Some(row) = self.rows.get_mut(id) else {
            return false;
        };
        let was = row.connected();
        row.nodes.retain(|held| !held.raw);
        row.away.retain(|away| !away.raw);
        was && !row.connected()
    }

    /// The device at `path` has gone, which unplugs each pad anchored to
    /// it or to a device under it: its nodes have gone before it. Returns
    /// each pad that leaves no longer connected, once: what was anchored
    /// there is no more.
    pub(crate) fn unplugged(&mut self, path: &Path) -> Vec<String> {
        let under = |anchor: &Anchor| anchor.path.starts_with(path);
        self.rows
            .iter_mut()
            .filter_map(|(id, row)| {
                let was = row.connected();
                row.away.retain(|away| !under(&away.anchor));
                row.nodes
                    .retain(|held| !held.anchor.as_ref().is_some_and(under));
                (was && !row.connected()).then(|| id.clone())
            })
            .collect()
    }

    /// Whether the pad on the event node `node`, at `syspath`, can rumble,
    /// learned by opening it after it arrived. Returns the pad if that is
    /// news; nothing for a node gone meanwhile.
    pub(crate) fn learned(&mut self, node: &Path, syspath: &Path, rumble: bool) -> Option<String> {
        let (id, row) = self.rows.iter_mut().find(|(_, row)| {
            row.nodes
                .iter()
                .any(|held| held.node == node && held.syspath == syspath)
        })?;
        let news = row.known.rumble != Some(rumble);
        row.known.rumble = Some(rumble);
        news.then(|| id.clone())
    }

    /// Puts the book right after the hotplug watch lost events: every pad
    /// `connected` now, and every device a pad can be `anchored` to now.
    /// A node not among the connected has gone: the pad is away if the
    /// node was going for a re-bind and its anchor is there, and it has
    /// left otherwise; a pad away whose anchor isn't there, or is another
    /// enumeration there, is unplugged. Returns each pad no longer
    /// connected. Each pad connected that the book hasn't taken in is
    /// then to be taken in as it [arrives](Self::arrived), back where a
    /// node of its anchor is away.
    pub(crate) fn reconcile(&mut self, connected: &[Found], anchors: &[Anchor]) -> Vec<String> {
        let there = |anchor: &Anchor| anchors.contains(anchor);
        self.rows
            .iter_mut()
            .filter_map(|(id, row)| {
                let was = row.connected();
                let (kept, gone): (Vec<Held>, Vec<Held>) = std::mem::take(&mut row.nodes)
                    .into_iter()
                    .partition(|held| {
                        connected
                            .iter()
                            .any(|found| found.node == held.node && found.syspath == held.syspath)
                    });
                row.nodes = kept;
                row.away.extend(gone.into_iter().filter_map(Held::away));
                row.away.retain(|away| there(&away.anchor));
                (was && !row.connected()).then(|| id.clone())
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

    /// Whether the pad `id` is a Steam Controller, as it is remembered:
    /// one is read through its raw node, connected or not.
    pub(crate) fn is_steam_controller(&self, id: &str) -> bool {
        self.rows.get(id).is_some_and(|row| {
            ctrlassist_device::is_steam_controller_model(row.known.vendor, row.known.product)
        })
    }

    /// The id of the pad `exact`, a pad's name as a profile gives it,
    /// names: a known pad's whose id it is in other capitals, as a serial
    /// typed so, or itself.
    pub(crate) fn known_as(&self, exact: &str) -> String {
        if self.rows.contains_key(exact) {
            return exact.to_string();
        }
        self.rows
            .keys()
            .find(|id| PadRef::names(exact, id))
            .cloned()
            .unwrap_or_else(|| exact.to_string())
    }

    /// The id of the pad `reference` names: an id; a name a profile takes
    /// for one pad, an address in either case among them; a bare address,
    /// as a profile takes a bare MAC address; an id's key segment; or
    /// `model:<vendor>:<product>` when one pad fits. Twelve hex digits are
    /// refused as a profile refuses them, as a MAC address without its
    /// colons.
    pub(crate) fn resolve(&self, reference: &str) -> Result<&str, WireError> {
        if let Some((id, _)) = self.rows.get_key_value(reference) {
            return Ok(id);
        }
        if is_bare_mac(reference.strip_prefix("address:").unwrap_or(reference)) {
            return Err(WireError::refusal(
                ErrorCode::UnknownPad,
                format!(
                    "{reference} looks like a MAC address without its colons: write it as 0a:1b:2c:3d:4e:5f"
                ),
            ));
        }
        let written = [reference.to_string(), format!("address:{reference}")]
            .into_iter()
            .find_map(|text| match PadRef::parse(&text) {
                Some(PadRef::Exact(exact)) => Some(exact),
                _ => None,
            });
        if let Some(exact) = written
            && let Some(id) = self.rows.keys().find(|id| PadRef::names(&exact, id))
        {
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
        // As it is published: one away is connected.
        if self.rows.get(&id).is_some_and(Row::connected) {
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

    /// Whether the pad is away: connected, as it is published, with no
    /// node there, each of its nodes gone for a re-bind.
    pub(crate) fn is_away(&self, id: &str) -> bool {
        self.rows
            .get(id)
            .is_some_and(|row| row.nodes.is_empty() && !row.away.is_empty())
    }

    /// A node the pad is connected on, if it is.
    pub(crate) fn node(&self, id: &str) -> Option<&Path> {
        self.rows
            .get(id)?
            .nodes
            .first()
            .map(|held| held.node.as_path())
    }

    /// A node the pad is connected on, if it is, with its place in sysfs,
    /// which tells it from a node given its name since.
    pub(crate) fn placed_node(&self, id: &str) -> Option<(&Path, &Path)> {
        self.rows
            .get(id)?
            .nodes
            .first()
            .map(|held| (held.node.as_path(), held.syspath.as_path()))
    }

    /// Whether the pad can be rumbled now, which `identify` asks and
    /// which is published: as its event node said when last asked, but
    /// for a Steam Controller a setup uses, which is rumbled through its
    /// raw node whatever its kernel driver has.
    pub(crate) fn rumble(&self, id: &str) -> Option<bool> {
        self.rows.get(id).and_then(Row::rumble)
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
            rumble: row.rumble(),
            bus: row.connected().then_some(row.bus),
        })
    }
}

#[cfg(test)]
mod tests {
    use ctrlassist_device::Identifiers;

    use super::*;

    /// The place in sysfs of a node at `node`. A re-bind makes nodes
    /// anew, and a node it makes may take an old node's name but never
    /// its place; most of these tests' nodes that come back under the
    /// same name have the same place, which tells nothing apart that
    /// matters there, and those that need it are [elsewhere](elsewhere).
    fn syspath(node: &str) -> PathBuf {
        Path::new("/sys/devices/test").join(Path::new(node).file_name().unwrap())
    }

    /// `found` on a node made since, which has taken its name.
    fn elsewhere(found: Found) -> Found {
        let name = found.syspath.file_name().unwrap().to_owned();
        Found {
            syspath: Path::new("/sys/devices/since").join(name),
            ..found
        }
    }

    fn found(node: &str, name: &str, address: Option<&str>, port: Option<&str>) -> Found {
        Found {
            node: PathBuf::from(node),
            syspath: syspath(node),
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
            book.arrived(&dualsense("/dev/input/event5")),
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
        assert_eq!(pad.rumble, None, "not yet learned");
        assert!(!pad.hidden && pad.setup.is_none());
        // What is learned by opening it follows; news once.
        let node = Path::new("/dev/input/event5");
        let at = syspath("/dev/input/event5");
        assert_eq!(book.learned(node, &at, true), Some(id.clone()));
        assert_eq!(book.learned(node, &at, true), None);
        assert_eq!(book.pad(&id).unwrap().rumble, Some(true));
        let elsewhere = syspath("/dev/input/event6");
        assert_eq!(book.learned(node, &elsewhere, false), None, "another node");
        assert_eq!(book.node(&id), Some(Path::new("/dev/input/event5")));
        assert_eq!(book.ids().collect::<Vec<_>>(), [id.as_str()]);

        // The same report again changes nothing.
        assert_eq!(book.arrived(&dualsense("/dev/input/event5")), []);

        assert_eq!(book.left(&syspath("/dev/input/event5")), Some(id.clone()));
        assert_eq!(book.left(&syspath("/dev/input/event5")), None);
        assert!(!book.pad(&id).unwrap().connected);
        assert_eq!(book.node(&id), None);
        assert_eq!(book.known().len(), 1);

        // Back on another node: connected again, nothing new to remember,
        // and the last answer kept until it is asked again.
        assert_eq!(
            book.arrived(&dualsense("/dev/input/event9")),
            [Changed {
                id: id.clone(),
                remembered: false
            }]
        );
        assert_eq!(book.rumble(&id), Some(true));
        assert_eq!(book.pad("address:none"), None);
        assert_eq!(book.rumble("address:none"), None);
    }

    /// A Steam Controller whose kernel driver has no force feedback can
    /// be rumbled while a setup uses it, through its raw node, and is
    /// published so; free, it can't. Another pad without rumble has none
    /// in a setup either.
    #[test]
    fn a_steam_controller_in_a_setup_can_rumble_whatever_its_driver_has() {
        let mut book = Book::new(Vec::new());
        let steam = Found {
            identifiers: Identifiers {
                name: "Steam Controller".to_string(),
                vendor: 0x28de,
                product: 0x1302,
                address: Some("FX12345678".to_string()),
                ..dualsense("/dev/input/event7").identifiers
            },
            ..dualsense("/dev/input/event7")
        };
        book.arrived(&steam);
        book.arrived(&dualsense("/dev/input/event5"));
        let steam_id = "address:FX12345678";
        let other = format!("address:{ADDRESS}");
        for (node, id) in [
            ("/dev/input/event7", steam_id),
            ("/dev/input/event5", &other),
        ] {
            book.learned(Path::new(node), &syspath(node), false);
            assert_eq!(book.rumble(id), Some(false), "{id}");
            book.claim(id, "couch");
        }
        assert_eq!(book.rumble(steam_id), Some(true));
        assert_eq!(book.pad(steam_id).unwrap().rumble, Some(true));
        assert_eq!(book.rumble(&other), Some(false));
        assert_eq!(book.release("couch").len(), 2);
        assert_eq!(book.rumble(steam_id), Some(false));
        let remembered = book.known().into_iter().find(|known| known.id == steam_id);
        assert_eq!(
            remembered.unwrap().rumble,
            Some(false),
            "what is remembered"
        );
    }

    /// The protocol's list of the Steam Controller's numbers, which the
    /// clients word things by, is the device crate's.
    #[test]
    fn the_clients_know_a_steam_controller_as_the_daemon_does() {
        for vendor in [0x28de, 0x054c] {
            for product in 0..=u16::MAX {
                assert_eq!(
                    payload::is_steam_controller(vendor, product),
                    ctrlassist_device::is_steam_controller_model(vendor, product),
                    "{vendor:04x}:{product:04x}"
                );
            }
        }
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
        let changed = book.arrived(&dualsense("/dev/input/event5"));
        assert!(changed[0].remembered, "its name is news");
        assert_eq!(book.known()[0].name, "DualSense");
        let at = syspath("/dev/input/event5");
        let learned = book.learned(Path::new("/dev/input/event5"), &at, false);
        assert_eq!(learned.as_deref(), Some(known.id.as_str()));
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
        assert_eq!(book.arrived(&own), []);
        assert!(book.known().is_empty());
    }

    #[test]
    fn two_pads_that_report_nothing_apart_share_a_row() {
        let mut book = Book::default();
        let first = found("/dev/input/event5", "Pad", None, None);
        let second = found("/dev/input/event6", "Pad", None, None);
        let id = book.arrived(&first).remove(0).id;
        assert_eq!(id, "model:054c:0ce6:Pad");
        assert_eq!(
            book.arrived(&second),
            [Changed {
                id: id.clone(),
                remembered: false
            }]
        );
        assert_eq!(book.known().len(), 1);
        // One going leaves the row connected, and isn't told as gone.
        assert_eq!(book.left(&syspath("/dev/input/event5")), None);
        assert!(book.pad(&id).unwrap().connected, "one of the two is left");
        assert_eq!(book.node(&id), Some(Path::new("/dev/input/event6")));
        assert_eq!(book.left(&syspath("/dev/input/event6")), Some(id.clone()));
        assert!(!book.pad(&id).unwrap().connected);
    }

    /// A pad's address can arrive after the pad does: the node then
    /// belongs to the pad it turned out to be.
    #[test]
    fn a_node_that_reports_another_name_leaves_the_pad_it_was() {
        let mut book = Book::default();
        let early = found("/dev/input/event5", "DualSense", None, Some("usb-1/input0"));
        let was = book.arrived(&early).remove(0).id;
        let changed = book.arrived(&dualsense("/dev/input/event5"));
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
        book.arrived(&dualsense("/dev/input/event5"));
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
        book.arrived(&other);
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
        book.arrived(&second);
        book.arrived(&first);
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
        book.left(&syspath("/dev/input/event6"));
        let refused = book.forget("aa:bb:cc:dd:ee:02").unwrap_err();
        assert_eq!(refused.code, ErrorCode::PadInUse);

        // One that left and returned is the latest to connect.
        assert_eq!(book.release("couch"), ["address:aa:bb:cc:dd:ee:02"]);
        assert!(!book.pad("address:aa:bb:cc:dd:ee:02").unwrap().hidden);
        assert_eq!(book.release("couch"), Vec::<String>::new());
        book.arrived(&second);
        assert_eq!(
            book.fitting(&model),
            ["address:aa:bb:cc:dd:ee:01", "address:aa:bb:cc:dd:ee:02"]
        );
    }

    #[test]
    fn only_a_pad_that_is_not_connected_can_be_forgotten() {
        let mut book = Book::default();
        book.arrived(&dualsense("/dev/input/event5"));
        let refused = book.forget(ADDRESS).unwrap_err();
        assert_eq!(refused.code, ErrorCode::PadConnected);
        book.left(&syspath("/dev/input/event5"));
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

    /// A pad whose node goes for a re-bind the hiding service asked, as
    /// many times as it asks, is away: still connected, nothing
    /// published, and no node to open. Back on the same anchor, under
    /// another node as it may be, it isn't a pad plugged in. Its anchor's
    /// removal, after its node's, is its unplugging, told once.
    #[test]
    fn a_pad_re_bound_at_the_service_s_request_never_left() {
        let mut book = Book::default();
        let hid = anchor(HID, 1);
        let id = book.arrived(&on("/dev/input/event5", &hid)).remove(0).id;
        assert!(!book.was_restored(&id), "plugged in");
        for (went, back) in [("event5", "event7"), ("event7", "event5")] {
            // As the request is heard, the new node is there already.
            let node = format!("/dev/input/{back}");
            book.rebinding(&hid.path, &[syspath(&node)]);
            assert_eq!(book.left(&syspath(&format!("/dev/input/{went}"))), None);
            let away = book.pad(&id).unwrap();
            assert!(away.connected && away.bus.is_some());
            assert_eq!(book.node(&id), None, "not there to be used meanwhile");
            assert_eq!(book.arrived(&on(&node, &hid)).len(), 1);
            assert!(book.was_restored(&id), "back");
            assert!(!book.was_restored(&id), "asked once");
        }

        // Unplugged in the moment of a re-bind: its node goes first, and
        // then its anchor, which unplugs it, away as it is.
        book.rebinding(&hid.path, &[]);
        assert_eq!(book.left(&syspath("/dev/input/event5")), None);
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
        book.arrived(&on("/dev/input/event5", &again));
        assert!(!book.was_restored(&id), "plugged in");
    }

    /// A pad on a wireless receiver, switched off: its node goes and its
    /// anchor, the receiver, stays, with no request for a re-bind. It has
    /// left, and switched on again it is a pad plugged in. So with a pad
    /// re-bound by anyone but the hiding service.
    #[test]
    fn a_node_gone_with_no_request_has_left() {
        let mut book = Book::default();
        let receiver = anchor("/sys/devices/usb1/1-1/1-1:1.0", 1);
        let id = book
            .arrived(&on("/dev/input/event5", &receiver))
            .remove(0)
            .id;
        assert_eq!(book.left(&syspath("/dev/input/event5")), Some(id.clone()));
        let gone = book.pad(&id).unwrap();
        assert!(!gone.connected && gone.bus.is_none());
        book.arrived(&on("/dev/input/event6", &receiver));
        assert!(!book.was_restored(&id), "plugged in");
        assert!(book.pad(&id).unwrap().connected);
    }

    /// The request marks only the nodes it took away, those gone as it is
    /// heard, and only under the device it names: a node still there, as
    /// one is when the rule didn't act, or another pad's, that goes later
    /// has left. Each mark goes with its node.
    #[test]
    fn a_request_marks_only_the_nodes_it_took() {
        let mut book = Book::default();
        let (receiver, other) = (
            anchor("/sys/devices/usb1/1-1/1-1:1.0", 1),
            anchor("/sys/devices/usb1/1-2/1-2:1.0", 1),
        );
        let pad = |node: &str, anchor: &Anchor, address: &str| Found {
            anchor: Some(anchor.clone()),
            ..found(node, "Pad", Some(address), None)
        };
        let id = book
            .arrived(&pad("/dev/input/event5", &receiver, "aa:00:00:00:00:01"))
            .remove(0)
            .id;
        let next = book
            .arrived(&pad("/dev/input/event6", &other, "aa:00:00:00:00:02"))
            .remove(0)
            .id;
        // A request the rule didn't act on: the node is still there.
        book.rebinding(&receiver.path, &[syspath("/dev/input/event5")]);
        assert_eq!(book.left(&syspath("/dev/input/event5")), Some(id.clone()));
        // Another device's request leaves this one's nodes unmarked.
        book.rebinding(&receiver.path, &[]);
        assert_eq!(book.left(&syspath("/dev/input/event6")), Some(next));

        // A mark used goes with its node: the node made by the re-bind,
        // switched off later, has left.
        book.arrived(&pad("/dev/input/event5", &receiver, "aa:00:00:00:00:01"));
        book.rebinding(&receiver.path, &[syspath("/dev/input/event7")]);
        assert_eq!(book.left(&syspath("/dev/input/event5")), None, "away");
        book.arrived(&pad("/dev/input/event7", &receiver, "aa:00:00:00:00:01"));
        assert!(book.was_restored(&id));
        assert_eq!(book.left(&syspath("/dev/input/event7")), Some(id.clone()));
        assert!(!book.pad(&id).unwrap().connected);
    }

    /// Two requests heard back to back, as a hide and the showing again
    /// written before udev ran the first, both before any node goes: the
    /// pad is away once and back once, the node the first re-bind made
    /// being gone before udev sent its arrival, and never taken in. And
    /// the second heard once the first's node is back: away and back for
    /// each.
    #[test]
    fn requests_back_to_back_leave_the_pad_away_for_each() {
        let mut book = Book::default();
        let hid = anchor(HID, 1);
        let id = book.arrived(&on("/dev/input/event5", &hid)).remove(0).id;
        let last = syspath("/dev/input/event7");
        book.rebinding(&hid.path, std::slice::from_ref(&last));
        book.rebinding(&hid.path, std::slice::from_ref(&last));
        assert_eq!(book.left(&syspath("/dev/input/event5")), None, "away");
        // The first re-bind's node, never taken in, goes.
        assert_eq!(book.left(&syspath("/dev/input/event6")), None);
        assert!(book.pad(&id).unwrap().connected);
        book.arrived(&on("/dev/input/event7", &hid));
        assert!(book.was_restored(&id), "back");

        // One after another as heard.
        for (went, back) in [("event7", "event8"), ("event8", "event9")] {
            let node = format!("/dev/input/{back}");
            book.rebinding(&hid.path, &[syspath(&node)]);
            assert_eq!(book.left(&syspath(&format!("/dev/input/{went}"))), None);
            assert!(book.pad(&id).unwrap().connected);
            book.arrived(&on(&node, &hid));
            assert!(book.was_restored(&id), "back from {went}");
        }
    }

    /// The device above an anchor going unplugs the pad too, its own
    /// removal unheard of or not; one at the anchor's place enumerated
    /// afresh is another device, its going unheard of.
    #[test]
    fn an_anchor_gone_another_way_unplugs_the_pad() {
        let mut book = Book::default();
        let interface = anchor("/sys/devices/usb1/1-1/1-1:1.0", 1);
        let id = book
            .arrived(&on("/dev/input/event5", &interface))
            .remove(0)
            .id;
        book.rebinding(&interface.path, &[]);
        book.left(&syspath("/dev/input/event5"));
        assert_eq!(
            book.unplugged(Path::new("/sys/devices/usb1/1-1")),
            std::slice::from_ref(&id)
        );
        assert!(!book.pad(&id).unwrap().connected);

        // Away, and then a pad at its port that is another enumeration.
        book.arrived(&on("/dev/input/event5", &interface));
        book.rebinding(&interface.path, &[]);
        book.left(&syspath("/dev/input/event5"));
        book.arrived(&on(
            "/dev/input/event6",
            &anchor(&interface.path.to_string_lossy(), 2),
        ));
        assert!(!book.was_restored(&id), "plugged in");
        book.rebinding(&interface.path, &[]);
        assert_eq!(book.left(&syspath("/dev/input/event6")), None);
        assert_eq!(book.unplugged(&interface.path), [id]);
    }

    /// A pad whose anchor couldn't be read is its own: its node going is
    /// its unplugging, and its node coming its plugging in.
    #[test]
    fn a_pad_without_an_anchor_is_unplugged_with_its_node() {
        let mut book = Book::default();
        let id = book.arrived(&dualsense("/dev/input/event5")).remove(0).id;
        assert_eq!(book.left(&syspath("/dev/input/event5")), Some(id.clone()));
        assert!(!book.pad(&id).unwrap().connected);
        book.arrived(&dualsense("/dev/input/event5"));
        assert!(!book.was_restored(&id));
    }

    /// Two pads that share a row are told apart by their anchors: one
    /// away and back while the other is unplugged, each is seen for what
    /// it does, and the row is connected while either is, and told as
    /// gone only once neither is.
    #[test]
    fn two_pads_of_one_row_are_told_apart_by_their_anchors() {
        let mut book = Book::default();
        let (one, other) = (anchor(HID, 1), anchor("/sys/devices/usb1/1-2/1-2:1.0", 1));
        let pad = |node: &str, anchor: &Anchor| Found {
            anchor: Some(anchor.clone()),
            ..found(node, "Pad", None, None)
        };
        let id = book.arrived(&pad("/dev/input/event5", &one)).remove(0).id;
        book.arrived(&pad("/dev/input/event6", &other));
        book.rebinding(&one.path, &[]);
        book.rebinding(&other.path, &[]);
        assert_eq!(book.left(&syspath("/dev/input/event5")), None, "away");
        assert_eq!(book.left(&syspath("/dev/input/event6")), None, "away");
        // Unplugged, the other leaves the row connected through the one
        // away, and isn't told as gone.
        assert!(book.unplugged(&other.path).is_empty());
        assert!(book.pad(&id).unwrap().connected, "the one away is there");
        book.arrived(&pad("/dev/input/event7", &one));
        assert!(book.was_restored(&id), "back, not plugged in");
        book.rebinding(&one.path, &[]);
        book.left(&syspath("/dev/input/event7"));
        assert_eq!(book.unplugged(&one.path), std::slice::from_ref(&id));
        assert!(!book.pad(&id).unwrap().connected);
    }

    /// Two joystick nodes on one anchor, as a twin-joystick adapter has,
    /// each away and back on its own through a re-bind: reporting
    /// nothing apart, they share a row, and the second back is back as
    /// the first is, not a pad plugged in.
    #[test]
    fn two_nodes_of_one_anchor_and_one_row_are_each_back() {
        let mut book = Book::default();
        let hid = anchor(HID, 1);
        let pad = |node: &str| Found {
            anchor: Some(hid.clone()),
            ..found(node, "Twin", None, None)
        };
        let id = book.arrived(&pad("/dev/input/event5")).remove(0).id;
        assert_eq!(book.arrived(&pad("/dev/input/event6")).len(), 1);
        book.was_restored(&id);
        let new = [syspath("/dev/input/event7"), syspath("/dev/input/event8")];
        book.rebinding(&hid.path, &new);
        assert_eq!(book.left(&syspath("/dev/input/event5")), None);
        assert_eq!(book.left(&syspath("/dev/input/event6")), None);
        for node in ["/dev/input/event7", "/dev/input/event8"] {
            book.arrived(&pad(node));
            assert!(book.was_restored(&id), "{node} back");
            assert!(book.pad(&id).unwrap().connected);
        }
        assert!(book.rows[&id].away.is_empty(), "none left away");
    }

    /// Two joystick nodes on one anchor that report different names,
    /// each its own row: through a re-bind each is back in its own row,
    /// and neither is published as gone meanwhile.
    #[test]
    fn two_nodes_of_one_anchor_and_two_rows_are_each_back() {
        let mut book = Book::default();
        let hid = anchor(HID, 1);
        let pad = |node: &str, address: &str| Found {
            anchor: Some(hid.clone()),
            ..found(node, "Twin", Some(address), None)
        };
        let (a, b) = ("aa:00:00:00:00:01", "aa:00:00:00:00:02");
        let one = book.arrived(&pad("/dev/input/event5", a)).remove(0).id;
        let two = book.arrived(&pad("/dev/input/event6", b)).remove(0).id;
        book.rebinding(&hid.path, &[]);
        assert_eq!(book.left(&syspath("/dev/input/event5")), None);
        assert_eq!(book.left(&syspath("/dev/input/event6")), None);
        let changed = book.arrived(&pad("/dev/input/event7", a));
        assert_eq!(
            changed,
            [Changed {
                id: one.clone(),
                remembered: false
            }]
        );
        assert!(book.was_restored(&one));
        assert!(book.pad(&two).unwrap().connected, "still away, not gone");
        let changed = book.arrived(&pad("/dev/input/event8", b));
        assert_eq!(
            changed,
            [Changed {
                id: two.clone(),
                remembered: false
            }]
        );
        assert!(book.was_restored(&two), "back, not plugged in");
        assert!(book.rows[&one].away.is_empty() && book.rows[&two].away.is_empty());
    }

    /// A pad already connected that reports a change is no node come: it
    /// takes no other node's place away, so that node's return is still
    /// its return.
    #[test]
    fn a_change_of_a_node_there_is_no_return() {
        let mut book = Book::default();
        let hid = anchor(HID, 1);
        let pad = |node: &str| Found {
            anchor: Some(hid.clone()),
            ..found(node, "Twin", None, None)
        };
        let id = book.arrived(&pad("/dev/input/event5")).remove(0).id;
        book.arrived(&pad("/dev/input/event6"));
        book.rebinding(&hid.path, &[syspath("/dev/input/event5")]);
        assert_eq!(book.left(&syspath("/dev/input/event6")), None);
        assert_eq!(book.arrived(&pad("/dev/input/event5")), []);
        assert!(!book.was_restored(&id));
        book.arrived(&pad("/dev/input/event7"));
        assert!(book.was_restored(&id), "back");
    }

    /// After the hotplug watch lost events, what is connected and which
    /// anchors are there put the book right: a pad unplugged is gone, as
    /// is one away whose anchor is another enumeration now; a pad whose
    /// node went for a re-bind is away, and back as its new node is taken
    /// in; a receiver's pad switched off has left; a pad still there is
    /// as it was. Only what is no longer connected is told. What was
    /// queued before the loss is heard before the look, the request for
    /// the re-bind among it; what is heard after it came after the loss,
    /// and may be in the look already, which changes nothing.
    #[test]
    fn an_overrun_is_put_right_by_what_is_connected() {
        let mut book = Book::default();
        let at = |port: u8| anchor(&format!("/sys/devices/usb1/1-{port}/1-{port}:1.0"), 1);
        let pad = |node: &str, port: u8| Found {
            anchor: Some(at(port)),
            ..found(node, "Pad", Some(&format!("aa:00:00:00:00:0{port}")), None)
        };
        let mut id = |node: &str, port: u8| book.arrived(&pad(node, port)).remove(0).id;
        let unplugged = id("/dev/input/event1", 1);
        let replugged = id("/dev/input/event2", 2);
        let rebound = id("/dev/input/event3", 3);
        let switched_off = id("/dev/input/event4", 4);
        let kept = id("/dev/input/event5", 5);
        // Queued before the loss.
        book.rebinding(&at(2).path, &[]);
        assert_eq!(book.left(&syspath("/dev/input/event2")), None);
        book.rebinding(&at(3).path, &[syspath("/dev/input/event6")]);

        // The rest went unheard, and a pad plugged in since has the
        // unplugged one's node.
        let newcomer = elsewhere(pad("/dev/input/event1", 6));
        let connected = [
            pad("/dev/input/event5", 5),
            pad("/dev/input/event6", 3),
            newcomer.clone(),
        ];
        let replaced = Anchor {
            enumeration: Some(2),
            ..at(2)
        };
        let anchors = [replaced, at(3), at(4), at(5), at(6)];
        let mut gone = book.reconcile(&connected, &anchors);
        gone.sort();
        let mut told = vec![unplugged.clone(), replugged, switched_off.clone()];
        told.sort();
        assert_eq!(gone, told);
        assert!(book.pad(&rebound).unwrap().connected, "away");
        assert_eq!(book.node(&rebound), None);

        for found in &connected {
            book.arrived(found);
        }
        assert!(book.was_restored(&rebound), "back");
        assert!(!book.was_restored(&kept));
        assert_eq!(book.node(&kept), Some(Path::new("/dev/input/event5")));
        assert!(!book.pad(&switched_off).unwrap().connected);
        let newcomer_id = newcomer.identifiers.id().to_string();
        assert!(!book.was_restored(&newcomer_id), "plugged in");

        // Heard after the look, which has them already: the unplugged
        // pad's going leaves the newcomer on its node, and the arrivals
        // are of pads taken in.
        assert_eq!(book.left(&syspath("/dev/input/event1")), None);
        assert_eq!(book.left(&syspath("/dev/input/event4")), None);
        for found in &connected {
            assert_eq!(book.arrived(found), [], "{}", found.node.display());
        }
        assert!(book.pad(&newcomer_id).unwrap().connected);
        assert_eq!(
            book.node(&newcomer_id),
            Some(Path::new("/dev/input/event1"))
        );
        assert!(!book.was_restored(&rebound), "back once");

        // Switched on again, its anchor never gone: a pad plugged in.
        book.arrived(&pad("/dev/input/event4", 4));
        assert!(!book.was_restored(&switched_off));
        assert_eq!(
            book.reconcile(&[], &anchors).len(),
            4,
            "each that was connected"
        );
        assert!(!book.pad(&unplugged).unwrap().connected);
    }

    /// A removal heard after another pad's node has taken the name, as at
    /// start, where what was queued as the list was made is heard after
    /// it, leaves the pad on the node now as it is.
    #[test]
    fn a_late_removal_leaves_the_pad_on_the_node_now() {
        let mut book = Book::default();
        let now = elsewhere(found(
            "/dev/input/event5",
            "Other",
            Some("aa:bb:cc:dd:ee:ff"),
            None,
        ));
        let id = book.arrived(&now).remove(0).id;
        assert_eq!(book.left(&syspath("/dev/input/event5")), None);
        assert!(book.pad(&id).unwrap().connected);
        assert_eq!(book.left(&now.syspath), Some(id));
    }

    /// After a loss, the request for a re-bind can be read before the
    /// look with the node it took already gone, that node's going lost:
    /// and the node the re-bind made can be read arriving before the look
    /// too, as the kernel queues what udev sends again once the backlog
    /// has been read empty, and the monitor says the loss only once
    /// nothing is left to read. The arrival says the node marked has
    /// gone: the pad is back, not plugged in, and the look leaves it
    /// nothing away, so that switched off later it has left. So whether
    /// the node made has the old one's name or another.
    #[test]
    fn a_pad_back_before_the_look_after_a_loss_is_left_nothing_away() {
        for name in ["/dev/input/event6", "/dev/input/event5"] {
            let mut book = Book::default();
            let receiver = anchor("/sys/devices/usb1/1-1/1-1:1.0", 1);
            let id = book
                .arrived(&on("/dev/input/event5", &receiver))
                .remove(0)
                .id;
            // From the backlog, the node gone as it is read.
            book.rebinding(&receiver.path, &[]);
            // Its going lost, and the node made read before the look.
            let back = elsewhere(on(name, &receiver));
            book.arrived(&back);
            assert!(book.was_restored(&id), "{name} back, not plugged in");
            let gone = book.reconcile(std::slice::from_ref(&back), std::slice::from_ref(&receiver));
            assert!(gone.is_empty(), "{gone:?}");
            assert_eq!(book.arrived(&back), [], "taken in");
            assert!(book.rows[&id].away.is_empty(), "{name}: nothing away");
            // Switched off later.
            assert_eq!(book.left(&back.syspath), Some(id.clone()), "{name}");
            assert!(!book.pad(&id).unwrap().connected);
        }

        // Two nodes of one anchor and one row, one's going lost: each is
        // back as its own node comes.
        let mut book = Book::default();
        let hid = anchor(HID, 1);
        let pad = |node: &str| Found {
            anchor: Some(hid.clone()),
            ..found(node, "Twin", None, None)
        };
        let id = book.arrived(&pad("/dev/input/event5")).remove(0).id;
        book.arrived(&pad("/dev/input/event6"));
        book.rebinding(&hid.path, &[]);
        assert_eq!(book.left(&syspath("/dev/input/event5")), None);
        for node in ["/dev/input/event7", "/dev/input/event8"] {
            book.arrived(&pad(node));
            assert!(book.was_restored(&id), "{node} back");
        }
        assert!(book.rows[&id].away.is_empty(), "none left away");
        assert_eq!(book.rows[&id].nodes.len(), 2);
    }

    /// A node arriving under the name of one held at another place says
    /// the one held has gone, its going unheard, udev sending the one
    /// before the other. Not marked for a re-bind, it has left: the node
    /// come is a pad plugged in, told by its own place from then on. Held
    /// for another pad, that pad has left.
    #[test]
    fn a_node_come_under_a_held_name_says_the_one_held_has_gone() {
        let mut book = Book::default();
        let receiver = anchor("/sys/devices/usb1/1-1/1-1:1.0", 1);
        let first = on("/dev/input/event5", &receiver);
        let id = book.arrived(&first).remove(0).id;
        let again = elsewhere(first.clone());
        assert_eq!(
            book.arrived(&again),
            [Changed {
                id: id.clone(),
                remembered: false
            }]
        );
        assert!(!book.was_restored(&id), "plugged in");
        assert_eq!(book.rows[&id].nodes.len(), 1);
        assert_eq!(
            book.learned(&again.node, &again.syspath, true),
            Some(id.clone())
        );
        assert_eq!(book.left(&first.syspath), None, "the one before");
        assert_eq!(book.left(&again.syspath), Some(id.clone()));

        // Another pad's, under its name.
        book.arrived(&first);
        let other = elsewhere(found(
            "/dev/input/event5",
            "Other",
            Some("aa:bb:cc:dd:ee:ff"),
            None,
        ));
        let changed = book.arrived(&other);
        assert_eq!(
            changed[0],
            Changed {
                id: id.clone(),
                remembered: false
            }
        );
        assert!(!book.pad(&id).unwrap().connected);
        assert!(book.pad(&changed[1].id).unwrap().connected);
    }

    /// A pad away that comes back reporting another name, as one whose
    /// address comes late does, is back all the same, and the row it
    /// was away from is left.
    #[test]
    fn a_pad_back_under_another_name_is_back() {
        let mut book = Book::default();
        let hid = anchor(HID, 1);
        let id = book.arrived(&on("/dev/input/event5", &hid)).remove(0).id;
        book.rebinding(&hid.path, &[]);
        book.left(&syspath("/dev/input/event5"));
        let early = Found {
            anchor: Some(hid),
            ..found("/dev/input/event5", "DualSense", None, Some("usb-1/input0"))
        };
        let changed = book.arrived(&early);
        assert_eq!(changed[0].id, id, "left");
        assert!(!book.pad(&id).unwrap().connected);
        assert!(book.was_restored(&changed[1].id), "back");
    }

    /// A pad whose raw node a setup holds, as a Steam Controller's: its
    /// node, marked before the raw node is opened, goes as the driver
    /// takes it away, and the pad is away, connected and nothing
    /// published, as many times as it is held. Let go, its node comes back
    /// on the same anchor, under another place and perhaps the same name:
    /// back, not plugged in, which starts nothing. Its anchor's going
    /// while it is away unplugs it.
    #[test]
    fn a_pad_whose_raw_node_is_held_never_left() {
        let mut book = Book::default();
        let hid = anchor(HID, 1);
        let first = on("/dev/input/event5", &hid);
        let id = book.arrived(&first).remove(0).id;
        let mut at = first.syspath;
        for round in 0..3 {
            book.holding(&at);
            assert_eq!(book.left(&at), None, "round {round}: away");
            let away = book.pad(&id).unwrap();
            assert!(away.connected && away.bus.is_some());
            assert!(book.is_away(&id) && book.node(&id).is_none());
            let back = elsewhere(on("/dev/input/event5", &hid));
            let back = Found {
                syspath: back.syspath.join(round.to_string()),
                ..back
            };
            assert_eq!(book.arrived(&back).len(), 1);
            assert!(book.was_restored(&id), "round {round}: back");
            at = back.syspath;
        }
        book.holding(&at);
        assert_eq!(book.left(&at), None);
        assert_eq!(book.unplugged(&hid.path), std::slice::from_ref(&id));
        assert!(!book.pad(&id).unwrap().connected);
    }

    /// A raw node that wasn't opened after all leaves the node where it
    /// is, and its going later is the pad's own; a mark on a node the
    /// book doesn't hold marks nothing.
    #[test]
    fn a_raw_node_not_opened_leaves_the_node_the_pad_s() {
        let mut book = Book::default();
        let hid = anchor(HID, 1);
        let found = on("/dev/input/event5", &hid);
        let id = book.arrived(&found).remove(0).id;
        book.holding(Path::new("/sys/devices/elsewhere/event9"));
        book.holding(&found.syspath);
        book.not_holding(&found.syspath);
        assert_eq!(book.left(&found.syspath), Some(id.clone()));
        assert!(!book.pad(&id).unwrap().connected);
    }

    /// A pad held by its raw node that the input thread says has gone is
    /// gone, whether its node's going was heard yet or not; its next node
    /// is a pad plugged in, and a late removal of the old one says
    /// nothing. One whose new node came first, as when what reads it
    /// failed and let it go, is there on that node.
    #[test]
    fn a_pad_lost_while_its_raw_node_is_held_is_gone() {
        let hid = anchor(HID, 1);
        for heard in [true, false] {
            let mut book = Book::default();
            let found = on("/dev/input/event5", &hid);
            let id = book.arrived(&found).remove(0).id;
            book.holding(&found.syspath);
            if heard {
                assert_eq!(book.left(&found.syspath), None);
            }
            assert!(book.lost(&id), "heard: {heard}");
            assert!(!book.pad(&id).unwrap().connected);
            assert_eq!(book.left(&found.syspath), None, "said once");
            assert!(!book.lost(&id), "once");
            book.arrived(&elsewhere(on("/dev/input/event5", &hid)));
            assert!(!book.was_restored(&id), "plugged in");
        }

        let mut book = Book::default();
        let found = on("/dev/input/event5", &hid);
        let id = book.arrived(&found).remove(0).id;
        book.holding(&found.syspath);
        book.left(&found.syspath);
        let back = elsewhere(on("/dev/input/event6", &hid));
        book.arrived(&back);
        assert!(book.was_restored(&id));
        assert!(!book.lost(&id), "there on its new node");
        assert_eq!(book.node(&id), Some(Path::new("/dev/input/event6")));
        assert!(!book.lost("address:none"));
    }

    /// One pad on two transports, one row and two nodes: a hold of one
    /// node's raw node takes the mark off the other, and the pad lost
    /// through the one held is still there on the other.
    #[test]
    fn a_hold_and_its_loss_are_of_one_node_of_the_pad() {
        let mut book = Book::default();
        let (cable, bluetooth) = (
            anchor(HID, 1),
            anchor("/sys/devices/virtual/misc/uhid/0005", 1),
        );
        let on_cable = on("/dev/input/event5", &cable);
        let on_bluetooth = on("/dev/input/event6", &bluetooth);
        let id = book.arrived(&on_cable).remove(0).id;
        book.arrived(&on_bluetooth);
        book.holding(&on_bluetooth.syspath);
        book.holding(&on_cable.syspath);
        assert_eq!(book.left(&on_cable.syspath), None, "away");
        assert!(!book.lost(&id), "there on Bluetooth");
        assert_eq!(book.node(&id), Some(Path::new("/dev/input/event6")));
        assert!(book.rows[&id].away.is_empty(), "the hold's away went");
        // Its mark went as the other was held: its going is the pad's.
        assert_eq!(book.left(&on_bluetooth.syspath), Some(id.clone()));

        // Away on one for a re-bind and lost on the other: the re-bind's
        // stays, and the pad is back from it.
        let mut book = Book::default();
        let id = book.arrived(&on_cable).remove(0).id;
        book.arrived(&on_bluetooth);
        book.rebinding(&bluetooth.path, &[]);
        assert_eq!(book.left(&on_bluetooth.syspath), None);
        book.holding(&on_cable.syspath);
        assert_eq!(book.left(&on_cable.syspath), None);
        assert!(!book.lost(&id), "away for the re-bind");
        assert_eq!(book.rows[&id].away.len(), 1);
        book.arrived(&elsewhere(on("/dev/input/event6", &bluetooth)));
        assert!(book.was_restored(&id));
    }

    /// After the hotplug watch lost events, a node marked for a hold that
    /// went is the pad away, its anchor being there, and the node made
    /// since is the pad back; a mark on a node still there stays.
    #[test]
    fn an_overrun_takes_a_held_node_gone_for_the_pad_away() {
        let mut book = Book::default();
        let hid = anchor(HID, 1);
        let found = on("/dev/input/event5", &hid);
        let id = book.arrived(&found).remove(0).id;
        book.holding(&found.syspath);
        assert!(book.reconcile(&[], std::slice::from_ref(&hid)).is_empty());
        assert!(book.pad(&id).unwrap().connected && book.is_away(&id));
        let back = elsewhere(on("/dev/input/event5", &hid));
        book.arrived(&back);
        assert!(book.was_restored(&id));

        book.holding(&back.syspath);
        assert!(
            book.reconcile(std::slice::from_ref(&back), std::slice::from_ref(&hid))
                .is_empty()
        );
        assert_eq!(book.left(&back.syspath), None, "still marked");
    }

    /// A pad is named in a reference as a profile names it: a serial in
    /// either case, a MAC address in capitals; twelve hex digits are a MAC
    /// address without its colons, refused with how to write it.
    #[test]
    fn a_reference_is_read_as_a_profile_reads_it() {
        let mut book = Book::default();
        let mut found = dualsense("/dev/input/event5");
        found.identifiers.address = Some("FX12AB34CD".to_string());
        book.arrived(&found);
        book.arrived(&dualsense("/dev/input/event6"));
        for reference in ["address:FX12AB34CD", "address:fx12ab34cd", "fx12ab34cd"] {
            assert_eq!(
                book.resolve(reference),
                Ok("address:FX12AB34CD"),
                "{reference}"
            );
        }
        assert_eq!(book.known_as("address:fx12ab34cd"), "address:FX12AB34CD");
        assert_eq!(book.known_as("address:nobody"), "address:nobody");
        assert_eq!(
            book.resolve(&ADDRESS.to_ascii_uppercase()),
            Ok(format!("address:{ADDRESS}").as_str())
        );
        let bare = book.resolve("0A1B2C3D4E14").unwrap_err();
        assert!(bare.message.contains("without its colons"), "{bare:?}");
    }

    /// A pad being shown again is published as hidden until that is
    /// done, as many times as it is under way.
    #[test]
    fn a_pad_is_hidden_until_it_is_shown_again() {
        let mut book = Book::default();
        let id = book.arrived(&dualsense("/dev/input/event5")).remove(0).id;
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
