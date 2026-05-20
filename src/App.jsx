import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import HomeView from "./views/HomeView.jsx";
import SettingsView from "./views/SettingsView.jsx";
import DictionaryView from "./views/DictionaryView.jsx";
import InsightsView from "./views/InsightsView.jsx";
import SnippetsView from "./views/SnippetsView.jsx";
import TransformsView from "./views/TransformsView.jsx";
import StyleView from "./views/StyleView.jsx";
import "./App.css";

const ICONS = {
  home: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="m3 9 9-7 9 7v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
      <path d="M9 22V12h6v10" />
    </svg>
  ),
  insights: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <line x1="6" x2="6" y1="20" y2="10" />
      <line x1="12" x2="12" y1="20" y2="4" />
      <line x1="18" x2="18" y1="20" y2="14" />
    </svg>
  ),
  dictionary: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M4 19.5v-15A2.5 2.5 0 0 1 6.5 2H20v20H6.5a2.5 2.5 0 0 1 0-5H20" />
    </svg>
  ),
  snippets: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <circle cx="6" cy="6" r="3" />
      <path d="M8.12 8.12 12 12" />
      <path d="M20 4 8.12 15.88" />
      <circle cx="6" cy="18" r="3" />
      <path d="M14.8 14.8 20 20" />
    </svg>
  ),
  transforms: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M15 4V2" />
      <path d="M15 16v-2" />
      <path d="M8 9h2" />
      <path d="M20 9h2" />
      <path d="M17.8 11.8 19 13" />
      <path d="M17.8 6.2 19 5" />
      <path d="m3 21 9-9" />
      <path d="M12.2 6.2 11 5" />
    </svg>
  ),
  style: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M4 7V4h16v3" />
      <path d="M9 20h6" />
      <path d="M12 4v16" />
    </svg>
  ),
  scratchpad: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M14 4H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-8" />
      <path d="m18 2 4 4-10 10H8v-4z" />
    </svg>
  ),
  settings: (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M12.22 2h-.44a2 2 0 0 0-2 2v.18a2 2 0 0 1-1 1.73l-.43.25a2 2 0 0 1-2 0l-.15-.08a2 2 0 0 0-2.73.73l-.22.38a2 2 0 0 0 .73 2.73l.15.1a2 2 0 0 1 1 1.72v.51a2 2 0 0 1-1 1.74l-.15.09a2 2 0 0 0-.73 2.73l.22.38a2 2 0 0 0 2.73.73l.15-.08a2 2 0 0 1 2 0l.43.25a2 2 0 0 1 1 1.73V20a2 2 0 0 0 2 2h.44a2 2 0 0 0 2-2v-.18a2 2 0 0 1 1-1.73l.43-.25a2 2 0 0 1 2 0l.15.08a2 2 0 0 0 2.73-.73l.22-.39a2 2 0 0 0-.73-2.73l-.15-.08a2 2 0 0 1-1-1.74v-.5a2 2 0 0 1 1-1.74l.15-.09a2 2 0 0 0 .73-2.73l-.22-.38a2 2 0 0 0-2.73-.73l-.15.08a2 2 0 0 1-2 0l-.43-.25a2 2 0 0 1-1-1.73V4a2 2 0 0 0-2-2z" />
      <circle cx="12" cy="12" r="3" />
    </svg>
  ),
};

const SECTIONS = [
  { id: "home", label: "Home", working: true },
  { id: "insights", label: "Insights", working: true },
  { id: "dictionary", label: "Dictionary", working: true },
  { id: "snippets", label: "Snippets", working: true },
  { id: "transforms", label: "Transforms", working: false },
  { id: "style", label: "Style", working: true },
  { id: "scratchpad", label: "Scratchpad", working: false },
  { id: "settings", label: "Settings", working: true },
];

