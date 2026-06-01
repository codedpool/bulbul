import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl } from "@tauri-apps/plugin-opener";
import bulbulMark from "../assets/bulbul-mark.png";
import { applyTheme } from "../theme.js";
import "./onboarding.css";

const HOTKEY_PRESETS = [
  {
    value: "Ctrl+Win",
    label: "Ctrl + Win",
    detail: "Hold both keys to dictate. commercial apps' default — instant muscle memory if you're switching over.",
  },
  {
    value: "Alt+Win",
    label: "Alt + Win",
    detail: "Same hold-to-talk feel, different fingers. Pick this if Ctrl + Win is taken by another app.",
  },
  {
    value: "Ctrl+Shift+Space",
    label: "Ctrl + Shift + Space",
    detail: "Three keys, but almost never clashes with anything. The safest pick.",
  },
  {
    value: "custom",
    label: "Custom combo…",
    detail: "Capture any combination you like.",
  },
];

const VIDEO_URL = "https://www.youtube.com/watch?v=9VDbhptCzlU";
const VIDEO_EMBED = "https://www.youtube-nocookie.com/embed/9VDbhptCzlU";

// Sample line deliberately seeded with "um", "uh", "like" so the cleanup
// pass visibly removes them — the user sees Bulbul not just transcribe but
// clean. If they're on Raw mode every filler stays, which is also useful
// feedback ("ah, that's what Raw mode means").
const SAMPLE_LINE = "Hi Bulbul, um, this is, uh, my first test — and like, it looks great.";

export default function OnboardingWizard({ config, updateConfig, onComplete }) {
  const [step, setStep] = useState(0);
  const totalSteps = 4;

  const themePref = config.theme || "dark";
  const resolvedTheme =
    themePref === "system"
      ? window.matchMedia("(prefers-color-scheme: dark)").matches
        ? "dark"
        : "light"
      : themePref === "light"
      ? "light"
      : "dark";

  function toggleTheme() {
    const next = resolvedTheme === "dark" ? "light" : "dark";
    applyTheme(next);
    updateConfig({ ...config, theme: next });
  }

  async function finish() {
    await invoke("complete_onboarding");
    onComplete();
  }

  const win = getCurrentWindow();

  return (
    <div className="onb-shell">
      <header className="onb-top">
        <div className="onb-brand">
          <img src={bulbulMark} alt="" className="onb-brand-mark" aria-hidden />
          <span>bulbul</span>
        </div>
        <div className="onb-progress" aria-label={`Step ${step + 1} of ${totalSteps}`}>
          {Array.from({ length: totalSteps }, (_, i) => (
            <span key={i} className={`onb-dot ${i === step ? "active" : i < step ? "done" : ""}`} />
          ))}
        </div>
        <div className="onb-top-right">
          <button
            className="onb-tb-btn"
            onClick={toggleTheme}
            aria-label={resolvedTheme === "dark" ? "Switch to light mode" : "Switch to dark mode"}
            title={resolvedTheme === "dark" ? "Switch to light mode" : "Switch to dark mode"}
          >
            {resolvedTheme === "dark" ? <SunIcon /> : <MoonIcon />}
          </button>
          <button
            className="onb-tb-btn"
            onClick={() => win.minimize().catch(() => {})}
            aria-label="Minimize"
            title="Minimize"
          >
            <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
              <line x1="1.5" y1="5" x2="8.5" y2="5" stroke="currentColor" strokeWidth="1" strokeLinecap="round" />
            </svg>
          </button>
          <button
            className="onb-tb-btn onb-tb-close"
            onClick={() => win.close().catch(() => {})}
            aria-label="Close"
            title="Close"
          >
            <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
              <line x1="1.5" y1="1.5" x2="8.5" y2="8.5" stroke="currentColor" strokeWidth="1" strokeLinecap="round" />
              <line x1="8.5" y1="1.5" x2="1.5" y2="8.5" stroke="currentColor" strokeWidth="1" strokeLinecap="round" />
            </svg>
          </button>
        </div>
      </header>

      <main className="onb-page" key={step}>
        {step === 0 && <StepWelcome onNext={() => setStep(1)} />}
        {step === 1 && (
          <StepApiKey
            config={config}
            updateConfig={updateConfig}
            onBack={() => setStep(0)}
            onNext={() => setStep(2)}
          />
        )}
        {step === 2 && (
          <StepHotkey
            config={config}
            updateConfig={updateConfig}
            onBack={() => setStep(1)}
            onNext={() => setStep(3)}
          />
        )}
        {step === 3 && <StepDone onFinish={finish} hotkey={config.hotkey} />}
      </main>
    </div>
  );
}

