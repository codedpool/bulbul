// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

//! Keeps Mouse mode's button from reaching other apps on X11.
//!
//! `linux_mouse.rs` swallows the button by taking a plain mouse over
//! exclusively. That is impossible for pointers that report absolute
//! positions (touchpads, pointing sticks, and the "tablet" pointer a virtual
//! machine presents), so for those the click is only *watched*, and the
//! desktop still acts on it. On Linux a middle click pastes the selection,
//! so every Mouse mode click also pasted whatever was last highlighted.
//!
//! On an X11 session the window system can be told to keep the button
//! instead: a passive grab on the root window delivers the press and release
//! to us and to nobody else. It works for any kind of pointer, and it needs
//! no cleanup if Bulbul dies: the X server drops a client's grabs when its
//! connection closes, so unlike a device grab it can never leave the pointer
//! stuck. We don't use the grabbed events themselves (the evdev watcher in
//! `linux_mouse.rs` is what toggles dictation); they are only read and
//! dropped so the connection's queue doesn't fill up.
//!
//! Does nothing on Wayland sessions: there, an X grab would only cover X11
//! apps running under XWayland, not the desktop itself.

use std::thread;
use std::time::Duration;

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ButtonIndex, ConnectionExt as _, EventMask, GrabMode, ModMask};
use x11rb::rust_connection::RustConnection;

/// How often the grab is checked against the current setting. A change to
/// Mouse mode (or its button) takes effect within this long.
const POLL: Duration = Duration::from_millis(100);

/// True when this process runs inside an X11 session where a root-window
/// grab will actually affect the apps on screen.
pub fn available() -> bool {
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    if var("DISPLAY").is_empty() {
        return false;
    }
    let wayland = var("XDG_SESSION_TYPE").eq_ignore_ascii_case("wayland")
        || !var("WAYLAND_DISPLAY").is_empty();
    !wayland
}

/// Starts the grab thread. `alive` says whether this registration is still
/// the current one (the thread ends, releasing the grab, once it is not).
/// `wanted` is the X button number to keep from other apps right now (2 =
/// middle, 8 = back, 9 = forward), or `None` while Mouse mode is off.
/// Re-read every cycle, so changing the button in Settings needs no restart.
pub fn spawn<A, W>(alive: A, wanted: W)
where
    A: Fn() -> bool + Send + 'static,
    W: Fn() -> Option<u8> + Send + 'static,
{
    if !available() {
        tracing::debug!("mouse mode: no X11 session, not reserving the button at the display level");
        return;
    }
    let spawned = thread::Builder::new().name("bulbul-xgrab".into()).spawn(move || {
        if let Err(e) = run(&alive, &wanted) {
            tracing::warn!("mouse mode: X11 button reservation stopped: {e}");
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("mouse mode: couldn't start the X11 button reservation: {e}");
    }
}

fn run(alive: &dyn Fn() -> bool, wanted: &dyn Fn() -> Option<u8>) -> Result<(), Box<dyn std::error::Error>> {
    let (conn, screen_num) = RustConnection::connect(None)?;
    let root = conn.setup().roots[screen_num].root;
    // `requested` is the button we last tried to hold (so a refused grab is
    // not retried every cycle); `held` is the one we actually hold.
    let mut requested: Option<u8> = None;
    let mut held: Option<u8> = None;

    while alive() {
        let want = wanted();
        if want != requested {
            requested = want;
            if let Some(old) = held.take() {
                conn.ungrab_button(ButtonIndex::from(old), root, ModMask::ANY)?;
            }
            if let Some(btn) = want {
                // Any modifier combination: Shift+click and friends are kept
                // too, matching how Windows suppresses the button.
                let grab = conn.grab_button(
                    false,
                    root,
                    EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE,
                    GrabMode::ASYNC,
                    GrabMode::ASYNC,
                    x11rb::NONE,
                    x11rb::NONE,
                    ButtonIndex::from(btn),
                    ModMask::ANY,
                )?;
                match grab.check() {
                    Ok(()) => {
                        tracing::info!("mouse mode: keeping X button {btn} from other apps");
                        held = Some(btn);
                    }
                    // Another program already holds a grab on this button.
                    // Not fatal: the click still toggles dictation, it just
                    // reaches other apps too.
                    Err(e) => tracing::warn!("mouse mode: couldn't reserve X button {btn}: {e}"),
                }
            }
            conn.flush()?;
        }
        // Read and drop whatever the grab delivered.
        while conn.poll_for_event()?.is_some() {}
        thread::sleep(POLL);
    }

    if let Some(old) = held {
        let _ = conn.ungrab_button(ButtonIndex::from(old), root, ModMask::ANY);
        let _ = conn.flush();
    }
    Ok(())
}
