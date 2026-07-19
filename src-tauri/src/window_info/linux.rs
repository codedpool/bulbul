//! Linux foreground-app detection.
//!
//! X11 path (also works on Wayland sessions that have XWayland — i.e.
//! most modern desktops, since GNOME and KDE both ship XWayland by
//! default for compatibility): walk from the focused window up to the
//! top-level window, read its WM_CLASS, return the class name. That's
//! the canonical app identifier on X11 (e.g. `"firefox"`, `"code"`,
//! `"org.gnome.Terminal"`).
//!
//! Pure-Wayland sessions (no XWayland): x11rb::connect() fails fast,
//! we return None. The cleanup pipeline degrades gracefully — no venue
//! hint, neutral style. Documented limitation, not a bug.
//!
//! We open a fresh X11 connection per call. That's ~5-10ms per dictation
//! which is well under the perceived-latency budget. If profiling later
//! shows it as a hotspot, cache the connection in a Mutex<Option<...>>.

use super::AppInfo;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, Window};

const WM_CLASS_VALUE_BYTES: u32 = 1024;

// X11 / XWayland only. A GNOME Wayland fallback via
// `org.gnome.Shell.Introspect.GetWindows` was tried and removed: GNOME refuses
// it for unprivileged clients — `AccessDenied: GetWindows is not allowed`
// (confirmed on Fedora 44 + Ubuntu GNOME) — and Wayland exposes no other
// unprivileged API for the focused app. So per-app detection is unavailable for
// native-Wayland windows on GNOME; the venue/Style hint degrades gracefully.
// An X11/Xorg session (or an XWayland app) restores it. A wlroots path
// (wlr-foreign-toplevel) could cover sway/Hyprland later, but not GNOME.
pub fn foreground_app() -> Option<AppInfo> {
    x11_foreground_app()
}

fn x11_foreground_app() -> Option<AppInfo> {
    let (conn, screen_num) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots.get(screen_num)?.root;

    let focused = conn.get_input_focus().ok()?.reply().ok()?.focus;
    // PointerRoot (1) and None (0) aren't real windows. Reject.
    if focused == 0 || focused == 1 {
        return None;
    }

    let top_level = walk_to_top_level(&conn, focused, root)?;
    // WM_CLASS is the id; no separate OS display name on X11 (a friendly name
    // comes from the curated table).
    let class = read_wm_class(&conn, top_level)?;
    Some(AppInfo {
        id: class,
        display: None,
    })
}

pub fn foreground_hwnd() -> isize {
    let Ok((conn, _)) = x11rb::connect(None) else {
        return 0;
    };
    let Ok(cookie) = conn.get_input_focus() else { return 0; };
    let Ok(reply) = cookie.reply() else { return 0; };
    // PointerRoot (1) and None (0) aren't real windows.
    if reply.focus == 0 || reply.focus == 1 {
        return 0;
    }
    reply.focus as isize
}

/// Walk up the X11 window tree until we hit a child of the root window.
/// That's the top-level (the WM-managed window whose WM_CLASS identifies
/// the app). Returns None if the tree query fails at any depth, or if we
/// reach the root itself before finding a top-level (shouldn't happen
/// with a valid focus, but defensive).
fn walk_to_top_level<C: Connection>(conn: &C, mut window: Window, root: Window) -> Option<Window> {
    // X11 trees are shallow (typically 2-5 levels for normal apps).
    // Cap at 32 just in case to defend against pathological cycles.
    for _ in 0..32 {
        if window == root {
            return None;
        }
        let tree = conn.query_tree(window).ok()?.reply().ok()?;
        if tree.parent == root || tree.parent == 0 {
            return Some(window);
        }
        window = tree.parent;
    }
    None
}

/// Read the WM_CLASS property on a top-level window. WM_CLASS is two
/// null-terminated strings concatenated: `instance\0class\0`. The class
/// half is the canonical app identifier; the instance half is sometimes
/// distinct (e.g. `Navigator\0Firefox\0`) but isn't what we want.
fn read_wm_class<C: Connection>(conn: &C, window: Window) -> Option<String> {
    let reply = conn
        .get_property(
            false,
            window,
            AtomEnum::WM_CLASS,
            AtomEnum::STRING,
            0,
            WM_CLASS_VALUE_BYTES,
        )
        .ok()?
        .reply()
        .ok()?;
    let value = reply.value;
    // Split on NUL, drop empty trailing pieces. We want the SECOND piece
    // (the class). Fall back to the first if for some reason class is empty.
    let parts: Vec<&[u8]> = value.split(|&b| b == 0).filter(|s| !s.is_empty()).collect();
    let class_bytes = parts.get(1).copied().or_else(|| parts.first().copied())?;
    Some(String::from_utf8_lossy(class_bytes).into_owned())
}
