import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { IS_WINDOWS } from "./platform.js";

// Only modifier-only chords (Ctrl+Win, Alt+Win) are affected by the WebView2
// focus bug this works around — key-based hotkeys (e.g. Ctrl+Shift+Space) still
// fire through the OS global shortcut even inside Bulbul's own windows, so we
// must NOT in-page-trigger those or we'd double-fire.
const MODS = ["Ctrl", "Shift", "Alt", "Win"];

function modName(e) {
  if (e.key === "Control") return "Ctrl";
  if (e.key === "Shift") return "Shift";
  if (e.key === "Alt") return "Alt";
  if (e.key === "Meta" || e.key === "OS") return "Win";
  return null;
}

// Parse a stored hotkey ("Ctrl+Win") into its parts IF it is modifier-only;
// returns null for key-based hotkeys or empty input.
function modifierOnlyParts(hotkey) {
  const parts = String(hotkey || "")
    .split("+")
    .map((p) => p.trim())
    .filter(Boolean);
  if (parts.length === 0) return null;
  if (!parts.every((p) => MODS.includes(p))) return null;
  return parts;
}

/**
 * In-page hotkey fallback for Bulbul's own WebView windows (setup wizard,
 * scratchpad).
 *
 * A recent WebView2 update stopped delivering modifier-only chords to our
 * global keyboard hook while Bulbul's own window is focused, so pressing the
 * hotkey inside those windows never started dictation. The keys DO still reach
 * the page, so we detect the completed chord here and drive the real pipeline
 * via the `inpage_hotkey` command: press starts recording (listening tray),
 * release transcribes and routes the text back into the focused Bulbul window.
 *
 * Pass a `hotkeyOverride` (the wizard passes the chord being tested); omit it
 * and the hook reads the saved `config.hotkey`.
 *
 * Returns `fallbackUsed` — true once this path has actually fired, which only
 * happens when the global hook did NOT intercept the chord in-window. Callers
 * can use it to surface an honest "if it doesn't work in other apps, use
 * Ctrl+Shift+Space" note.
 */
export function useInPageChordFallback(hotkeyOverride) {
  const [hotkey, setHotkey] = useState(
    typeof hotkeyOverride === "string" ? hotkeyOverride : null,
  );
  const [fallbackUsed, setFallbackUsed] = useState(false);
  const heldRef = useRef(new Set());
  const firedRef = useRef(false);

  // Resolve the hotkey: explicit override wins; otherwise read the saved config.
  useEffect(() => {
    if (typeof hotkeyOverride === "string") {
      setHotkey(hotkeyOverride);
      return;
    }
    let alive = true;
    invoke("get_config")
      .then((cfg) => {
        if (alive) setHotkey(cfg && cfg.hotkey ? cfg.hotkey : null);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [hotkeyOverride]);

  useEffect(() => {
    // Windows-only: the WebView2 focus bug and modifier-only LL-hook chords are
    // Windows concepts; other platforms handle these hotkeys differently and
    // must not be in-page-triggered (it would double-fire).
    if (!IS_WINDOWS) return undefined;
    const required = modifierOnlyParts(hotkey);
    heldRef.current = new Set();
    const evaluate = () => {
      if (!required) return;
      const complete = required.every((m) => heldRef.current.has(m));
      if (complete && !firedRef.current) {
        firedRef.current = true;
        setFallbackUsed(true);
        invoke("inpage_hotkey", { pressed: true }).catch(() => {});
      } else if (!complete && firedRef.current) {
        firedRef.current = false;
        invoke("inpage_hotkey", { pressed: false }).catch(() => {});
      }
    };
    const onDown = (e) => {
      const m = modName(e);
      if (!m) return;
      heldRef.current.add(m);
      evaluate();
      // If this press completed the chord, swallow the Win key's default so
      // Windows doesn't treat the eventual release as a Start-menu tap. WebView2
      // 151 delivers the Win key to the page, so preventDefault reaches it here.
      // Guarded on the chord being engaged — a lone Win press still opens Start.
      if (m === "Win" && firedRef.current) {
        try {
          e.preventDefault();
        } catch (_) {}
      }
    };
    const onUp = (e) => {
      const m = modName(e);
      if (!m) return;
      const wasEngaged = firedRef.current;
      heldRef.current.delete(m);
      evaluate();
      if (m === "Win" && wasEngaged) {
        try {
          e.preventDefault();
        } catch (_) {}
      }
    };
    // Losing focus mid-press would strand the recording — clear + release.
    const onBlur = () => {
      heldRef.current.clear();
      evaluate();
    };
    window.addEventListener("keydown", onDown, true);
    window.addEventListener("keyup", onUp, true);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onDown, true);
      window.removeEventListener("keyup", onUp, true);
      window.removeEventListener("blur", onBlur);
      // Safety: release if we unmount mid-press so no recording is stranded.
      if (firedRef.current) {
        firedRef.current = false;
        invoke("inpage_hotkey", { pressed: false }).catch(() => {});
      }
    };
  }, [hotkey]);

  return fallbackUsed;
}
