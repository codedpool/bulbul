// Lightweight runtime platform detection for the frontend. Tauri's
// webview UA reliably contains "Mac" / "Linux" / "Windows" tokens, so
// we read straight off navigator.userAgent — no plugin round-trip
// needed, and these helpers can be imported at module top-level
// (constants are resolved at module-evaluation time).
//
// If a future Tauri version ships a webview where UA detection is
// unreliable, swap these for `@tauri-apps/plugin-os` calls.

export const IS_MAC = /Mac/i.test(navigator.userAgent);
export const IS_LINUX = /Linux/i.test(navigator.userAgent) && !IS_MAC;
export const IS_WINDOWS = !IS_MAC && !IS_LINUX;

/// Platform-aware label for the autostart toggle in Settings.
export const AUTOSTART_LABEL = IS_MAC
  ? "Open Bulbul at login"
  : IS_LINUX
    ? "Start Bulbul automatically"
    : "Start Bulbul with Windows";

/// Platform-aware "where do I find Bulbul if I close the window"
/// hint. Tray icon hide path uses this; the wording matches what users
/// would actually look for on their OS.
export const RELAUNCH_HINT = IS_MAC
  ? "Re-launch Bulbul from Spotlight or Applications."
  : IS_LINUX
    ? "Re-launch Bulbul from your application launcher."
    : "Re-launch Bulbul from the Start menu.";

/// Generic OS noun for hero copy ("Hold your hotkey anywhere on X").
export const OS_NOUN = IS_MAC ? "macOS" : IS_LINUX ? "Linux" : "Windows";

/// Theme/appearance hint — Mac and Linux both have system-level
/// dark mode preferences; the label can be more universal.
export const THEME_FOLLOW_HINT = IS_MAC
  ? "Light, dark, or follow macOS."
  : IS_LINUX
    ? "Light, dark, or follow your system."
    : "Light, dark, or follow Windows.";

/// Pretty name for the OS-level modifier key. Used in coaching text
/// ("Now also hold X") where the raw "Win" reads awkwardly.
///   Windows: Win key  → "Windows"
///   macOS:   ⌘ Cmd    → "Command"
///   Linux:   Super    → "Super"
export const META_KEY_NAME = IS_MAC
  ? "Command"
  : IS_LINUX
    ? "Super"
    : "Windows";

// Tag <html> with a platform class so CSS can apply OS-specific
// rules — Mac wants a transparent body + rounded shell corners since
// the native window is borderless + transparent (decorations: false +
// macOSPrivateApi: true). Windows + Linux keep the existing opaque
// shell since their native borderless windows handle corner rounding
// at the OS level (Win11 corner radius, GNOME/KDE compositor).
if (typeof document !== "undefined") {
  const cls = IS_MAC ? "platform-mac" : IS_LINUX ? "platform-linux" : "platform-windows";
  document.documentElement.classList.add(cls);
}
