// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

import { useEffect, useState } from "react";

// Standard DOM MouseEvent.button values Chromium (Bulbul's WebView on
// Windows/Linux) and most browsers deliver for the buttons Mouse mode
// supports: 1 = middle, 3 = back, 4 = forward. Left (0) and right (2)
// are never offered — they're needed for normal clicking. Side-button
// support in the webview varies by platform engine (reliable on
// Chromium/WebView2; less consistent on WebKit), so a mouse without
// working side buttons in the recorder can still use Middle.
const BUTTON_TO_VALUE = { 1: "middle", 3: "back", 4: "forward" };
const VALUE_TO_LABEL = { middle: "Middle click", back: "Side button (back)", forward: "Side button (forward)" };

// `value` falsy (null/undefined) means "nothing recorded yet" — distinct
// from any real button choice, so the recorder can show a clear
// call-to-action instead of silently defaulting to "Middle click" as if
// that had already been picked.
export function mouseButtonLabel(value) {
  if (!value) return "Click to record a button";
  return VALUE_TO_LABEL[value] || VALUE_TO_LABEL.middle;
}

/**
 * Compact click recorder for Mouse mode's trigger button. Mirrors
 * TransformsView's HotkeyRecorder: click "Record", then click the mouse
 * button you want (middle or a side button); Escape cancels. Reuses the
 * same .hotkey-recorder / .hotkey-record-btn styling.
 */
export default function MouseButtonRecorder({ value, onChange }) {
  const [recording, setRecording] = useState(false);
  const [err, setErr] = useState("");

  useEffect(() => {
    if (!recording) return;
    const onMouseDown = (e) => {
      const picked = BUTTON_TO_VALUE[e.button];
      if (!picked) {
        e.preventDefault();
        setErr("That's not a supported button — try the middle click or a side button.");
        return;
      }
      e.preventDefault();
      onChange(picked);
      setRecording(false);
      setErr("");
    };
    const onKeyDown = (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        setRecording(false);
        setErr("");
      }
    };
    window.addEventListener("mousedown", onMouseDown, true);
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      window.removeEventListener("mousedown", onMouseDown, true);
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, [recording, onChange]);

  return (
    <div className="hotkey-recorder">
      {recording ? (
        <button
          type="button"
          className="hotkey-record-btn recording"
          onClick={() => {
            setRecording(false);
            setErr("");
          }}
        >
          Click a mouse button… <span className="muted small">Esc to cancel</span>
        </button>
      ) : (
        <button
          type="button"
          className="hotkey-record-btn"
          onClick={() => {
            setErr("");
            setRecording(true);
          }}
        >
          {mouseButtonLabel(value)}
        </button>
      )}
      {err && (
        <p className="err small" role="alert">
          {err}
        </p>
      )}
    </div>
  );
}
