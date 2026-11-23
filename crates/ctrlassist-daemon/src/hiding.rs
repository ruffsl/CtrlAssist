//! How a setup's pads are kept from games: by the hiding service where
//! it is there, and by a grab on each pad where it isn't.
//!
//! The service is another process, and one that may never answer. So no
//! request to it holds up anything but the work that asked it, and that
//! work can be ended at any time:
//!
//! - **Each pad the service hides is hidden on a connection of its own,**
//!   which every later request about that pad goes on, one at a time,
//!   and each other request on a connection made for it. Nothing is held
//!   across a round trip but that connection, so a request that never
//!   returns holds that one pad, never another setup's.
//! - **A setup's requests are its [`Errand`].** Ending it ends every
//!   request it has under way at once, and anything it asks after is
//!   refused without asking. A hide is given up with a `cancel`, which
//!   has the service leave the pad as it was and say what that took (see
//!   below); any other request, and a hide asked of a service from before
//!   `cancel`, by hanging up its connection, after which the service
//!   shows again whatever was hidden through it, as it does for any
//!   connection that closes. A setup's errand ends when it is stopped,
//!   fails or is given up.
//! - **As the daemon stops, every request is hung up** and none is made
//!   after, so nothing it waits on is a round trip to the service.
//!
//! **Each re-bind the service makes goes and comes back, and the daemon
//! counts them.** The book is told to expect a going and coming back for
//! each hide before it is asked, and one more for a hide that is given up
//! as it is asked, for the showing again that follows: so neither is
//! published, nor taken for a pad plugged in, whenever its events come.
//! What the service then says each took sets the count right: a hide
//! refused before acting re-bound nothing, and one given up says, once
//! [settled](Hiding::settle), whether it had re-bound the pad, and whether
//! the pad was then shown again by another re-bind. Settling waits on the
//! service, and nothing else waits on settling.

use std::collections::{BTreeMap, BTreeSet};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use ctrlassist_hider::{Client, Error, Hid, Refusal};
use ctrlassist_protocol::payload;
use rustix::event::{EventfdFlags, eventfd};
use tracing::{debug, info, warn};

/// The daemon's name for a pad the hiding service has hidden for it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct Token(pub(crate) u64);

/// The requests to the hiding service made for one setup, or for the
/// daemon itself: ended together, as the setup stops or fails, or as the
/// daemon does.
#[derive(Debug)]
pub(crate) struct Errand {
    ended: AtomicBool,
    /// Readable once it has ended: what a hide under way waits on beside
    /// its reply, to be given up with a `cancel`. None where it couldn't
    /// be made, and then a hide is hung up on instead.
    wake: Option<OwnedFd>,
}

impl Default for Errand {
    fn default() -> Self {
        let wake = eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)
            .inspect_err(|error| {
                warn!(%error, "a setup's hides can't be given up with a cancel, and are hung up on instead");
            })
            .ok();
        Self {
            ended: AtomicBool::new(false),
            wake,
        }
    }
}

impl Errand {
    /// Whether it has been ended: what it asks from now on isn't asked.
    pub(crate) fn has_ended(&self) -> bool {
        self.ended.load(Ordering::Relaxed)
    }

    fn end(&self) {
        self.ended.store(true, Ordering::Relaxed);
        if let Some(wake) = &self.wake
            && let Err(error) = rustix::io::write(wake, &1u64.to_ne_bytes())
        {
            // Full, after more ends than there can be: readable already.
            debug!(%error, "couldn't wake a setup's hides");
        }
    }
}

/// One connection to the service.
#[derive(Debug)]
struct Connection {
    client: Client,
    /// Held for each round trip: one request at a time on a connection.
    turn: Mutex<()>,
    /// Whether it has been hung up, after which the service shows again
    /// what was hidden through it.
    hung_up: AtomicBool,
    /// Whether the service it is to counts its re-binds and takes a
    /// `cancel`, as it said when it was found.
    counts: AtomicBool,
}

