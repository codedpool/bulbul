// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl } from "@tauri-apps/plugin-opener";
import bulbulMark from "../assets/bulbul-mark.png";
import onboardGroq from "../assets/onboard-groq.png";
import onboardLanguage from "../assets/onboard-language.png";
import onboardHero from "../assets/onboard-hero.png";
import onboardMic from "../assets/onboard-mic.png";
import onboardOverlay from "../assets/onboard-overlay.png";
import onboardAccessibility from "../assets/onboard-accessibility.png";
import { applyTheme } from "../theme.js";
import { IS_ANDROID, IS_LINUX, IS_MAC, IS_WINDOWS, META_KEY_NAME } from "../platform.js";
import { useInPageChordFallback } from "../inPageHotkey.js";
import MouseButtonRecorder, { mouseButtonLabel } from "../components/MouseButtonRecorder.jsx";
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

// The "how to get a Groq key" walkthrough differs by platform (the mobile UI +
// key-paste flow differ from desktop), so Android gets its own recording while
// desktop keeps the original.
const VIDEO_ID = IS_ANDROID ? "DFdMRX1sPHI" : "lRo4r_b4twI";
const VIDEO_URL = `https://www.youtube.com/watch?v=${VIDEO_ID}`;
const VIDEO_EMBED = `https://www.youtube-nocookie.com/embed/${VIDEO_ID}`;

// Sample line deliberately seeded with "um", "uh", "like" so the cleanup
// pass visibly removes them — the user sees Bulbul not just transcribe but
// clean. If they're on Raw mode every filler stays, which is also useful
// feedback ("ah, that's what Raw mode means").
const SAMPLE_LINE = "Hi Bulbul, um, this is, uh, my first test, and like, it looks great.";

// Mac inserts a one-time Permissions step between Welcome and the API
// key entry. Non-Mac platforms skip it (Windows has no permission gate;
// Linux X11 needs none, Linux Wayland prompts via portal on first use).
//
// Android's marketing "welcome" beat and its three system permissions are
// normally handled by the native SetupActivity walker BEFORE this wizard
// ever mounts (see SetupActivity.kt) — but StepWelcome also quietly
// collected the user's display name, which is real functionality (signs
// Compose drafts, greets on the home page), not decoration, so it needs
// its own step now that the native hero owns the marketing beat: "name"
// replaces "welcome" here, not just drops it. Also adds the two steps
// that follow in the redesigned journey: tuning the floating bubble's
// size/opacity, and a quick dictate-test preview.
//
// REPLAY is the exception: "Re-run setup wizard" (Settings ▸ About) only
// flips onboarding_completed back to false — it can't re-trigger the
// native walker (permissions are already granted, and there'd be no real
// system dialog to show), so without this branch a replay would jump
// straight to "name", skipping the four screens the native walker owns
// and making the whole thing feel like two separate half-flows stitched
// together (that's the actual bug report this fixed: "always starts on
// apiKey", "the name step is skipped"). onboarding_ever_completed
// (config.rs) is set once, on the first-ever completion, and never reset
// by a replay — so it's what tells a genuine first run from a replay.
// On a replay, this wizard shows its OWN illustrated recap of the four
// native screens first (StepRecap, reusing the same generated art), so
// the full ten-screen journey plays as one continuous flow with no seam,
// even though only the very first run ever touches native code at all.
function androidStepSequence(config) {
  if (!IS_ANDROID) return null;
  const isReplay = !!config.onboarding_ever_completed;
  const recap = isReplay
    ? ["heroRecap", "micRecap", "overlayRecap", "accessibilityRecap"]
    : [];
  return [...recap, "name", "apiKey", "language", "overlayAdjuster", "dictateTest", "done"];
}

