// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";

// Windows-only in practice. Modifier-only hotkeys (Ctrl+Win, Alt+Win) rely on a
// low-level keyboard hook that security software / keyboard utilities can block
// so it never installs — and then hold-to-talk silently does nothing. The Rust
// hook watchdog emits `hotkey-status` {ok:false} when that happens while a chord
// hotkey is bound (and {ok:true} when it recovers). We surface it here and offer
// a key-based fallback (Ctrl+Shift+Space), which uses RegisterHotKey — a benign
// API that AV doesn't block, so it works where the hook is blocked.
export default function HotkeyHealthBanner({ config, updateConfig }) {
  const [down, setDown] = useState(null); // { detail } while the hotkey is dead
  const [dismissed, setDismissed] = useState(false);

  useEffect(() => {
    let unlisten;
    listen("hotkey-status", (e) => {
      const p = e.payload || {};
      if (p.ok) {
        setDown(null);
        setDismissed(false);
      } else {
        setDown({ detail: p.detail || "" });
      }
    }).then((f) => {
      unlisten = f;
    });
    return () => {
      if (unlisten) unlisten();
    };
  }, []);

  if (!down || dismissed) return null;

  const current = config?.hotkey || "your hotkey";
  const FALLBACK = "Ctrl+Shift+Space";
  const alreadyFallback = (config?.hotkey || "").toLowerCase() === FALLBACK.toLowerCase();

  async function useFallback() {
    if (config && updateConfig) {
      try {
        await updateConfig({ ...config, hotkey: FALLBACK });
      } catch {
        /* surfacing the toast/error is the caller's job; keep the UI responsive */
      }
    }
    setDismissed(true);
  }

  return (
    <div className="hotkey-health-banner" role="alert">
      <span className="hhb-icon" aria-hidden="true">⚠️</span>
      <div className="hhb-text">
        <strong>Your dictation hotkey ({current}) is not active.</strong>{" "}
        {down.detail} You can switch to a key-based shortcut, or restart Bulbul.
      </div>
      {!alreadyFallback && (
        <button className="hhb-action" onClick={useFallback}>
          Use {FALLBACK}
        </button>
      )}
      <button
        className="hhb-dismiss"
        onClick={() => setDismissed(true)}
        aria-label="Dismiss"
      >
        ×
      </button>
    </div>
  );
}
