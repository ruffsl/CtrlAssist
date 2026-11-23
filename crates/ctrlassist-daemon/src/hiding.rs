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
//!   refused without asking. A hide is given up by hanging up its
//!   connection, the service showing again whatever was hidden through a
//!   connection that closes; a request on a hidden pad's own connection,
//!   by leaving its reply to be read past, so the pad stays hidden until
//!   it is shown again as asked; any other request, by hanging up. A
//!   setup's errand ends when it is stopped, fails or is given up.
//! - **A pad is busy until the service has shown it again,** and a hide
//!   of it waits for that, within the hide's own errand, as the service
//!   would refuse it meanwhile: so a pad hidden again follows its last
//!   report too. A pad is shown again as a [`Reading`], made at once
//!   with no round trip and read by a task of its own that waits only on
//!   its own connection, and, to show a pad written whole reports, first
//!   on the pad's writer having left it at rest; a stop is answered
//!   without one. A connection hung up so that the service shows its pad
//!   stops sending and no more, and the service closes its end once it
//!   has: until then, the pad is busy.
//! - **As the daemon stops, every request and every reading is hung
//!   up** and none is made after, so nothing it waits on is a round trip
//!   to the service.
//!
//! Hiding a pad and showing it again re-bind its driver, which the
//! [book](crate::book) knows by the device that stays.

use std::collections::BTreeMap;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use ctrlassist_hider::{Client, Error, Refusal};
use ctrlassist_protocol::payload;
use rustix::event::{EventfdFlags, eventfd};
use tracing::{debug, info, warn};

/// The most readings under way at once: past it, the oldest stops
/// sending, which has the service give up waiting for its re-bind. Each
/// holds a connection and, while it is read, a thread of the blocking
/// pool. Sixteen is the pads the service's cap of connections for a user
/// at a seat is sized for.
pub(crate) const MOST_READINGS: usize = 16;

/// The daemon's name for a pad the hiding service has hidden for it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct Token(pub(crate) u64);

/// The requests to the hiding service made for one setup, or for the
/// daemon itself: ended together, as the setup stops or fails, or as the
/// daemon does.
#[derive(Debug)]
pub(crate) struct Errand {
    ended: AtomicBool,
    /// Readable once it has ended: what a request under way waits on
    /// beside its reply, to be given up without hanging up. None where it
    /// couldn't be made, and then a request is hung up on instead.
    wake: Option<OwnedFd>,
}

impl Default for Errand {
    fn default() -> Self {
        let wake = eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)
            .inspect_err(|error| {
                warn!(%error, "a setup's requests to the hiding service can't be given up, and are hung up on instead");
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
            debug!(%error, "couldn't wake a setup's requests");
        }
    }

    fn wake(&self) -> Option<BorrowedFd<'_>> {
        self.wake.as_ref().map(AsFd::as_fd)
    }
}

/// One connection to the service.
#[derive(Debug)]
struct Connection {
    client: Client,
    /// Held for each round trip: one request at a time on a connection.
    turn: Mutex<()>,
    /// Whether nothing more is asked on it: it was hung up, or stopped
    /// sending, after which the service shows again what was hidden
    /// through it.
    hung_up: AtomicBool,
    /// How many replies are still to come to requests whose callers gave
    /// up waiting for them, which come before any other's.
    unread: AtomicU32,
}

impl Connection {
    fn new(client: Client) -> Arc<Self> {
        Arc::new(Self {
            client,
            turn: Mutex::new(()),
            hung_up: AtomicBool::new(false),
            unread: AtomicU32::new(0),
        })
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

    /// Hangs up sending alone: what the service says until it closes
    /// its end can still be read, and its closing is that it has shown
    /// again what was hidden through the connection.
    fn stop_sending(&self) {
        self.hung_up.store(true, Ordering::Relaxed);
        self.client.stop_sending();
    }

    fn is_hung_up(&self) -> bool {
        self.hung_up.load(Ordering::Relaxed)
    }

    /// Reads past the replies to requests given up, with its turn held.
    /// Returns whether it has, and not been given up itself, as `wake`
    /// becoming readable gives it up.
    fn read_past(&self, wake: Option<BorrowedFd<'_>>, ended: &dyn Fn() -> bool) -> bool {
        while self.unread.load(Ordering::Relaxed) > 0 {
            if self.client.wait(wake).is_err() || ended() {
                return false;
            }
            if let Err(error) = self.client.skip() {
                debug!(%error, "a reply given up couldn't be read past");
                return false;
            }
            self.unread.fetch_sub(1, Ordering::Relaxed);
        }
        true
    }

    /// Reads past whatever the service still says, until it closes its
    /// end, or the connection was hung up whole. Returns whether it has,
    /// and not been given up itself, as `wake` becoming readable gives it
    /// up.
    fn wait_closed(&self, wake: Option<BorrowedFd<'_>>, ended: &dyn Fn() -> bool) -> bool {
        loop {
            if self.client.wait(wake).is_err() || ended() {
                return false;
            }
            if self.client.skip().is_err() {
                return true;
            }
        }
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
    /// It was there when last asked something, and said this release, or
    /// none from one too old to be asked.
    There { release: Option<String> },
}

/// What a request under way is, and so how ending its errand gives it
/// up.
#[derive(Debug)]
enum Kind {
    /// A hide: given up by stopping sending, where the errand can wake
    /// it, and its pad is busy until the service closes its end.
    Hide,
    /// A request on the connection a pad was hidden on: given up by
    /// leaving its reply to be read past, where the errand can wake it,
    /// so the pad stays hidden until it is shown again as asked.
    OnHeld,
    /// A wait for the service to close its end of a connection that
    /// stopped sending: given up by leaving it, where the errand can wake
    /// it, so that the next wait can see the end.
    Closing,
    /// Any other: hung up on.
    Other,
}

/// A request under way.
#[derive(Debug)]
struct Asking {
    errand: Arc<Errand>,
    connection: Arc<Connection>,
    kind: Kind,
    /// Whether its errand ended as it was asked, which gives a hide up.
    cut: bool,
}

/// A reading under way, as [`State`] keeps it.
#[derive(Debug)]
struct Read {
    number: u64,
    pad: String,
    connection: Arc<Connection>,
}

#[derive(Debug)]
struct State {
    found: Found,
    /// The pads it has hidden, by the daemon's name for each.
    held: BTreeMap<Token, Held>,
    /// The last [`Token`] given.
    tokens: u64,
    /// Each request under way.
    asking: Vec<Asking>,
    /// Each reading under way, oldest first.
    readings: Vec<Read>,
    /// The last reading's number.
    read: u64,
    /// Each connection that has stopped sending so that the service shows
    /// its pad, and whose end the service hasn't been seen to close: a
    /// hide given up, or a hidden pad given up without a reading.
    closing: Vec<(String, Arc<Connection>)>,
    /// Whether the daemon is stopping: nothing more is asked.
    closed: bool,
}

/// What is shared with the readings and with the thread that makes
/// connections, which end or report on their own.
#[derive(Debug)]
struct Shared {
    state: Mutex<State>,
    /// Told whenever the service has been looked for, an errand ends, a
    /// reading ends, a connection is made, or the daemon stops: what a
    /// request waiting on any of them waits on.
    changed: Condvar,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .expect("nothing panics while what is known of the hiding service is held")
    }

    fn wait<'a>(&self, state: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
        self.changed
            .wait(state)
            .expect("nothing panics while what is known of the hiding service is held")
    }

