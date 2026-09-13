// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./Overlay.css";

const LANGUAGES = [
  { code: "auto", label: "Auto-detect" },
  { code: "en", label: "English" },
  { code: "hi", label: "Hindi" },
  { code: "es", label: "Spanish" },
  { code: "fr", label: "French" },
  { code: "de", label: "German" },
  { code: "it", label: "Italian" },
  { code: "pt", label: "Portuguese" },
  { code: "nl", label: "Dutch" },
  { code: "ru", label: "Russian" },
  { code: "ja", label: "Japanese" },
  { code: "ko", label: "Korean" },
  { code: "zh", label: "Chinese" },
  { code: "ar", label: "Arabic" },
  { code: "tr", label: "Turkish" },
  { code: "pl", label: "Polish" },
  { code: "uk", label: "Ukrainian" },
  { code: "sv", label: "Swedish" },
  { code: "fi", label: "Finnish" },
  { code: "id", label: "Indonesian" },
  { code: "vi", label: "Vietnamese" },
  { code: "th", label: "Thai" },
  { code: "he", label: "Hebrew" },
  { code: "el", label: "Greek" },
];

const COMPACT_HEIGHT = 48;
const DROPDOWN_HEIGHT = 260;

// Values match config.rs's `overlay_position` / desktop.rs's
// nearest_overlay_zone exactly — "left"/"right" dock to a screen edge with
// a vertical layout, anything else (the default "bottom-center") is the
// original bottom-anchored horizontal pill.
function isVerticalAnchor(pos) {
  return pos === "left" || pos === "right";
}

const ZONE_LABEL = { "bottom-center": "Bottom", left: "Left", right: "Right" };