export default function OnboardingWizard({ config, updateConfig, onComplete }) {
  const androidSequence = androidStepSequence(config);
  const STEP_SEQUENCE = androidSequence
    ?? (IS_MAC
      ? ["welcome", "permissions", "apiKey", "language", "hotkey", "mouseMode", "done"]
      : ["welcome", "apiKey", "language", "hotkey", "mouseMode", "done"]);

  // A first run shows 4 screens natively before this wizard ever mounts,
  // so its progress bar continues that same 10-screen count rather than
  // restarting at "1 of 6" (must match TOTAL_ONBOARDING_STEPS in
  // SetupActivity.kt). A replay shows all 10 screens itself — including
  // its own recap of the native four — so it needs no offset at all.
  const isAndroidReplay = IS_ANDROID && !!config.onboarding_ever_completed;
  const GLOBAL_STEP_OFFSET = IS_ANDROID && !isAndroidReplay ? 4 : 0;
  const GLOBAL_TOTAL_STEPS = IS_ANDROID ? 10 : null;

  const [step, setStep] = useState(0);
  const totalSteps = GLOBAL_TOTAL_STEPS ?? STEP_SEQUENCE.length;
  const currentStepName = STEP_SEQUENCE[step];
  const goNext = () => setStep((s) => Math.min(s + 1, STEP_SEQUENCE.length - 1));
  const goBack = () => setStep((s) => Math.max(s - 1, 0));

  // Android hardware/gesture back for the wizard's own step navigation.
  // Without this the wizard never pushes any history, so a system Back
  // press has nothing of ITS OWN to act on — it either no-ops or falls
  // through to the WebView's default, neither of which steps the wizard
  // backward one screen the way the in-app "← Back" link does. This
  // mirrors the exact push-on-advance / go(-n)-on-retreat / guarded-pop
  // pattern App.jsx uses for the Settings overlay, just scoped to this
  // component's own step count via its own refs — the two never run at
  // the same time (this wizard replaces the whole app UI while
  // onboarding_completed is false, so Settings isn't mounted), so sharing
  // the same window.history stack is safe: each effect only reacts to
  // changes in ITS OWN tracked depth.
  const stepDepthRef = useRef(0);
  const ignoreWizardPopRef = useRef(false);

  useEffect(() => {
    if (!IS_ANDROID) return;
    const prev = stepDepthRef.current;
    if (step > prev) {
      for (let i = prev; i < step; i++) window.history.pushState({ onbStep: i + 1 }, "");
    } else if (step < prev) {
      ignoreWizardPopRef.current = true;
      window.history.go(-(prev - step));
    }
    stepDepthRef.current = step;
  }, [step]);

  useEffect(() => {
    if (!IS_ANDROID) return;
    const onPop = () => {
      if (ignoreWizardPopRef.current) {
        ignoreWizardPopRef.current = false;
        return;
      }
      // Browser already consumed one history entry; pre-decrement so the
      // depth-sync effect above sees a balanced stack and doesn't re-push
      // (same reasoning as App.jsx's identical guard).
      stepDepthRef.current = Math.max(0, stepDepthRef.current - 1);
      setStep((s) => Math.max(0, s - 1));
    };
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, []);

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

  // The wizard has no maximize/restore control, so double-clicking the
  // draggable top bar would maximize the window and strand the user with no way
  // back. Disable maximizing while the wizard is shown (and un-maximize if
  // we're already stuck there); restore it when onboarding finishes.
  useEffect(() => {
    // Hard guarantee against the "stuck maximized" trap: the wizard has no
    // restore control, so revert any maximize while it's shown. We also disable
    // maximizing (ideally it never happens) and only the header is a drag
    // region now, so double-clicking the body can't maximize either. Restored
    // on unmount.
    win.setMaximizable(false).catch(() => {});
    const revertIfMaximized = () =>
      win
        .isMaximized()
        .then((m) => {
          if (m) win.unmaximize();
        })
        .catch(() => {});
    revertIfMaximized();
    let unlisten;
    win
      .onResized(revertIfMaximized)
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {});
    return () => {
      win.setMaximizable(true).catch(() => {});
      if (unlisten) unlisten();
    };
  }, []);

  return (
    <div className="onb-shell">
      <header className="onb-top">
        <div className="onb-brand">
          <img src={bulbulMark} alt="" className="onb-brand-mark" aria-hidden />
          <span className="onb-brand-text">bulbul</span>
        </div>
        <div
          className="onb-progress"
          aria-label={`Step ${step + 1 + GLOBAL_STEP_OFFSET} of ${totalSteps}`}
        >
          {Array.from({ length: totalSteps }, (_, i) => {
            // The first GLOBAL_STEP_OFFSET slots represent screens the
            // native walker already showed before this wizard mounted —
            // always "done", since there's no way back into them from here.
            const globalIndex = i - GLOBAL_STEP_OFFSET;
            const state =
              globalIndex < 0 || globalIndex < step ? "done" : globalIndex === step ? "active" : "";
            return <span key={i} className={`onb-dot ${state}`} />;
          })}
        </div>
        <div className="onb-top-right">
          {/* Android's onboarding art is light-theme-only (see the
              .onb-shell CSS override), so a toggle that visibly does
              nothing would just read as broken — dropped here, not just
              hidden. Desktop/Mac never show these images and keep the
              toggle as before. */}
          {!IS_ANDROID && (
            <button
              className="onb-tb-btn"
              onClick={toggleTheme}
              aria-label={resolvedTheme === "dark" ? "Switch to light mode" : "Switch to dark mode"}
              title={resolvedTheme === "dark" ? "Switch to light mode" : "Switch to dark mode"}
            >
              {resolvedTheme === "dark" ? <SunIcon /> : <MoonIcon />}
            </button>
          )}
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
        {currentStepName === "heroRecap" && (
          <StepRecap
            image={onboardHero}
            title="Bulbul is faster than typing"
            onNext={goNext}
          />
        )}
        {currentStepName === "micRecap" && (
          <StepRecap
            image={onboardMic}
            title="Microphone"
            blurb="Already allowed — this is what that screen looked like."
            onBack={goBack}
            onNext={goNext}
          />
        )}
        {currentStepName === "overlayRecap" && (
          <StepRecap
            image={onboardOverlay}
            title="Display over other apps"
            blurb="Already allowed — lets the floating bubble appear above your keyboard in any app."
            onBack={goBack}
            onNext={goNext}
          />
        )}
        {currentStepName === "accessibilityRecap" && (
          <StepRecap
            image={onboardAccessibility}
            title="Accessibility"
            blurb="Already turned on — lets Bulbul paste cleaned-up transcripts into any text field."
            onBack={goBack}
            onNext={goNext}
          />
        )}
        {currentStepName === "name" && (
          <StepName config={config} updateConfig={updateConfig} onBack={goBack} onNext={goNext} />
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
        {currentStepName === "mouseMode" && (
          <StepMouseMode
            config={config}
            updateConfig={updateConfig}
            onBack={goBack}
            onNext={goNext}
          />
        )}
        {currentStepName === "overlayAdjuster" && (
          <StepOverlayAdjuster
            config={config}
            updateConfig={updateConfig}
            onBack={goBack}
            onNext={goNext}
          />
        )}
        {currentStepName === "dictateTest" && (
          <StepDictateTest onBack={goBack} onNext={goNext} />
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

// Android only: the native hero screen (SetupActivity.kt) took over
// StepWelcome's headline beat, but StepWelcome also carried the three
// value-prop facts (own-key privacy, open source, quick setup) AND the
// user's display name — real content, not decoration — so both come
// along here rather than being dropped. Always has Back (goBack no-ops
// harmlessly on a genuine first run, where this is step 0 — same
// footer treatment as every other screen rather than a special case).
function StepName({ config, updateConfig, onBack, onNext }) {
  const [name, setName] = useState(config?.display_name || "");

  function commitAndNext() {
    const trimmed = name.trim();
    if (trimmed !== (config?.display_name || "")) {
      updateConfig({ ...config, display_name: trimmed });
    }
    onNext();
  }

  return (
    <div className="onb-page-inner">
      <header className="onb-step-head">
        <h2>What should I call you?</h2>
        <p className="onb-sub">Optional, stays on your device.</p>
      </header>

      <div className="onb-value-grid onb-value-grid-compact">
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
          <p>API key, language, done. You'll be dictating before your coffee's cold.</p>
        </div>
      </div>

      <div className="onb-name-block">
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
          autoFocus
        />
        <p className="muted small onb-name-hint">
          Used to sign your Compose drafts and greet you on the home page. Never sent anywhere.
        </p>
      </div>

      <div className="onb-actions">
        <button className="onb-btn ghost" onClick={onBack}>← Back</button>
        <button className="onb-btn primary" onClick={commitAndNext}>Continue →</button>
      </div>
    </div>
  );
}

// Android replay only: after a real first run, the native SetupActivity
// never runs again (permissions are already granted — there'd be no real
// system dialog to show), but "Re-run setup wizard" still wants the full
// journey to feel like one continuous flow. This shows the same four
// native illustrations again, entirely inside React, with a single
// Continue since there's nothing left to actually grant. See
// androidStepSequence above and onboarding_ever_completed in config.rs.
function StepRecap({ image, title, blurb, onBack, onNext }) {
  return (
    <div className="onb-page-inner onb-page-inner-bleed">
      <div className="onb-step-body">
        <div className="onb-recap-frame">
          <img src={image} alt="" className="onb-recap-image" />
          <div className="onb-recap-overlay">
            <h2>{title}</h2>
            {blurb && <p className="onb-sub">{blurb}</p>}
          </div>
        </div>
      </div>
      <div className={onBack ? "onb-actions" : "onb-actions onb-actions-center"}>
        {onBack && <button className="onb-btn ghost" onClick={onBack}>← Back</button>}
        <button className="onb-btn primary" onClick={onNext}>Continue →</button>
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
    <div className={`onb-page-inner${IS_ANDROID ? " onb-page-inner-bleed" : ""}`}>
      <div className="onb-step-body">
      {IS_ANDROID ? (
        <div className="onb-hero-frame">
          <img src={onboardGroq} alt="" className="onb-hero-image" aria-hidden />
          <div className="onb-hero-overlay">
            <h2>Paste your Groq API key</h2>
            <p className="onb-sub">
              Need a free key? <a href="#" onClick={(e) => { e.preventDefault(); openUrl("https://console.groq.com/keys"); }}>console.groq.com/keys</a>
            </p>
          </div>
        </div>
      ) : (
        <header className="onb-step-head">
          <h2>Paste your Groq API key</h2>
          <p className="onb-sub">
            Bulbul uses Groq to transcribe and clean up what you say. You need a free key
            from <a href="#" onClick={(e) => { e.preventDefault(); openUrl("https://console.groq.com/keys"); }}>console.groq.com/keys</a>.
          </p>
        </header>
      )}

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

      <p className="onb-groq-privacy">
        Bulbul sends your audio &amp; text to Groq (with your own key) to
        transcribe and clean it. Groq's docs say API data isn't stored or used
        for training by default —{" "}
        <a
          href="#"
          onClick={(e) => {
            e.preventDefault();
            openUrl("https://groq.com/privacy-policy");
          }}
        >
          always check Groq's latest privacy policy
        </a>
        .
      </p>

      <div className="onb-theme-block">
        <div className="onb-theme-label">Appearance</div>
        <div className="segmented onb-theme-seg">
          {THEME_OPTIONS.map((t) => (
            <button
              key={t.value}
              type="button"
              className={`segmented-btn ${(config.theme || "light") === t.value ? "selected" : ""}`}
              onClick={() => {
                // Saved for the main app once setup finishes, but not
                // applied live here — the wizard itself is locked to
                // light on Android (see the .onb-shell CSS override),
                // so previewing dark mid-wizard would just clash with
                // the light-generated illustrations on the surrounding
                // screens.
                if (!IS_ANDROID) applyTheme(t.value);
                updateConfig({ ...config, theme: t.value });
              }}
            >
              {t.label}
            </button>
          ))}
        </div>
      </div>

      <div className="onb-video-block">
        <div className="onb-video-frame">
          <iframe
            src={VIDEO_EMBED}
            title="How to get a Groq API key"
            allow="accelerometer; autoplay; clipboard-write; encrypted-media; gyroscope; picture-in-picture"
            allowFullScreen
          />
        </div>
        <div className="onb-video-side">
          <div className="onb-video-caption">Don't have a key yet?</div>
          <p className="onb-video-desc">
            A 60-second walkthrough for grabbing a free Groq API key.
          </p>
          <button
            className="onb-video-link"
            onClick={() => openUrl(VIDEO_URL)}
            type="button"
          >
            Open on YouTube →
          </button>
        </div>
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
    <div className={`onb-page-inner${IS_ANDROID ? " onb-page-inner-bleed" : ""}`}>
      <div className="onb-step-body">
      {IS_ANDROID ? (
        <>
          <header className="onb-step-head onb-step-head-android">
            <h2>What language do you dictate in?</h2>
            <p className="onb-sub">Mixing English in is fine — Bulbul handles that either way.</p>
          </header>
          <div className="onb-hero-frame onb-hero-frame-plain">
            <img src={onboardLanguage} alt="" className="onb-hero-image" aria-hidden />
          </div>
        </>
      ) : (
        <header className="onb-step-head">
          <h2>What language do you dictate in?</h2>
          <p className="onb-sub">
            Pick the one you use most. Mixing English in is fine — Bulbul handles that for any choice.
          </p>
        </header>
      )}

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

// Android only: live-tunes the same config.overlay_size / overlay_opacity
// fields Settings → Overlay uses (see PaneOverlay in SettingsView.jsx) —
// dragging here previews the actual floating bubble's look immediately,
// via the exact same config keys the running app reads, not a separate
// wizard-only draft. Deliberately no illustration here (see
// onboarding-motion-plan.md) — the live preview pill *is* the explanation.
function StepOverlayAdjuster({ config, updateConfig, onBack, onNext }) {
  const opacityPct = Math.round((config.overlay_opacity ?? 0.65) * 100);
  const size = config.overlay_size ?? 52;

  return (
    <div className="onb-page-inner">
      <header className="onb-step-head">
        <h2>Make the bubble yours</h2>
        <p className="onb-sub">
          Drag to see it change live — you can always adjust this later in Settings → Overlay.
        </p>
      </header>

      <div className="onb-pill-preview-wrap">
        <div
          className="onb-pill-demo"
          style={{ width: `${size}px`, height: `${size}px`, opacity: opacityPct / 100 }}
        >
          <img src={bulbulMark} alt="" />
        </div>
      </div>

      <div className="onb-slider-row">
        <label className="onb-slider-label">Opacity</label>
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
        <span className="onb-slider-val">{opacityPct}%</span>
      </div>
      <div className="onb-slider-row">
        <label className="onb-slider-label">Size</label>
        <input
          type="range"
          min="44"
          max="96"
          step="4"
          value={size}
          onChange={(e) => updateConfig({ ...config, overlay_size: Number(e.target.value) })}
          aria-label="Bubble size"
        />
        <span className="onb-slider-val">{size}px</span>
      </div>

      <div className="onb-actions">
        <button className="onb-btn ghost" onClick={onBack}>← Back</button>
        <button className="onb-btn primary" onClick={onNext}>Continue →</button>
      </div>
    </div>
  );
}

// Encodes captured mic audio into a real WAV file (44-byte header + PCM16
// mono data) entirely client-side — Groq's Whisper endpoint (and the
// transcribe_dictate_sample command below) expects genuine WAV bytes, and
// the browser's own MediaRecorder only produces compressed formats, so
// this is built by hand from the raw Float32 samples the Web Audio API
// hands over in startListening below.
function encodeWav(float32Chunks, sampleRate) {
  let length = 0;
  for (const c of float32Chunks) length += c.length;
  const pcm = new Int16Array(length);
  let o = 0;
  for (const c of float32Chunks) {
    for (let i = 0; i < c.length; i++) {
      const s = Math.max(-1, Math.min(1, c[i]));
      pcm[o++] = s < 0 ? s * 0x8000 : s * 0x7fff;
    }
  }
  const dataSize = pcm.length * 2;
  const buf = new ArrayBuffer(44 + dataSize);
  const view = new DataView(buf);
  const writeStr = (offset, str) => {
    for (let i = 0; i < str.length; i++) view.setUint8(offset + i, str.charCodeAt(i));
  };
  writeStr(0, "RIFF");
  view.setUint32(4, 36 + dataSize, true);
  writeStr(8, "WAVE");
  writeStr(12, "fmt ");
  view.setUint32(16, 16, true);
  view.setUint16(20, 1, true); // PCM
  view.setUint16(22, 1, true); // mono
  view.setUint32(24, sampleRate, true);
  view.setUint32(28, sampleRate * 2, true); // byte rate
  view.setUint16(32, 2, true); // block align
  view.setUint16(34, 16, true); // bits per sample
  writeStr(36, "data");
  view.setUint32(40, dataSize, true);
  new Int16Array(buf, 44).set(pcm);
  return new Uint8Array(buf);
}

const DICTATE_PROMPTS = {
  idle: "Tap the bubble below to try it.",
  listening: "Listening — tap again when you're done.",
  processing: "Cleaning it up…",
  done: "That's exactly what you said — the real bubble works the same way.",
  empty: "Didn't catch anything — go ahead and try again.",
  error: "Something went wrong — tap to try again.",
};

// Android only: a rehearsal of dictating that follows the REAL bubble's
// own mechanism, not an approximation of it — same gesture (a single tap
// starts recording, a second tap stops it; see onBubbleTap in
// BulbulForegroundService.kt, the bubble's default interaction) and real
// transcription (the webview captures actual mic audio via the Web Audio
// API, encodes it to WAV itself, and ships it to Groq Whisper through the
// same endpoint + model fallback chain GroqClient.kt uses — nothing here
// is a canned string, whatever was said is exactly what comes back). The
// one real difference: the result renders in this screen's own field
// instead of being injected into another app, since nothing else is
// focused during setup.
function StepDictateTest({ onBack, onNext }) {
  const [phase, setPhase] = useState("idle"); // idle | listening | processing | done | empty | error
  const [resultText, setResultText] = useState("");
  const [errorText, setErrorText] = useState("");
  const audioCtxRef = useRef(null);
  const streamRef = useRef(null);
  const processorRef = useRef(null);
  const sourceRef = useRef(null);
  const gainRef = useRef(null);
  const chunksRef = useRef([]);
  const timersRef = useRef([]);

  useEffect(() => {
    return () => {
      timersRef.current.forEach(clearTimeout);
      teardownAudio();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  function schedule(fn, ms) {
    timersRef.current.push(setTimeout(fn, ms));
  }

  function teardownAudio() {
    try { processorRef.current?.disconnect(); } catch {}
    try { sourceRef.current?.disconnect(); } catch {}
    try { gainRef.current?.disconnect(); } catch {}
    try { streamRef.current?.getTracks().forEach((t) => t.stop()); } catch {}
    try { audioCtxRef.current?.close(); } catch {}
    processorRef.current = null;
    sourceRef.current = null;
    gainRef.current = null;
    streamRef.current = null;
    audioCtxRef.current = null;
  }

  async function startListening() {
    setErrorText("");
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      const AudioCtx = window.AudioContext || window.webkitAudioContext;
      const ctx = new AudioCtx();
      const source = ctx.createMediaStreamSource(stream);
      // A ScriptProcessorNode only fires while it's part of a live graph
      // reaching the destination — routed through a zero-gain node so the
      // mic is never actually played back through the speaker.
      const processor = ctx.createScriptProcessor(4096, 1, 1);
      const silentGain = ctx.createGain();
      silentGain.gain.value = 0;
      chunksRef.current = [];
      processor.onaudioprocess = (e) => {
        chunksRef.current.push(new Float32Array(e.inputBuffer.getChannelData(0)));
      };
      source.connect(processor);
      processor.connect(silentGain);
      silentGain.connect(ctx.destination);

      streamRef.current = stream;
      audioCtxRef.current = ctx;
      sourceRef.current = source;
      processorRef.current = processor;
      gainRef.current = silentGain;
      setPhase("listening");
    } catch {
      setErrorText("Couldn't access the microphone.");
      setPhase("error");
      schedule(() => setPhase("idle"), 2200);
    }
  }

  async function stopAndTranscribe() {
    const ctx = audioCtxRef.current;
    const sampleRate = ctx?.sampleRate || 48000;
    const chunks = chunksRef.current;
    chunksRef.current = [];
    teardownAudio();

    if (chunks.length === 0) {
      setPhase("idle");
      return;
    }
    setPhase("processing");
    try {
      const wav = encodeWav(chunks, sampleRate);
      const text = await invoke("transcribe_dictate_sample", { wavBytes: Array.from(wav) });
      if (!text || !text.trim()) {
        setPhase("empty");
        schedule(() => setPhase("idle"), 1800);
        return;
      }
      setResultText(text.trim());
      setPhase("done");
    } catch (err) {
      setErrorText(typeof err === "string" ? err : "Transcription failed.");
      setPhase("error");
      schedule(() => setPhase("idle"), 2400);
    }
  }

  function handleTap() {
    if (phase === "listening") {
      stopAndTranscribe();
    } else if (phase !== "processing") {
      setResultText("");
      startListening();
    }
  }

  const showHint = phase === "idle";
  const fieldText =
    phase === "done" ? resultText :
    phase === "processing" ? "Cleaning it up…" :
    phase === "listening" ? "Listening…" :
    phase === "empty" ? "Didn't catch any speech — try again." :
    phase === "error" ? errorText :
    "Dictated text lands here, like in any app";

  return (
    <div className="onb-page-inner">
      <header className="onb-step-head">
        <h2>Try it yourself</h2>
        <p className="onb-sub">A real rehearsal — the exact same gesture and transcription as the real bubble.</p>
      </header>

      <div className="onb-dictate-demo">
        {/* Text alone gets skipped, so the "tap to start" instruction also
            has to exist as something glance-able: a small icon+label chip
            plus a pulsing ring around the target itself. Both drop away
            once the gesture has actually been discovered. */}
        <div className={`onb-dictate-hint ${showHint ? "" : "onb-dictate-hint-hidden"}`}>
          <TouchIcon />
          <span>Tap to start</span>
        </div>

        <div className="onb-dictate-pill-wrap">
          {showHint && <span className="onb-dictate-invite" aria-hidden />}
          <button
            type="button"
            className={`onb-dictate-pill state-${phase}`}
            onClick={handleTap}
            aria-label={phase === "listening" ? "Tap to stop and transcribe" : "Tap to start dictating"}
          >
            <img src={bulbulMark} alt="" />
          </button>
        </div>

        <p className={`onb-dictate-prompt state-${phase}`}>{DICTATE_PROMPTS[phase]}</p>

        {/* Where the text actually lands — mirrors the real thing: Bulbul
            doesn't show its own transcript anywhere, it types straight
            into whatever field you were already in. */}
        <div className={`onb-dictate-field state-${phase}`}>
          <span className={`onb-dictate-field-text${phase === "done" ? " filled" : ""}`}>
            {fieldText}
          </span>
          {phase === "done" && <span className="onb-dictate-cursor" aria-hidden />}
        </div>
      </div>

      <div className="onb-actions">
        <button className="onb-btn ghost" onClick={onBack}>← Back</button>
        <button className="onb-btn primary" onClick={onNext}>Continue →</button>
      </div>
    </div>
  );
}

// Simple "tap" glyph — a fingertip with a soft ripple arcing off it —
// rather than a literal hand illustration, so it reads clearly at this
// small a size without needing a new image asset.
function TouchIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 16 16" aria-hidden>
      <circle cx="8" cy="10.5" r="2.6" fill="currentColor" />
      <path d="M3.4 6.2a5.2 5.2 0 0 1 9.2 0" stroke="currentColor" strokeWidth="1.3" fill="none" strokeLinecap="round" opacity="0.65" />
      <path d="M1.4 4.1a7.6 7.6 0 0 1 13.2 0" stroke="currentColor" strokeWidth="1.3" fill="none" strokeLinecap="round" opacity="0.35" />
    </svg>
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

  // In-page hotkey fallback — a recent WebView2 update stopped delivering
  // modifier-only chords (Ctrl+Win) to our global keyboard hook while Bulbul's
  // own window is focused, so the wizard's live test never fired. The shared
  // hook detects the chord in-page and drives the real dictation. It returns
  // whether that fallback actually fired — which only happens when the global
  // hook ISN'T intercepting the chord in-window — so we can show an honest note
  // pointing at the key-based alternative for anyone whose hotkey is blocked.
  const usedInPageFallback = useInPageChordFallback(activeHotkey);

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

      {IS_WINDOWS && (
        <div className="onb-hotkey-note" role="note">
          <InfoIcon />
          <p>
            <strong>Some shortcuts can be blocked on your PC.</strong> Security
            software (antivirus, corporate, or anti-cheat) sometimes blocks
            modifier-only shortcuts like{" "}
            <code>{formatComboForDisplay("Ctrl+Win")}</code> — that's your machine,
            not a Bulbul bug. If yours doesn't respond, pick{" "}
            <code>{formatComboForDisplay("Ctrl+Shift+Space")}</code>, which works
            everywhere.
          </p>
        </div>
      )}

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
              </div>
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

          {usedInPageFallback && (
            <div className="onb-fallback-note">
              Note: if {formatComboForDisplay(activeHotkey)} doesn't fire in your other apps, a
              recent Windows update may be blocking it — you can switch to{" "}
              <code>{formatComboForDisplay("Ctrl+Shift+Space")}</code> anytime in Settings.
            </div>
          )}
        </div>
      </div>

      <div className="onb-actions">
        <button className="onb-btn ghost" onClick={onBack}>← Back</button>
        <button className="onb-btn primary" onClick={onNext}>Continue →</button>
      </div>
    </div>
  );
}

// Mouse mode's own live-test step, mirroring StepHotkey's bulbul-status /
// bulbul-focused-insert wiring (same event stream the production overlay
// uses) but without any of the chord-assembly visuals — a click is a
// single discrete action, not something to assemble key-by-key. Skippable:
// mouse_mode defaults on regardless, so skipping just means the user
// didn't sit through the demo, not that the feature gets turned off.
function StepMouseMode({ config, updateConfig, onBack, onNext }) {
  const [transcript, setTranscript] = useState("");
  const [clickState, setClickState] = useState("idle");
  const [errorMsg, setErrorMsg] = useState("");
  const textareaRef = useRef(null);
  const mouseButton = config.mouse_button || "middle";
  // Tracks whether "Custom" is the selected radio, independent of whether
  // a real button has actually been recorded yet — so picking "Custom"
  // shows a clear "click to record" prompt instead of silently guessing
  // a button (e.g. "back") and displaying it as if it were already set.
  const [customPicked, setCustomPicked] = useState(mouseButton !== "middle");

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

  useEffect(() => {
    const un = listen("bulbul-status", (e) => {
      const { state, message } = e.payload || {};
      if (state === "listening") {
        setClickState("listening");
      } else if (state === "processing" || state === "injecting") {
        setClickState("processing");
      } else if (state === "done") {
        setClickState("done");
      } else if (state === "error") {
        setClickState("error");
        setErrorMsg(message || "");
      } else if (state === "idle") {
        if (message && /too short/i.test(message)) {
          setClickState("too_short");
        } else if (message && /(silence|no speech)/i.test(message)) {
          setClickState("silent");
        } else {
          setClickState("idle");
        }
      }
    });
    return () => { un.then((f) => f()); };
  }, []);

  useEffect(() => {
    if (clickState === "idle" || clickState === "listening" || clickState === "processing") return;
    const dwell = clickState === "done" ? 2200 : 3000;
    const t = setTimeout(() => setClickState("idle"), dwell);
    return () => clearTimeout(t);
  }, [clickState]);

  let title;
  let subtitle;
  switch (clickState) {
    case "listening":
      title = "Listening — click again to stop";
      subtitle = "Say a sentence, then click your mouse button once more.";
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
      title = "Stopped too quickly";
      subtitle = "Leave a moment between the two clicks — that clip was too short to transcribe.";
      break;
    case "silent":
      title = "Couldn't hear you";
      subtitle = "Try speaking a bit louder, or check your mic is on the right input.";
      break;
    case "error":
      title = "Something went wrong";
      subtitle = errorMsg || "Look at the dashboard's overlay for details, or try again.";
      break;
    default:
      title = `Click ${mouseButtonLabel(mouseButton).toLowerCase()} to start`;
      subtitle = "Click it again when you're done speaking.";
  }

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
        <h2>Or dictate with your mouse</h2>
        <p className="onb-sub">
          Click a mouse button to start, click again to stop — no holding required. On by default; change the button or turn it off anytime in Settings.
        </p>
      </header>

      <div className="onb-hotkey-grid">
        <div className="onb-hotkey-list">
          <label className={`onb-hotkey-row ${!customPicked ? "selected" : ""}`}>
            <input
              type="radio"
              name="mouseButton"
              checked={!customPicked}
              onChange={() => {
                setCustomPicked(false);
                updateConfig({ ...config, mouse_button: "middle" });
              }}
            />
            <div className="onb-hotkey-meta">
              <div className="onb-hotkey-label">Middle click</div>
              <div className="onb-hotkey-detail">The safest default — rarely bound to anything else.</div>
            </div>
          </label>
          <label className={`onb-hotkey-row ${customPicked ? "selected" : ""}`}>
            <input
              type="radio"
              name="mouseButton"
              checked={customPicked}
              onChange={() => setCustomPicked(true)}
            />
            <div className="onb-hotkey-meta">
              <div className="onb-hotkey-label">Custom</div>
              <div className="onb-hotkey-detail">
                Record a side button instead, if your mouse has one and nothing else has claimed it.
              </div>
              {customPicked && (
                <div className="onb-hotkey-custom">
                  <MouseButtonRecorder
                    value={mouseButton === "middle" ? null : mouseButton}
                    onChange={(v) => updateConfig({ ...config, mouse_button: v })}
                  />
                </div>
              )}
            </div>
          </label>
        </div>

        <div className="onb-test-pane">
          <div className="onb-test-header">
            <div className="onb-test-eyebrow">Try it now</div>
            <div className="onb-test-instructions">
              Click {mouseButtonLabel(mouseButton).toLowerCase()}, read the sample line aloud, then click again:
            </div>
            <div className="onb-sample-line">"{SAMPLE_LINE}"</div>
          </div>

          <div className={`onb-hold-indicator state-${clickState}`} role="status" aria-live="polite">
            <div className="onb-hold-visual">
              {clickState === "listening" && (
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
              {clickState === "processing" && <div className="onb-hold-spinner" aria-hidden />}
              {clickState === "done" && <div className="onb-hold-check" aria-hidden>✓</div>}
              {(clickState === "too_short" || clickState === "silent") && (
                <div className="onb-hold-warn" aria-hidden>!</div>
              )}
              {clickState === "error" && <div className="onb-hold-error" aria-hidden>✕</div>}
              {clickState === "idle" && (
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

          <textarea
            ref={textareaRef}
            className="onb-textarea"
            placeholder="Click, speak, click again — the transcript will land here…"
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
        <div className="onb-actions-right">
          <button
            className="onb-btn ghost"
            onClick={() => {
              // Skip only means "didn't sit through the demo" — mouse_mode
              // stays whatever it already was (on, by default), since a
              // harmless watcher that never fires for a missing/unused
              // button costs nothing.
              onNext();
            }}
          >
            Skip
          </button>
          <button className="onb-btn primary" onClick={onNext}>Continue →</button>
        </div>
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

function InfoIcon() {
  return (
    <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <circle cx="12" cy="12" r="9" />
      <path d="M12 16v-4M12 8h.01" />
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