    /// Notes that `connection` has stopped sending so that the service
    /// shows `pad` again: the pad is busy until the service closes its
    /// end.
    fn closing(&self, pad: &str, connection: Arc<Connection>) {
        let mut state = self.state();
        if state.closed {
            connection.hang_up();
        }
        state.closing.push((pad.to_string(), connection));
    }

    /// Gives up `pad`, hidden as `token`, without a reading: its
    /// connection stops sending, and the service shows the pad again.
    fn give_up(&self, pad: &str, token: Token) {
        let held = self.state().held.remove(&token);
        if let Some(held) = held {
            held.connection.stop_sending();
            self.closing(pad, held.connection);
        }
    }
}

/// How a request went, when it didn't go as asked.
#[derive(Debug)]
enum Unasked {
    /// Its errand ended, or its connection was hung up, before it was
    /// asked or as it was.
    Ended,
    /// The service refused, as this says.
    Refused(Refusal),
    /// The service isn't there, or its connection broke.
    NotThere,
}

/// Showing a pad again, once [read](Hiding::read). Until it is read, or
/// dropped, the pad is busy, and a hide of it waits.
#[must_use = "dropping a reading closes its connection, which has the service show its pad again unasked"]
#[derive(Debug)]
pub(crate) struct Reading {
    shared: Arc<Shared>,
    number: u64,
    connection: Arc<Connection>,
    token: ctrlassist_hider::Token,
}

impl Drop for Reading {
    /// Ends it: a hide of its pad waits no longer, and its connection,
    /// which nothing is asked on after it, closes once nothing holds it.
    fn drop(&mut self) {
        let mut state = self.shared.state();
        state.readings.retain(|read| read.number != self.number);
        drop(state);
        self.shared.changed.notify_all();
    }
}

/// A pad the service has just hidden, until it is [kept](Self::keep):
/// what drops it before then, a panic's unwinding among them, gives it up,
/// and the service shows it again.
#[must_use = "dropping it has the service show the pad again"]
#[derive(Debug)]
pub(crate) struct Hold {
    shared: Arc<Shared>,
    pad: String,
    token: Token,
    kept: bool,
}

impl Hold {
    /// The daemon's name for the pad.
    pub(crate) fn token(&self) -> Token {
        self.token
    }

    /// Keeps the pad hidden, as the token returned names it: from here,
    /// whoever has the token shows it again.
    pub(crate) fn keep(mut self) -> Token {
        self.kept = true;
        self.token
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        if !self.kept {
            self.shared.give_up(&self.pad, self.token);
        }
    }
}

/// A pad the service has just hidden.
#[derive(Debug)]
pub(crate) struct Hidden {
    /// Its event node, opened by the service once it was hidden.
    pub(crate) event: OwnedFd,
    pub(crate) hold: Hold,
}

/// Why a pad wasn't hidden.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Unhidden {
    /// The errand it was asked for ended first.
    Ended,
    /// The service isn't there, or refused: the pad is to be grabbed.
    Not,
}

/// A connection being made by [`Connector`], for a request that waits.
#[derive(Debug, Default)]
struct Dialing {
    made: Mutex<Option<Result<Client, Error>>>,
    /// Whether the request gave up waiting: what is made is dropped.
    given_up: AtomicBool,
}

/// Makes the connections that wait for room in the service's queue of
/// connections to accept, one at a time, on a thread of its own, which
/// it starts when first needed. A request that needs one waits for it
/// as it waits for anything else, and gives up as its errand ends,
/// leaving the connection to be dropped once made. One thread, however
/// many give up, so a service that never accepts holds that thread alone.
#[derive(Debug, Default)]
struct Connector {
    queue: Mutex<Option<mpsc::Sender<Arc<Dialing>>>>,
}