export default function Overlay() {
  const [status, setStatus] = useState({ state: "idle", message: null });
  const [hovered, setHovered] = useState(false);
  const [lang, setLang] = useState("auto");
  const [langOpen, setLangOpen] = useState(false);
  // Bulbul's pipeline emits state="idle" with a message string for the
  // two silent-rejection paths ("Too short, ignored." and "Silence —
  // nothing to transcribe."). Without inflating those into a distinct
  // transient state, the overlay swallows them — the pill just shrinks
  // back to its dot with no clue why nothing was typed. Many dictation
  // apps silently fail this same way; we surface a brief amber pill
  // instead so users can self-diagnose.
  const [transientReject, setTransientReject] = useState(null);
  // Which edge the pill is currently docked to — drives the resting
  // capsule's rotated (9x40 vs 40x9) sizing in Overlay.css. Kept in sync
  // with the backend (the source of truth, since dragging and the
  // Settings picker both write it there) via the overlay-position-changed
  // event, not written locally.
  const [dockAnchor, setDockAnchor] = useState("bottom-center");
  // Drag-to-reposition state (see start_overlay_drag/end_overlay_drag in
  // desktop.rs). `dragZone` mirrors whichever of the three dock points is
  // currently nearest, live, for the "release here" hint.
  const [dragging, setDragging] = useState(false);
  const [dragZone, setDragZone] = useState(null);

  useEffect(() => {
    document.body.style.background = "transparent";
    document.documentElement.style.background = "transparent";

    invoke("get_config")
      .then((cfg) => {
        setLang(cfg.language || "auto");
        setDockAnchor(cfg.overlay_position || "bottom-center");
      })
      .catch(() => {});

    const un1 = listen("bulbul-status", (e) => {
      const payload = e.payload || { state: "idle", message: null };
      setStatus(payload);
      if (payload.state === "idle" && payload.message) {
        let kind = null;
        if (/too short/i.test(payload.message)) kind = "too_short";
        else if (/(silence|no speech)/i.test(payload.message)) kind = "silent";
        if (kind) setTransientReject({ kind, message: payload.message });
      }
    });
    const un2 = listen("overlay-hover", async (e) => {
      setHovered(e.payload);
      if (e.payload) {
        try {
          const cfg = await invoke("get_config");
          setLang(cfg.language || "auto");
        } catch {}
      }
    });
    const un3 = listen("overlay-position-changed", (e) => setDockAnchor(e.payload));
    const un4 = listen("overlay-drag-zone", (e) => setDragZone(e.payload));
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
      un3.then((f) => f());
      un4.then((f) => f());
    };
  }, []);

  // Auto-clear the transient rejection pill after a brief dwell. 2.2s
  // matches the wizard's "done" celebration dwell — long enough to read
  // a short label, short enough not to obstruct the next attempt.
  useEffect(() => {
    if (!transientReject) return;
    const t = setTimeout(() => setTransientReject(null), 2200);
    return () => clearTimeout(t);
  }, [transientReject]);

  // When a transient reject is showing, treat it as the active visible
  // state for the overlay's expand/collapse + render decisions, but
  // keep `status` (true backend state) untouched.
  const effectiveState = transientReject ? transientReject.kind : status.state;
  const effectiveMessage = transientReject ? transientReject.message : status.message;

  // Close dropdown when cursor leaves or a dictation starts.
  useEffect(() => {
    if (!hovered || effectiveState !== "idle") {
      if (langOpen) setLangOpen(false);
    }
  }, [hovered, effectiveState]);

  // Close on a click anywhere else in the overlay — the hover-exit check
  // above only fires once the cursor actually leaves the window, so a
  // click on empty space or another satellite button while the dropdown
  // is open did nothing. Excludes the toggle button itself: its own
  // onClick already flips langOpen, and this handler firing first (on
  // pointerdown, before the button's click event) would otherwise close
  // it and let the button's toggle immediately reopen it.
  useEffect(() => {
    if (!langOpen) return;
    function handlePointerDown(e) {
      const dropdown = document.querySelector(".lang-dropdown");
      const btn = document.querySelector(".lang-btn");
      if (dropdown && !dropdown.contains(e.target) && !(btn && btn.contains(e.target))) {
        setLangOpen(false);
      }
    }
    document.addEventListener("pointerdown", handlePointerDown, true);
    return () => document.removeEventListener("pointerdown", handlePointerDown, true);
  }, [langOpen]);

  // Resize the overlay window when the dropdown opens or closes.
  useEffect(() => {
    invoke("set_overlay_height", { height: langOpen ? DROPDOWN_HEIGHT : COMPACT_HEIGHT })
      .catch(() => {});
  }, [langOpen]);

  async function selectLanguage(code) {
    try {
      const cfg = await invoke("get_config");
      await invoke("save_config", { newCfg: { ...cfg, language: code } });
      setLang(code);
    } finally {
      setLangOpen(false);
    }
  }

  const vertical = isVerticalAnchor(dockAnchor);
  const showSatellites = hovered && effectiveState === "idle";
  const expanded = showSatellites || effectiveState !== "idle";

  // Drag-to-reposition: press on the resting pill (not while a dictation
  // is actually in flight — that shouldn't be interruptible by an
  // accidental drag) starts the gesture; the Rust-side hover-watcher does
  // the actual window-following (see start_overlay_drag/end_overlay_drag
  // in desktop.rs). Pointer capture keeps move/up events targeting this
  // element even once the window has moved out from under the cursor
  // between polling ticks.
  function onPillPointerDown(e) {
    if (effectiveState !== "idle" || e.button !== 0) return;
    e.currentTarget.setPointerCapture(e.pointerId);
    setDragging(true);
    invoke("start_overlay_drag").catch(() => setDragging(false));
  }
  function onPillPointerUp(e) {
    if (!dragging) return;
    e.currentTarget.releasePointerCapture(e.pointerId);
    setDragging(false);
    setDragZone(null);
    invoke("end_overlay_drag").catch(() => {});
  }

  return (
    <div
      className={`overlay ${expanded ? "expanded" : "collapsed"} ${hovered ? "hovered" : ""} ${vertical ? "vertical" : ""} dock-${dockAnchor} ${dragging ? "dragging" : ""}`}
    >
      {langOpen && (
        <div className="lang-dropdown" role="listbox">
          {LANGUAGES.map((l) => (
            <div
              key={l.code}
              className={`lang-option ${lang === l.code ? "active" : ""}`}
              onClick={() => selectLanguage(l.code)}
              role="option"
              aria-selected={lang === l.code}
            >
              <span className="lang-code">{l.code === "auto" ? "—" : l.code.toUpperCase()}</span>
              <span className="lang-name">{l.label}</span>
            </div>
          ))}
        </div>
      )}

      <div className="pill-row">
        {showSatellites && (
          <button
            className={`sat lang-btn ${langOpen ? "open" : ""}`}
            title="Change language"
            onClick={() => setLangOpen((v) => !v)}
          >
            {langDisplay(lang)}
          </button>
        )}

        <div
          className={`pill pill-${effectiveState} ${dragging ? "drag-hint" : ""}`}
          onPointerDown={onPillPointerDown}
          onPointerUp={onPillPointerUp}
          // Vertical dock never shows the label text (see below) — a tall,
          // narrow pill has no room for a sentence without either
          // overflowing or being rotated unreadably small. The native
          // title tooltip keeps that text reachable (hover to read it)
          // instead of silently dropping it, which matters for the
          // states that carry real information (why a take was
          // rejected, how long a rate-limit backoff is), not just a
          // decorative state name.
          title={
            vertical && expanded && effectiveState !== "idle"
              ? (effectiveState === "rate_limited" ? (effectiveMessage || "Rate limited…") : label(effectiveState))
              : undefined
          }
        >
          {dragging ? (
            <span className="pill-label drag-label">{ZONE_LABEL[dragZone] || "…"}</span>
          ) : (
            <>
              <span className="pill-icon">{renderIcon(effectiveState, hovered)}</span>
              {expanded && effectiveState !== "idle" && !vertical && (
                <span className="pill-label">
                  {effectiveState === "rate_limited"
                    ? (effectiveMessage || "Rate limited…")
                    : label(effectiveState)}
                </span>
              )}
            </>
          )}
        </div>

        {showSatellites && (
          <button
            className="sat scratch-btn"
            title="Open Scratchpad"
            onClick={() => invoke("open_scratchpad").catch(() => {})}
          >
            <NoteIcon />
          </button>
        )}

        {showSatellites && (
          <button
            className="sat hide-btn"
            title="Hide pill (Settings > General > Hide tray icon to bring it back)"
            // Same command, same config field the Settings toggle uses —
            // there's no separate "pill hidden" flag to keep in sync,
            // this just flips hide_tray directly. set_tray_visible(false)
            // already hides the overlay itself immediately (not just the
            // tray icon), so no extra call is needed here.
            onClick={() => invoke("set_tray_visible", { visible: false }).catch(() => {})}
          >
            <EyeOffIcon />
          </button>
        )}
      </div>
    </div>
  );
}

function EyeOffIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M9.88 9.88a3 3 0 1 0 4.24 4.24" />
      <path d="M10.73 5.08A10.43 10.43 0 0 1 12 5c7 0 10 7 10 7a13.16 13.16 0 0 1-1.67 2.68" />
      <path d="M6.61 6.61A13.53 13.53 0 0 0 2 12s3 7 10 7a9.74 9.74 0 0 0 5.39-1.61" />
      <line x1="2" x2="22" y1="2" y2="22" />
    </svg>
  );
}

function NoteIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M14 4H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-8" />
      <path d="m18 2 4 4-10 10H8v-4z" />
    </svg>
  );
}

function renderIcon(state, hovered) {
  switch (state) {
    case "listening":
      return (
        <div className="bars" aria-hidden>
          <span /><span /><span /><span />
        </div>
      );
    case "processing":
    case "injecting":
    case "rate_limited":
      return <div className="spinner" aria-hidden />;
    case "done":
      return <span className="glyph">✓</span>;
    case "error":
      return <span className="glyph">!</span>;
    case "too_short":
    case "silent":
      return <span className="glyph">!</span>;
    default:
      return hovered ? <MicIcon /> : null;
  }
}

function MicIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M12 2a3 3 0 0 0-3 3v7a3 3 0 0 0 6 0V5a3 3 0 0 0-3-3Z" />
      <path d="M19 10v2a7 7 0 0 1-14 0v-2" />
      <line x1="12" x2="12" y1="19" y2="22" />
    </svg>
  );
}

function label(state) {
  switch (state) {
    case "listening": return "Listening";
    case "processing": return "Transcribing";
    case "injecting": return "Inserting";
    case "done": return "Done";
    case "error": return "Error";
    case "too_short": return "Too short — try again";
    case "silent": return "No audio — check mic";
    default: return "";
  }
}

function langDisplay(code) {
  if (!code || code === "auto") return "ALL";
  return code.toUpperCase();
}

