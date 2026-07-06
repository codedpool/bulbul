import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { applyTheme } from "../theme.js";
import Combobox from "../components/Combobox.jsx";
import { AUTOSTART_LABEL, IS_ANDROID, IS_MAC, RELAUNCH_HINT, THEME_FOLLOW_HINT } from "../platform.js";

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
  { code: "auto", label: "Auto-detect (English-leaning)" },
  { code: "en", label: "English" },
  { code: "hi", label: "Hindi / Hinglish" },
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

const CATEGORIES = [
  { id: "general", label: "General" },
  { id: "account", label: "Account" },
  { id: "hotkeys", label: "Hotkeys" },
  { id: "personalization", label: "Personalization" },
  { id: "startup", label: "Startup" },
  { id: "privacy", label: "Privacy" },
  { id: "about", label: "About" },
];

// Android settings sections (drill-down list). Each row's hint previews what's
// inside so the list reads as a menu, not a wall of controls.
const M_SECTIONS = [
  { id: "general", label: "General", hint: "Cleanup mode, language, appearance" },
  { id: "account", label: "Account", hint: "Your Groq API key" },
  { id: "overlay", label: "Overlay", hint: "Dictation bubble opacity and size" },
  { id: "personalization", label: "Personalization", hint: "Your name and cleanup preferences" },
  { id: "privacy", label: "Privacy", hint: "Anonymous usage stats" },
  { id: "about", label: "About", hint: "Updates, help, and version" },
];

/**
 * Settings popup. Mount it once near the root; render is conditional
 * on `open`. The popup owns its own internal category nav (left) +
 * content pane (right). All the actual settings are split across
 * <Pane*> components below.
 */
