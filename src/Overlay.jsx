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

  useEffect(() => {
    document.body.style.background = "transparent";
    document.documentElement.style.background = "transparent";

    invoke("get_config").then((cfg) => setLang(cfg.language || "auto")).catch(() => {});

    const un1 = listen("bulbul-status", (e) => setStatus(e.payload));
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

  // Close dropdown when cursor leaves or a dictation starts.
  useEffect(() => {
    if (!hovered || status.state !== "idle") {
      if (langOpen) setLangOpen(false);
    }
  }, [hovered, status.state]);

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

  const showSatellites = hovered && status.state === "idle";
  const expanded = showSatellites || status.state !== "idle";

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

        <div className={`pill pill-${status.state}`}>
          <span className="pill-icon">{renderIcon(status.state, hovered)}</span>
          {expanded && status.state !== "idle" && (
            <span className="pill-label">{label(status.state)}</span>
          )}
        </div>

        {showSatellites && (
          <button
            className="sat polish-btn"
            title="Polish selected text"
            onClick={() => invoke("polish_now").catch(() => {})}
          >
            <WandIcon />
          </button>
        )}
      </div>
    </div>
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
      return <div className="spinner" aria-hidden />;
    case "done":
      return <span className="glyph">✓</span>;
    case "error":
      return <span className="glyph">!</span>;
    default:
      return hovered
        ? (
          <div className="dots-row" aria-hidden>
            <span /><span /><span /><span /><span /><span /><span />
          </div>
        )
        : <span className="dot" aria-hidden />;
  }
}

function label(state) {
  switch (state) {
    case "listening": return "Listening";
    case "processing": return "Transcribing";
    case "injecting": return "Inserting";
    case "done": return "Done";
    case "error": return "Error";
    default: return "";
  }
}

function langDisplay(code) {
  if (!code || code === "auto") return "ALL";
  return code.toUpperCase();
}

function WandIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M15 4V2" />
      <path d="M15 16v-2" />
      <path d="M8 9h2" />
      <path d="M20 9h2" />
      <path d="M17.8 11.8 19 13" />
      <path d="M15 9h0" />
      <path d="M17.8 6.2 19 5" />
      <path d="m3 21 9-9" />
      <path d="M12.2 6.2 11 5" />
    </svg>
  );
}