impl Connection {
    fn new(client: Client) -> Arc<Self> {
        Arc::new(Self {
            client,
            turn: Mutex::new(()),
            hung_up: AtomicBool::new(false),
            counts: AtomicBool::new(false),
        })
    }

    fn counts(&self) -> bool {
        self.counts.load(Ordering::Relaxed)
    }

    /// Its turn: one request at a time.
    fn turn(&self) -> MutexGuard<'_, ()> {
        self.turn
            .lock()
            .expect("nothing panics while a connection to the hiding service is asked on")
    }

    fn hang_up(&self) {
        self.hung_up.store(true, Ordering::Relaxed);
        self.client.hang_up();
    }

    fn is_hung_up(&self) -> bool {
        self.hung_up.load(Ordering::Relaxed)
    }
}

/// A pad the service has hidden: the connection it was hidden on, and
/// the service's name for it there.
#[derive(Debug)]
struct Held {
    connection: Arc<Connection>,
    token: ctrlassist_hider::Token,
}

/// What is known of the service.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Found {
    /// Not yet: it is being looked for as the daemon starts.
    Looking,
    /// It wasn't there when last looked for, or its connection broke.
    Absent,
    /// It was there when last asked something, and said this release,
    /// none from one too old to be asked, and whether it counts its
    /// re-binds and takes a `cancel`.
    There {
        release: Option<String>,
        counts: bool,
    },
}

/// A request under way.
#[derive(Debug)]
struct Asking {
    errand: Arc<Errand>,
    connection: Arc<Connection>,
    /// For a hide that can be given up with a `cancel`, the pad.
    hide_of: Option<String>,
    /// Whether its errand ended as it was asked, which gives it up.
    cut: bool,
}

#[derive(Debug)]
struct State {
    found: Found,
    /// The pads it has hidden, by the daemon's name for each.
    held: BTreeMap<Token, Held>,
    /// The last [`Token`] given.
    tokens: u64,
    /// The pads whose driver the service re-bound to hide them, as it
    /// said: it re-binds it again to show them, so each goes and comes
    /// back then.
    rebound: BTreeSet<String>,
    /// Each request under way.
    asking: Vec<Asking>,
    /// Whether the daemon is stopping: nothing more is asked.
    closed: bool,
}

/// How a request went, when it didn't go as asked.
#[derive(Debug)]
enum Unasked {
    /// Its errand ended, or its connection was hung up, before or while
    /// it was asked: the service shows again what was hidden through
    /// that connection.
    Ended,
    /// A hide whose errand ended as it was asked, given up with a
    /// `cancel`.
    Cut(Cut),
    /// The service refused.
    Refused,
    /// The service isn't there, or its connection broke.
    NotThere,
}

/// A hide given up as it was asked, which the service leaves the pad as
/// it was from, and says what that took once [settled](Hiding::settle).
#[derive(Debug)]
pub(crate) struct Cut {
    connection: Arc<Connection>,
}

/// Two are the same hide given up when they are on the same connection.
impl PartialEq for Cut {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.connection, &other.connection)
    }
}

impl Eq for Cut {}

/// How many times asking the service to hide a pad made the pad go and
/// come back, as the service said.
#[derive(Debug)]
pub(crate) enum Rebinds {
    /// Known now: none where nothing was asked, or nothing re-bound.
    Known(u8),
    /// A hide given up as it was asked: known once settled, and two at
    /// most, the hiding and the showing again.
    Cut(Cut),
}

/// A pad the service has just hidden.
#[derive(Debug)]
pub(crate) struct Hidden {
    pub(crate) token: Token,
    /// Its event node, opened by the service once it was hidden.
    pub(crate) event: OwnedFd,
    /// How many times the service re-bound it to hide it: once.
    pub(crate) rebinds: u8,
}

/// Why a pad wasn't hidden.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Unhidden {
    /// The errand it was asked for ended first: nothing was re-bound, or
    /// with a service from before `cancel`, what was isn't known.
    Ended,
    /// The service isn't there, or refused: the pad is to be grabbed. It
    /// re-bound the pad this many times all the same, as it does when a
    /// hide fails after the re-bind and it shows the pad again.
    Not(u8),
    /// The errand it was asked for ended as it was asked.
    Cut(Cut),
}