impl Connector {
    /// Has a connection made to `socket` for `dialing`, telling `shared`
    /// once it is. Returns whether it is under way.
    fn dial(&self, socket: &Path, shared: &Arc<Shared>, dialing: Arc<Dialing>) -> bool {
        let mut queue = self
            .queue
            .lock()
            .expect("nothing panics while the connector's queue is held");
        if queue.is_none() {
            let (sender, receiver) = mpsc::channel::<Arc<Dialing>>();
            let (socket, shared) = (socket.to_path_buf(), Arc::clone(shared));
            let started = std::thread::Builder::new()
                .name("hider-connect".into())
                .spawn(move || {
                    for dialing in receiver {
                        if dialing.given_up.load(Ordering::Relaxed) {
                            continue;
                        }
                        let made = Client::connect_waiting(&socket);
                        let state = shared.state();
                        *dialing
                            .made
                            .lock()
                            .expect("nothing panics while a connection being made is held") =
                            Some(made);
                        drop(state);
                        shared.changed.notify_all();
                    }
                });
            match started {
                Ok(_) => *queue = Some(sender),
                Err(error) => {
                    warn!(%error, "couldn't start the thread that waits for room at the hiding service");
                    return false;
                }
            }
        }
        queue
            .as_ref()
            .is_some_and(|queue| queue.send(dialing).is_ok())
    }
}

/// How a setup's pads are kept from games: through the hiding service
/// where it is there, and by a grab on each pad where it isn't.
#[derive(Debug)]
pub(crate) struct Hiding {
    /// Where the service listens; none where there is none to look for.
    socket: Option<PathBuf>,
    shared: Arc<Shared>,
    /// The daemon's own requests: looking for the service, copying a
    /// pad. Ended only as the daemon stops.
    own: Arc<Errand>,
    connector: Connector,
}

