import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl } from "@tauri-apps/plugin-opener";
import bulbulMark from "../assets/bulbul-mark.png";
import { applyTheme } from "../theme.js";
import { IS_ANDROID, IS_LINUX, IS_MAC, META_KEY_NAME } from "../platform.js";
import "./onboarding.css";

// The stored hotkey VALUES are platform-independent — Bulbul's parser maps
// "Win" to the OS meta key (Command on Mac, Super on Linux, Windows key on
// Windows) and "Alt" to Option on Mac. Only the user-facing label + detail
// copy needs to differ per platform so the wizard reads correctly.
const HOTKEY_PRESETS_DESKTOP = [
  {
    value: "Ctrl+Win",
    label: "Ctrl + Win",
    detail: "Hold both keys to dictate. Two-modifier chord — minimum reach from the home row.",
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

const HOTKEY_PRESETS_MAC = [
  {
    value: "Ctrl+Win",
    label: "⌃ Control + ⌘ Command",
    detail: "Hold both keys to dictate. Two-modifier chord — minimum reach from the home row.",
  },
  {
    value: "Alt+Win",
    label: "⌥ Option + ⌘ Command",
    detail: "Same hold-to-talk feel, different fingers. Pick this if ⌃⌘ is taken by another app.",
  },
  {
    value: "Ctrl+Shift+Space",
    label: "⌃ Control + ⇧ Shift + Space",
    detail: "Three keys, but almost never clashes with anything. The safest pick.",
  },
  {
    value: "custom",
    label: "Custom combo…",
    detail: "Capture any combination you like.",
  },
];

// Linux presets skip modifier-only chords: the Super key belongs to the
// compositor (GNOME Activities, KDE launcher), and Wayland's shortcut
// portal can only bind combos that contain a real key.
const HOTKEY_PRESETS_LINUX = [
  {
    value: "Ctrl+Alt+Space",
    label: "Ctrl + Alt + Space",
    detail: "Hold to dictate. Doesn't fight the Super key, and works on both X11 and Wayland.",
  },
  {
    value: "Ctrl+Shift+Space",
    label: "Ctrl + Shift + Space",
    detail: "Same hold-to-talk feel, different fingers. Pick this if Ctrl + Alt + Space is taken.",
  },
  {
    value: "custom",
    label: "Custom combo…",
    detail: "Capture any combination you like.",
  },
];

const HOTKEY_PRESETS = IS_MAC
  ? HOTKEY_PRESETS_MAC
  : IS_LINUX
    ? HOTKEY_PRESETS_LINUX
    : HOTKEY_PRESETS_DESKTOP;

const VIDEO_URL = "https://www.youtube.com/watch?v=9VDbhptCzlU";
const VIDEO_EMBED = "https://www.youtube-nocookie.com/embed/9VDbhptCzlU";

// Sample line deliberately seeded with "um", "uh", "like" so the cleanup
// pass visibly removes them — the user sees Bulbul not just transcribe but
// clean. If they're on Raw mode every filler stays, which is also useful
// feedback ("ah, that's what Raw mode means").
const SAMPLE_LINE = "Hi Bulbul, um, this is, uh, my first test, and like, it looks great.";

// Mac inserts a one-time Permissions step between Welcome and the API
// key entry. Non-Mac platforms skip it (Windows has no permission gate;
// Linux X11 needs none, Linux Wayland prompts via portal on first use).
// Android has no global hotkey (dictation is the floating bubble) and its
// system permissions are granted through the native setup screen, so the
// wizard is just the essentials: welcome, key, language, done.
const STEP_SEQUENCE = IS_ANDROID
  ? ["welcome", "apiKey", "language", "done"]
  : IS_MAC
  ? ["welcome", "permissions", "apiKey", "language", "hotkey", "done"]
  : ["welcome", "apiKey", "language", "hotkey", "done"];

export default function OnboardingWizard({ config, updateConfig, onComplete }) {
  const [step, setStep] = useState(0);
  const totalSteps = STEP_SEQUENCE.length;
  const currentStepName = STEP_SEQUENCE[step];
  const goNext = () => setStep((s) => Math.min(s + 1, STEP_SEQUENCE.length - 1));
  const goBack = () => setStep((s) => Math.max(s - 1, 0));

  const themePref = config.theme || "light";
  const resolvedTheme =
    themePref === "system"
      ? window.matchMedia("(prefers-color-scheme: dark)").matches
        ? "dark"
        : "light"
      : themePref === "dark"
      ? "dark"
      : "light";

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
          {!IS_ANDROID && !IS_MAC && (
            <>
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
            </>
          )}
        </div>
      </header>

      <main className="onb-page" key={step}>
        {currentStepName === "welcome" && (
          <StepWelcome
            config={config}
            updateConfig={updateConfig}
            onNext={goNext}
          />
        )}
        {currentStepName === "permissions" && (
          <StepPermissions onBack={goBack} onNext={goNext} />
        )}
        {currentStepName === "apiKey" && (
          <StepApiKey
            config={config}
            updateConfig={updateConfig}
            onBack={goBack}
            onNext={goNext}
          />
        )}
        {currentStepName === "language" && (
          <StepLanguage
            config={config}
            updateConfig={updateConfig}
            onBack={goBack}
            onNext={goNext}
          />
        )}
        {currentStepName === "hotkey" && (
          <StepHotkey
            config={config}
            updateConfig={updateConfig}
            onBack={goBack}
            onNext={goNext}
          />
        )}
        {currentStepName === "done" && (
          <StepDone
            onFinish={finish}
            hotkey={config.hotkey}
            telemetryEnabled={!!config.telemetry_enabled}
            onToggleTelemetry={(v) => updateConfig({ ...config, telemetry_enabled: v })}
          />
        )}
      </main>
    </div>
  );
}

function StepWelcome({ config, updateConfig, onNext }) {
  // Local draft so typing doesn't write to disk per keystroke. Commit
  // on blur or on Continue. Pre-populated when the user has filled
  // this before and is revisiting the wizard.
  const [name, setName] = useState(config?.display_name || "");

  function commitAndNext() {
    const trimmed = name.trim();
    if (trimmed !== (config?.display_name || "")) {
      updateConfig({ ...config, display_name: trimmed });
    }
    onNext();
  }

  return (
    <div className="onb-page-inner onb-welcome">
      <img src={bulbulMark} alt="" className="onb-hero-mark" aria-hidden />
      <h1>Welcome to Bulbul.</h1>
      <p className="onb-lead">
        {IS_ANDROID
          ? "Tap the floating bubble in any app. Speak. Your words appear where your cursor is."
          : "Hold a hotkey anywhere on your computer. Speak. Text appears where your cursor is."}
      </p>
      <div className="onb-value-grid">
        <div className="onb-value">
          <div className="onb-value-title">Your key, your audio</div>
          <p>Bulbul talks to Groq using your own API key. Nothing is logged on our servers — there are no servers.</p>
        </div>
        <div className="onb-value">
          <div className="onb-value-title">Free and open source</div>
          <p>Local app, no subscription, no surprises. Yours to fork.</p>
        </div>
        <div className="onb-value">
          <div className="onb-value-title">Two minutes to set up</div>
          <p>API key, hotkey, done. You'll be dictating before your coffee's cold.</p>
        </div>
      </div>

      <div className="onb-name-block">
        <label htmlFor="onb-name-input" className="onb-name-label">
          What should I call you? <span className="muted small">(optional, stays on your machine)</span>
        </label>
        <input
          id="onb-name-input"
          type="text"
          className="onb-name-input"
          placeholder="First name"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              commitAndNext();
            }
          }}
          spellCheck={false}
          autoComplete="off"
          maxLength={48}
        />
        <p className="muted small onb-name-hint">
          Used to sign your Compose drafts and greet you on the home page. Never sent anywhere — Bulbul keeps it in your local config.
        </p>
      </div>

      <div className="onb-actions onb-actions-center">
        <button className="onb-btn primary" onClick={commitAndNext}>Get started →</button>
      </div>
    </div>
  );
}

