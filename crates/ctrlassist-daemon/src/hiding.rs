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
//! - **A setup's requests are its [`Errand`].** Ending it hangs up every
//!   connection it has a request under way on, which ends the request at
//!   once, and the service shows again whatever was hidden through that
//!   connection, as it does for any connection that closes; anything it
//!   asks after is refused without asking. A setup's errand ends when it
//!   is stopped, fails or is given up.
//! - **As the daemon stops, every request is hung up** and none is made
//!   after, so nothing it waits on is a round trip to the service.

use std::collections::{BTreeMap, BTreeSet};
use std::os::fd::{BorrowedFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use ctrlassist_hider::{Client, Error, Refusal};
use ctrlassist_protocol::payload;
use tracing::{debug, info, warn};

/// The daemon's name for a pad the hiding service has hidden for it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub(crate) struct Token(pub(crate) u64);

/// The requests to the hiding service made for one setup, or for the
/// daemon itself: ended together, as the setup stops or fails, or as the
/// daemon does.
#[derive(Debug, Default)]
pub(crate) struct Errand {
    ended: AtomicBool,
}

impl Errand {
    /// Whether it has been ended: what it asks from now on isn't asked.
    pub(crate) fn has_ended(&self) -> bool {
        self.ended.load(Ordering::Relaxed)
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
}

impl Connection {
    fn new(client: Client) -> Arc<Self> {
        Arc::new(Self {
            client,
            turn: Mutex::new(()),
            hung_up: AtomicBool::new(false),
        })
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
    /// It was there when last asked something, and said this release:
    /// none from one too old to be asked.
    There(Option<String>),
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
    /// Each request under way: for which errand, on which connection.
    asking: Vec<(Arc<Errand>, Arc<Connection>)>,
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
    /// The service refused.
    Refused,
    /// The service isn't there, or its connection broke.
    NotThere,
}

/// A pad the service has just hidden.
#[derive(Debug)]
pub(crate) struct Hidden {
    pub(crate) token: Token,
    /// Its event node, opened by the service once it was hidden.
    pub(crate) event: OwnedFd,
}

/// Why a pad wasn't hidden.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Unhidden {
    /// The errand it was asked for ended first.
    Ended,
    /// The service isn't there, or refused: the pad is to be grabbed.
    Not,
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
                Found::There(release) => {
                    info!(?release, "the hiding service is there");
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

    /// Ends `errand`: each request it has under way is hung up, and
    /// nothing it asks from now on is asked.
    pub(crate) fn end(&self, errand: &Arc<Errand>) {
        let state = self.state();
        errand.ended.store(true, Ordering::Relaxed);
        for (asker, connection) in &state.asking {
            if Arc::ptr_eq(asker, errand) {
                connection.hang_up();
            }
        }
        drop(state);
        self.changed.notify_all();
    }

    /// Hangs up every request under way, as the daemon stops, and asks
    /// nothing from now on: nothing is left waiting on the service. The
    /// pads still hidden are shown again by the service as the
    /// connections they were hidden on close, with the daemon.
    pub(crate) fn close(&self) {
        let mut state = self.state();
        state.closed = true;
        self.own.ended.store(true, Ordering::Relaxed);
        for (_, connection) in &state.asking {
            connection.hang_up();
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
            Found::There(_) => payload::Hiding::Service,
        }
    }

    /// The found service's release, for the daemon's state: none with
    /// no service, or from one too old to say.
    pub(crate) fn service_release(&self) -> Option<String> {
        match &self.state().found {
            Found::There(release) => release.clone(),
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
        {
            let mut state = self.state();
            if state.closed || errand.has_ended() {
                return Err(Unasked::Ended);
            }
            state
                .asking
                .push((Arc::clone(errand), Arc::clone(connection)));
        }
        let outcome = {
            let _turn = connection
                .turn
                .lock()
                .expect("nothing panics while a connection to the hiding service is asked on");
            (!connection.is_hung_up()).then(|| ask(&connection.client))
        };
        let mut state = self.state();
        let asking = state
            .asking
            .iter()
            .position(|(asker, on)| Arc::ptr_eq(asker, errand) && Arc::ptr_eq(on, connection));
        if let Some(asking) = asking {
            state.asking.swap_remove(asking);
        }
        drop(state);
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

    /// A new connection to the service, for `errand`. One not known to
    /// be there says its release first.
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
        if !matches!(self.state().found, Found::There(_)) {
            match self.ask(errand, &connection, Client::version) {
                Ok(release) => self.found(Found::There(Some(release))),
                // From before it could be asked.
                Err(Unasked::Refused) => self.found(Found::There(None)),
                Err(unasked) => return Err(unasked),
            }
        }
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
        let hidden = self.connect(errand).and_then(|connection| {
            let hidden = self.ask(errand, &connection, |client| client.hide(fd))?;
            Ok((connection, hidden))
        });
        match hidden {
            Ok((connection, hidden)) => {
                let mut state = self.state();
                // Hidden by re-binding its driver, it is shown again the
                // same way.
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
                })
            }
            Err(Unasked::Ended) => Err(Unhidden::Ended),
            Err(Unasked::Refused | Unasked::NotThere) => Err(Unhidden::Not),
        }
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
    /// same; not if it refused, or the pad isn't one it hid.
    fn show(&self, token: Token) -> bool {
        let Some(held) = self.state().held.remove(&token) else {
            return false;
        };
        let own = Arc::clone(&self.own);
        let shown = self.ask(&own, &held.connection, |client| client.restore(held.token));
        !matches!(shown, Err(Unasked::Refused))
    }

    /// Restores each pad named, once what read them has been dropped.
    /// Returns those the service won't show again, by what they are
    /// known by.
    pub(crate) fn restore(&self, pads: &[(String, Token)]) -> Vec<String> {
        pads.iter()
            .filter(|(_, token)| !self.show(*token))
            .map(|(pad, _)| pad.clone())
            .collect()
    }

    /// Restores a pad nothing else is told of.
    pub(crate) fn restore_token(&self, token: Token) {
        self.show(token);
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
        hiding.found(Found::There(None));
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
            Err(Unhidden::Not)
        );
        assert_eq!(
            hiding.restore(&[("pad".to_string(), Token(1))]),
            ["pad".to_string()]
        );
        assert!(hiding.reopen(&errand, Token(1)).is_none());
    }
}
