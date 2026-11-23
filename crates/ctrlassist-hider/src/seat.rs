//! Who may use the service: the user whose session is in front on a seat.
//!
//! The rule is TisVlay's, learned there the hard way. It never asks
//! whether a user or a session is "active", because a plain SSH login
//! reports itself active. It asks which session each seat has in front,
//! and accepts that session only as a person at the machine: class `user`
//! (not the greeter), type `x11` or `wayland` (not a bare console), and
//! not remote.
//!
//! The answers come from logind through systemd's login library, which
//! reads them without a D-Bus connection.

#![expect(
    unsafe_code,
    reason = "systemd's login library is C; each call is one operation with its contract stated"
)]

use std::ffi::{CStr, CString, c_char, c_int};

#[link(name = "systemd")]
unsafe extern "C" {
    fn sd_seat_get_active(
        seat: *const c_char,
        session: *mut *mut c_char,
        uid: *mut libc::uid_t,
    ) -> c_int;
    fn sd_session_get_class(session: *const c_char, class: *mut *mut c_char) -> c_int;
    fn sd_session_get_type(session: *const c_char, kind: *mut *mut c_char) -> c_int;
    fn sd_session_is_remote(session: *const c_char) -> c_int;
}

/// A string the login library allocated, freed when dropped.
struct Owned(*mut c_char);

impl Owned {
    fn as_c_str(&self) -> &CStr {
        // SAFETY: the pointer came from the login library on success,
        // which returns a valid NUL-terminated string that lives until it
        // is freed, and this type frees it only on drop.
        unsafe { CStr::from_ptr(self.0) }
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the pointer was allocated by the login library with
        // malloc, as its documentation states, and is freed exactly once.
        unsafe { libc::free(self.0.cast()) }
    }
}

/// The session in front on `seat` and the user who owns it.
fn foreground(seat: &CStr) -> Option<(Owned, u32)> {
    let mut session = std::ptr::null_mut();
    let mut uid: libc::uid_t = 0;
    // SAFETY: `seat` is a valid C string; `session` and `uid` are valid
    // places for the function to write its two results.
    let result = unsafe { sd_seat_get_active(seat.as_ptr(), &raw mut session, &raw mut uid) };
    (result >= 0 && !session.is_null()).then(|| (Owned(session), uid))
}

fn class(session: &CStr) -> Option<Owned> {
    let mut value = std::ptr::null_mut();
    // SAFETY: `session` is a valid C string and `value` a valid place for
    // the result.
    let result = unsafe { sd_session_get_class(session.as_ptr(), &raw mut value) };
    (result >= 0 && !value.is_null()).then_some(Owned(value))
}

fn kind(session: &CStr) -> Option<Owned> {
    let mut value = std::ptr::null_mut();
    // SAFETY: `session` is a valid C string and `value` a valid place for
    // the result.
    let result = unsafe { sd_session_get_type(session.as_ptr(), &raw mut value) };
    (result >= 0 && !value.is_null()).then_some(Owned(value))
}

fn is_remote(session: &CStr) -> bool {
    // SAFETY: `session` is a valid C string. Anything but zero, an error
    // included, is treated as remote.
    unsafe { sd_session_is_remote(session.as_ptr()) != 0 }
}

/// The seats on which `uid` owns the session in front, where that session
/// is a person at the machine.
pub(crate) fn seats_in_front(uid: u32) -> Vec<String> {
    // Seats are the entries of this directory; the login library's own
    // seat listing reads the same place.
    let Ok(seats) = std::fs::read_dir("/run/systemd/seats") else {
        return Vec::new();
    };
    seats
        .flatten()
        .filter_map(|seat| {
            let name = seat.file_name().into_string().ok()?;
            let seat = CString::new(name.as_bytes()).ok()?;
            let (session, owner) = foreground(&seat)?;
            let session = session.as_c_str();
            let person = owner == uid
                && class(session).is_some_and(|class| class.as_c_str() == c"user")
                && kind(session)
                    .is_some_and(|kind| matches!(kind.as_c_str().to_bytes(), b"x11" | b"wayland"))
                && !is_remote(session);
            person.then_some(name)
        })
        .collect()
}

/// Whether `uid` owns the session in front on some seat, and that session
/// is a person at the machine.
pub(crate) fn is_in_front(uid: u32) -> bool {
    !seats_in_front(uid).is_empty()
}