/// How a setup's pads are kept from games: through the hiding service
/// where it is there, and by a grab on each pad where it isn't.
#[derive(Debug)]
pub(crate) struct Hiding {
    /// Where the service listens; none where there is none to look for.
    socket: Option<PathBuf>,
    state: Mutex<State>,
    /// Told whenever the service has been looked for, an errand ends or
    /// the daemon stops: what a start waiting on the first look waits on.
    changed: Condvar,
    /// The daemon's own requests: looking for the service, copying a
    /// pad, showing pads again. Ended only as the daemon stops.
    own: Arc<Errand>,
}

impl Hiding {
    fn with(socket: Option<PathBuf>, found: Found) -> Self {
        Self {
            socket,
            state: Mutex::new(State {
                found,
                held: BTreeMap::new(),
                tokens: 0,
                rebound: BTreeSet::new(),
                asking: Vec::new(),
                closed: false,
            }),
            changed: Condvar::new(),
            own: Arc::default(),
        }
    }

    /// Hiding through the service at `socket`, which is still to be
    /// [looked for](Self::look).
    pub(crate) fn new(socket: &Path) -> Self {
        Self::with(Some(socket.to_path_buf()), Found::Looking)
    }

    /// Hiding with no service to look for.
    #[cfg(test)]
    pub(crate) fn grab() -> Self {
        Self::with(None, Found::Absent)
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .expect("nothing panics while what is known of the hiding service is held")
    }

    /// Notes what was found of the service, and tells whoever waits.
    fn found(&self, found: Found) {
        let mut state = self.state();
        if state.found != found {
            match &found {
                Found::There { release, counts } => {
                    info!(?release, counts, "the hiding service is there");
                }
                Found::Absent => {
                    info!(socket = ?self.socket, "the hiding service isn't there; a setup's pads will be grabbed, which hides them from most games but not from a program that reads them some other way. It is looked for again each time it is needed");
                }
                Found::Looking => {}
            }
            state.found = found;
        }
        drop(state);
        self.changed.notify_all();
    }

    /// Looks for the service, as the daemon starts, and asks its release.
    /// A start that needs it meanwhile waits for this.
    pub(crate) fn look(&self) {
        let own = Arc::clone(&self.own);
        // What was found is noted as it is found.
        drop(self.connect(&own));
        let mut state = self.state();
        if state.found == Found::Looking {
            // The daemon stopped first.
            state.found = Found::Absent;
        }
        drop(state);
        self.changed.notify_all();
    }

    /// Ends `errand`: each request it has under way ends at once, and
    /// nothing it asks from now on is asked. A hide under way is given up
    /// with a `cancel`, where the service takes one, and the pads of
    /// those are returned: the service shows each again if it re-bound
    /// it, so each is to go and come back once more than was expected of
    /// its hide, until [settled](Self::settle). Any other request is hung
    /// up.
    pub(crate) fn end(&self, errand: &Arc<Errand>) -> Vec<String> {
        let mut state = self.state();
        errand.end();
        let mut cut = Vec::new();
        for asking in &mut state.asking {
            if !Arc::ptr_eq(&asking.errand, errand) {
                continue;
            }
            match &asking.hide_of {
                Some(pad) if asking.connection.counts() && errand.wake.is_some() => {
                    asking.cut = true;
                    cut.push(pad.clone());
                }
                _ => asking.connection.hang_up(),
            }
        }
        drop(state);
        self.changed.notify_all();
        cut
    }

    /// Hangs up every request under way, as the daemon stops, and asks
    /// nothing from now on: nothing is left waiting on the service. The
    /// pads still hidden are shown again by the service as the
    /// connections they were hidden on close, with the daemon.
    pub(crate) fn close(&self) {
        let mut state = self.state();
        state.closed = true;
        self.own.end();
        for asking in &state.asking {
            asking.connection.hang_up();
        }
        drop(state);
        self.changed.notify_all();
    }

