import { useEffect, useRef, useState } from "react";
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
import ScratchpadView from "./views/ScratchpadView.jsx";
import bulbulMark from "./assets/bulbul-mark.png";
import OnboardingWizard from "./onboarding/OnboardingWizard.jsx";
import TooltipProvider from "./components/TooltipProvider.jsx";
import { applyTheme } from "./theme.js";
import { RELAUNCH_HINT, IS_ANDROID } from "./platform.js";
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
  { id: "transforms", label: "Transforms", working: true },
  { id: "style", label: "Style", working: true },
  { id: "scratchpad", label: "Scratchpad", working: true },
  { id: "settings", label: "Settings", working: true },
];

function App() {
  const [section, setSection] = useState("home");
  const [moreOpen, setMoreOpen] = useState(false); // mobile "More" sheet
  // Settings now lives as a modal popup over the rest of the app, not
  // a routed page. The sidebar's Settings button toggles this; the
  // modal owns its own internal category sidebar + content pane.
  const [settingsOpen, setSettingsOpen] = useState(false);
  // Which settings section is drilled into on Android (null = the section
  // list). Lifted here so the hardware-back handler can treat it as its own
  // navigation level.
  const [settingsSection, setSettingsSection] = useState(null);
  const [config, setConfig] = useState(null);
  const [showPrivacy, setShowPrivacy] = useState(false);
  const [status, setStatus] = useState({ state: "idle" });
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [autostart, setAutostart] = useState(false);
  const [stagedUpdate, setStagedUpdate] = useState(null);
  const [installing, setInstalling] = useState(false);
  const [systemDark, setSystemDark] = useState(
    () => window.matchMedia("(prefers-color-scheme: dark)").matches
  );

  useEffect(() => {
    const m = window.matchMedia("(prefers-color-scheme: dark)");
    const h = (e) => setSystemDark(e.matches);
    if (m.addEventListener) m.addEventListener("change", h);
    else m.addListener(h);
    return () => {
      if (m.removeEventListener) m.removeEventListener("change", h);
      else m.removeListener(h);
    };
  }, []);

  // Android hardware/gesture back. Without this, Back on any open overlay
  // falls straight through to the OS and exits the whole app. We model the
  // open overlays as a stack with a "depth" and keep the browser history depth
  // in sync, so each Back press pops exactly one level — e.g. Settings ▸
  // General → Back → Settings list → Back → dashboard → Back → exit.
  //
  //   0 = dashboard   1 = More sheet OR Settings list   2 = Settings ▸ section
  const overlayDepth = moreOpen
    ? 1
    : settingsOpen
    ? (settingsSection ? 2 : 1)
    : 0;
  const depthRef = useRef(0);
  const ignorePopRef = useRef(false);

  // Keep browser history depth == logical overlay depth. Opening a level pushes
  // an entry; closing one via an in-app control pops it (guarded so its
  // popstate doesn't double-close).
  useEffect(() => {
    if (!IS_ANDROID) return;
    const prev = depthRef.current;
    if (overlayDepth > prev) {
      for (let i = prev; i < overlayDepth; i++) window.history.pushState({ d: i + 1 }, "");
    } else if (overlayDepth < prev) {
      ignorePopRef.current = true;
      window.history.go(-(prev - overlayDepth));
    }
    depthRef.current = overlayDepth;
  }, [overlayDepth]);

  // Hardware Back → close just the top level.
  useEffect(() => {
    if (!IS_ANDROID) return;
    const onPop = () => {
      if (ignorePopRef.current) {
        ignorePopRef.current = false;
        return;
      }
      // The browser already consumed one history entry; pre-decrement so the
      // depth-sync effect above sees a balanced stack and doesn't re-push.
      depthRef.current = Math.max(0, depthRef.current - 1);
      if (settingsOpen && settingsSection) setSettingsSection(null);
      else if (settingsOpen) setSettingsOpen(false);
      else if (moreOpen) setMoreOpen(false);
    };
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, [settingsOpen, settingsSection, moreOpen]);

  useEffect(() => {
    invoke("get_config").then((cfg) => {
      setConfig(cfg);
      if (!cfg.privacy_acknowledged) setShowPrivacy(true);
      if (!cfg.has_api_key && !cfg.groq_api_key) setSettingsOpen(true);
    });
    invoke("get_autostart").then(setAutostart).catch(() => {});
    // Mode-B auto-update: the Rust watcher emits this event after it
    // downloads a new installer. If the user reopens the app between
    // checks, the version is still in the slot — fetch it on mount.
    invoke("get_staged_update_version").then(setStagedUpdate).catch(() => {});
    const unStaged = listen("update-staged", (e) => setStagedUpdate(e.payload));
    const un = listen("bulbul-status", (e) => setStatus(e.payload));
    const onKey = (e) => {
      // Escape-to-hide is desktop-only — Android handles app dismissal
      // via the system back button and home gesture, and there's no
      // physical Escape key on a phone keyboard worth wiring.
      if (e.key === "Escape" && !IS_ANDROID) getCurrentWindow().hide().catch(() => {});
    };
    window.addEventListener("keydown", onKey);
    return () => {
      un.then((f) => f());
      unStaged.then((f) => f());
      window.removeEventListener("keydown", onKey);
    };
  }, []);

  async function installUpdate() {
    setInstalling(true);
    try {
      // The Rust command returns only on failure — on success the
      // installer kills this process mid-call.
      await invoke("install_staged_update");
    } catch (e) {
      console.error("install_staged_update failed:", e);
      setInstalling(false);
    }
  }

  async function updateConfig(next) {
    await invoke("save_config", { newCfg: next });
    setConfig(next);
  }

  async function toggleAutostart(next) {
    try {
      await invoke("set_autostart", { enabled: next });
      setAutostart(next);
    } catch (e) {
      console.error("autostart toggle failed", e);
    }
  }

  // Show/hide the system-tray icon. The Rust side persists hide_tray
  // into the config file and flips the live tray visibility in one
  // call, so we mirror the value into local state on success.
  async function toggleHideTray(hide) {
    try {
      await invoke("set_tray_visible", { visible: !hide });
      setConfig((prev) => ({ ...prev, hide_tray: hide }));
    } catch (e) {
      console.error("hide-tray toggle failed", e);
    }
  }

  function setThemePref(pref) {
    applyTheme(pref); // instant, before the async save round-trips
    updateConfig({ ...config, theme: pref });
  }

  async function ackPrivacy() {
    await updateConfig({ ...config, privacy_acknowledged: true });
    setShowPrivacy(false);
  }

  if (!config) return <div className="loading">Loading…</div>;

  const themePref = config.theme || "light";
  const resolvedTheme =
    themePref === "system" ? (systemDark ? "dark" : "light") : themePref === "dark" ? "dark" : "light";

  if (!config.onboarding_completed) {
    return (
      <>
        <OnboardingWizard
          config={config}
          updateConfig={updateConfig}
          onComplete={() => setConfig({ ...config, onboarding_completed: true })}
        />
        <TooltipProvider />
      </>
    );
  }

  const mainView = (
    <>
      {section === "home" && <HomeView displayName={config.display_name} />}
      {section === "dictionary" && <DictionaryView />}
      {section === "insights" && <InsightsView />}
      {section === "snippets" && <SnippetsView />}
      {section === "transforms" && <TransformsView />}
      {section === "style" && <StyleView config={config} updateConfig={updateConfig} />}
      {section === "scratchpad" && <ScratchpadView />}
      {!["home", "dictionary", "insights", "snippets", "transforms", "style", "scratchpad"].includes(section) && <ComingSoon id={section} />}
    </>
  );

  // ── Mobile shell ──────────────────────────────────────────────
  // Phones get their own idiom instead of a squeezed desktop: a slim
  // app bar, content, a 4-slot bottom tab bar (the rest of the nav
  // lives in a "More" bottom sheet so tabs stay thumb-sized).
  if (IS_ANDROID) {
    // Bottom tab bar, in this explicit order (Scratchpad sits where Dictionary
    // used to be); everything else lives in the "More" hamburger sheet.
    const PRIMARY_IDS = ["home", "insights", "scratchpad", "transforms"];
    const primary = PRIMARY_IDS.map((id) => SECTIONS.find((s) => s.id === id)).filter(Boolean);
    const secondary = SECTIONS.filter((s) => !PRIMARY_IDS.includes(s.id));
    return (
      <>
      <div className="app-shell m-shell">
        {showPrivacy && <PrivacyModal onAck={ackPrivacy} />}
        <header className="m-appbar">
          <button
            className="m-icon-btn"
            aria-label="Menu"
            onClick={() => setMoreOpen(true)}
          >
            <svg width="27" height="27" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" aria-hidden>
              <line x1="4" y1="7" x2="20" y2="7" />
              <line x1="4" y1="12" x2="20" y2="12" />
              <line x1="4" y1="17" x2="20" y2="17" />
            </svg>
          </button>
          <div className="m-appbar-brand">
            <img src={bulbulMark} alt="" className="m-brand-mark" aria-hidden />
            <span className="m-brand-text">bulbul</span>
          </div>
          <button
            className="m-icon-btn"
            aria-label={resolvedTheme === "dark" ? "Switch to light mode" : "Switch to dark mode"}
            title={resolvedTheme === "dark" ? "Light mode" : "Dark mode"}
            onClick={() => setThemePref(resolvedTheme === "dark" ? "light" : "dark")}
          >
            {resolvedTheme === "dark" ? (
              <svg width="23" height="23" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.9" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
                <circle cx="12" cy="12" r="4" />
                <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" />
              </svg>
            ) : (
              <svg width="23" height="23" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.9" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
                <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
              </svg>
            )}
          </button>
        </header>
        <main className="content m-content">{mainView}</main>

        <nav className="m-tabbar">
          {primary.map((s) => (
            <button
              key={s.id}
              className={`m-tab ${section === s.id && !moreOpen ? "active" : ""}`}
              onClick={() => { setSection(s.id); setMoreOpen(false); }}
            >
              <span className="m-tab-icon">{ICONS[s.id]}</span>
              <span className="m-tab-label">{s.label}</span>
            </button>
          ))}
        </nav>

        {moreOpen && (
          <div className="m-sheet-overlay" onClick={() => setMoreOpen(false)}>
            <div className="m-sheet" onClick={(e) => e.stopPropagation()}>
              <div className="m-sheet-grab" />
              {secondary.map((s) => {
                const isSettings = s.id === "settings";
                return (
                  <button
                    key={s.id}
                    className={`m-sheet-item ${(!isSettings && section === s.id) ? "active" : ""}`}
                    onClick={() => {
                      if (isSettings) { setSettingsSection(null); setSettingsOpen(true); }
                      else setSection(s.id);
                      setMoreOpen(false);
                    }}
                  >
                    <span className="m-sheet-icon">{ICONS[s.id]}</span>
                    <span>{s.label}</span>
                  </button>
                );
              })}
              <div className="m-sheet-foot">
                <span className="m-sheet-brand">
                  <img src={bulbulMark} alt="" className="m-sheet-brand-mark" aria-hidden />
                  <span className="m-sheet-brand-text">bulbul</span>
                </span>
                <span className="muted small">v1.0.1 · MIT</span>
              </div>
            </div>
          </div>
        )}
      </div>
      <SettingsView
        open={settingsOpen}
        onClose={() => setSettingsOpen(false)}
        config={config}
        updateConfig={updateConfig}
        autostart={autostart}
        onAutostartChange={toggleAutostart}
        onHideTrayChange={toggleHideTray}
        section={settingsSection}
        onSectionChange={setSettingsSection}
      />
      <TooltipProvider />
      </>
    );
  }

  return (
    <>
    <div className={`app-shell ${sidebarOpen ? "" : "sidebar-collapsed"}`}>
      {!IS_ANDROID && (
        <TitleBar
          sidebarOpen={sidebarOpen}
          onToggleSidebar={() => setSidebarOpen((v) => !v)}
          resolvedTheme={resolvedTheme}
          onToggleTheme={() => setThemePref(resolvedTheme === "dark" ? "light" : "dark")}
        />
      )}
      {showPrivacy && <PrivacyModal onAck={ackPrivacy} />}

      <aside className="sidebar">
        <div className="brand">
          <img src={bulbulMark} alt="" className="brand-mark" aria-hidden />
          <div className="brand-text">bulbul</div>
        </div>
        <nav className="nav">
          {SECTIONS.map((s) => {
            // Settings is a modal popup, not a routed page — clicking it
            // opens the overlay instead of swapping the main content.
            const isSettings = s.id === "settings";
            const isActive = isSettings ? settingsOpen : section === s.id;
            return (
            <button
              key={s.id}
              className={`nav-item ${isActive ? "active" : ""} ${s.working ? "" : "pending"}`}
              onClick={() => (isSettings ? setSettingsOpen(true) : setSection(s.id))}
            >
              <span className="nav-icon">{ICONS[s.id]}</span>
              <span className="nav-label">{s.label}</span>
              {!s.working && <span className="nav-tag">soon</span>}
            </button>
            );
          })}
        </nav>
        <div className="sidebar-footer">
          {!IS_ANDROID && (
            <>
              <label
                className="sidebar-toggle-row"
                title="When on, the dashboard pops up at startup. When off, Bulbul boots silently to the tray — the pill still appears when you dictate."
              >
                <span className="sidebar-toggle-label">Open at startup</span>
                <span className={`toggle ${config.open_dashboard_on_launch ? "on" : ""}`}>
                  <input
                    type="checkbox"
                    checked={!!config.open_dashboard_on_launch}
                    onChange={(e) => updateConfig({ ...config, open_dashboard_on_launch: e.target.checked })}
                  />
                  <span className="toggle-thumb" />
                </span>
              </label>
              <label
                className="sidebar-toggle-row"
                title={`When on, the system-tray icon disappears. Bulbul keeps running in the background — the pill only appears while you're dictating. ${RELAUNCH_HINT}`}
              >
                <span className="sidebar-toggle-label">Hide tray icon</span>
                <span className={`toggle ${config.hide_tray ? "on" : ""}`}>
                  <input
                    type="checkbox"
                    checked={!!config.hide_tray}
                    onChange={(e) => toggleHideTray(e.target.checked)}
                  />
                  <span className="toggle-thumb" />
                </span>
              </label>
            </>
          )}
          <div className={`status status-${status.state}`}>
            <span className="dot" />
            <span>{statusLabel(status.state)}</span>
          </div>
          <div className="version muted small">v1.0.1 · MIT</div>
        </div>
      </aside>

      <main className="content">
        {stagedUpdate && (
          <div className="update-banner" role="status">
            <span className="update-banner-dot" aria-hidden />
            <span className="update-banner-text">
              <strong>Bulbul v{stagedUpdate}</strong> is ready — restart to install.
            </span>
            <button
              className="update-banner-btn"
              onClick={installUpdate}
              disabled={installing}
            >
              {installing ? "Installing…" : "Install & restart"}
            </button>
          </div>
        )}
        {mainView}
      </main>
    </div>
    <SettingsView
      open={settingsOpen}
      onClose={() => setSettingsOpen(false)}
      config={config}
      updateConfig={updateConfig}
      autostart={autostart}
      onAutostartChange={toggleAutostart}
      onHideTrayChange={toggleHideTray}
    />
    <TooltipProvider />
    </>
  );
}

function TitleBar({ sidebarOpen, onToggleSidebar, resolvedTheme, onToggleTheme }) {
  const [isMaximized, setIsMaximized] = useState(false);
  const win = getCurrentWindow();

  useEffect(() => {
    let mounted = true;
    win.isMaximized().then((m) => mounted && setIsMaximized(m)).catch(() => {});
    const un = win.onResized(() => {
      win.isMaximized().then((m) => mounted && setIsMaximized(m)).catch(() => {});
    });
    return () => {
      mounted = false;
      un.then((f) => f()).catch(() => {});
    };
  }, []);

  return (
    <div className="titlebar" data-tauri-drag-region>
      <div className="titlebar-left">
        <button
          className="tb-btn tb-sidebar"
          aria-label={sidebarOpen ? "Collapse sidebar" : "Expand sidebar"}
          title={sidebarOpen ? "Collapse sidebar" : "Expand sidebar"}
          onClick={onToggleSidebar}
        >
          <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden>
            <rect x="1.5" y="2.5" width="13" height="11" rx="1.6" fill="none" stroke="currentColor" strokeWidth="1.2" />
            <line x1="6" y1="3" x2="6" y2="13" stroke="currentColor" strokeWidth="1.2" />
          </svg>
        </button>
      </div>
      <div className="titlebar-spacer" data-tauri-drag-region />
      <div className="titlebar-controls">
        <button
          className="tb-btn"
          aria-label={resolvedTheme === "dark" ? "Switch to light theme" : "Switch to dark theme"}
          title={resolvedTheme === "dark" ? "Light theme" : "Dark theme"}
          onClick={onToggleTheme}
        >
          {resolvedTheme === "dark" ? (
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
              <circle cx="12" cy="12" r="4" />
              <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" />
            </svg>
          ) : (
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
              <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
            </svg>
          )}
        </button>
        <button
          className="tb-btn"
          aria-label="Minimize"
          title="Minimize"
          onClick={() => win.minimize().catch(() => {})}
        >
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
            <line x1="1.5" y1="5" x2="8.5" y2="5" stroke="currentColor" strokeWidth="1" strokeLinecap="round" />
          </svg>
        </button>
        <button
          className="tb-btn"
          aria-label={isMaximized ? "Restore" : "Maximize"}
          title={isMaximized ? "Restore" : "Maximize"}
          onClick={() => win.toggleMaximize().catch(() => {})}
        >
          {isMaximized ? (
            <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
              <rect x="2" y="3.2" width="5.5" height="5.5" fill="none" stroke="currentColor" strokeWidth="1" />
              <path d="M3.2 3.2 V1.5 H8.5 V6.8 H6.8" fill="none" stroke="currentColor" strokeWidth="1" />
            </svg>
          ) : (
            <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
              <rect x="1.5" y="1.5" width="7" height="7" fill="none" stroke="currentColor" strokeWidth="1" />
            </svg>
          )}
        </button>
        <button
          className="tb-btn tb-close"
          aria-label="Close"
          title="Close"
          onClick={() => win.close().catch(() => {})}
        >
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
            <line x1="1.5" y1="1.5" x2="8.5" y2="8.5" stroke="currentColor" strokeWidth="1" strokeLinecap="round" />
            <line x1="8.5" y1="1.5" x2="1.5" y2="8.5" stroke="currentColor" strokeWidth="1" strokeLinecap="round" />
          </svg>
        </button>
      </div>
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
          transcription and cleanup, using <em>your</em> API key. Your transcripts,
          audio, dictionary, and notes never leave your machine for any other purpose.
        </p>
        <p className="muted small">
          Anonymous usage stats (counts, durations, error categories) are on by default
          so I can see what works and what breaks. They never include your transcripts,
          audio, or which app you're typing into. Flip them off in Settings → Privacy
          if you'd rather not share.
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