function StepWelcome({ onNext }) {
  return (
    <div className="onb-page-inner onb-welcome">
      <img src={bulbulMark} alt="" className="onb-hero-mark" aria-hidden />
      <h1>Welcome to Bulbul.</h1>
      <p className="onb-lead">
        Hold a hotkey anywhere on your computer. Speak. Text appears where your cursor is.
      </p>
      <div className="onb-value-grid">
        <div className="onb-value">
          <div className="onb-value-title">Your key, your audio</div>
          <p>Bulbul talks to Groq using your own API key. Nothing is logged on our servers — there are no servers.</p>
        </div>
        <div className="onb-value">
          <div className="onb-value-title">Free and open source</div>
          <p>Local app, no subscription, no telemetry. Yours to fork.</p>
        </div>
        <div className="onb-value">
          <div className="onb-value-title">Two minutes to set up</div>
          <p>API key, hotkey, done. You'll be dictating before your coffee's cold.</p>
        </div>
      </div>
      <div className="onb-actions onb-actions-center">
        <button className="onb-btn primary" onClick={onNext}>Get started →</button>
      </div>
    </div>
  );
}

function StepApiKey({ config, updateConfig, onBack, onNext }) {
  const [keyValue, setKeyValue] = useState(config.groq_api_key || "");
  const [keyState, setKeyState] = useState(
    config.groq_api_key && config.groq_api_key.trim().length > 0 ? "valid" : "idle"
  );
  const [keyError, setKeyError] = useState("");
  const [showKey, setShowKey] = useState(false);
  const [videoOpen, setVideoOpen] = useState(false);

  async function validateAndSave(value) {
    const v = (value ?? keyValue).trim();
    if (!v) {
      setKeyState("idle");
      setKeyError("");
      return;
    }
    setKeyState("checking");
    setKeyError("");
    try {
      await invoke("validate_api_key", { apiKey: v });
      setKeyState("valid");
      await updateConfig({ ...config, groq_api_key: v });
    } catch (e) {
      setKeyState("invalid");
      setKeyError(String(e));
    }
  }

  function handlePaste(e) {
    const pasted = e.clipboardData?.getData("text") ?? "";
    if (pasted.trim()) {
      // The input will receive the paste itself; we just kick off the
      // verification with the new value rather than waiting for blur.
      setTimeout(() => validateAndSave(pasted.trim()), 0);
    }
  }

  const verifyLabel =
    keyState === "valid" ? "Verified ✓" :
    keyState === "checking" ? "Checking…" :
    keyState === "invalid" ? "Retry" :
    "Verify";

  return (
    <div className="onb-page-inner">
      <header className="onb-step-head">
        <h2>Paste your Groq API key</h2>
        <p className="onb-sub">
          Bulbul uses Groq to transcribe and clean up what you say. You need a free key
          from <a href="#" onClick={(e) => { e.preventDefault(); openUrl("https://console.groq.com/keys"); }}>console.groq.com/keys</a>.
        </p>
      </header>

      <div className="onb-key-row">
        <div className={`onb-key-input-wrap ${keyState === "valid" ? "ok" : keyState === "invalid" ? "bad" : ""}`}>
          <input
            type={showKey ? "text" : "password"}
            className="onb-input"
            placeholder="gsk_..."
            value={keyValue}
            onChange={(e) => { setKeyValue(e.target.value); setKeyState("idle"); setKeyError(""); }}
            onPaste={handlePaste}
            onBlur={() => validateAndSave()}
            spellCheck={false}
            autoFocus
          />
          <button
            type="button"
            className="onb-key-eye"
            onClick={() => setShowKey((v) => !v)}
            aria-label={showKey ? "Hide key" : "Show key"}
            title={showKey ? "Hide" : "Show"}
            tabIndex={-1}
          >
            {showKey ? <EyeOffIcon /> : <EyeIcon />}
          </button>
        </div>
        <button
          type="button"
          className={`onb-verify-btn state-${keyState}`}
          onClick={() => validateAndSave()}
          disabled={keyState === "checking" || keyState === "valid" || keyValue.trim().length === 0}
        >
          {verifyLabel}
        </button>
      </div>

      <div className="onb-key-status">
        {keyState === "invalid" && <span className="onb-bad">{keyError}</span>}
        {keyState === "idle" && keyValue.trim().length === 0 && (
          <span className="onb-muted">We'll verify it the moment you paste.</span>
        )}
      </div>

      <div className="onb-video-block">
        {!videoOpen ? (
          <button className="onb-video-toggle" onClick={() => setVideoOpen(true)} type="button">
            Don't have a key? Watch the 60-second walkthrough →
          </button>
        ) : (
          <>
            <div className="onb-video-frame">
              <iframe
                src={VIDEO_EMBED}
                title="How to get a Groq API key"
                allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture"
                allowFullScreen
              />
            </div>
            <button
              className="onb-video-link"
              onClick={() => openUrl(VIDEO_URL)}
              type="button"
            >
              Open on YouTube →
            </button>
          </>
        )}
      </div>

      <div className="onb-actions">
        <button className="onb-btn ghost" onClick={onBack}>← Back</button>
        <button
          className="onb-btn primary"
          onClick={onNext}
          disabled={keyState !== "valid"}
        >
          Continue →
        </button>
      </div>
    </div>
  );
}