    /// The mechanism, for the daemon's state: the service if it was
    /// there when last asked something. It is looked for again when it
    /// is next needed, not here.
    pub(crate) fn mechanism(&self) -> payload::Hiding {
        match self.state().found {
            Found::Looking => payload::Hiding::Looking,
            Found::Absent => payload::Hiding::Grab,
            Found::There { .. } => payload::Hiding::Service,
        }
    }

    /// The found service's release, for the daemon's state: none with
    /// no service, or from one too old to say.
    pub(crate) fn service_release(&self) -> Option<String> {
        match &self.state().found {
            Found::There { release, .. } => release.clone(),
            Found::Looking | Found::Absent => None,
        }
    }

    /// Whether showing the pad again re-binds its driver, as hiding it
    /// last did.
    pub(crate) fn rebinds(&self, pad: &str) -> bool {
        self.state().rebound.contains(pad)
    }

    /// Notes that hiding `pad` re-bound its driver, or didn't.
    #[cfg(test)]
    pub(crate) fn rebound(&self, pad: &str) {
        self.state().rebound.insert(pad.to_string());
    }

    /// Asks one thing on `connection` for `errand`, unless the errand has
    /// ended. Ending it meanwhile ends the request.
    fn ask<T>(
        &self,
        errand: &Arc<Errand>,
        connection: &Arc<Connection>,
        ask: impl FnOnce(&Client) -> Result<T, Error>,
    ) -> Result<T, Unasked> {
        self.asking(errand, connection, None)?;
        let outcome = {
            let _turn = connection.turn();
            (!connection.is_hung_up()).then(|| ask(&connection.client))
        };
        self.unask(errand, connection);
        self.answered(connection, outcome)
    }

    /// Notes a request under way for `errand` on `connection`, unless the
    /// errand has ended; `hide_of` names the pad of a hide that can be
    /// given up with a `cancel`.
    fn asking(
        &self,
        errand: &Arc<Errand>,
        connection: &Arc<Connection>,
        hide_of: Option<&str>,
    ) -> Result<(), Unasked> {
        let mut state = self.state();
        if state.closed || errand.has_ended() {
            return Err(Unasked::Ended);
        }
        state.asking.push(Asking {
            errand: Arc::clone(errand),
            connection: Arc::clone(connection),
            hide_of: hide_of.map(str::to_string),
            cut: false,
        });
        Ok(())
    }

    /// Notes that the request under way for `errand` on `connection` is
    /// done with. Returns whether its errand ended as it was asked, which
    /// gave it up.
    fn unask(&self, errand: &Arc<Errand>, connection: &Arc<Connection>) -> bool {
        let mut state = self.state();
        let asking = state.asking.iter().position(|asking| {
            Arc::ptr_eq(&asking.errand, errand) && Arc::ptr_eq(&asking.connection, connection)
        });
        asking.is_some_and(|asking| state.asking.swap_remove(asking).cut)
    }

    /// What a request on `connection` came to: none where it wasn't
    /// asked.
    fn answered<T>(
        &self,
        connection: &Connection,
        outcome: Option<Result<T, Error>>,
    ) -> Result<T, Unasked> {
        match outcome {
            Some(Ok(answer)) => Ok(answer),
            // Hung up as it was asked, or before.
            Some(Err(_)) | None if connection.is_hung_up() => {
                debug!("a request to the hiding service was ended");
                Err(Unasked::Ended)
            }
            // A pad that has just left can't be opened again yet: the
            // usual case of a pad leaving a setup, and nothing wrong.
            Some(Err(Error::Refused(Refusal::Absent))) => {
                debug!("the hiding service has nothing to open: the pad isn't connected");
                Err(Unasked::Refused)
            }
            Some(Err(Error::Refused(refusal))) => {
                warn!(%refusal, "the hiding service didn't do as asked");
                Err(Unasked::Refused)
            }
            Some(Err(error)) => {
                // A connection that broke is given up: the service shows
                // again what was hidden through it, and it is looked for
                // again when next needed, as a service that was restarted
                // is found.
                warn!(%error, "the hiding service didn't do as asked");
                connection.hang_up();
                self.found(Found::Absent);
                Err(Unasked::NotThere)
            }
            None => Err(Unasked::Ended),
        }
    }

