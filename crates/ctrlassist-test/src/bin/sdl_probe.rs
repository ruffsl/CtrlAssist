//! What SDL3 makes of the pads on this machine, said a line at a time for
//! a virtual machine check to read: each pad as SDL adds it, with what
//! SDL takes it for, and each button a pad presses, for as long as asked.
//!
//! SDL reads a Steam Controller with a driver of its own, through the
//! pad's raw node, and a DualSense likewise: what this says of a virtual
//! pad is what a game built on SDL sees of it. Built only with the
//! `sdl-probe` feature, which links the system's SDL3.

#![expect(
    unsafe_code,
    reason = "SDL is a C library, called through sdl3-sys's bindings"
)]

use std::ffi::{CStr, c_char};
use std::io::Write as _;
use std::process::ExitCode;

use sdl3_sys::everything::{
    SDL_EVENT_GAMEPAD_ADDED, SDL_EVENT_GAMEPAD_BUTTON_DOWN, SDL_Event, SDL_GetError,
    SDL_GetGamepadNameForID, SDL_GetGamepadPathForID, SDL_GetGamepadProductForID,
    SDL_GetGamepadStringForButton, SDL_GetGamepadStringForType, SDL_GetGamepadTypeForID,
    SDL_GetGamepadVendorForID, SDL_GetTicks, SDL_INIT_GAMEPAD, SDL_Init, SDL_OpenGamepad, SDL_Quit,
    SDL_WaitEventTimeout,
};

/// The text SDL gave, or nothing for none.
fn text(given: *const c_char) -> String {
    if given.is_null() {
        return String::new();
    }
    // SAFETY: SDL returns a NUL-terminated string that stays valid at
    // least until the next call into SDL, and it is copied here first.
    let given = unsafe { CStr::from_ptr(given) };
    given.to_string_lossy().into_owned()
}

fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    // A reader that has gone is nothing to stop for.
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn main() -> ExitCode {
    let millis: u64 = std::env::args()
        .nth(1)
        .and_then(|given| given.parse().ok())
        .unwrap_or(4000);
    // SAFETY: called once, before any other SDL call, from the one thread
    // this program has.
    if !unsafe { SDL_Init(SDL_INIT_GAMEPAD) } {
        say(&format!("init-failed {}", text(SDL_GetError())));
        return ExitCode::FAILURE;
    }
    // SAFETY: SDL is initialized, and the call takes nothing.
    let until = unsafe { SDL_GetTicks() }.saturating_add(millis);
    loop {
        // SAFETY: as above.
        let now = unsafe { SDL_GetTicks() };
        let Some(left) = until.checked_sub(now).filter(|left| *left > 0) else {
            break;
        };
        let wait = i32::try_from(left).unwrap_or(i32::MAX);
        let mut event = SDL_Event::default();
        // SAFETY: `event` is a valid place for SDL to write one event.
        if !unsafe { SDL_WaitEventTimeout(&raw mut event, wait) } {
            continue;
        }
        // SAFETY: every event starts with its type, whichever it is.
        let kind = unsafe { event.r#type };
        if kind == SDL_EVENT_GAMEPAD_ADDED.0 {
            // SAFETY: an event of this type is a gamepad device event.
            let id = unsafe { event.gdevice.which };
            // Each call below takes the id SDL just gave, and returns a
            // value or a string of SDL's own.
            // SAFETY: as said.
            let name = text(unsafe { SDL_GetGamepadNameForID(id) });
            // SAFETY: as said.
            let path = text(unsafe { SDL_GetGamepadPathForID(id) });
            // SAFETY: as said.
            let vendor = unsafe { SDL_GetGamepadVendorForID(id) };
            // SAFETY: as said.
            let product = unsafe { SDL_GetGamepadProductForID(id) };
            // SAFETY: as said.
            let kind = unsafe { SDL_GetGamepadTypeForID(id) };
            // SAFETY: SDL takes any type and returns a string of its own.
            let kind = text(unsafe { SDL_GetGamepadStringForType(kind) });
            say(&format!(
                "added id={} vid={vendor:04x} pid={product:04x} type={kind} path={path} name={name}",
                id.0
            ));
            // Opened, so that SDL reads it and says what it presses. It
            // is left open: the program ends soon.
            // SAFETY: the id is one SDL just gave.
            if unsafe { SDL_OpenGamepad(id) }.is_null() {
                let why = text(SDL_GetError());
                say(&format!("open-failed id={} {why}", id.0));
            }
        } else if kind == SDL_EVENT_GAMEPAD_BUTTON_DOWN.0 {
            // SAFETY: an event of this type is a gamepad button event.
            let pressed = unsafe { event.gbutton };
            let (id, button) = (pressed.which, pressed.button);
            // SAFETY: SDL takes any button number and returns a string
            // of its own, or null.
            let named = text(unsafe {
                SDL_GetGamepadStringForButton(sdl3_sys::everything::SDL_GamepadButton(i32::from(
                    button,
                )))
            });
            say(&format!("button id={} {named} down", id.0));
        }
    }
    say("done");
    // SAFETY: nothing of SDL's is used after this.
    unsafe { SDL_Quit() };
    ExitCode::SUCCESS
}