// Mac-only permissions gate. Bulbul needs Microphone (to capture audio
// from your dictation hotkey) and Accessibility (to inject text into
// other apps and read which app is focused). macOS exposes both behind
// the same Privacy & Security pane in System Settings.
//
// Both status checks are programmatic and polled every 1.5s while the
// step is on screen:
//   - AX:  AXIsProcessTrusted()
//   - Mic: AVCaptureDevice.authorizationStatusForMediaType(.audio)
// Continue unlocks the moment both flip to granted; no user
// confirmation step needed.
function StepPermissions({ onBack, onNext }) {
  const [axGranted, setAxGranted] = useState(false);
  const [micStatus, setMicStatus] = useState("not_determined");
  const micGranted = micStatus === "granted";
  // Case 2 (stale grant) detection: set right before a "Quit & Relaunch".
  // If the app comes back and AX is STILL not granted, relaunching didn't
  // help — so the card offers "Reset permission" (tccutil reset) instead.
  // Persisted in localStorage so it survives the relaunch; cleared once AX
  // finally reads granted.
  const [relaunchTried, setRelaunchTried] = useState(
    () => localStorage.getItem("bulbul_ax_relaunched") === "1",
  );

  useEffect(() => {
    let cancelled = false;
    async function check() {
      try {
        const [ax, mic] = await Promise.all([
          invoke("check_accessibility_status_mac"),
          invoke("check_microphone_status_mac"),
        ]);
        if (!cancelled) {
          setAxGranted(!!ax);
          setMicStatus(typeof mic === "string" ? mic : "not_determined");
          if (ax) {
            // Grant finally landed — clear the Case-2 relaunch flag so a
            // later not-granted state starts fresh at "Quit & Relaunch".
            localStorage.removeItem("bulbul_ax_relaunched");
            setRelaunchTried(false);
          }
        }
      } catch {
        // Silent — commands rarely fail. If they do, the user can
        // grant manually in System Settings and re-launch onboarding.
      }
    }
    check();
    const interval = setInterval(check, 1500);
    return () => {
      cancelled = true;
      clearInterval(interval);
    };
  }, []);

  // Trigger the macOS mic-permission prompt once on step entry.
  // macOS only adds Bulbul to the Microphone TCC list (visible in
  // System Settings → Privacy → Microphone) AFTER an app has actually
  // called AVCaptureDevice.requestAccess. Without this, the Settings
  // pane opens but shows no Bulbul row to toggle. Idempotent — calling
  // it after the user has already responded is a no-op.
  //
  // Same reasoning drives prime_accessibility_mac: enigo's Enigo::new
  // internally calls AXIsProcessTrustedWithOptions({prompt: true})
  // which registers Bulbul with the Accessibility TCC list AND pops
  // the native "Bulbul wants Accessibility" system dialog. Without
  // this priming call, the user opens Settings → Accessibility and
  // finds no Bulbul row — they'd have to click `+` and browse to
  // Bulbul.app themselves. Priming makes the toggle appear where
  // they're already looking. If AX is already granted, the call
  // succeeds silently and doesn't re-prompt.
  useEffect(() => {
    invoke("request_microphone_access_mac").catch(() => {});
    invoke("prime_accessibility_mac").catch(() => {
      // Expected on first run before user grants — the wizard's
      // polling still drives the ✓ state, and the system dialog has
      // already fired at this point (that's a side-effect of the
      // AXIsProcessTrustedWithOptions call, not the Rust return
      // value). Silent catch keeps the console clean.
    });
  }, []);

  async function openSettings(pane) {
    // Shelling out to the macOS `open` CLI on the backend is more
    // reliable than tauri-plugin-opener's openUrl for custom URL
    // schemes like x-apple.systempreferences: — the plugin's default
    // capabilities only allow http/https and silently reject the rest.
    //
    // Before opening the pane for "microphone" specifically, request
    // access again to guarantee Bulbul is in the TCC list — covers
    // the edge case where the user reached this step without the
    // initial useEffect having completed (rare, but cheap to defend).
    if (pane === "microphone") {
      try {
        await invoke("request_microphone_access_mac");
      } catch {}
    }
    try {
      await invoke("open_mac_settings_pane", { pane });
    } catch {
      // Last-resort generic Privacy & Security pane.
      try {
        await invoke("open_mac_settings_pane", { pane: "privacy" });
      } catch {}
    }
  }

  function doRelaunch() {
    // Mark that we tried a relaunch, so if AX is still false when we come
    // back we can offer the Case-2 reset instead. localStorage survives
    // the restart (same webview data dir).
    localStorage.setItem("bulbul_ax_relaunched", "1");
    invoke("relaunch_app").catch(() => {});
  }

  async function doResetAccessibility() {
    try {
      await invoke("reset_accessibility_mac");
    } catch {
      // Quiet — if tccutil fails, the manual System Settings path still
      // works and the 1.5s polling keeps driving the ✓ state.
    }
    // Fresh slate: clear the flag so the card returns to the normal
    // grant → relaunch flow for the newly-reset permission.
    localStorage.removeItem("bulbul_ax_relaunched");
    setRelaunchTried(false);
  }

  const ready = axGranted && micGranted;

  // Human-readable status label for the mic card. Distinguishes
  // "not asked yet" (user hasn't opened Settings or hit the hotkey) from
  // "explicitly denied" (different remediation: re-enable a slider
  // they previously turned off).
  const micStatusLabel = (() => {
    switch (micStatus) {
      case "granted":
        return "Detected — ready to go.";
      case "denied":
        return "Microphone access is currently denied. Toggle Bulbul on in System Settings.";
      case "restricted":
        return "Microphone access restricted by a system policy.";
      default:
        return "Status updates here automatically once you grant access.";
    }
  })();

  return (
    <div className="onb-page-inner">
      <header className="onb-step-head">
        <h2>Grant macOS permissions</h2>
        <p className="onb-sub">
          Bulbul needs two macOS permissions to capture audio and inject text into other apps. Both grant via System Settings → Privacy &amp; Security.
        </p>
      </header>

      <div className="onb-perm-cards">
        <article className={`onb-perm-card ${micGranted ? "granted" : ""}`}>
          <header className="onb-perm-head">
            <span className="onb-perm-status" aria-hidden>
              {micGranted ? "✓" : "○"}
            </span>
            <h2>Microphone</h2>
          </header>
          <p className="muted small">
            Captures audio from your dictation hotkey. Without this, recording fails silently.
          </p>
          <div className="onb-perm-actions">
            <button className="onb-btn" onClick={() => openSettings("microphone")}>
              Open Microphone Settings
            </button>
          </div>
          <p className="onb-perm-confirm muted small">{micStatusLabel}</p>
        </article>

        <article className={`onb-perm-card ${axGranted ? "granted" : ""}`}>
          <header className="onb-perm-head">
            <span className="onb-perm-status" aria-hidden>
              {axGranted ? "✓" : "○"}
            </span>
            <h2>Accessibility</h2>
          </header>
          <p className="muted small">
            Lets Bulbul inject text into other apps and detect which app you're dictating into. Without this, paste-after-dictation does nothing.
          </p>
          <div className="onb-perm-actions">
            <button className="onb-btn" onClick={() => openSettings("accessibility")}>
              Open Accessibility Settings
            </button>
            {!axGranted && !relaunchTried && (
              <button
                className="onb-btn ghost"
                onClick={doRelaunch}
                title="macOS sometimes won't notice the new permission until Bulbul restarts"
              >
                Quit &amp; Relaunch
              </button>
            )}
            {!axGranted && relaunchTried && (
              <button
                className="onb-btn ghost"
                onClick={doResetAccessibility}
                title="Relaunching didn't help — clear a stale permission left by a previous install, then grant again"
              >
                Reset permission
              </button>
            )}
          </div>
          <p className="onb-perm-confirm muted small">
            {axGranted
              ? "Detected — ready to go."
              : relaunchTried
                ? "Still not detected after a relaunch — this usually means a stale permission left by a previous install. Click Reset permission to clear it, then toggle Bulbul on when the dialog reappears and relaunch once more."
                : "macOS just popped a system dialog asking to grant Accessibility. Click Open System Settings in it, toggle Bulbul on, then come back. If the check mark doesn't appear within a few seconds, click Quit & Relaunch — macOS sometimes needs Bulbul to restart before the new permission takes effect."}
          </p>
        </article>
      </div>

      <div className="onb-actions">
        <button className="onb-btn ghost" onClick={onBack}>
          Back
        </button>
        <button className="onb-btn primary" onClick={onNext} disabled={!ready}>
          Continue →
        </button>
      </div>
    </div>
  );
}