    /// Asks the service on `connection` to hide `pad`, whose event node
    /// `fd` is open on, for `errand`. Ending the errand meanwhile gives
    /// the hide up: with a `cancel` where the service takes one, which
    /// [`end`](Self::end) has said, and by hanging up where it doesn't.
    fn ask_hide(
        &self,
        errand: &Arc<Errand>,
        connection: &Arc<Connection>,
        pad: &str,
        fd: BorrowedFd<'_>,
    ) -> Result<Hid, Unasked> {
        self.asking(errand, connection, Some(pad))?;
        let turn = connection.turn();
        let sent = (!connection.is_hung_up()).then(|| {
            connection.client.send_hide(fd)?;
            // The reply, or the errand's end, whichever is first.
            let wake = errand.wake.as_ref().map(AsFd::as_fd);
            connection.client.wait(wake)
        });
        if self.unask(errand, connection) {
            debug!(pad, "a hide was given up as it was asked");
            // Nothing more is asked after it on its connection but what
            // settling it reads.
            if let Err(error) = connection.client.cancel() {
                debug!(%error, "the hiding service couldn't be told to give a hide up");
            }
            drop(turn);
            return Err(Unasked::Cut(Cut {
                connection: Arc::clone(connection),
            }));
        }
        let outcome = sent.map(|sent| sent.and_then(|()| connection.client.hid()));
        drop(turn);
        self.answered(connection, outcome)
    }

    /// Waits for what a hide given up came to, as the service says once
    /// it has left the pad as it was: how many times it re-bound the pad
    /// for the hide and the cancel together, which is how many times the
    /// pad goes and comes back for them. None counted where the service
    /// went, or the daemon stops, before saying. A request of the
    /// daemon's own, so only the daemon stopping ends it.
    pub(crate) fn settle(&self, cut: Cut) -> u8 {
        let own = Arc::clone(&self.own);
        self.ask(&own, &cut.connection, Client::cancelled)
            .unwrap_or_default()
    }

    /// A new connection to the service, for `errand`. One not known to
    /// be there says its release first, and whether it counts.
    fn connect(&self, errand: &Arc<Errand>) -> Result<Arc<Connection>, Unasked> {
        let socket = self.socket.as_deref().ok_or(Unasked::NotThere)?;
        if self.state().closed || errand.has_ended() {
            return Err(Unasked::Ended);
        }
        let connection = match Client::connect(socket) {
            Ok(client) => Connection::new(client),
            Err(error) => {
                debug!(%error, "the hiding service can't be reached");
                self.found(Found::Absent);
                return Err(Unasked::NotThere);
            }
        };
        let found = self.state().found.clone();
        let counts = match found {
            Found::There { counts, .. } => counts,
            Found::Looking | Found::Absent => {
                match self.ask(errand, &connection, Client::release) {
                    Ok((release, counts)) => {
                        self.found(Found::There {
                            release: Some(release),
                            counts,
                        });
                        counts
                    }
                    // From before it could be asked.
                    Err(Unasked::Refused) => {
                        self.found(Found::There {
                            release: None,
                            counts: false,
                        });
                        false
                    }
                    Err(unasked) => return Err(unasked),
                }
            }
        };
        connection.counts.store(counts, Ordering::Relaxed);
        Ok(connection)
    }

    /// Waits until the service has been looked for as the daemon starts,
    /// or `errand` ends.
    fn looked(&self, errand: &Errand) -> Result<(), Unhidden> {
        let mut state = self.state();
        while state.found == Found::Looking {
            if state.closed || errand.has_ended() {
                return Err(Unhidden::Ended);
            }
            state = self
                .changed
                .wait(state)
                .expect("nothing panics while what is known of the hiding service is held");
        }
        Ok(())
    }