function App() {
  const [section, setSection] = useState("home");
  const [config, setConfig] = useState(null);
  const [showPrivacy, setShowPrivacy] = useState(false);
  const [status, setStatus] = useState({ state: "idle" });

  useEffect(() => {
    invoke("get_config").then((cfg) => {
      setConfig(cfg);
      if (!cfg.privacy_acknowledged) setShowPrivacy(true);
      if (!cfg.has_api_key && !cfg.groq_api_key) setSection("settings");
    });
    const un = listen("bulbul-status", (e) => setStatus(e.payload));
    const onKey = (e) => {
      if (e.key === "Escape") getCurrentWindow().hide();
    };
    window.addEventListener("keydown", onKey);
    return () => {
      un.then((f) => f());
      window.removeEventListener("keydown", onKey);
    };
  }, []);

  async function updateConfig(next) {
    await invoke("save_config", { newCfg: next });
    setConfig(next);
  }

  async function ackPrivacy() {
    await updateConfig({ ...config, privacy_acknowledged: true });
    setShowPrivacy(false);
  }

  if (!config) return <div className="loading">Loading…</div>;

  return (
    <div className="app-shell">
      {showPrivacy && <PrivacyModal onAck={ackPrivacy} />}

      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark" aria-hidden />
          <div className="brand-text">Bulbul</div>
        </div>
        <nav className="nav">
          {SECTIONS.map((s) => (
            <button
              key={s.id}
              className={`nav-item ${section === s.id ? "active" : ""} ${s.working ? "" : "pending"}`}
              onClick={() => setSection(s.id)}
              title={s.working ? s.label : `${s.label} (coming soon)`}
            >
              <span className="nav-icon">{ICONS[s.id]}</span>
              <span className="nav-label">{s.label}</span>
              {!s.working && <span className="nav-tag">soon</span>}
            </button>
          ))}
        </nav>
        <div className="sidebar-footer">
          <div className={`status status-${status.state}`}>
            <span className="dot" />
            <span>{statusLabel(status.state)}</span>
          </div>
          <div className="version muted small">v0.1.0 · MIT</div>
        </div>
      </aside>

      <main className="content">
        {section === "home" && <HomeView />}
        {section === "settings" && (
          <SettingsView config={config} updateConfig={updateConfig} />
        )}
        {section === "dictionary" && <DictionaryView />}
        {section === "insights" && <InsightsView />}
        {section === "snippets" && <SnippetsView />}
        {section === "transforms" && <TransformsView />}
        {section === "style" && <StyleView config={config} updateConfig={updateConfig} />}
        {!["home", "settings", "dictionary", "insights", "snippets", "transforms", "style"].includes(section) && <ComingSoon id={section} />}
      </main>
    </div>
  );
}

function PrivacyModal({ onAck }) {
  return (
    <div className="modal-overlay">
      <div className="modal">
        <h2>Before you start</h2>
        <p>
          Bulbul sends your spoken audio to <strong>Groq's servers</strong> for
          transcription and cleanup, using <em>your</em> API key. No data is sent
          anywhere else — no Bulbul server, no telemetry.
        </p>
        <p className="muted">
          Make sure you trust Groq's privacy policy before dictating sensitive content.
        </p>
        <button className="primary" onClick={onAck}>Got it</button>
      </div>
    </div>
  );
}

function ComingSoon({ id }) {
  const titles = {
    insights: "Insights",
    dictionary: "Dictionary",
    snippets: "Snippets",
    transforms: "Transforms",
    style: "Style",
    scratchpad: "Scratchpad",
  };
  const blurbs = {
    insights: "Usage stats, your voice profile, top corrections — coming as part of the V2 build.",
    dictionary: "Manage word substitutions Bulbul applies before injection.",
    snippets: "Saved phrases that expand on trigger (e.g. \"my email\" → real email).",
    transforms: "Multiple polish prompts — Polish, Make Formal, Bulletize, and your own.",
    style: "Your voice profile: most-used words, peak hours, catchphrases.",
    scratchpad: "Standalone notes window with Transforms applied to selections.",
  };
  return (
    <div className="page coming-soon">
      <header className="page-header">
        <h1>{titles[id] || id}</h1>
        <p className="muted small">{blurbs[id]}</p>
      </header>
      <div className="empty-state">
        <p className="muted">Coming soon.</p>
      </div>
    </div>
  );
}

function statusLabel(state) {
  switch (state) {
    case "listening": return "Listening";
    case "processing": return "Processing";
    case "injecting": return "Injecting";
    case "done": return "Done";
    case "error": return "Error";
    default: return "Idle";
  }
}

export default App;