const THEME_OPTIONS = [
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
  { value: "system", label: "System" },
];

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
            onBlur={() => { if (keyState !== "valid" && keyState !== "checking") validateAndSave(); }}
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

      <div className="onb-theme-block">
        <div className="onb-theme-label">Appearance</div>
        <div className="segmented onb-theme-seg">
          {THEME_OPTIONS.map((t) => (
            <button
              key={t.value}
              type="button"
              className={`segmented-btn ${(config.theme || "light") === t.value ? "selected" : ""}`}
              onClick={() => {
                applyTheme(t.value);
                updateConfig({ ...config, theme: t.value });
              }}
            >
              {t.label}
            </button>
          ))}
        </div>
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

// Languages exposed under "Another language" — same set as the Settings
// dropdown minus the buckets we already surface as primary radios (English,
// Hindi/Hinglish) and minus "auto" (its own radio). Alphabetised by display
// label so users can scan to the one they want.
const OTHER_LANGUAGES = [
  { code: "ar", label: "Arabic" },
  { code: "zh", label: "Chinese" },
  { code: "nl", label: "Dutch" },
  { code: "fi", label: "Finnish" },
  { code: "fr", label: "French" },
  { code: "de", label: "German" },
  { code: "el", label: "Greek" },
  { code: "he", label: "Hebrew" },
  { code: "id", label: "Indonesian" },
  { code: "it", label: "Italian" },
  { code: "ja", label: "Japanese" },
  { code: "ko", label: "Korean" },
  { code: "pl", label: "Polish" },
  { code: "pt", label: "Portuguese" },
  { code: "ru", label: "Russian" },
  { code: "es", label: "Spanish" },
  { code: "sv", label: "Swedish" },
  { code: "th", label: "Thai" },
  { code: "tr", label: "Turkish" },
  { code: "uk", label: "Ukrainian" },
  { code: "vi", label: "Vietnamese" },
];
const OTHER_LANGUAGE_CODES = OTHER_LANGUAGES.map((l) => l.code);