function StepHotkey({ config, updateConfig, onBack, onNext }) {
  const [selected, setSelected] = useState(matchPreset(config.hotkey));
  const [customCombo, setCustomCombo] = useState(
    !matchPreset(config.hotkey).match ? config.hotkey : ""
  );
  const [capturing, setCapturing] = useState(false);
  const [listening, setListening] = useState(false);
  const [transcript, setTranscript] = useState("");
  const textareaRef = useRef(null);

  const activeHotkey = selected.value === "custom"
    ? (customCombo || config.hotkey)
    : selected.value;

  useEffect(() => {
    let unlistenStatus = null;
    (async () => {
      unlistenStatus = await listen("bulbul-status", (e) => {
        const s = e.payload?.state;
        if (s === "listening") setListening(true);
        if (s === "idle" || s === "processing") setListening(false);
      });
    })();
    return () => { if (unlistenStatus) unlistenStatus(); };
  }, []);

  // Keep the textarea focused so the existing Win32 SendInput inject path
  // delivers the transcript right into it after a dictation.
  useEffect(() => {
    if (textareaRef.current) textareaRef.current.focus();
  }, [selected.value]);

  async function choose(value) {
    setTranscript("");
    setListening(false);
    const next = HOTKEY_PRESETS.find((p) => p.value === value) || HOTKEY_PRESETS[0];
    setSelected(next);
    if (value !== "custom") {
      await updateConfig({ ...config, hotkey: value });
    }
    // Restore focus to the textarea after the radio click
    setTimeout(() => textareaRef.current?.focus(), 50);
  }

  useEffect(() => {
    if (!capturing) return;
    const handler = (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        setCapturing(false);
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
      if (!k) { setCapturing(false); return; }
      parts.push(k);
      const combo = parts.join("+");
      setCustomCombo(combo);
      setCapturing(false);
      updateConfig({ ...config, hotkey: combo });
      setTranscript("");
    };
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [capturing, config, updateConfig]);

  function clearTest() {
    setTranscript("");
    if (textareaRef.current) {
      textareaRef.current.value = "";
      textareaRef.current.focus();
    }
  }

  return (
    <div className="onb-page-inner">
      <header className="onb-step-head">
        <h2>Pick your dictation hotkey</h2>
        <p className="onb-sub">Hold the keys to record. Release to transcribe and paste.</p>
      </header>

      <div className="onb-hotkey-grid">
        <div className="onb-hotkey-list">
          {HOTKEY_PRESETS.map((p) => (
            <label key={p.value} className={`onb-hotkey-row ${selected.value === p.value ? "selected" : ""}`}>
              <input
                type="radio"
                name="hotkey"
                checked={selected.value === p.value}
                onChange={() => choose(p.value)}
              />
              <div className="onb-hotkey-meta">
                <div className="onb-hotkey-label">{p.label}</div>
                <div className="onb-hotkey-detail">{p.detail}</div>
              </div>
              {p.value === "custom" && selected.value === "custom" && (
                <div className="onb-hotkey-custom">
                  <code>{customCombo || config.hotkey || "—"}</code>
                  <button
                    className="onb-btn ghost small"
                    type="button"
                    onClick={(e) => { e.preventDefault(); setCapturing(true); }}
                  >
                    {capturing ? "Press keys…" : "Record"}
                  </button>
                </div>
              )}
            </label>
          ))}

          <div className="onb-conflict-hint">
            Already using another dictation app like commercial dictation apps on <code>Ctrl + Win</code>? Pick a different combo
            above and the conflict goes away.
          </div>
        </div>

        <div className="onb-test-pane">
          <div className="onb-test-header">
            <div className="onb-test-eyebrow">Try it now</div>
            <div className="onb-test-instructions">
              Hold <code>{formatComboForDisplay(activeHotkey)}</code> and read this aloud:
            </div>
            <div className="onb-sample-line">"{SAMPLE_LINE}"</div>
          </div>

          <div className={`onb-textarea-wrap ${listening ? "listening" : ""}`}>
            <textarea
              ref={textareaRef}
              className="onb-textarea"
              placeholder="Hold your hotkey, speak, and the transcript will land here…"
              onChange={(e) => setTranscript(e.target.value)}
              spellCheck={false}
            />
            {listening && (
              <div className="onb-listening-badge">
                <span className="onb-pulse" />
                Listening…
              </div>
            )}
          </div>

          <div className="onb-test-foot">
            {transcript.trim().length > 0 ? (
              <span className="onb-ok">✓ Dictation works — that's your transcript.</span>
            ) : listening ? (
              <span className="onb-muted">Speak the line above…</span>
            ) : (
              <span className="onb-muted">No transcript yet. Hold the keys for a moment.</span>
            )}
            <button className="onb-link" type="button" onClick={clearTest}>Clear and try again</button>
          </div>
        </div>
      </div>

      <div className="onb-actions">
        <button className="onb-btn ghost" onClick={onBack}>← Back</button>
        <button className="onb-btn primary" onClick={onNext}>Continue →</button>
      </div>
    </div>
  );
}

function StepDone({ onFinish, hotkey }) {
  return (
    <div className="onb-page-inner onb-done">
      <div className="onb-done-check">✓</div>
      <h2>You're all set.</h2>
      <p className="onb-lead">
        Press <code>{formatComboForDisplay(hotkey)}</code> anywhere — in your browser, in Word, in a terminal — speak,
        and Bulbul will type what you said.
      </p>
      <div className="onb-tour-grid">
        <div className="onb-tour-card">
          <div className="onb-tour-title">Transform selections</div>
          <p>Select text anywhere and press <code>Alt + 1…5</code> to polish, formalize, or rephrase it in place.</p>
        </div>
        <div className="onb-tour-card">
          <div className="onb-tour-title">Stays out of your way</div>
          <p>Close the window to send Bulbul to the tray. Click the tray icon to bring it back.</p>
        </div>
        <div className="onb-tour-card">
          <div className="onb-tour-title">Tune everything</div>
          <p>Change hotkeys, model, theme, mic in Settings — anytime.</p>
        </div>
      </div>
      <div className="onb-actions onb-actions-center">
        <button className="onb-btn primary" onClick={onFinish}>Open Bulbul →</button>
      </div>
    </div>
  );
}

function EyeIcon() {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M2 12s3-7 10-7 10 7 10 7-3 7-10 7-10-7-10-7z" />
      <circle cx="12" cy="12" r="3" />
    </svg>
  );
}

