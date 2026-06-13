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

export default function Overlay() {
  const [status, setStatus] = useState({ state: "idle", message: null });
  const [hovered, setHovered] = useState(false);
  const [lang, setLang] = useState("auto");
  const [langOpen, setLangOpen] = useState(false);
  // Bulbul's pipeline emits state="idle" with a message string for the
  // two silent-rejection paths ("Too short, ignored." and "Silence —
  // nothing to transcribe."). Without inflating those into a distinct
  // transient state, the overlay swallows them — the pill just shrinks
  // back to its dot with no clue why nothing was typed. Open-source
  // peers (Handy, FreeFlow, Mesmer) all silently fail this same way;
  // we surface a brief amber pill instead so users can self-diagnose.
  const [transientReject, setTransientReject] = useState(null);

  useEffect(() => {
    document.body.style.background = "transparent";
    document.documentElement.style.background = "transparent";

    invoke("get_config").then((cfg) => setLang(cfg.language || "auto")).catch(() => {});

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
    return () => { un1.then((f) => f()); un2.then((f) => f()); };
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

  const showSatellites = hovered && effectiveState === "idle";
  const expanded = showSatellites || effectiveState !== "idle";

  return (
    <div className={`overlay ${expanded ? "expanded" : "collapsed"} ${hovered ? "hovered" : ""}`}>
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

        <div className={`pill pill-${effectiveState}`}>
          <span className="pill-icon">{renderIcon(effectiveState, hovered)}</span>
          {expanded && effectiveState !== "idle" && (
            <span className="pill-label">
              {effectiveState === "rate_limited"
                ? (effectiveMessage || "Rate limited…")
                : label(effectiveState)}
            </span>
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
      </div>
    </div>
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
      return hovered ? <MicIcon /> : <span className="dot" aria-hidden />;
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