// Map BCP-47 system locale to a wizard "bucket" so we can pre-select a
// sensible default. Pakistani/Indian Urdu speakers also get suggested
// Hindi/Hinglish: in practice they want Devanagari output for code-switched
// Hindustani, and they can flip to Arabic-script Urdu via "Another language"
// if they really want it. English-locale users get English.
function detectDefaultLanguage() {
  const tag = (navigator.language || "en").toLowerCase();
  const primary = tag.split("-")[0];
  if (primary === "hi" || primary === "ur") return { bucket: "hindi", otherCode: "" };
  if (primary === "en") return { bucket: "english", otherCode: "" };
  if (OTHER_LANGUAGE_CODES.includes(primary)) return { bucket: "other", otherCode: primary };
  return { bucket: "auto", otherCode: "" };
}

function codeFromPick(bucket, otherCode) {
  if (bucket === "english") return "en";
  if (bucket === "hindi") return "hi";
  if (bucket === "other") return otherCode || "es";
  return "auto";
}

function bucketFromCode(code) {
  if (code === "en") return "english";
  if (code === "hi") return "hindi";
  if (code === "auto" || !code) return null;
  return "other";
}

function StepLanguage({ config, updateConfig, onBack, onNext }) {
  const detected = useRef(detectDefaultLanguage()).current;
  const initialBucket =
    bucketFromCode(config.language) ?? detected.bucket;
  const initialOther =
    config.language && bucketFromCode(config.language) === "other"
      ? config.language
      : detected.otherCode || "es";
  const [pick, setPick] = useState(initialBucket);
  const [other, setOther] = useState(initialOther);

  // Sync config to match the initial pick on mount. For fresh installs this
  // commits the locale-suggested default so users who just click Continue
  // still get a sensible language. For returning visits it's a no-op.
  useEffect(() => {
    const code = codeFromPick(initialBucket, initialOther);
    if (code !== config.language) {
      updateConfig({ ...config, language: code });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function choose(nextBucket, nextOther) {
    setPick(nextBucket);
    if (nextOther !== undefined) setOther(nextOther);
    const code = codeFromPick(nextBucket, nextOther ?? other);
    updateConfig({ ...config, language: code });
  }

  return (
    <div className="onb-page-inner">
      <header className="onb-step-head">
        <h2>What language do you dictate in?</h2>
        <p className="onb-sub">
          Pick the one you use most. Mixing English in is fine — Bulbul handles that for any choice.
        </p>
      </header>

      <div className="onb-lang-list">
        <LangRow
          checked={pick === "english"}
          onClick={() => choose("english")}
          label="English"
          detail="Best for purely English dictation."
          suggested={detected.bucket === "english"}
        />
        <LangRow
          checked={pick === "hindi"}
          onClick={() => choose("hindi")}
          label="Hindi / Hinglish"
          detail="Handles pure Hindi (Devanagari) and code-switched Hinglish. English-only sentences still come out in Latin."
          suggested={detected.bucket === "hindi"}
        />
        <LangRow
          checked={pick === "other"}
          onClick={() => choose("other")}
          label="Another language"
          detail={pick === "other" ? null : "Pick from the full list."}
          suggested={detected.bucket === "other"}
        >
          {pick === "other" && (
            <LanguageCombo
              value={other}
              options={OTHER_LANGUAGES}
              onChange={(code) => choose("other", code)}
            />
          )}
        </LangRow>
        <LangRow
          checked={pick === "auto"}
          onClick={() => choose("auto")}
          label="Auto-detect"
          detail="English-leaning. Avoid if you dictate in Hindi — it occasionally outputs Urdu/Arabic script for the same audio."
          suggested={detected.bucket === "auto"}
        />
      </div>

      <div className="onb-actions">
        <button className="onb-btn ghost" onClick={onBack}>← Back</button>
        <button className="onb-btn primary" onClick={onNext}>Continue →</button>
      </div>
    </div>
  );
}

// Custom themed combobox for the "Another language" sub-picker. We use this
// instead of a native <select> because the OS-rendered dropdown ignores our
// theme tokens (background, border, text colour), and on Windows it pops up
// stretched across the whole viewport with no scroll cap — both jarring next
// to the rest of the wizard.
function LanguageCombo({ value, options, onChange }) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef(null);

  // Close when clicking anywhere outside, or pressing Escape. Mousedown
  // (not click) so we close before any click handler on outside content runs.
  useEffect(() => {
    if (!open) return;
    const onDocDown = (e) => {
      if (rootRef.current && !rootRef.current.contains(e.target)) setOpen(false);
    };
    const onKey = (e) => { if (e.key === "Escape") setOpen(false); };
    document.addEventListener("mousedown", onDocDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDocDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const selected = options.find((o) => o.code === value) || options[0];

  return (
    <div className="onb-combo" ref={rootRef}>
      <button
        type="button"
        className={`onb-combo-trigger ${open ? "open" : ""}`}
        onClick={(e) => { e.preventDefault(); e.stopPropagation(); setOpen((v) => !v); }}
        aria-haspopup="listbox"
        aria-expanded={open}
      >
        <span className="onb-combo-value">{selected.label}</span>
        <ComboChevron />
      </button>
      {open && (
        <div className="onb-combo-list" role="listbox">
          {options.map((o) => (
            <button
              key={o.code}
              type="button"
              role="option"
              aria-selected={o.code === value}
              className={`onb-combo-item ${o.code === value ? "selected" : ""}`}
              onClick={(e) => {
                e.preventDefault();
                e.stopPropagation();
                onChange(o.code);
                setOpen(false);
              }}
            >
              {o.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function ComboChevron() {
  return (
    <svg width="10" height="6" viewBox="0 0 10 6" aria-hidden>
      <path d="M1 1l4 4 4-4" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}

function LangRow({ checked, onClick, label, detail, suggested, children }) {
  return (
    <label className={`onb-hotkey-row ${checked ? "selected" : ""}`}>
      <input
        type="radio"
        name="onb-language"
        checked={checked}
        onChange={onClick}
      />
      <div className="onb-hotkey-meta">
        <div className="onb-hotkey-label">
          {label}
          {suggested && <span className="onb-lang-suggested">Suggested for your system</span>}
        </div>
        {detail && <div className="onb-hotkey-detail">{detail}</div>}
        {children}
      </div>
    </label>
  );
}

function StepHotkey({ config, updateConfig, onBack, onNext }) {
  const [selected, setSelected] = useState(matchPreset(config.hotkey));
  const [customCombo, setCustomCombo] = useState(
    !matchPreset(config.hotkey).match ? config.hotkey : ""
  );
  const [capturing, setCapturing] = useState(false);
  const [captureError, setCaptureError] = useState("");
  const [transcript, setTranscript] = useState("");
  // Set of canonical key names ("Ctrl", "Win", "Space"…) currently held
  // down inside the wizard window. We track this purely so we can light
  // up the visual keycaps one-by-one as the user assembles the chord —
  // the actual hotkey firing is still done by the OS-level global shortcut
  // (which surfaces via the bulbul-status event below), so partial state
  // never triggers a dictation.
  const [pressedKeys, setPressedKeys] = useState(() => new Set());
  // Visual state machine for the press-and-hold demo. Driven by the
  // bulbul-status events the Rust orchestrator emits — same machinery
  // that drives the production overlay, so the wizard's feedback matches
  // the real app exactly.
  //   idle      → "Hold the keys to start" (default)
  //   listening → mic pulsing + waveform animation ("keep holding")
  //   processing→ spinner ("transcribing...")
  //   done      → green check, brief pulse, auto-reset to idle
  //   too_short → amber warning, "released too early"
  //   silent    → muted, "no speech detected — try speaking louder"
  //   error     → red, surfaces the message
  const [hotkeyState, setHotkeyState] = useState("idle");
  const [errorMsg, setErrorMsg] = useState("");
  const textareaRef = useRef(null);

  const activeHotkey = selected.value === "custom"
    ? (customCombo || config.hotkey)
    : selected.value;

  // Deliver the dictation into the test box. During onboarding Bulbul's own
  // window is focused, so the orchestrator detects Bulbul as the foreground
  // app and routes the transcript as a `bulbul-focused-insert` event to the
  // main window instead of OS-typing it into the focused field. Only
  // ScratchpadView listened for that, so on platforms with working
  // foreground detection (Linux/X11) the wizard's test box received nothing
  // even though dictation worked everywhere else. Insert it at the caret,
  // mirroring ScratchpadView. On platforms/paths that OS-type into the
  // textarea instead, this event isn't emitted, so there's no double insert.
  useEffect(() => {
    const un = listen("bulbul-focused-insert", (event) => {
      const text = String(event.payload || "");
      const el = textareaRef.current;
      if (!text || !el) return;
      const start = el.selectionStart ?? el.value.length;
      const end = el.selectionEnd ?? el.value.length;
      el.value = el.value.slice(0, start) + text + el.value.slice(end);
      const caret = start + text.length;
      el.focus();
      el.setSelectionRange(caret, caret);
      setTranscript(el.value);
    });
    return () => {
      un.then((f) => f()).catch(() => {});
    };
  }, []);

  // Subscribe to the same status events the production overlay uses. The
  // wizard window may not have focus when the user holds their hotkey
  // (intentional — chord hotkeys fire globally), so we can't rely on
  // browser keydown events.
  useEffect(() => {
    const un = listen("bulbul-status", (e) => {
      const { state, message } = e.payload || {};
      if (state === "listening") {
        setHotkeyState("listening");
      } else if (state === "processing" || state === "injecting") {
        setHotkeyState("processing");
      } else if (state === "done") {
        setHotkeyState("done");
      } else if (state === "error") {
        setHotkeyState("error");
        setErrorMsg(message || "");
      } else if (state === "idle") {
        // Idle with a message means the pipeline rejected the take. The
        // message text tells us why so we can pick the right coaching.
        if (message && /too short/i.test(message)) {
          setHotkeyState("too_short");
        } else if (message && /(silence|no speech)/i.test(message)) {
          setHotkeyState("silent");
        } else {
          setHotkeyState("idle");
        }
      }
    });
    return () => { un.then((f) => f()); };
  }, []);

  // Auto-reset to idle a beat after a terminal state so the next attempt
  // starts clean. "done" gets a longer dwell so the celebration lands;
  // failure states reset faster so the user can re-try quickly.
  useEffect(() => {
    if (hotkeyState === "idle" || hotkeyState === "listening" || hotkeyState === "processing") {
      return;
    }
    const dwell = hotkeyState === "done" ? 2200 : 3000;
    const t = setTimeout(() => setHotkeyState("idle"), dwell);
    return () => clearTimeout(t);
  }, [hotkeyState]);

  // Keep the textarea focused so the existing Win32 SendInput inject path
  // delivers the transcript right into it after a dictation.
  useEffect(() => {
    if (textareaRef.current) textareaRef.current.focus();
  }, [selected.value]);

  // Track which keys are currently held inside the wizard window so the
  // ChordDisplay can light up each keycap as the user assembles the
  // chord. Listening on window (capture phase) so we get the events
  // regardless of which element has focus inside the wizard.
  useEffect(() => {
    const onDown = (e) => {
      const name = keyEventToName(e);
      if (!name) return;
      setPressedKeys((prev) => {
        if (prev.has(name)) return prev;
        const next = new Set(prev);
        next.add(name);
        return next;
      });
    };
    const onUp = (e) => {
      const name = keyEventToName(e);
      if (!name) return;
      setPressedKeys((prev) => {
        if (!prev.has(name)) return prev;
        const next = new Set(prev);
        next.delete(name);
        return next;
      });
    };
    // Losing focus mid-press would otherwise leave keycaps stuck "down"
    // because the keyup never reaches us.
    const onBlur = () => setPressedKeys(new Set());
    window.addEventListener("keydown", onDown);
    window.addEventListener("keyup", onUp);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("keydown", onDown);
      window.removeEventListener("keyup", onUp);
      window.removeEventListener("blur", onBlur);
    };
  }, []);

  const requiredParts = parseChordParts(activeHotkey);

  async function choose(value) {
    setTranscript("");
    const next = HOTKEY_PRESETS.find((p) => p.value === value) || HOTKEY_PRESETS[0];
    setSelected(next);
    if (value !== "custom") {
      await updateConfig({ ...config, hotkey: value });
    }
    // Restore focus to the textarea after the radio click
    setTimeout(() => textareaRef.current?.focus(), 50);
  }

  // Same state machine as SettingsView's recorder — see comment there
  // for the full reasoning. Short version: support modifier-only chords
  // (Ctrl+Win) by waiting for the final keyup, surface unsupported keys
  // inline via setCaptureError instead of silently aborting, and never
  // commit a single-modifier "tap" (would register a useless hotkey
  // that fires on every plain Ctrl press).
  useEffect(() => {
    if (!capturing) return;
    setCaptureError("");
    let peak = { ctrl: false, shift: false, alt: false, meta: false };
    let nonModPressed = false;

    const reset = () => {
      peak = { ctrl: false, shift: false, alt: false, meta: false };
      nonModPressed = false;
    };

    const commit = (combo) => {
      setCustomCombo(combo);
      setCapturing(false);
      setCaptureError("");
      updateConfig({ ...config, hotkey: combo });
      setTranscript("");
    };

    const onKeyDown = (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        setCapturing(false);
        setCaptureError("");
        return;
      }
      e.preventDefault();
      e.stopImmediatePropagation();
      peak = {
        ctrl: e.ctrlKey || peak.ctrl,
        shift: e.shiftKey || peak.shift,
        alt: e.altKey || peak.alt,
        meta: e.metaKey || peak.meta,
      };
      if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return;
      nonModPressed = true;
      const k = domKeyToName(e.code);
      if (!k) {
        setCaptureError(
          `"${e.key || e.code}" isn't supported. Try a letter, digit, function key, arrow, or punctuation.`,
        );
        return;
      }
      const parts = [];
      if (peak.ctrl) parts.push("Ctrl");
      if (peak.shift) parts.push("Shift");
      if (peak.alt) parts.push("Alt");
      if (peak.meta) parts.push("Win");
      parts.push(k);
      commit(parts.join("+"));
    };

    const onKeyUp = (e) => {
      if (nonModPressed) return;
      const stillHeld = e.ctrlKey || e.shiftKey || e.altKey || e.metaKey;
      if (stillHeld) return;
      const count =
        (peak.ctrl ? 1 : 0) +
        (peak.shift ? 1 : 0) +
        (peak.alt ? 1 : 0) +
        (peak.meta ? 1 : 0);
      if (count < 2) {
        reset();
        return;
      }
      const parts = [];
      if (peak.ctrl) parts.push("Ctrl");
      if (peak.shift) parts.push("Shift");
      if (peak.alt) parts.push("Alt");
      if (peak.meta) parts.push("Win");
      commit(parts.join("+"));
    };

    window.addEventListener("keydown", onKeyDown, true);
    window.addEventListener("keyup", onKeyUp, true);
    return () => {
      window.removeEventListener("keydown", onKeyDown, true);
      window.removeEventListener("keyup", onKeyUp, true);
    };
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
                    onClick={(e) => {
                      e.preventDefault();
                      setCaptureError("");
                      setCapturing(true);
                    }}
                  >
                    {capturing ? "Press keys…" : "Record"}
                  </button>
                  {capturing && captureError && (
                    <div className="onb-hotkey-error">{captureError}</div>
                  )}
                  {capturing && !captureError && (
                    <div className="onb-hotkey-hint">
                      {IS_LINUX
                        ? "Include a regular key (letter, Space, F-key) — modifier-only chords can't be bound on Wayland."
                        : "Modifier-only chords (Ctrl+Win, Alt+Win) work too — release to confirm."}
                    </div>
                  )}
                </div>
              )}
            </label>
          ))}

          <div className="onb-conflict-hint">
            Already using another dictation app on <code>{formatComboForDisplay("Ctrl+Win")}</code>? Pick a different combo
            above and the conflict goes away.
          </div>
        </div>

        <div className="onb-test-pane">
          <div className="onb-test-header">
            <div className="onb-test-eyebrow">Try it now</div>
            <div className="onb-test-instructions">
              Press and hold all of these keys together, then read the sample line aloud:
            </div>
            <ChordDisplay parts={requiredParts} pressedKeys={pressedKeys} />
            <div className="onb-sample-line">"{SAMPLE_LINE}"</div>
          </div>

          <HoldIndicator
            state={hotkeyState}
            hotkey={formatComboForDisplay(activeHotkey)}
            errorMsg={errorMsg}
            requiredParts={requiredParts}
            pressedKeys={pressedKeys}
          />

          <textarea
            ref={textareaRef}
            className="onb-textarea"
            placeholder="Hold your hotkey, speak, and the transcript will land here…"
            onChange={(e) => setTranscript(e.target.value)}
            spellCheck={false}
          />

          <div className="onb-test-foot">
            {transcript.trim().length > 0 && (
              <span className="onb-ok">✓ Dictation works — that's your transcript.</span>
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

function StepDone({ onFinish, hotkey, telemetryEnabled, onToggleTelemetry }) {
  return (
    <div className="onb-page-inner onb-done">
      <div className="onb-done-check">✓</div>
      <h2>You're all set.</h2>
      <p className="onb-lead">
        {IS_ANDROID ? (
          <>Tap the floating bubble in any app — a chat, your notes, a browser — speak, and Bulbul types what you said.</>
        ) : (
          <>Press <code>{formatComboForDisplay(hotkey)}</code> anywhere — in your browser, in Word, in a terminal — speak,
          and Bulbul will type what you said.</>
        )}
      </p>
      <div className="onb-tour-grid">
        <div className="onb-tour-card">
          <div className="onb-tour-title">Transform selections</div>
          <p>
            {IS_ANDROID
              ? "Select text in any app and tap Bulbul in the popup toolbar to polish, formalize, or rephrase it in place."
              : <>Select text anywhere and press <code>{displayPart(IS_MAC ? "Win" : "Alt")} + 1…6</code> to polish, formalize, or rephrase it in place.</>}
          </p>
        </div>
        <div className="onb-tour-card">
          <div className="onb-tour-title">{IS_ANDROID ? "Your words, spelled right" : "Stays out of your way"}</div>
          <p>
            {IS_ANDROID
              ? "Add names, brands, and jargon to your Dictionary, and save frequent phrases as Snippets — both apply automatically."
              : "Close the window to send Bulbul to the tray. Click the tray icon to bring it back."}
          </p>
        </div>
        <div className="onb-tour-card">
          <div className="onb-tour-title">Tune everything</div>
          <p>
            {IS_ANDROID
              ? "Change the bubble's size and opacity, cleanup mode, and theme in Settings — anytime."
              : "Change hotkeys, model, theme, mic in Settings — anytime."}
          </p>
        </div>
      </div>

      <label className="onb-telemetry-row">
        <span className={`toggle ${telemetryEnabled ? "on" : ""}`}>
          <input
            type="checkbox"
            checked={telemetryEnabled}
            onChange={(e) => onToggleTelemetry(e.target.checked)}
          />
          <span className="toggle-thumb" />
        </span>
        <span className="onb-telemetry-text">
          <strong>Anonymous usage stats are on.</strong>
          <span className="onb-muted small">
            Bulbul is solo-built — counts and error categories help me know what to fix. Never your transcripts, audio, dictionary, or which app you're typing into. Flip this off if you'd rather not share — you can change it anytime in Settings → Privacy.
          </span>
        </span>
      </label>

      <div className="onb-actions onb-actions-center">
        <button className="onb-btn primary" onClick={onFinish}>Open Bulbul →</button>
      </div>
    </div>
  );
}

/// Animated press-and-hold feedback for the wizard's hotkey test. Five
/// visual states; each combines an icon, a primary line, and a coaching
/// subline. Driven by the bulbul-status event stream (see the useEffect
/// in StepHotkey), so the UI mirrors what the production app does.
///
/// While idle we ALSO look at requiredParts vs pressedKeys to coach the
/// user through assembling the chord — e.g. "Now also hold Win" once
/// they've pressed Ctrl. The bulbul-status "listening" event only fires
/// once the OS-level global shortcut completes; this partial state lives
/// purely in the wizard.
function HoldIndicator({ state, hotkey, errorMsg, requiredParts, pressedKeys }) {
  const isListening = state === "listening";
  const isProcessing = state === "processing";
  const isDone = state === "done";
  const isTooShort = state === "too_short";
  const isSilent = state === "silent";
  const isError = state === "error";

  let title;
  let subtitle;
  switch (state) {
    case "listening":
      title = "Listening — keep holding";
      subtitle = "Release when you finish reading the sentence.";
      break;
    case "processing":
      title = "Transcribing…";
      subtitle = "One quick round-trip to Groq, then your text lands below.";
      break;
    case "done":
      title = "Got it!";
      subtitle = "Your transcript is in the box below. Try once more if you like.";
      break;
    case "too_short":
      title = "Released too early";
      subtitle = `Hold ${hotkey} for at least half a second before releasing. The clip was too short to transcribe.`;
      break;
    case "silent":
      title = "Couldn't hear you";
      subtitle = "Try speaking a bit louder, or check your mic is on the right input.";
      break;
    case "error":
      title = "Something went wrong";
      subtitle = errorMsg || "Look at the dashboard's overlay for details, or try again.";
      break;
    case "idle":
    default: {
      const parts = requiredParts || [];
      const heldSet = pressedKeys || new Set();
      const held = parts.filter((k) => heldSet.has(k));
      const missing = parts.filter((k) => !heldSet.has(k));
      if (parts.length === 0) {
        title = "Pick a hotkey on the left to test it";
        subtitle = "Choose one of the presets, or record a custom combo.";
      } else if (held.length === 0) {
        title = `Hold ${hotkey} to start`;
        subtitle = "Each key on the right lights up as you press it.";
      } else if (missing.length > 0) {
        title = `Now also hold ${missing.map(prettyKeyName).join(" + ")}`;
        subtitle = `Keep ${held.map(prettyKeyName).join(" + ")} pressed.`;
      } else {
        title = "Holding — start speaking";
        subtitle = "Read the sample line aloud. Release the keys when you're done.";
      }
      break;
    }
  }

  return (
    <div className={`onb-hold-indicator state-${state}`} role="status" aria-live="polite">
      <div className="onb-hold-visual">
        {isListening && (
          <>
            <div className="onb-hold-mic">
              <MicIcon active />
              <div className="onb-hold-pulse" aria-hidden />
            </div>
            <div className="onb-hold-waveform" aria-hidden>
              <span /><span /><span /><span /><span />
            </div>
          </>
        )}
        {isProcessing && (
          <div className="onb-hold-spinner" aria-hidden />
        )}
        {isDone && (
          <div className="onb-hold-check" aria-hidden>✓</div>
        )}
        {(isTooShort || isSilent) && (
          <div className="onb-hold-warn" aria-hidden>!</div>
        )}
        {isError && (
          <div className="onb-hold-error" aria-hidden>✕</div>
        )}
        {state === "idle" && (
          <div className="onb-hold-mic idle">
            <MicIcon />
          </div>
        )}
      </div>
      <div className="onb-hold-meta">
        <div className="onb-hold-title">{title}</div>
        <div className="onb-hold-sub">{subtitle}</div>
      </div>
    </div>
  );
}

function MicIcon({ active }) {
  return (
    <svg
      viewBox="0 0 24 24"
      width="22"
      height="22"
      fill="none"
      stroke={active ? "currentColor" : "currentColor"}
      strokeWidth="2"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
    >
      <rect x="9" y="2" width="6" height="11" rx="3" />
      <path d="M5 11a7 7 0 0 0 14 0" />
      <line x1="12" y1="18" x2="12" y2="22" />
      <line x1="8" y1="22" x2="16" y2="22" />
    </svg>
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

const MOD_ORDER = ["Ctrl", "Shift", "Alt", "Win"];

// Canonicalise modifier order for display (Ctrl → Shift → Alt → Win → key).
// Same string the backend would have produced via hotkey.rs::format_combo,
// independent of how the combo happens to be stored in config. On Mac,
// modifier parts are rendered as their canonical glyphs (⌃ ⌥ ⇧ ⌘) so
// every surface showing the active hotkey matches what the rest of the
// OS uses to describe key combinations.
function formatComboForDisplay(combo) {
  if (!combo) return "—";
  return parseChordParts(combo).map(displayPart).join(" + ");
}

// Split a combo string into its ordered parts (modifiers first in
// canonical order, then the trigger key). Returns [] for empty / invalid
// strings so callers can render an empty state cleanly.
function parseChordParts(combo) {
  if (!combo) return [];
  const parts = combo.split("+").map((p) => p.trim()).filter(Boolean);
  const mods = parts
    .filter((p) => MOD_ORDER.includes(p))
    .sort((a, b) => MOD_ORDER.indexOf(a) - MOD_ORDER.indexOf(b));
  const keys = parts.filter((p) => !MOD_ORDER.includes(p));
  return [...mods, ...keys];
}

// Map a browser keydown / keyup event to the canonical key name we use
// in combo strings ("Ctrl", "Win", "Space", "P"…). Returns null if the
// key isn't one we represent in any combo we'd accept.
function keyEventToName(e) {
  if (e.key === "Control") return "Ctrl";
  if (e.key === "Shift") return "Shift";
  if (e.key === "Alt") return "Alt";
  if (e.key === "Meta" || e.key === "OS") return "Win";
  if (!e.code) return null;
  if (e.code === "Space") return "Space";
  if (e.code === "Tab") return "Tab";
  if (e.code === "Enter" || e.code === "NumpadEnter") return "Enter";
  if (e.code === "Escape") return "Escape";
  if (e.code === "Backspace") return "Backspace";
  if (e.code.startsWith("Key")) return e.code.slice(3);
  if (e.code.startsWith("Digit")) return e.code.slice(5);
  if (/^F\d+$/.test(e.code)) return e.code;
  return null;
}

// "Win" → platform-appropriate name (Windows / Command / Super) in
// coaching text where "Now also hold Win" reads awkwardly.
// Everything else stays as-is.
function prettyKeyName(part) {
  return part === "Win" ? META_KEY_NAME : part;
}

// Renders the active combo as a row of pressable keycaps. Each cap lights
// up the instant its key is held inside the wizard window, so the user
// sees the chord assembling key-by-key instead of having to read text.
function ChordDisplay({ parts, pressedKeys }) {
  if (!parts || parts.length === 0) {
    return (
      <div className="onb-chord onb-chord-empty">
        Pick a hotkey on the left to see its keys here.
      </div>
    );
  }
  return (
    <div className="onb-chord" role="group" aria-label="Hotkey keys">
      {parts.map((part, i) => (
        <span className="onb-chord-cell" key={`${part}:${i}`}>
          {i > 0 && <span className="onb-chord-plus" aria-hidden>+</span>}
          <KeyCap part={part} pressed={pressedKeys ? pressedKeys.has(part) : false} />
        </span>
      ))}
    </div>
  );
}

// On macOS, render modifier names as their canonical glyphs so the
// hotkey display matches what users see everywhere else on the OS
// (⌃ ⌥ ⇧ ⌘). Trigger keys (letters, digits, Space, F-keys) keep their
// text form on every platform. The underlying combo string stored in
// config is unchanged — only the visual representation differs.
const MAC_MOD_GLYPH = {
  Ctrl: "⌃",
  Control: "⌃",
  Shift: "⇧",
  Alt: "⌥",
  Option: "⌥",
  Win: "⌘",
  Cmd: "⌘",
  Meta: "⌘",
  Super: "⌘",
};

function displayPart(part) {
  if (IS_MAC && MAC_MOD_GLYPH[part]) {
    return MAC_MOD_GLYPH[part];
  }
  return part;
}

function KeyCap({ part, pressed }) {
  const display = displayPart(part);
  const isGlyph = display !== part && display.length === 1;
  // Glyphs are single-char and look better as the narrow keycap; long
  // text labels ("Ctrl") get the wide cap.
  const wide = !isGlyph && display.length > 1;
  const extraWide = display === "Space";
  return (
    <kbd
      className={`onb-keycap ${pressed ? "pressed" : ""} ${wide ? "wide" : ""} ${extraWide ? "extra-wide" : ""}`}
      aria-pressed={pressed}
      aria-label={part}
    >
      {display}
    </kbd>
  );
}

// Mirror of SettingsView.jsx's domKeyToName. Kept in sync because the
// onboarding wizard records hotkeys before the user has access to the
// Settings UI. Any name returned here must round-trip through the
// backend's normalize_key_name in hotkey.rs.
function domKeyToName(code) {
  if (!code) return null;
  if (code.startsWith("Key")) return code.slice(3);
  if (code.startsWith("Digit")) return code.slice(5);
  if (/^F\d+$/.test(code)) return code;
  switch (code) {
    case "Space": return "Space";
    case "Tab": return "Tab";
    case "Enter": return "Enter";
    case "Escape": return "Escape";
    case "Backspace": return "Backspace";
    case "ArrowUp": return "Up";
    case "ArrowDown": return "Down";
    case "ArrowLeft": return "Left";
    case "ArrowRight": return "Right";
    case "Insert": return "Insert";
    case "Delete": return "Delete";
    case "Home": return "Home";
    case "End": return "End";
    case "PageUp": return "PageUp";
    case "PageDown": return "PageDown";
    case "Semicolon": return ";";
    case "Quote": return "'";
    case "Comma": return ",";
    case "Period": return ".";
    case "Slash": return "/";
    case "Backslash": return "\\";
    case "BracketLeft": return "[";
    case "BracketRight": return "]";
    case "Minus": return "-";
    case "Equal": return "=";
    case "Backquote": return "`";
    default: return null;
  }
}