    /// Has the service hide the pad known as `pad`, whose event node
    /// `fd` is open on, for `errand`. Until the service has been looked
    /// for as the daemon starts, this waits for that.
    pub(crate) fn hide(
        &self,
        errand: &Arc<Errand>,
        pad: &str,
        fd: BorrowedFd<'_>,
    ) -> Result<Hidden, Unhidden> {
        self.looked(errand)?;
        let hid = self.connect(errand).and_then(|connection| {
            let hid = self.ask_hide(errand, &connection, pad, fd)?;
            Ok((connection, hid))
        });
        let (connection, hidden, rebinds) = match hid {
            Ok((connection, Hid { outcome, rebinds })) => match outcome {
                Ok(hidden) => (connection, hidden, rebinds),
                Err(refusal) => {
                    if refusal == Refusal::Absent {
                        debug!(
                            pad,
                            "the hiding service has nothing to hide: the pad isn't connected"
                        );
                    } else {
                        warn!(pad, %refusal, "the hiding service didn't do as asked");
                    }
                    // A service that doesn't count re-binds nothing it
                    // refuses, or so it would say.
                    return Err(Unhidden::Not(rebinds.unwrap_or(0)));
                }
            },
            Err(Unasked::Ended) => return Err(Unhidden::Ended),
            Err(Unasked::Cut(cut)) => return Err(Unhidden::Cut(cut)),
            Err(Unasked::Refused | Unasked::NotThere) => return Err(Unhidden::Not(0)),
        };
        let mut state = self.state();
        // Hidden by re-binding its driver, it is shown again the same way.
        if hidden.dislodged {
            state.rebound.insert(pad.to_string());
        } else {
            state.rebound.remove(pad);
        }
        state.tokens += 1;
        let token = Token(state.tokens);
        state.held.insert(
            token,
            Held {
                connection,
                token: hidden.token,
            },
        );
        Ok(Hidden {
            token,
            event: hidden.event,
            // A service that doesn't count re-bound it if it says it
            // dislodged it.
            rebinds: rebinds.unwrap_or(u8::from(hidden.dislodged)),
        })
    }

    /// The connection a hidden pad was hidden on.
    fn held(&self, token: Token) -> Option<(Arc<Connection>, ctrlassist_hider::Token)> {
        let state = self.state();
        let held = state.held.get(&token)?;
        Some((Arc::clone(&held.connection), held.token))
    }

    /// Opens a hidden pad again after it has reconnected, for `errand`.
    pub(crate) fn reopen(&self, errand: &Arc<Errand>, token: Token) -> Option<OwnedFd> {
        let (connection, token) = self.held(token)?;
        let hidden = self.ask(errand, &connection, |client| client.reopen(token));
        hidden.ok().map(|hidden| hidden.event)
    }

    /// Opens the raw HID node of a pad hidden for this daemon, for
    /// `errand`.
    pub(crate) fn raw_of(&self, errand: &Arc<Errand>, token: Token) -> Option<OwnedFd> {
        let (connection, token) = self.held(token)?;
        self.ask(errand, &connection, |client| client.raw_of(token))
            .ok()
    }

    /// Opens the raw HID node of the pad whose event node `fd` is open
    /// on, for the daemon itself.
    pub(crate) fn raw(&self, fd: BorrowedFd<'_>) -> Option<OwnedFd> {
        let own = Arc::clone(&self.own);
        let connection = self.connect(&own).ok()?;
        self.ask(&own, &connection, |client| client.raw(fd)).ok()
    }

    /// Shows a hidden pad again, and gives up its connection. Returns
    /// whether the service shows it again: it did as asked, or the
    /// connection was hung up or broke, which has it shown again all the
    /// same; not if it refused, or the pad isn't one it hid. With that,
    /// how many times it re-bound the pad to, where it said.
    fn show(&self, token: Token) -> (bool, Option<u8>) {
        let Some(held) = self.state().held.remove(&token) else {
            return (false, None);
        };
        let own = Arc::clone(&self.own);
        match self.ask(&own, &held.connection, |client| client.restore(held.token)) {
            Ok(rebinds) => (true, rebinds),
            Err(Unasked::Refused) => (false, None),
            Err(_) => (true, None),
        }
    }

