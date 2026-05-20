import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl } from "@tauri-apps/plugin-opener";
import "./App.css";

const MODES = [
  { value: "raw", label: "Raw", hint: "Fix obvious errors only. Keeps every word." },
  { value: "clean", label: "Clean", hint: "Remove fillers, fix punctuation. Default." },
  { value: "polished", label: "Polished", hint: "Rewrite for clarity. Preserves intent." },
];

function App() {
  const [config, setConfig] = useState(null);
  const [draftKey, setDraftKey] = useState("");
  const [keyState, setKeyState] = useState("idle");
  const [keyError, setKeyError] = useState("");
  const [status, setStatus] = useState({ state: "idle", message: null });
  const [showPrivacy, setShowPrivacy] = useState(false);
  const [recordingHotkey, setRecordingHotkey] = useState(false);
  const [updateState, setUpdateState] = useState({ state: "idle", message: "" });

  useEffect(() => {
    invoke("get_config").then((cfg) => {
      setConfig(cfg);
      setDraftKey(cfg.groq_api_key || "");
      if (!cfg.privacy_acknowledged) setShowPrivacy(true);
    });
    const un = listen("bulbul-status", (e) => setStatus(e.payload));
    const onKey = (e) => {
      if (e.key === "Escape" && !recordingHotkey) getCurrentWindow().hide();
    };
    window.addEventListener("keydown", onKey);
    return () => {
      un.then((f) => f());
      window.removeEventListener("keydown", onKey);
    };
  }, []);

  useEffect(() => {
    if (!recordingHotkey || !config) return;
    const handler = (e) => {
      if (e.key === "Escape") { setRecordingHotkey(false); return; }
      if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return;
      e.preventDefault();
      e.stopPropagation();
      const parts = [];
      if (e.ctrlKey) parts.push("Ctrl");
      if (e.shiftKey) parts.push("Shift");
      if (e.altKey) parts.push("Alt");
      if (e.metaKey) parts.push("Win");
      const k = domKeyToName(e.code);
      if (!k) { setRecordingHotkey(false); return; }
      parts.push(k);
      const combo = parts.join("+");
      saveConfig({ ...config, hotkey: combo });
      setRecordingHotkey(false);
    };
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [recordingHotkey, config]);

  if (!config) return <main className="empty">Loading…</main>;

  const hasKey = config.groq_api_key && config.groq_api_key.trim().length > 0;

  async function saveConfig(next) {
    await invoke("save_config", { newCfg: next });
    setConfig(next);
  }

  async function saveKey() {
    setKeyState("checking");
    setKeyError("");
    try {
      await invoke("validate_api_key", { apiKey: draftKey });
      await saveConfig({ ...config, groq_api_key: draftKey.trim() });
      setKeyState("valid");
    } catch (e) {
      setKeyState("invalid");
      setKeyError(String(e));
    }
  }

  async function ackPrivacy() {
    await saveConfig({ ...config, privacy_acknowledged: true });
    setShowPrivacy(false);
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
    <main className="app">
      {showPrivacy && (
        <div className="modal-overlay">
          <div className="modal">
            <h2>Before you start</h2>
            <p>
              Bulbul sends your spoken audio to <strong>Groq's servers</strong> for
              transcription and cleanup, using <em>your</em> API key. No data is sent
              anywhere else — no Bulbul server, no telemetry.
            </p>
            <p className="muted">
              Make sure you trust Groq's privacy policy before dictating sensitive
              content.
            </p>
            <button className="primary" onClick={ackPrivacy}>Got it</button>
          </div>
        </div>
      )}

      <header>
        <div className="title">Bulbul</div>
        <div className={`status status-${status.state}`}>
          <span className="dot" />
          <span>{statusLabel(status.state)}</span>
        </div>
      </header>

      <section>
        <h3>Groq API key</h3>
        {!hasKey && (
          <p className="muted">
            Paste your Groq API key to get started.{" "}
            <a
              href="https://console.groq.com/keys"
              onClick={(e) => { e.preventDefault(); openUrl("https://console.groq.com/keys"); }}
            >
              Get one here →
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
      </section>

      <section>
        <h3>Cleanup mode</h3>
        <div className="modes">
          {MODES.map((m) => (
            <label
              key={m.value}
              className={`mode ${config.mode === m.value ? "selected" : ""}`}
            >
              <input
                type="radio"
                name="mode"
                value={m.value}
                checked={config.mode === m.value}
                onChange={() => saveConfig({ ...config, mode: m.value })}
              />
              <div>
                <div className="mode-label">{m.label}</div>
                <div className="mode-hint">{m.hint}</div>
              </div>
            </label>
          ))}
        </div>
      </section>

      <section>
        <h3>Hotkey</h3>
        <div className="row hotkey-row">
          <div className="hotkey-display">
            {recordingHotkey
              ? <span className="muted">Press a key combo… (Esc to cancel)</span>
              : formatHotkey(config.hotkey).map((part, i) => (
                  <span key={i}>
                    {i > 0 && <span className="plus">+</span>}
                    <kbd>{part}</kbd>
                  </span>
                ))}
          </div>
          <button onClick={() => setRecordingHotkey((v) => !v)}>
            {recordingHotkey ? "Cancel" : "Change"}
          </button>
        </div>
        <p className="muted small">
          Hold the combo anywhere in Windows to dictate. Modifiers + letter / number / function-key are supported.
        </p>
      </section>

      <section>
        <h3>Updates</h3>
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
      </section>

      <footer>
        <span className="muted small">v0.1.0 · MIT · Press Esc to hide window</span>
      </footer>
    </main>
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

export default App;