impl Hiding {
    fn with(socket: Option<PathBuf>, found: Found) -> Self {
        Self {
            socket,
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    found,
                    held: BTreeMap::new(),
                    tokens: 0,
                    asking: Vec::new(),
                    readings: Vec::new(),
                    read: 0,
                    closing: Vec::new(),
                    closed: false,
                }),
                changed: Condvar::new(),
            }),
            own: Arc::default(),
            connector: Connector::default(),
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
        self.shared.state()
    }

    fn notify(&self) {
        self.shared.changed.notify_all();
    }

    /// Notes what was found of the service, and tells whoever waits.
    fn found(&self, found: Found) {
        let mut state = self.state();
        if state.found != found {
            match &found {
                Found::There { release } => info!(?release, "the hiding service is there"),
                Found::Absent => {
                    info!(socket = ?self.socket, "the hiding service isn't there; a setup's pads will be grabbed, which hides them from most games but not from a program that reads them some other way. It is looked for again each time it is needed");
                }
                Found::Looking => {}
            }
            state.found = found;
        }
        drop(state);
        self.notify();
    }

    /// Looks for the service, as the daemon starts, and asks its release.
    /// A start that needs it meanwhile waits for this.
    pub(crate) fn look(&self) {
        let own = Arc::clone(&self.own);
        // What was found is noted as it is found.
        drop(self.connect(&own));
        let mut state = self.state();
        if state.found == Found::Looking {
            // The daemon stopped first, or the service wouldn't say, as
            // to a user not in front of a seat: pads are grabbed until a
            // later connection finds it.
            state.found = Found::Absent;
        }
        drop(state);
        self.notify();
    }

    /// Ends `errand`: each request it has under way ends at once, and
    /// nothing it asks from now on is asked. A hide under way stops
    /// sending, where the errand can wake it; a request on a hidden pad's
    /// own connection leaves its reply to be read past; any other is hung
    /// up.
    pub(crate) fn end(&self, errand: &Arc<Errand>) {
        let mut state = self.state();
        errand.end();
        let wakes = errand.wake.is_some();
        for asking in &mut state.asking {
            // Given up already, by an end before this one.
            if !Arc::ptr_eq(&asking.errand, errand) || asking.cut {
                continue;
            }
            match &asking.kind {
                Kind::Hide if wakes => {
                    asking.cut = true;
                    asking.connection.stop_sending();
                }
                // Woken by the errand, and its reply read past later, or
                // its end seen by the next wait.
                Kind::OnHeld | Kind::Closing if wakes => {}
                _ => asking.connection.hang_up(),
            }
        }
        drop(state);
        self.notify();
    }

    /// Hangs up every request, every reading and every connection closing
    /// under way, as the daemon stops, and asks nothing from now on:
    /// nothing is left waiting on the service. The pads still hidden are
    /// shown again by the service as the connections they were hidden on
    /// close, with the daemon.
    pub(crate) fn close(&self) {
        let mut state = self.state();
        state.closed = true;
        self.own.end();
        let connections = state
            .asking
            .iter()
            .map(|asking| &asking.connection)
            .chain(state.readings.iter().map(|read| &read.connection))
            .chain(state.closing.iter().map(|(_, connection)| connection));
        for connection in connections {
            connection.hang_up();
        }
        drop(state);
        self.notify();
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
            Found::There { release } => release.clone(),
            Found::Looking | Found::Absent => None,
        }
    }

    /// Asks one thing on `connection` for `errand`, unless the errand has
    /// ended. Ending it meanwhile hangs the request up.
    fn ask<T>(
        &self,
        errand: &Arc<Errand>,
        connection: &Arc<Connection>,
        ask: impl FnOnce(&Client) -> Result<T, Error>,
    ) -> Result<T, Unasked> {
        self.asking(errand, connection, Kind::Other)?;
        let outcome = {
            let _turn = connection.turn();
            (!connection.is_hung_up()).then(|| ask(&connection.client))
        };
        self.unask(errand, connection);
        self.answered(connection, outcome)
    }

    /// Asks one thing on the connection a pad was hidden on, for
    /// `errand`, unless the errand has ended: `send` sends it and `read`
    /// reads its reply. Ending the errand meanwhile gives it up, leaving
    /// its reply to be read past by the next request, so the connection,
    /// and the pad hidden through it, stay.
    fn ask_held<T>(
        &self,
        errand: &Arc<Errand>,
        connection: &Arc<Connection>,
        send: impl FnOnce(&Client) -> Result<(), Error>,
        read: impl FnOnce(&Client) -> Result<T, Error>,
    ) -> Result<T, Unasked> {
        self.asking(errand, connection, Kind::OnHeld)?;
        let outcome = {
            let _turn = connection.turn();
            let ended = || errand.has_ended();
            if connection.is_hung_up() || ended() {
                None
            } else if !connection.read_past(errand.wake(), &ended) {
                Some(Err(Unasked::Ended))
            } else {
                let sent =
                    send(&connection.client).and_then(|()| connection.client.wait(errand.wake()));
                match sent {
                    // Woken by the errand's end, or the reply: if both, the
                    // reply is read past all the same.
                    Ok(()) if ended() && errand.wake.is_some() => {
                        connection.unread.fetch_add(1, Ordering::Relaxed);
                        Some(Err(Unasked::Ended))
                    }
                    sent => Some(Ok(sent.and_then(|()| read(&connection.client)))),
                }
            }
        };
        self.unask(errand, connection);
        match outcome {
            Some(Err(unasked)) => Err(unasked),
            Some(Ok(outcome)) => self.answered(connection, Some(outcome)),
            None => self.answered::<T>(connection, None),
        }
    }

    /// Notes a request under way for `errand` on `connection`, unless the
    /// errand has ended.
    fn asking(
        &self,
        errand: &Arc<Errand>,
        connection: &Arc<Connection>,
        kind: Kind,
    ) -> Result<(), Unasked> {
        let mut state = self.state();
        if state.closed || errand.has_ended() {
            return Err(Unasked::Ended);
        }
        state.asking.push(Asking {
            errand: Arc::clone(errand),
            connection: Arc::clone(connection),
            kind,
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
            // Hung up as it was asked.
            Some(Err(_)) if connection.is_hung_up() => {
                debug!("a request to the hiding service was ended");
                Err(Unasked::Ended)
            }
            // A pad that has just left can't be opened again yet: the
            // usual case of a pad leaving a setup, and nothing wrong.
            Some(Err(Error::Refused(Refusal::Absent))) => {
                debug!("the hiding service has nothing to open: the pad isn't connected");
                Err(Unasked::Refused(Refusal::Absent))
            }
            Some(Err(Error::Refused(refusal))) => {
                warn!(%refusal, "the hiding service didn't do as asked");
                Err(Unasked::Refused(refusal))
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
    /// the hide up: its connection stops sending, as [`end`](Self::end)
    /// has it, and is noted as closing, the pad busy until it has.
    fn ask_hide(
        &self,
        errand: &Arc<Errand>,
        connection: &Arc<Connection>,
        pad: &str,
        fd: BorrowedFd<'_>,
    ) -> Result<ctrlassist_hider::Hidden, Unasked> {
        self.asking(errand, connection, Kind::Hide)?;
        let turn = connection.turn();
        // Not sent once its errand has ended, as seen with the turn held:
        // an end from here on finds a hide sent to give up.
        if errand.has_ended() || connection.is_hung_up() {
            drop(turn);
            self.unask(errand, connection);
            return Err(Unasked::Ended);
        }
        let sent = connection
            .client
            .send_hide(fd)
            // The reply, or the errand's end, whichever is first.
            .and_then(|()| connection.client.wait(errand.wake()));
        if self.unask(errand, connection) {
            debug!(pad, "a hide was given up as it was asked");
            drop(turn);
            self.shared.closing(pad, Arc::clone(connection));
            return Err(Unasked::Ended);
        }
        let outcome = sent.and_then(|()| connection.client.hid());
        drop(turn);
        if let Err(Error::Refused(refusal)) = outcome {
            if refusal == Refusal::Absent {
                debug!(
                    pad,
                    "the hiding service has nothing to hide: the pad isn't connected"
                );
            } else {
                warn!(pad, %refusal, "the hiding service didn't do as asked");
            }
            return Err(Unasked::Refused(refusal));
        }
        self.answered(connection, Some(outcome))
    }

    /// Notes a reading of `pad`'s on `connection`. Past [`MOST_READINGS`]
    /// the oldest still sending stops: the service gives up waiting for
    /// its re-bind, and its pad is busy until the service closes its end.
    fn reading(
        &self,
        pad: &str,
        connection: &Arc<Connection>,
        token: ctrlassist_hider::Token,
    ) -> Reading {
        let mut state = self.state();
        state.read += 1;
        let number = state.read;
        state.readings.push(Read {
            number,
            pad: pad.to_string(),
            connection: Arc::clone(connection),
        });
        let mut live = state
            .readings
            .iter()
            .filter(|read| !read.connection.is_hung_up());
        if live.clone().count() > MOST_READINGS
            && let Some(oldest) = live.next()
        {
            warn!(
                pad = oldest.pad,
                most = MOST_READINGS,
                "the hiding service hasn't shown again a pad of the most under way; it is hung up on, and shows the pad as it closes the connection"
            );
            oldest.connection.stop_sending();
        }
        if state.closed {
            connection.hang_up();
        }
        Reading {
            shared: Arc::clone(&self.shared),
            number,
            connection: Arc::clone(connection),
            token,
        }
    }

    /// Asks the service to show again the pad `reading` is of, and waits
    /// until it has: until it says so, or, once the reading has stopped
    /// sending, until it closes its end. Waits on the service, on its own
    /// connection alone, and nothing else waits on it.
    pub(crate) fn read(&self, reading: Reading) {
        let connection = &reading.connection;
        let _turn = connection.turn();
        let asked = !connection.is_hung_up() && connection.read_past(None, &|| false);
        if asked {
            match connection.client.restore(reading.token) {
                Ok(()) => return,
                Err(Error::Refused(refusal)) => {
                    warn!(%refusal, "the hiding service didn't show a pad again");
                }
                Err(error) if !connection.is_hung_up() => {
                    debug!(%error, "the hiding service didn't say it showed a pad again");
                }
                Err(_) => {}
            }
        }
        // Shown as the connection closes: waited for, so that a hide of
        // the pad waits too.
        connection.stop_sending();
        connection.wait_closed(None, &|| false);
    }

    /// A new connection to the service, for `errand`. The first, and the
    /// first after the service was found gone, asks its release, which
    /// the daemon publishes.
    fn connect(&self, errand: &Arc<Errand>) -> Result<Arc<Connection>, Unasked> {
        let socket = self.socket.as_deref().ok_or(Unasked::NotThere)?;
        if self.state().closed || errand.has_ended() {
            return Err(Unasked::Ended);
        }
        let client = match Client::connect(socket) {
            Ok(client) => client,
            // There, with its queue of connections to accept full: waited
            // for, as the request waits for anything.
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::WouldBlock => {
                debug!(
                    "the hiding service has no room for another connection yet; waiting for room"
                );
                self.dial(socket, errand)?
            }
            Err(error) => {
                debug!(%error, "the hiding service can't be reached");
                self.found(Found::Absent);
                return Err(Unasked::NotThere);
            }
        };
        let connection = Connection::new(client);
        if matches!(self.state().found, Found::There { .. }) {
            return Ok(connection);
        }
        match self.ask(errand, &connection, Client::version) {
            Ok(release) => self.found(Found::There {
                release: Some(release),
            }),
            // Not understood: a service from before it could be asked.
            Err(Unasked::Refused(Refusal::BadRequest)) => {
                self.found(Found::There { release: None });
            }
            // Any other refusal, as of a caller not in front of a seat,
            // says nothing of what the service is: what is known of it
            // stays as it was, and the next connection asks again.
            Err(unasked) => return Err(unasked),
        }
        Ok(connection)
    }

    /// Waits for a connection to `socket` that waits for room, unless
    /// `errand` ends or the daemon stops first.
    fn dial(&self, socket: &Path, errand: &Errand) -> Result<Client, Unasked> {
        let dialing = Arc::new(Dialing::default());
        if !self
            .connector
            .dial(socket, &self.shared, Arc::clone(&dialing))
        {
            return Err(Unasked::NotThere);
        }
        let mut state = self.state();
        loop {
            let made = dialing
                .made
                .lock()
                .expect("nothing panics while a connection being made is held")
                .take();
            if let Some(made) = made {
                drop(state);
                return made.map_err(|error| {
                    debug!(%error, "the hiding service can't be reached");
                    self.found(Found::Absent);
                    Unasked::NotThere
                });
            }
            if state.closed || errand.has_ended() {
                dialing.given_up.store(true, Ordering::Relaxed);
                return Err(Unasked::Ended);
            }
            state = self.shared.wait(state);
        }
    }

    /// Waits until the service has been looked for as the daemon starts,
    /// and until `pad` is busy no longer, or `errand` ends: until no
    /// reading of it is under way, and the service has closed its end of
    /// every connection that stopped sending to show it, which this reads
    /// to the end itself.
    fn wait_for(&self, errand: &Arc<Errand>, pad: &str) -> Result<(), Unhidden> {
        loop {
            let mut state = self.state();
            while state.found == Found::Looking || state.readings.iter().any(|read| read.pad == pad)
            {
                if state.closed || errand.has_ended() {
                    return Err(Unhidden::Ended);
                }
                state = self.shared.wait(state);
            }
            let Some((_, closing)) = state.closing.iter().find(|(held, _)| held == pad) else {
                return Ok(());
            };
            let closing = Arc::clone(closing);
            drop(state);
            self.asking(errand, &closing, Kind::Closing)
                .map_err(|_| Unhidden::Ended)?;
            let closed = closing.wait_closed(errand.wake(), &|| errand.has_ended());
            self.unask(errand, &closing);
            if !closed || errand.has_ended() {
                return Err(Unhidden::Ended);
            }
            self.state()
                .closing
                .retain(|(_, connection)| !Arc::ptr_eq(connection, &closing));
        }
    }

    /// Has the service hide the pad known as `pad`, whose event node
    /// `fd` is open on, for `errand`. Until the service has been looked
    /// for as the daemon starts, this waits for that; and while the pad
    /// is busy being shown again, for that: the service holds the pad
    /// until its re-bind is done, and would refuse it as busy meanwhile.
    pub(crate) fn hide(
        &self,
        errand: &Arc<Errand>,
        pad: &str,
        fd: BorrowedFd<'_>,
    ) -> Result<Hidden, Unhidden> {
        self.wait_for(errand, pad)?;
        let connection = match self.connect(errand) {
            Ok(connection) => connection,
            Err(Unasked::Ended) => return Err(Unhidden::Ended),
            // Nothing was asked of a service that isn't there or refused
            // to say its release.
            Err(Unasked::Refused(_) | Unasked::NotThere) => return Err(Unhidden::Not),
        };
        let hidden = match self.ask_hide(errand, &connection, pad, fd) {
            Ok(hidden) => hidden,
            Err(Unasked::Ended) => return Err(Unhidden::Ended),
            Err(Unasked::Refused(_) | Unasked::NotThere) => return Err(Unhidden::Not),
        };
        let mut state = self.state();
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
            event: hidden.event,
            hold: Hold {
                shared: Arc::clone(&self.shared),
                pad: pad.to_string(),
                token,
                kept: false,
            },
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
        let hidden = self.ask_held(
            errand,
            &connection,
            |client| client.send_reopen(token),
            Client::reopened,
        );
        hidden.ok().map(|hidden| hidden.event)
    }

    /// Opens the raw HID node of a pad hidden for this daemon, for
    /// `errand`.
    pub(crate) fn raw_of(&self, errand: &Arc<Errand>, token: Token) -> Option<OwnedFd> {
        let (connection, token) = self.held(token)?;
        self.ask_held(
            errand,
            &connection,
            |client| client.send_raw_of(token),
            Client::raw_opened,
        )
        .ok()
    }

    /// Opens the raw HID node of the pad whose event node `fd` is open
    /// on, for the daemon itself.
    pub(crate) fn raw(&self, fd: BorrowedFd<'_>) -> Option<OwnedFd> {
        let own = Arc::clone(&self.own);
        let connection = self.connect(&own).ok()?;
        self.ask(&own, &connection, |client| client.raw(fd)).ok()
    }

    /// Sets about showing the hidden pad `pad`, known as `token`, again,
    /// with no round trip: the reading returned asks it once
    /// [read](Self::read). None for a pad the service doesn't have hidden
    /// for this daemon.
    pub(crate) fn show(&self, pad: &str, token: Token) -> Option<Reading> {
        let held = self.state().held.remove(&token)?;
        Some(self.reading(pad, &held.connection, held.token))
    }

    /// Gives up the hidden pad `pad`, known as `token`, with no reading:
    /// for a pad gone, whose name is given up for another, which waits for
    /// no writer. The service shows it again as the connection it was
    /// hidden on closes, and the pad is busy until then.
    pub(crate) fn give_up(&self, pad: &str, token: Token) {
        self.shared.give_up(pad, token);
    }
}

#[cfg(test)]
mod tests {
    //! The hiding service as the daemon's client to it sees one that
    //! never answers (a socket that is listened on and never accepted
    //! from, so a connection is made and a request is never read), and
    //! one scripted to answer as each test needs.

    use std::fs::File;
    use std::os::fd::AsFd;
    use std::time::Duration;

    use ctrlassist_test::{Asked, FakeHider, Script};
    use rustix::net::{AddressFamily, SocketAddrUnix, SocketType, accept, bind, listen, socket};
    use tempfile::TempDir;

    use super::*;

    /// A directory for a socket, short enough for its path.
    fn a_dir() -> TempDir {
        tempfile::Builder::new()
            .prefix("ca")
            .tempdir_in("/tmp")
            .unwrap()
    }

    /// A service that takes `backlog` connections and never answers.
    fn silent_with(backlog: i32) -> (TempDir, OwnedFd, PathBuf) {
        let dir = a_dir();
        let path = dir.path().join("hider.sock");
        let listener = socket(AddressFamily::UNIX, SocketType::SEQPACKET, None).unwrap();
        bind(&listener, &SocketAddrUnix::new(&path).unwrap()).unwrap();
        listen(&listener, backlog).unwrap();
        (dir, listener, path)
    }

    /// A service that takes connections and never answers.
    fn silent() -> (TempDir, OwnedFd, PathBuf) {
        silent_with(16)
    }

    /// A scripted stand-in, and the hiding that has looked for it.
    fn scripted(script: Script) -> (TempDir, FakeHider, Arc<Hiding>) {
        let dir = a_dir();
        let path = dir.path().join("hider.sock");
        let hider = FakeHider::scripted(&path, script).unwrap();
        let hiding = Arc::new(Hiding::new(&path));
        hiding.look();
        (dir, hider, hiding)
    }

    /// Runs `work` on a thread of its own, and hands back what it says.
    fn aside<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> mpsc::Receiver<T> {
        let (said, says) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = said.send(work());
        });
        says
    }

    /// Something to pass as a pad's descriptor, which neither a service
    /// that never reads its requests nor the stand-in looks at.
    fn a_descriptor() -> OwnedFd {
        File::open("/dev/null").unwrap().into()
    }

    /// Hides `pad` for `errand` aside, and hands back what came of it:
    /// the daemon's name for it, kept, if it was hidden.
    fn hiding_aside(
        hiding: &Arc<Hiding>,
        errand: &Arc<Errand>,
        pad: &'static str,
    ) -> mpsc::Receiver<Result<Token, Unhidden>> {
        let (hiding, errand) = (Arc::clone(hiding), Arc::clone(errand));
        aside(move || {
            let fd = a_descriptor();
            hiding
                .hide(&errand, pad, fd.as_fd())
                .map(|hidden| hidden.hold.keep())
        })
    }

    /// Reads `reading` aside.
    fn reading_aside(hiding: &Arc<Hiding>, reading: Reading) -> mpsc::Receiver<()> {
        let hiding = Arc::clone(hiding);
        aside(move || hiding.read(reading))
    }

    /// How many hides the stand-in was asked.
    fn hides(hider: &FakeHider) -> usize {
        hider
            .asked()
            .iter()
            .filter(|asked| matches!(asked, Asked::Hide(_)))
            .count()
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
        let starting = hiding_aside(&hiding, &errand, "pad");
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
        // Found already, so no start waits for the look.
        hiding.found(Found::There { release: None });
        let (first, second) = (Arc::new(Errand::default()), Arc::new(Errand::default()));
        let (one, other) = (
            hiding_aside(&hiding, &first, "pad"),
            hiding_aside(&hiding, &second, "other"),
        );
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
        let dir = a_dir();
        let hiding = Hiding::new(&dir.path().join("nothing.sock"));
        hiding.look();
        assert_eq!(hiding.mechanism(), payload::Hiding::Grab);
        assert_eq!(hiding.service_release(), None);
        let fd = a_descriptor();
        let errand = Arc::new(Errand::default());
        assert_eq!(
            hiding.hide(&errand, "pad", fd.as_fd()).map(|_| ()),
            Err(Unhidden::Not)
        );
        assert!(hiding.show("pad", Token(1)).is_none());
        assert!(hiding.reopen(&errand, Token(1)).is_none());
    }

    /// A hide given up as it is asked is hung up on, and the service
    /// shows the pad again as the connection closes: until it has, the
    /// pad is busy, and a hide of it waits, within its own errand, and
    /// is asked once the service has closed its end.
    #[test]
    fn a_hide_given_up_holds_its_pad_until_the_service_has_shown_it() {
        let (_dir, hider, hiding) = scripted(Script {
            hold_hides: true,
            ..Script::default()
        });
        let errand = Arc::new(Errand::default());
        let hiding_it = hiding_aside(&hiding, &errand, "pad");
        hider.wait_held();
        hiding.end(&errand);
        assert_eq!(hiding_it.recv().unwrap(), Err(Unhidden::Ended), "at once");

        let again = Arc::new(Errand::default());
        let waiting = hiding_aside(&hiding, &again, "pad");
        assert!(waiting.recv_timeout(SETTLE).is_err(), "it waits");
        hiding.end(&again);
        assert_eq!(waiting.recv().unwrap(), Err(Unhidden::Ended));

        let third = Arc::new(Errand::default());
        let waiting = hiding_aside(&hiding, &third, "pad");
        assert!(waiting.recv_timeout(SETTLE).is_err(), "it waits still");
        assert_eq!(hides(&hider), 1, "nothing asked of the pad meanwhile");
        hider.let_go();
        assert!(waiting.recv().unwrap().is_ok(), "asked once it is shown");
        assert_eq!(hides(&hider), 2);
        assert!(hider.asked().contains(&Asked::Closed));
        hiding.close();
    }

    /// A first look the service refuses, as it refuses a user who isn't
    /// in front of a seat, says nothing of what the service is: the pads
    /// are grabbed meanwhile, and the next connection asks again.
    #[test]
    fn a_first_look_refused_is_asked_again() {
        let (_dir, hider, hiding) = scripted(Script {
            refuse: 1,
            hold_hides: true,
            ..Script::default()
        });
        assert_eq!(hiding.mechanism(), payload::Hiding::Grab);
        assert_eq!(hiding.service_release(), None);

        let errand = Arc::new(Errand::default());
        let hiding_it = hiding_aside(&hiding, &errand, "pad");
        hider.wait_held();
        assert_eq!(hiding.mechanism(), payload::Hiding::Service);
        assert!(hiding.service_release().is_some(), "published once known");
        hider.let_go();
        assert!(hiding_it.recv().unwrap().is_ok());
    }

    /// Each pad is shown again on its own: a showing the service never
    /// answers holds its own reading alone, and another pad's is
    /// answered beside it. A hide of the pad still being shown waits for
    /// that, within its own errand, and is asked once it is done.
    #[test]
    fn a_showing_that_never_ends_holds_only_its_own_pad() {
        let (_dir, hider, hiding) = scripted(Script {
            hold_restores: 1,
            ..Script::default()
        });
        let errand = Arc::new(Errand::default());
        let hidden = |pad| hiding_aside(&hiding, &errand, pad).recv().unwrap().unwrap();
        let (one, other) = (hidden("one"), hidden("other"));
        let showing_one = hiding.show("one", one).unwrap();
        let showing_other = hiding.show("other", other).unwrap();
        let one_shown = reading_aside(&hiding, showing_one);
        hider.wait_held();
        hiding.read(showing_other);

        // Hidden again while its showing is out: the hide waits, and its
        // errand's end ends the wait, having asked nothing.
        let again = Arc::new(Errand::default());
        let waiting = hiding_aside(&hiding, &again, "one");
        assert!(waiting.recv_timeout(SETTLE).is_err(), "it waits");
        hiding.end(&again);
        assert_eq!(waiting.recv().unwrap(), Err(Unhidden::Ended));
        let waiting = hiding_aside(&hiding, &errand, "one");
        assert!(waiting.recv_timeout(SETTLE).is_err(), "it waits");
        assert_eq!(hides(&hider), 2, "nothing asked while it is shown");
        hider.let_go();
        one_shown.recv().unwrap();
        assert!(waiting.recv().unwrap().is_ok(), "asked once it is shown");
        assert_eq!(hides(&hider), 3);
        hiding.close();
    }

    /// Two showings of one pad, each on its own connection, are each
    /// read: neither is hung up for the other.
    #[test]
    fn two_showings_of_one_pad_are_each_read() {
        let (_dir, hider, hiding) = scripted(Script::default());
        let errand = Arc::new(Errand::default());
        let fd = a_descriptor();
        let hidden: Vec<Token> = (0..2)
            .map(|_| hiding.hide(&errand, "pad", fd.as_fd()).unwrap().hold.keep())
            .collect();
        let readings: Vec<Reading> = hidden
            .into_iter()
            .map(|token| hiding.show("pad", token).unwrap())
            .collect();
        for reading in readings {
            hiding.read(reading);
        }
        let restores = hider
            .asked()
            .iter()
            .filter(|asked| matches!(asked, Asked::Restore(_)))
            .count();
        assert_eq!(restores, 2);
    }

    /// Past the most readings under way, the oldest stops sending: the
    /// service gives up waiting for it, and its pad is busy until the
    /// service has answered or closed its end, a hide of it waiting until
    /// then.
    #[test]
    fn past_the_most_readings_the_oldest_stops_and_its_pad_stays_busy() {
        let (_dir, hider, hiding) = scripted(Script {
            hold_restores: MOST_READINGS + 1,
            ..Script::default()
        });
        let errand = Arc::new(Errand::default());
        let pads: Vec<&'static str> = (0..=MOST_READINGS)
            .map(|pad| &*format!("pad{pad}").leak())
            .collect();
        let fd = a_descriptor();
        let mut reads = Vec::new();
        for (count, pad) in pads.iter().enumerate() {
            let token = hiding.hide(&errand, pad, fd.as_fd()).unwrap().hold.keep();
            reads.push(reading_aside(&hiding, hiding.show(pad, token).unwrap()));
            if count < MOST_READINGS {
                hider.wait_holding(count + 1);
            }
        }
        let waiting = hiding_aside(&hiding, &errand, pads[0]);
        assert!(waiting.recv_timeout(SETTLE).is_err(), "it waits");
        assert!(reads[0].recv_timeout(SETTLE).is_err(), "not shown yet");
        assert_eq!(hides(&hider), MOST_READINGS + 1);
        hider.let_go();
        for read in reads {
            read.recv().unwrap();
        }
        assert!(waiting.recv().unwrap().is_ok(), "asked once it is shown");
    }

    /// A request on a hidden pad's own connection that its errand ends is
    /// given up without hanging up, so the service doesn't show the pad
    /// unasked: the pad stays hidden, and its showing again later reads
    /// past the reply given up.
    #[test]
    fn an_open_given_up_leaves_the_pad_hidden_until_shown() {
        let (_dir, hider, hiding) = scripted(Script {
            hold_opens: 1,
            ..Script::default()
        });
        let errand = Arc::new(Errand::default());
        let fd = a_descriptor();
        let token = hiding.hide(&errand, "pad", fd.as_fd()).unwrap().hold.keep();
        let opening = aside({
            let (hiding, errand) = (Arc::clone(&hiding), Arc::clone(&errand));
            move || hiding.raw_of(&errand, token).is_some()
        });
        hider.wait_held();
        hiding.end(&errand);
        assert!(!opening.recv().unwrap(), "given up");
        assert!(
            !hider.asked().contains(&Asked::Closed),
            "the pad's connection stays"
        );
        let showing = hiding.show("pad", token).unwrap();
        hider.let_go();
        hiding.read(showing);
        assert!(
            hider.asked().contains(&Asked::Restore(String::new())),
            "past the reply given up"
        );
    }

    /// A pad hidden whose hold is dropped before it is kept, as when what
    /// follows the hide panics, is given up: the service shows it again
    /// as its connection closes, and a hide of it waits for that.
    #[test]
    fn a_hidden_pad_not_kept_is_shown_again() {
        let (_dir, hider, hiding) = scripted(Script::default());
        let errand = Arc::new(Errand::default());
        let fd = a_descriptor();
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _hidden = hiding.hide(&errand, "pad", fd.as_fd()).unwrap();
            panic!("as a test");
        }));
        assert!(caught.is_err());
        let _token = hiding.hide(&errand, "pad", fd.as_fd()).unwrap().hold.keep();
        assert!(hider.asked().contains(&Asked::Closed), "shown before");
    }

    /// A service whose queue of connections to accept is full is there:
    /// a request waits for room, as it waits for anything, rather than
    /// taking the service for absent, and is made once there is room.
    #[test]
    fn a_full_queue_of_connections_is_waited_for() {
        let (_dir, listener, path) = silent_with(0);
        // Fills the queue.
        let filler = socket(AddressFamily::UNIX, SocketType::SEQPACKET, None).unwrap();
        rustix::net::connect(&filler, &SocketAddrUnix::new(&path).unwrap()).unwrap();
        let hiding = Arc::new(Hiding::new(&path));
        let looking = aside({
            let hiding = Arc::clone(&hiding);
            move || hiding.look()
        });
        assert!(looking.recv_timeout(SETTLE).is_err(), "it waits");
        assert_eq!(hiding.mechanism(), payload::Hiding::Looking);
        // Room: the daemon's connection is made and accepted.
        drop(accept(&listener).unwrap());
        drop(accept(&listener).unwrap());
        hiding.close();
        looking.recv().unwrap();
    }
}
