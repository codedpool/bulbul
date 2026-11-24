// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

// Shown when a hotkey being picked is already the other hold-to-talk hotkey
// (dictation and command mode). Nothing is changed until the person says so:
// "Swap" gives the other one the combo this one is leaving.
export default function HotkeyConflict({ message, onSwap, onCancel }) {
  return (
    <div className="hotkey-conflict" role="alertdialog" aria-live="polite">
      <p className="hotkey-conflict-text">{message}</p>
      <div className="hotkey-conflict-actions">
        <button type="button" onClick={onSwap} autoFocus>
          Swap them
        </button>
        <button type="button" className="text-btn" onClick={onCancel}>
          Keep what I had
        </button>
      </div>
    </div>
  );
}