    /// Restores each pad named, once what read them has been dropped.
    /// Returns those the service won't show again, by what they are
    /// known by.
    pub(crate) fn restore(&self, pads: &[(String, Token)]) -> Vec<String> {
        pads.iter()
            .filter(|(_, token)| !self.show(*token).0)
            .map(|(pad, _)| pad.clone())
            .collect()
    }

    /// Restores a pad nothing else is told of. Returns how many times the
    /// service re-bound it to: as it said, or once where it said nothing
    /// and showed it, as a service that doesn't count does.
    pub(crate) fn restore_token(&self, token: Token) -> u8 {
        match self.show(token) {
            (true, rebinds) => rebinds.unwrap_or(1),
            (false, _) => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    //! The hiding service as the daemon's client to it sees one that
    //! never answers: a socket that is listened on and never accepted
    //! from, so a connection is made and a request is never read.

    use std::fs::File;
    use std::os::fd::AsFd;
    use std::time::Duration;

    use rustix::net::{AddressFamily, SocketAddrUnix, SocketType, bind, listen, socket};
    use tempfile::TempDir;

    use super::*;

    /// A service that takes connections and never answers.
    fn silent() -> (TempDir, OwnedFd, PathBuf) {
        let dir = tempfile::Builder::new()
            .prefix("ca")
            .tempdir_in("/tmp")
            .unwrap();
        let path = dir.path().join("hider.sock");
        let listener = socket(AddressFamily::UNIX, SocketType::SEQPACKET, None).unwrap();
        bind(&listener, &SocketAddrUnix::new(&path).unwrap()).unwrap();
        listen(&listener, 16).unwrap();
        (dir, listener, path)
    }

    /// Runs `work` on a thread of its own, and hands back what it says.
    fn aside<T: Send + 'static>(
        work: impl FnOnce() -> T + Send + 'static,
    ) -> std::sync::mpsc::Receiver<T> {
        let (said, says) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = said.send(work());
        });
        says
    }

    /// Something to pass as a pad's descriptor, which a service that
    /// never reads its requests never looks at.
    fn a_descriptor() -> OwnedFd {
        File::open("/dev/null").unwrap().into()
    }

    /// Long enough for a thread that isn't waiting to have said so; how
    /// long only bounds how long a failing test takes to fail.
    const SETTLE: Duration = Duration::from_millis(200);

    /// A start that needs the service while the daemon is still looking
    /// for it waits for the look, and a look that is never answered
    /// holds nothing but that start, which a stop ends. As the daemon
    /// stops, the look ends too.
    #[test]
    fn a_start_waits_for_the_first_look_and_a_stop_ends_the_wait() {
        let (_dir, _listener, path) = silent();
        let hiding = Arc::new(Hiding::new(&path));
        assert_eq!(hiding.mechanism(), payload::Hiding::Looking);
        let looking = aside({
            let hiding = Arc::clone(&hiding);
            move || hiding.look()
        });
        let errand = Arc::new(Errand::default());
        let starting = aside({
            let (hiding, errand) = (Arc::clone(&hiding), Arc::clone(&errand));
            move || {
                let fd = a_descriptor();
                hiding.hide(&errand, "pad", fd.as_fd()).map(|_| ())
            }
        });
        assert!(starting.recv_timeout(SETTLE).is_err(), "it waits");
        // Saying how pads are hidden waits on none of it.
        assert_eq!(hiding.mechanism(), payload::Hiding::Looking);
        hiding.end(&errand);
        assert_eq!(starting.recv().unwrap(), Err(Unhidden::Ended));
        assert!(looking.recv_timeout(SETTLE).is_err(), "still looking");
        hiding.close();
        looking.recv().unwrap();
        assert_eq!(hiding.mechanism(), payload::Hiding::Grab);
    }

    /// A hide the service never answers holds its own setup's errand
    /// and nothing else: another setup's hide is asked beside it, and
    /// ending one ends that one alone. As the daemon stops, every
    /// request ends, and nothing is asked after.
    #[test]
    fn a_hide_that_is_never_answered_holds_only_its_own_setup() {
        let (_dir, _listener, path) = silent();
        let hiding = Arc::new(Hiding::new(&path));
        // Found already, so no release is asked first.
        hiding.found(Found::There {
            release: None,
            counts: false,
        });
        let hide = |errand: &Arc<Errand>| {
            let (hiding, errand) = (Arc::clone(&hiding), Arc::clone(errand));
            aside(move || {
                let fd = a_descriptor();
                hiding.hide(&errand, "pad", fd.as_fd()).map(|_| ())
            })
        };
        let (first, second) = (Arc::new(Errand::default()), Arc::new(Errand::default()));
        let (one, other) = (hide(&first), hide(&second));
        assert!(one.recv_timeout(SETTLE).is_err() && other.recv_timeout(SETTLE).is_err());
        hiding.end(&first);
        assert_eq!(one.recv().unwrap(), Err(Unhidden::Ended));
        assert!(other.recv_timeout(SETTLE).is_err(), "the other still waits");
        // An errand ended asks nothing more.
        let fd = a_descriptor();
        assert_eq!(
            hiding.hide(&first, "pad", fd.as_fd()).map(|_| ()),
            Err(Unhidden::Ended)
        );
        hiding.close();
        assert_eq!(other.recv().unwrap(), Err(Unhidden::Ended));
        let third = Arc::new(Errand::default());
        assert_eq!(
            hiding.hide(&third, "pad", fd.as_fd()).map(|_| ()),
            Err(Unhidden::Ended)
        );
        assert!(hiding.raw(fd.as_fd()).is_none());
    }

    /// Without a service there is nothing to wait for, and a pad is
    /// grabbed; a pad the daemon didn't have hidden isn't shown again.
    #[test]
    fn without_the_service_nothing_waits() {
        let dir = tempfile::Builder::new()
            .prefix("ca")
            .tempdir_in("/tmp")
            .unwrap();
        let hiding = Hiding::new(&dir.path().join("nothing.sock"));
        hiding.look();
        assert_eq!(hiding.mechanism(), payload::Hiding::Grab);
        assert_eq!(hiding.service_release(), None);
        let fd = a_descriptor();
        let errand = Arc::new(Errand::default());
        assert_eq!(
            hiding.hide(&errand, "pad", fd.as_fd()).map(|_| ()),
            Err(Unhidden::Not(0))
        );
        assert_eq!(
            hiding.restore(&[("pad".to_string(), Token(1))]),
            ["pad".to_string()]
        );
        assert!(hiding.reopen(&errand, Token(1)).is_none());
    }

    /// A hide on a service that counts, which then never answers: ending
    /// its errand gives it up at once, says which pad's hide was given
    /// up, and leaves only the settling waiting on the service, which the
    /// daemon stopping ends, with nothing counted.
    #[test]
    fn a_hide_given_up_on_a_service_that_never_answers_holds_only_its_settling() {
        let (_dir, _listener, path) = silent();
        let hiding = Arc::new(Hiding::new(&path));
        hiding.found(Found::There {
            release: Some("0.5.0".to_string()),
            counts: true,
        });
        let errand = Arc::new(Errand::default());
        let hiding_it = aside({
            let (hiding, errand) = (Arc::clone(&hiding), Arc::clone(&errand));
            move || {
                let fd = a_descriptor();
                hiding.hide(&errand, "pad", fd.as_fd())
            }
        });
        assert!(hiding_it.recv_timeout(SETTLE).is_err(), "it waits");
        assert_eq!(hiding.end(&errand), ["pad".to_string()]);
        let Err(Unhidden::Cut(cut)) = hiding_it.recv().unwrap() else {
            unreachable!("a hide under way is given up")
        };
        // Nothing else of the errand is under way to give up.
        assert!(hiding.end(&errand).is_empty());
        let settling = aside({
            let hiding = Arc::clone(&hiding);
            move || hiding.settle(cut)
        });
        assert!(settling.recv_timeout(SETTLE).is_err(), "it waits");
        hiding.close();
        assert_eq!(settling.recv().unwrap(), 0);
    }
}
