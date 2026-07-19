//! Foreground-app detection. The cleanup pipeline + correction watcher
//! both need to know which app the user is dictating into.
//!
//! Per-platform impl is in `windows.rs` / `macos.rs`. Callers do
//! `crate::window_info::foreground_app()` without caring.

/// A detected foreground app.
///
/// `id` is the stable identifier the rest of the pipeline matches on —
/// Windows exe stem (`Code.exe`), macOS bundle id (`com.apple.Safari`),
/// Linux WM_CLASS (`firefox`). It keys corrections, per-app Style, and the
/// curated name table, so it must stay stable across locales/sessions.
///
/// `display` is a human-readable name when the OS can give us one directly
/// (macOS `localizedName`). It's `None` on platforms/paths that only expose
/// the id; callers then fall back to `config::friendly_app_name(id)`.
pub struct AppInfo {
    pub id: String,
    pub display: Option<String>,
}

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::*;

/// Parse the focused window's `app-id` from the gdbus text output of
/// `org.gnome.Shell.Introspect.GetWindows`. The output is a dict
/// `({uint64 id: { ...props... }, ...},)` where each window's property dict has
/// no nested braces of its own; we descend through the outer brace(s) to those
/// innermost blocks, pick the one carrying `'has-focus': <true>`, and return
/// its `'app-id': <'...'>` (e.g. `org.mozilla.firefox`). Kept OS-agnostic so it
/// can be unit-tested off Linux. Returns None when nothing is focused / unparsable.
#[cfg(any(target_os = "linux", test))]
pub(crate) fn focused_app_id_from_gnome_introspect(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'{' {
            i += 1;
            continue;
        }
        // Find this block's end, descending if a nested '{' appears first.
        let mut j = i + 1;
        while j < bytes.len() && bytes[j] != b'}' && bytes[j] != b'{' {
            j += 1;
        }
        if j < bytes.len() && bytes[j] == b'{' {
            i = j; // outer brace — descend into the nested block
            continue;
        }
        if j >= bytes.len() {
            break;
        }
        let block = &text[i + 1..j];
        if block.contains("'has-focus': <true>") {
            if let Some(id) = variant_string_after(block, "'app-id': <'") {
                if !id.is_empty() {
                    return Some(id);
                }
            }
        }
        i = j + 1;
    }
    None
}

#[cfg(any(target_os = "linux", test))]
fn variant_string_after(block: &str, marker: &str) -> Option<String> {
    let start = block.find(marker)? + marker.len();
    let end = block[start..].find('\'')?;
    Some(block[start..start + end].to_string())
}

#[cfg(test)]
mod tests {
    use super::focused_app_id_from_gnome_introspect as parse;

    #[test]
    fn picks_the_focused_window_app_id() {
        let sample = "({uint64 111: {'app-id': <'org.mozilla.firefox'>, 'has-focus': <false>, 'title': <'Firefox'>}, \
                       uint64 222: {'app-id': <'com.google.Antigravity'>, 'has-focus': <true>, 'title': <'main.rs'>}},)";
        assert_eq!(parse(sample).as_deref(), Some("com.google.Antigravity"));
    }

    #[test]
    fn handles_focus_key_before_app_id() {
        let sample = "({uint64 5: {'has-focus': <true>, 'app-id': <'org.gnome.Console'>}},)";
        assert_eq!(parse(sample).as_deref(), Some("org.gnome.Console"));
    }

    #[test]
    fn none_when_nothing_focused_or_empty() {
        assert_eq!(parse("({uint64 1: {'app-id': <'org.gnome.Nautilus'>, 'has-focus': <false>}},)"), None);
        assert_eq!(parse("()"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn skips_focused_window_with_empty_app_id() {
        // A focused window can report an empty app-id; don't return "".
        let sample = "({uint64 9: {'app-id': <''>, 'has-focus': <true>}},)";
        assert_eq!(parse(sample), None);
    }
}