function EyeOffIcon() {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M9.88 9.88a3 3 0 1 0 4.24 4.24" />
      <path d="M10.73 5.08A10.43 10.43 0 0 1 12 5c7 0 10 7 10 7a13.16 13.16 0 0 1-1.67 2.68" />
      <path d="M6.61 6.61A13.526 13.526 0 0 0 2 12s3 7 10 7a9.74 9.74 0 0 0 5.39-1.61" />
      <line x1="2" y1="2" x2="22" y2="22" />
    </svg>
  );
}

function SunIcon() {
  return (
    <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <circle cx="12" cy="12" r="4" />
      <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M4.93 19.07l1.41-1.41M17.66 6.34l1.41-1.41" />
    </svg>
  );
}

function MoonIcon() {
  return (
    <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" />
    </svg>
  );
}

function matchPreset(hotkey) {
  const preset = HOTKEY_PRESETS.find((p) => p.value === hotkey && p.value !== "custom");
  return preset ? { ...preset, match: true } : { ...HOTKEY_PRESETS.find((p) => p.value === "custom"), match: false };
}

function formatComboForDisplay(combo) {
  if (!combo) return "—";
  return combo.replaceAll("+", " + ");
}

function domKeyToName(code) {
  if (code.startsWith("Key")) return code.slice(3);
  if (code.startsWith("Digit")) return code.slice(5);
  if (code.startsWith("F") && /^F\d+$/.test(code)) return code;
  const map = {
    Space: "Space",
    Tab: "Tab",
    Enter: "Enter",
    Escape: "Escape",
    Backspace: "Backspace",
  };
  return map[code] || null;
}