export default function SettingsView({
  open,
  onClose,
  config,
  updateConfig,
  autostart,
  onAutostartChange,
  onHideTrayChange,
  // Android drill-down nav is lifted to App so the hardware-back handler can
  // step through it. null = the section list; a section id = its detail.
  section: mSection = null,
  onSectionChange: setMSection = () => {},
}) {
  const [active, setActive] = useState("general");
  const [draftKey, setDraftKey] = useState(config?.groq_api_key || "");
  const [keyState, setKeyState] = useState("idle");
  const [keyError, setKeyError] = useState("");
  const [recordingHotkeyFor, setRecordingHotkeyFor] = useState(null);
  const [recordingError, setRecordingError] = useState("");
  const [updateState, setUpdateState] = useState({ state: "idle", message: "" });
  const paneRef = useRef(null);

  // Keep the draft key in sync if the config changes externally (e.g.,
  // the user just finished onboarding and the api key is now set).
  useEffect(() => {
    if (open) setDraftKey(config?.groq_api_key || "");
  }, [open, config?.groq_api_key]);

  // Reset the scroll position when the user switches categories so the
  // new content always starts from the top.
  useEffect(() => {
    if (paneRef.current) paneRef.current.scrollTop = 0;
  }, [active]);

  // ESC closes the popup. Capture phase + stopImmediatePropagation so
  // the global "ESC hides the window" handler in App.jsx doesn't fire
  // through. Skip when the user is recording a hotkey — that flow has
  // its own ESC-to-cancel handler, also in capture phase.
  useEffect(() => {
    if (!open) return;
    function onKey(e) {
      if (e.key !== "Escape") return;
      if (recordingHotkeyFor) return;
      e.stopImmediatePropagation();
      e.preventDefault();
      onClose?.();
    }
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, onClose, recordingHotkeyFor]);

  // Hotkey recording. Captures any key combo (incl. modifier-only chords
  // like Ctrl+Win) and writes it to either hotkey or polish_hotkey based
  // on which row called us.
  //
  // State machine, why it's not "just read e.code on keydown":
  //   * Modifier-only chords (Ctrl+Win, Alt+Win) have NO non-modifier
  //     key. To record one we have to watch the sequence — track which
  //     modifiers were held at peak, then commit on the final release
  //     IF the user never pressed a non-modifier in between.
  //   * Unrecognized keys (PrintScreen, ScrollLock, Eject, etc.) used
  //     to silently abort the recording, leaving the user thinking
  //     the feature was broken. Now we keep recording and surface the
  //     unsupported-key message inline.
  //   * Accidental single-modifier taps (the user hits Ctrl, thinks,
  //     then lifts) reset the peak instead of committing — committing
  //     a one-modifier chord would register a hotkey that triggers on
  //     every plain Ctrl press, which is unusable.
  useEffect(() => {
    if (!recordingHotkeyFor) return;
    setRecordingError("");
    // Peak modifier set (high-water mark; never decreases until the
    // sequence resets after a stray single-mod tap).
    let peak = { ctrl: false, shift: false, alt: false, meta: false };
    let nonModPressed = false;

    const reset = () => {
      peak = { ctrl: false, shift: false, alt: false, meta: false };
      nonModPressed = false;
    };

    const commit = (combo) => {
      const field = recordingHotkeyFor === "polish" ? "polish_hotkey" : "hotkey";
      updateConfig({ ...config, [field]: combo });
      setRecordingHotkeyFor(null);
      setRecordingError("");
    };

    const onKeyDown = (e) => {
      if (e.key === "Escape") {
        e.stopImmediatePropagation();
        e.preventDefault();
        setRecordingHotkeyFor(null);
        setRecordingError("");
        return;
      }
      e.preventDefault();
      e.stopImmediatePropagation();

      // Update high-water mark with the modifier state seen during
      // this event. Modifier keys themselves also set their flag on
      // e.<mod>Key during their own keydown.
      peak = {
        ctrl: e.ctrlKey || peak.ctrl,
        shift: e.shiftKey || peak.shift,
        alt: e.altKey || peak.alt,
        meta: e.metaKey || peak.meta,
      };

      if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) {
        // Pure modifier press — wait for either a non-modifier press
        // (regular combo) or the final release (modifier-only chord).
        return;
      }

      nonModPressed = true;
      const k = domKeyToName(e.code);
      if (!k) {
        setRecordingError(
          `"${e.key || e.code}" isn't supported as a hotkey. Try a letter, digit, function key, arrow, or punctuation.`,
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
      // If a non-modifier was pressed during this sequence, the combo
      // was already committed on keydown — ignore the trailing release.
      if (nonModPressed) return;
      // Only meaningful when all modifiers have just been released.
      const stillHeld = e.ctrlKey || e.shiftKey || e.altKey || e.metaKey;
      if (stillHeld) return;
      const count =
        (peak.ctrl ? 1 : 0) +
        (peak.shift ? 1 : 0) +
        (peak.alt ? 1 : 0) +
        (peak.meta ? 1 : 0);
      if (count < 2) {
        // Stray single-modifier tap — treat as noise and rearm so the
        // user can keep trying without re-clicking "Change".
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
  }, [recordingHotkeyFor, config, updateConfig]);

  if (!open || !config) return null;

  const hasKey = config.groq_api_key && config.groq_api_key.trim().length > 0;

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

  // Android: Settings is a full-screen page (not a centered popup) that opens
  // to a LIST of sections; tapping one drills into just that section's
  // controls, so the user isn't hit with every option at once. Hotkeys
  // becomes Overlay (opacity + size); Startup is dropped (autostart/tray/
  // open-on-launch are all desktop-only — Android restarts the accessibility
  // service on boot on its own).
  if (IS_ANDROID) {
    const current = M_SECTIONS.find((s) => s.id === mSection) || null;

    const renderPane = (id) => {
      switch (id) {
        case "general":
          return <PaneGeneral config={config} updateConfig={updateConfig} />;
        case "account":
          return (
            <PaneAccount
              hasKey={hasKey}
              draftKey={draftKey}
              setDraftKey={setDraftKey}
              saveKey={saveKey}
              keyState={keyState}
              keyError={keyError}
              setKeyState={setKeyState}
            />
          );
        case "overlay":
          return <PaneOverlay config={config} updateConfig={updateConfig} />;
        case "personalization":
          return <PanePersonalization config={config} updateConfig={updateConfig} />;
        case "privacy":
          return <PanePrivacy config={config} updateConfig={updateConfig} />;
        case "about":
          return (
            <PaneAbout
              checkUpdates={checkUpdates}
              updateState={updateState}
              onResetSetup={() => updateConfig({ ...config, onboarding_completed: false })}
            />
          );
        default:
          return null;
      }
    };

    return createPortal(
      <div className="m-settings-page" role="dialog" aria-modal="true" aria-label="Settings">
        <header className="m-settings-head">
          <button
            type="button"
            className="m-icon-btn"
            onClick={() => (current ? setMSection(null) : onClose())}
            aria-label="Back"
          >
            <BackIcon />
          </button>
          <span className="m-settings-title">{current ? current.label : "Settings"}</span>
          <span className="m-settings-head-spacer" />
        </header>
        {current ? (
          <div className="m-settings-scroll">
            <div className="m-settings-section-body">{renderPane(current.id)}</div>
          </div>
        ) : (
          <div className="m-settings-list">
            {M_SECTIONS.map((s) => (
              <button
                key={s.id}
                type="button"
                className="m-settings-row"
                onClick={() => setMSection(s.id)}
              >
                <span className="m-settings-row-icon"><CategoryIcon id={s.id} /></span>
                <span className="m-settings-row-text">
                  <span className="m-settings-row-label">{s.label}</span>
                  <span className="m-settings-row-hint">{s.hint}</span>
                </span>
                <span className="m-settings-row-chev"><ChevronIcon /></span>
              </button>
            ))}
          </div>
        )}
      </div>,
      document.body,
    );
  }

  return createPortal(
    <div
      className="settings-backdrop"
      onMouseDown={(e) => {
        // Click on the dim area outside the modal closes — but only
        // when the click originated AND released on the backdrop, so a
        // drag-select that ends outside doesn't accidentally close.
        if (e.target === e.currentTarget) onClose?.();
      }}
      role="dialog"
      aria-modal="true"
      aria-label="Settings"
    >
      <div className="settings-modal" onMouseDown={(e) => e.stopPropagation()}>
        <header className="settings-modal-header">
          <h1 className="settings-modal-title">Settings</h1>
          <button
            type="button"
            className="settings-modal-close"
            onClick={onClose}
            aria-label="Close settings"
          >
            <CloseIcon />
          </button>
        </header>
        <div className="settings-modal-body">
          <aside className="settings-modal-nav">
            {CATEGORIES.map((c) => (
              <button
                key={c.id}
                type="button"
                className={`settings-nav-item ${active === c.id ? "active" : ""}`}
                onClick={() => setActive(c.id)}
              >
                <span className="settings-nav-icon"><CategoryIcon id={c.id} /></span>
                <span className="settings-nav-label">{c.label}</span>
              </button>
            ))}
          </aside>
          <main className="settings-modal-pane" ref={paneRef}>
            {active === "general" && (
              <PaneGeneral config={config} updateConfig={updateConfig} />
            )}
            {active === "account" && (
              <PaneAccount
                hasKey={hasKey}
                draftKey={draftKey}
                setDraftKey={setDraftKey}
                saveKey={saveKey}
                keyState={keyState}
                keyError={keyError}
                setKeyState={setKeyState}
              />
            )}
            {active === "hotkeys" && (
              <PaneHotkeys
                config={config}
                recordingHotkeyFor={recordingHotkeyFor}
                setRecordingHotkeyFor={setRecordingHotkeyFor}
                recordingError={recordingError}
                clearRecordingError={() => setRecordingError("")}
              />
            )}
            {active === "personalization" && (
              <PanePersonalization config={config} updateConfig={updateConfig} />
            )}
            {active === "startup" && (
              <PaneStartup
                config={config}
                updateConfig={updateConfig}
                autostart={autostart}
                onAutostartChange={onAutostartChange}
                onHideTrayChange={onHideTrayChange}
              />
            )}
            {active === "privacy" && (
              <PanePrivacy config={config} updateConfig={updateConfig} />
            )}
            {active === "about" && (
              <PaneAbout
                checkUpdates={checkUpdates}
                updateState={updateState}
                onResetSetup={() => updateConfig({ ...config, onboarding_completed: false })}
              />
            )}
          </main>
        </div>
      </div>
    </div>,
    document.body
  );
}

// ─────────────── Panes ───────────────

// Combobox expects {code, label}; MODES uses {value, label} so we
// rename inline. Cheap and avoids changing MODES, which is read by
// other places that key on `value`.
const MODE_OPTIONS = MODES.map((m) => ({ code: m.value, label: m.label }));

function PaneGeneral({ config, updateConfig }) {
  const activeMode = MODES.find((m) => m.value === config.mode) || MODES[1];
  const activeTheme = config.theme || "light";
  return (
    <>
      <Row title="Cleanup mode" hint={activeMode.hint}>
        <Combobox
          value={config.mode || "clean"}
          options={MODE_OPTIONS}
          onChange={(v) => updateConfig({ ...config, mode: v })}
          width={180}
          ariaLabel="Cleanup mode"
        />
      </Row>
      <Row
        title="Language"
        hint="Pick a specific language if you dictate in anything other than English. Auto-detect is solid for English but occasionally flips Hindi audio to Urdu script. Hindi/Hinglish handles mixed English+Hindi automatically."
      >
        <Combobox
          value={config.language || "auto"}
          options={LANGUAGES}
          onChange={(v) => updateConfig({ ...config, language: v })}
          width={220}
          ariaLabel="Language"
        />
      </Row>
      <Row title="Appearance" hint={THEME_FOLLOW_HINT}>
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
      </Row>
    </>
  );
}

function PaneAccount({ hasKey, draftKey, setDraftKey, saveKey, keyState, keyError, setKeyState }) {
  return (
    <>
      <Row
        title="Groq API key"
        hint={hasKey
          ? "Connected. Pasting a new key updates it."
          : "Bulbul talks to Groq using your own key. Grab a free one from console.groq.com/keys."}
        stack
      >
        <div className="settings-key-row">
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
        {keyState === "valid" && <p className="ok small">Key validated and saved.</p>}
        {keyState === "invalid" && <p className="err small">{keyError}</p>}
        {!hasKey && (
          <p className="muted small">
            <a
              href="https://console.groq.com/keys"
              onClick={(e) => { e.preventDefault(); openUrl("https://console.groq.com/keys"); }}
            >
              Get a free key from console.groq.com →
            </a>
          </p>
        )}
      </Row>
    </>
  );
}

function PaneHotkeys({
  config,
  recordingHotkeyFor,
  setRecordingHotkeyFor,
  recordingError,
  clearRecordingError,
}) {
  const startRecording = (which) => {
    clearRecordingError?.();
    setRecordingHotkeyFor(which);
  };
  const cancelRecording = () => {
    clearRecordingError?.();
    setRecordingHotkeyFor(null);
  };
  return (
    <>
      <Row
        title="Dictation"
        hint="Hold to record. Release to transcribe with your current cleanup mode."
        stack
      >
        <HotkeyControl
          combo={config.hotkey}
          isRecording={recordingHotkeyFor === "dictation"}
          error={recordingHotkeyFor === "dictation" ? recordingError : ""}
          onStart={() => startRecording("dictation")}
          onCancel={cancelRecording}
        />
      </Row>
      <Row
        title="Polish dictation"
        hint="Hold to record. Release to transcribe with Polished mode forced — rewrites the transcript for clarity before pasting."
        stack
      >
        <HotkeyControl
          combo={config.polish_hotkey || "Shift+Alt+P"}
          isRecording={recordingHotkeyFor === "polish"}
          error={recordingHotkeyFor === "polish" ? recordingError : ""}
          onStart={() => startRecording("polish")}
          onCancel={cancelRecording}
        />
      </Row>
      <p className="muted small settings-note">
        Transform shortcuts ({IS_MAC ? <><kbd>⌘1</kbd>…<kbd>⌘6</kbd></> : <><kbd>Alt+1</kbd>…<kbd>Alt+6</kbd></>}) for rewriting selected text live on the Transforms page.
      </p>
    </>
  );
}

// Android-only. The floating dictation bubble's look: how see-through it is
// and how big. Both persist to config.json; the Kotlin foreground service
// reads them the next time it draws the bubble.
function PaneOverlay({ config, updateConfig }) {
  const opacityPct = Math.round((config.overlay_opacity ?? 0.65) * 100);
  const size = config.overlay_size ?? 52;
  return (
    <>
      <Row
        title="Bubble opacity"
        hint="How see-through the floating dictation bubble is."
        stack
      >
        <div className="settings-slider">
          <input
            type="range"
            min="30"
            max="100"
            step="5"
            value={opacityPct}
            onChange={(e) =>
              updateConfig({ ...config, overlay_opacity: Number(e.target.value) / 100 })
            }
            aria-label="Bubble opacity"
          />
          <span className="settings-slider-val">{opacityPct}%</span>
        </div>
      </Row>
      <Row title="Bubble size" hint="Diameter of the floating dictation bubble." stack>
        <div className="settings-slider">
          <input
            type="range"
            min="44"
            max="96"
            step="4"
            value={size}
            onChange={(e) => updateConfig({ ...config, overlay_size: Number(e.target.value) })}
            aria-label="Bubble size"
          />
          <span className="settings-slider-val">{size}px</span>
        </div>
      </Row>
      <p className="muted small settings-note">
        Changes take effect the next time the bubble appears.
      </p>
    </>
  );
}

function PanePersonalization({ config, updateConfig }) {
  return (
    <>
      <Row
        title="Your name"
        hint="Used to greet you on the home page and sign Compose drafts. Stays on your machine — never sent anywhere."
      >
        <input
          type="text"
          className="settings-text-input"
          value={config.display_name || ""}
          placeholder="First name"
          maxLength={48}
          spellCheck={false}
          autoComplete="off"
          onChange={(e) => updateConfig({ ...config, display_name: e.target.value })}
          onBlur={(e) => updateConfig({ ...config, display_name: e.target.value.trim() })}
        />
      </Row>
      <ToggleRow
        title="Personalize cleanup from past dictations"
        hint="Show the model recent examples from the same app. Adds ~150 tokens per dictation."
        checked={config.personalize_cleanup !== false}
        onChange={(v) => updateConfig({ ...config, personalize_cleanup: v })}
      />
      <ToggleRow
        title="Learn from my corrections"
        hint="When you edit what Bulbul typed, it remembers and applies the same fix next time. Password fields are always skipped."
        checked={config.learn_corrections !== false}
        onChange={(v) => updateConfig({ ...config, learn_corrections: v })}
      />
    </>
  );
}

function PaneStartup({ config, updateConfig, autostart, onAutostartChange, onHideTrayChange }) {
  return (
    <>
      <ToggleRow
        title={AUTOSTART_LABEL}
        hint="Boots silently in the tray on login."
        checked={autostart}
        onChange={onAutostartChange}
      />
      <ToggleRow
        title="Open this window when Bulbul starts"
        hint="Off = land straight in the tray."
        checked={!!config.open_dashboard_on_launch}
        onChange={(v) => updateConfig({ ...config, open_dashboard_on_launch: v })}
      />
      <ToggleRow
        title="Hide tray icon"
        hint={`Bulbul keeps running in the background and your hotkey still works. The pill only appears while you're dictating. ${RELAUNCH_HINT}`}
        checked={!!config.hide_tray}
        onChange={(v) => onHideTrayChange?.(v)}
      />
    </>
  );
}

function PanePrivacy({ config, updateConfig }) {
  return (
    <ToggleRow
      title="Share anonymous usage stats"
      hint="Counts, durations, error categories, mode/language. Never your transcripts, audio, dictionary, or which app you're typing into. On by default — flip off if you'd rather not share."
      checked={!!config.telemetry_enabled}
      onChange={(v) => updateConfig({ ...config, telemetry_enabled: v })}
    />
  );
}

function PaneAbout({ checkUpdates, updateState, onResetSetup }) {
  const [copied, setCopied] = useState(false);
  const copyEmail = async () => {
    try {
      await navigator.clipboard.writeText("support@bulbultypes.xyz");
      setCopied(true);
      setTimeout(() => setCopied(false), 1600);
    } catch {}
  };
  return (
    <>
      <Row title="Updates" hint="Bulbul checks GitHub releases on a schedule." stack>
        <div className="row">
          <button onClick={checkUpdates} disabled={updateState.state === "checking"}>
            {updateState.state === "checking" ? "Checking…" : "Check for updates"}
          </button>
        </div>
        {updateState.state === "available" && (
          <p className="ok small">Update available: {updateState.message}</p>
        )}
        {updateState.state === "uptodate" && (
          <p className="muted small">{updateState.message}</p>
        )}
        {updateState.state === "error" && (
          <p className="err small">{updateState.message}</p>
        )}
      </Row>
      <Row title="Setup wizard" hint="Re-do the first-run flow." stack>
        <div className="row">
          <button onClick={onResetSetup}>Re-run setup wizard</button>
        </div>
      </Row>
      <Row
        title="Help & support"
        hint="Questions, bugs, or feedback — every message is read."
        stack
      >
        <div
          className="row"
          style={{ display: "flex", gap: 8, flexWrap: "wrap", alignItems: "center" }}
        >
          <code style={{ userSelect: "text" }}>support@bulbultypes.xyz</code>
          <button onClick={copyEmail}>
            {copied ? "Copied" : "Copy email"}
          </button>
          <button onClick={() => openUrl("https://github.com/codedpool/bulbul/issues/new")}>
            Report on GitHub
          </button>
          <button onClick={() => openUrl("https://bulbultypes.xyz")}>
            Website
          </button>
        </div>
      </Row>
      <p className="muted small settings-note">
        Bulbul v1.0.1 · MIT-licensed · made with care · <a
          href="#"
          onClick={(e) => { e.preventDefault(); openUrl("https://bulbultypes.xyz"); }}
        >bulbultypes.xyz</a>
      </p>
    </>
  );
}

// ─────────────── Layout helpers ───────────────

/// One row inside a pane. Title + hint on the left, the control on
/// the right. `stack` flips it to two-row layout (control under the
/// hint) for wider controls like the API key field.
function Row({ title, hint, children, stack }) {
  return (
    <div className={`setting-row ${stack ? "setting-row-stack" : ""}`}>
      <div className="setting-row-meta">
        <div className="setting-row-title">{title}</div>
        {hint && <div className="setting-row-hint">{hint}</div>}
      </div>
      <div className="setting-row-control">{children}</div>
    </div>
  );
}

function ToggleRow({ title, hint, checked, onChange }) {
  return (
    <Row title={title} hint={hint}>
      <label className={`toggle ${checked ? "on" : ""}`}>
        <input
          type="checkbox"
          checked={checked}
          onChange={(e) => onChange(e.target.checked)}
        />
        <span className="toggle-thumb" />
      </label>
    </Row>
  );
}

function HotkeyControl({ combo, isRecording, error, onStart, onCancel }) {
  return (
    <div className="row hotkey-row">
      <div className="hotkey-display">
        {isRecording ? (
          error ? (
            <span className="hotkey-error">{error}</span>
          ) : (
            <span className="muted">
              Press a key combo… Modifier-only chords (Ctrl+Win, Alt+Win)
              also work — just release the keys. Esc to cancel.
            </span>
          )
        ) : (
          formatHotkey(combo).map((part, i) => (
            <span key={i}>
              {i > 0 && <span className="plus">+</span>}
              <kbd>{part}</kbd>
            </span>
          ))
        )}
      </div>
      <button onClick={isRecording ? onCancel : onStart}>
        {isRecording ? "Cancel" : "Change"}
      </button>
    </div>
  );
}

// ─────────────── Icons ───────────────

function CloseIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <line x1="6" y1="6" x2="18" y2="18" />
      <line x1="18" y1="6" x2="6" y2="18" />
    </svg>
  );
}

function BackIcon() {
  return (
    <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <line x1="19" y1="12" x2="5" y2="12" />
      <polyline points="12 19 5 12 12 5" />
    </svg>
  );
}

function ChevronIcon() {
  return (
    <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <polyline points="9 18 15 12 9 6" />
    </svg>
  );
}

function CategoryIcon({ id }) {
  const props = { width: 16, height: 16, viewBox: "0 0 24 24", fill: "none", stroke: "currentColor", strokeWidth: 2, strokeLinecap: "round", strokeLinejoin: "round", "aria-hidden": true };
  switch (id) {
    case "general":
      return (
        <svg {...props}>
          <circle cx="12" cy="12" r="3" />
          <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09a1.65 1.65 0 0 0-1-1.51 1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09a1.65 1.65 0 0 0 1.51-1 1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
        </svg>
      );
    case "account":
      return (
        <svg {...props}>
          <path d="M21 2l-2 2m-7.61 7.61a5.5 5.5 0 1 1-7.778 7.778 5.5 5.5 0 0 1 7.777-7.777zm0 0L15.5 7.5m0 0l3 3L22 7l-3-3m-3.5 3.5L19 4" />
        </svg>
      );
    case "hotkeys":
      return (
        <svg {...props}>
          <rect x="2" y="6" width="20" height="12" rx="2" />
          <line x1="6" y1="10" x2="6" y2="10" />
          <line x1="10" y1="10" x2="10" y2="10" />
          <line x1="14" y1="10" x2="14" y2="10" />
          <line x1="18" y1="10" x2="18" y2="10" />
          <line x1="7" y1="14" x2="17" y2="14" />
        </svg>
      );
    case "overlay":
      return (
        <svg {...props}>
          <circle cx="12" cy="12" r="9" />
          <circle cx="12" cy="12" r="3" />
        </svg>
      );
    case "personalization":
      return (
        <svg {...props}>
          <path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2" />
          <circle cx="12" cy="7" r="4" />
        </svg>
      );
    case "startup":
      return (
        <svg {...props}>
          <path d="M18.36 6.64a9 9 0 1 1-12.73 0" />
          <line x1="12" y1="2" x2="12" y2="12" />
        </svg>
      );
    case "privacy":
      return (
        <svg {...props}>
          <path d="M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10 8 10z" />
        </svg>
      );
    case "about":
      return (
        <svg {...props}>
          <circle cx="12" cy="12" r="10" />
          <line x1="12" y1="16" x2="12" y2="12" />
          <line x1="12" y1="8" x2="12.01" y2="8" />
        </svg>
      );
    default:
      return <svg {...props}><circle cx="12" cy="12" r="3" /></svg>;
  }
}

// ─────────────── Helpers ───────────────

const MOD_ORDER = ["Ctrl", "Shift", "Alt", "Win"];

function formatHotkey(s) {
  const parts = (s || "").split("+").map((p) => p.trim()).filter(Boolean);
  const mods = parts
    .filter((p) => MOD_ORDER.includes(p))
    .sort((a, b) => MOD_ORDER.indexOf(a) - MOD_ORDER.indexOf(b));
  const keys = parts.filter((p) => !MOD_ORDER.includes(p));
  return [...mods, ...keys];
}

// Map a DOM `KeyboardEvent.code` to the canonical key name that the Rust
// hotkey layer (hotkey.rs: normalize_key_name, key_name_to_code,
// key_name_to_vk) understands. Returning null means "this physical key
// isn't supported as a hotkey" — the recorder surfaces that to the user
// instead of silently aborting.
//
// The names produced here must round-trip through the backend's
// normalize_key_name unchanged, otherwise the saved config drifts
// away from what key_name_to_code can resolve.
function domKeyToName(code) {
  if (!code) return null;
  if (code.startsWith("Key")) return code.slice(3);
  if (code.startsWith("Digit")) return code.slice(5);
  if (/^F\d+$/.test(code)) return code;
  switch (code) {
    case "Space": return "Space";
    case "Enter": return "Enter";
    case "Backspace": return "Backspace";
    case "Tab": return "Tab";
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
