import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { applyTheme } from "../theme.js";

const MODES = [
  { value: "raw", label: "Raw", hint: "Fix obvious errors only. Keeps every word." },
  { value: "clean", label: "Clean", hint: "Remove fillers, fix punctuation. Default." },
  { value: "polished", label: "Polished", hint: "Rewrite for clarity. Preserves intent." },
];

const THEMES = [
  { value: "dark", label: "Dark" },
  { value: "light", label: "Light" },
  { value: "system", label: "System" },
];

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

export default function SettingsView({ config, updateConfig, autostart, onAutostartChange }) {
  const [draftKey, setDraftKey] = useState(config.groq_api_key || "");
  const [keyState, setKeyState] = useState("idle");
  const [keyError, setKeyError] = useState("");
  const [recordingHotkeyFor, setRecordingHotkeyFor] = useState(null);
  const [updateState, setUpdateState] = useState({ state: "idle", message: "" });

  useEffect(() => {
    if (!recordingHotkeyFor) return;
    const handler = (e) => {
      if (e.key === "Escape") {
        e.stopImmediatePropagation();
        setRecordingHotkeyFor(null);
        return;
      }
      if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      const parts = [];
      if (e.ctrlKey) parts.push("Ctrl");
      if (e.shiftKey) parts.push("Shift");
      if (e.altKey) parts.push("Alt");
      if (e.metaKey) parts.push("Win");
      const k = domKeyToName(e.code);
      if (!k) { setRecordingHotkeyFor(null); return; }
      parts.push(k);
      const combo = parts.join("+");
      const field = recordingHotkeyFor === "polish" ? "polish_hotkey" : "hotkey";
      updateConfig({ ...config, [field]: combo });
      setRecordingHotkeyFor(null);
    };
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [recordingHotkeyFor, config, updateConfig]);

  const hasKey = config.groq_api_key && config.groq_api_key.trim().length > 0;
  const activeMode = MODES.find((m) => m.value === config.mode) || MODES[1];
  const activeTheme = config.theme || "dark";

  async function saveKey() {
    setKeyState("checking");
    setKeyError("");
    try {
      await invoke("validate_api_key", { apiKey: draftKey });
      await updateConfig({ ...config, groq_api_key: draftKey.trim() });
      setKeyState("valid");
    } catch (e) {
      setKeyState("invalid");
      setKeyError(String(e));
    }
  }

  async function checkUpdates() {
    setUpdateState({ state: "checking", message: "" });
    try {
      const result = await invoke("check_for_updates");
      if (result) setUpdateState({ state: "available", message: `v${result}` });
      else setUpdateState({ state: "uptodate", message: "You're on the latest version." });
    } catch (e) {
      setUpdateState({ state: "error", message: String(e) });
    }
  }

  return (
    <div className="page">
      <header className="page-header">
        <h1>Settings</h1>
      </header>

      <div className="settings-grid">

        <Card className="wide" title="Groq API key" sub={hasKey ? "Connected." : "Paste your key to get started."}>
          {!hasKey && (
            <p className="muted small" style={{ marginTop: -2 }}>
              <a
                href="https://console.groq.com/keys"
                onClick={(e) => { e.preventDefault(); openUrl("https://console.groq.com/keys"); }}
              >
                Grab a free key from console.groq.com →
              </a>
            </p>
          )}
          <div className="row">
            <input
              type="password"
              value={draftKey}
              placeholder="gsk_…"
              onChange={(e) => { setDraftKey(e.target.value); setKeyState("idle"); }}
              spellCheck={false}
              autoComplete="off"
            />
            <button
              className="primary"
              disabled={!draftKey.trim() || keyState === "checking"}
              onClick={saveKey}
            >
              {keyState === "checking" ? "Checking…" : hasKey ? "Update" : "Save"}
            </button>
          </div>
          {keyState === "valid" && <p className="ok">Key validated and saved.</p>}
          {keyState === "invalid" && <p className="err">{keyError}</p>}
        </Card>

        <Card className="wide" title="Hotkeys" sub="Two combos. Dictation is hold-to-talk; Polish is single-press.">
          <HotkeyRow
            label="Dictation"
            hint="Hold to record. Release to transcribe and insert."
            combo={config.hotkey}
            isRecording={recordingHotkeyFor === "dictation"}
            onStart={() => setRecordingHotkeyFor("dictation")}
            onCancel={() => setRecordingHotkeyFor(null)}
          />
          <HotkeyRow
            label="Polish selection"
            hint="Select text anywhere, tap this combo — Bulbul rewrites it in place."
            combo={config.polish_hotkey || "Ctrl+Shift+P"}
            isRecording={recordingHotkeyFor === "polish"}
            onStart={() => setRecordingHotkeyFor("polish")}
            onCancel={() => setRecordingHotkeyFor(null)}
          />
        </Card>

        <Card title="Cleanup mode" sub={activeMode.hint}>
          <select
            className="select-input"
            value={config.mode || "clean"}
            onChange={(e) => updateConfig({ ...config, mode: e.target.value })}
          >
            {MODES.map((m) => (
              <option key={m.value} value={m.value}>{m.label}</option>
            ))}
          </select>
        </Card>

        <Card title="Appearance" sub="Light, dark, or follow Windows.">
          <div className="segmented">
            {THEMES.map((t) => (
              <button
                key={t.value}
                type="button"
                className={`segmented-btn ${activeTheme === t.value ? "selected" : ""}`}
                onClick={() => {
                  applyTheme(t.value);
                  updateConfig({ ...config, theme: t.value });
                }}
              >
                {t.label}
              </button>
            ))}
          </div>
        </Card>

        <Card title="Language" sub="Auto-detect is fine; a specific pick is slightly faster.">
          <select
            className="select-input"
            value={config.language || "auto"}
            onChange={(e) => updateConfig({ ...config, language: e.target.value })}
          >
            {LANGUAGES.map((l) => (
              <option key={l.code} value={l.code}>{l.label}</option>
            ))}
          </select>
        </Card>

        <Card title="Personalization" sub="Adapt to how you write.">
          <Toggle
            label="Personalize cleanup from past dictations"
            hint="Show the model recent examples from the same app. Adds ~150 tokens per dictation."
            checked={config.personalize_cleanup !== false}
            onChange={(v) => updateConfig({ ...config, personalize_cleanup: v })}
          />
          <Toggle
            label="Learn from my corrections"
            hint="When you edit what Bulbul typed, it remembers and applies the same fix next time. Password fields are always skipped."
            checked={config.learn_corrections !== false}
            onChange={(v) => updateConfig({ ...config, learn_corrections: v })}
          />
        </Card>

        <Card title="Startup" sub="What happens when Windows boots.">
          <Toggle
            label="Start Bulbul with Windows"
            hint="Boots silently in the tray on login."
            checked={autostart}
            onChange={onAutostartChange}
          />
          <Toggle
            label="Open this window when Bulbul starts"
            hint="Off = land straight in the tray."
            checked={!!config.open_dashboard_on_launch}
            onChange={(v) => updateConfig({ ...config, open_dashboard_on_launch: v })}
          />
        </Card>

        <Card title="Updates" sub="Bulbul checks GitHub releases.">
          <div className="row">
            <button onClick={checkUpdates} disabled={updateState.state === "checking"}>
              {updateState.state === "checking" ? "Checking…" : "Check for updates"}
            </button>
          </div>
          {updateState.state === "available" && (
            <p className="ok">Update available: {updateState.message}</p>
          )}
          {updateState.state === "uptodate" && (
            <p className="muted small">{updateState.message}</p>
          )}
          {updateState.state === "error" && (
            <p className="err">{updateState.message}</p>
          )}
        </Card>

        <Card title="Setup" sub="Re-do the first-run flow.">
          <div className="row">
            <button onClick={() => updateConfig({ ...config, onboarding_completed: false })}>
              Re-run setup wizard
            </button>
          </div>
        </Card>

      </div>
    </div>
  );
}

function Card({ title, sub, className = "", children }) {
  return (
    <div className={`settings-card ${className}`}>
      <div className="settings-card-head">
        <div className="settings-card-title">{title}</div>
        {sub && <div className="settings-card-sub">{sub}</div>}
      </div>
      <div className="settings-card-body">{children}</div>
    </div>
  );
}

function HotkeyRow({ label, hint, combo, isRecording, onStart, onCancel }) {
  return (
    <div className="hotkey-block">
      <div className="hotkey-meta">
        <div className="hotkey-label">{label}</div>
        {hint && <div className="hotkey-hint">{hint}</div>}
      </div>
      <div className="row hotkey-row">
        <div className="hotkey-display">
          {isRecording
            ? <span className="muted">Press a key combo… (Esc to cancel)</span>
            : formatHotkey(combo).map((part, i) => (
                <span key={i}>
                  {i > 0 && <span className="plus">+</span>}
                  <kbd>{part}</kbd>
                </span>
              ))}
        </div>
        <button onClick={isRecording ? onCancel : onStart}>
          {isRecording ? "Cancel" : "Change"}
        </button>
      </div>
    </div>
  );
}

function Toggle({ label, hint, checked, onChange }) {
  return (
    <label className="toggle-row">
      <div className="toggle-text">
        <div className="toggle-label">{label}</div>
        {hint && <div className="toggle-hint">{hint}</div>}
      </div>
      <span className={`toggle ${checked ? "on" : ""}`}>
        <input
          type="checkbox"
          checked={checked}
          onChange={(e) => onChange(e.target.checked)}
        />
        <span className="toggle-thumb" />
      </span>
    </label>
  );
}

function formatHotkey(s) {
  return (s || "").split("+").map((p) => p.trim()).filter(Boolean);
}

function domKeyToName(code) {
  if (!code) return null;
  if (code.startsWith("Key")) return code.slice(3);
  if (code.startsWith("Digit")) return code.slice(5);
  if (code === "Space") return "Space";
  if (/^F\d+$/.test(code)) return code;
  if (code === "Enter") return "Enter";
  if (code === "Backspace") return "Backspace";
  if (code === "Tab") return "Tab";
  return null;
}
