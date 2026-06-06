# Bulbul — Website Brief

> Self-contained product brief for handing off to an agent building the marketing site. Includes positioning, feature inventory, technical details, brand voice, and copy guardrails.

## In one sentence

Bulbul is a Windows voice dictation app: hold a hotkey, talk, release — cleaned-up text is pasted at your cursor in whatever app is focused, with no SaaS account, no subscription, and no audio touching any server you don't control.

## What it actually does

The full flow when a user holds the dictation hotkey:

1. Captures microphone audio locally (via `cpal`).
2. Sends audio to **Groq Whisper** for transcription using the user's own API key.
3. Runs the transcript through a **Groq 8B-class LLM** for cleanup — strips filler words, false starts, hesitations; matches the user's style.
4. Applies any learned **corrections** from the user's history (proper nouns, domain terms, brand spellings).
5. Pastes the result at the cursor via Win32 `SendInput` (Ctrl+V from the system clipboard).

Total turnaround is typically sub-second because Groq is fast. The user feels it as: hold key → talk → release → text appears.

## Core features

### Hold-to-talk dictation
- Default hotkey: **Ctrl+Win** (configurable). Hold-only — no toggle, no spoken activation.
- Works in every Windows app because injection happens at the OS level, not via app-specific integration.

### Cleanup / Polish mode
- Default hotkey: **Shift+Alt+P**.
- Select existing text in any app → hit hotkey → cleaned version pastes back.

### Correction memory (V3.1)
- Learns from the corrections the user actually makes to its output.
- Stored as a dictionary, applied as **suggestions** to future outputs — not few-shot-examples to the LLM (8B models echo few-shot prompts parrot-style; this approach avoids that pitfall).
- Result: fix a proper noun once ("Roman" not "roaming"), and it sticks.

### Transforms
- Select text, hit a transform hotkey, get a structured rewrite.
- Example: **Catch-up** (Alt+6) summarizes recently dictated content.

### Per-app context awareness
- Bulbul detects the foreground app and tunes cleanup style accordingly.
- ~50 known apps mapped: Slack/Teams/Outlook → "work" tone, WhatsApp/Telegram → "personal" tone, VS Code/JetBrains → "code/technical" tone, etc.
- Neutral fallback for unknown apps.

### Snippets
- Reusable text snippets, with clickable hero examples on the Snippets pane to lower the empty-state barrier.

### Insights
- Words-per-minute stats, usage totals, trends.
- Shared layout/data with the Home view's WPM display.

### Scratchpad window
- A dedicated dictation-only window for capture that isn't headed for another app.

### Onboarding wizard
- First launch asks: name, Groq API key, a couple of preference toggles.
- Sets sensible defaults so the app works the moment onboarding ends.

### Settings (modal)
- Personalization (cleanup learning on/off, correction memory on/off)
- Startup (start with Windows, open dashboard on launch)
- Hotkeys (rebind dictation + polish + transforms; guided wizard available)
- Privacy (hide tray, share telemetry)
- Transforms subhead with per-transform toggles

## What makes it different

**vs. Windows built-in voice typing**: Bulbul cleans up the raw transcript (no "um, uh, like" carry-through), learns the user's corrections, and styles per-app. Built-in does none of that.

**vs. SaaS dictation apps**: Bulbul is **BYOK** and **local-first**. No subscription, no Bulbul-hosted account, no audio routed through a vendor. The Groq API key belongs to the user; the transcript and audio never touch Bulbul-controlled infrastructure.

**vs. MacWhisper / WhisperWriter and similar Whisper wrappers**: Bulbul is hotkey-first — there's no app window to focus during dictation. Cleanup is automatic, not a manual second step. Plus per-app context, correction memory, and transforms aren't in those tools.

**vs. ChatGPT voice / Copilot voice**: Bulbul is for typing *into other apps*. ChatGPT/Copilot are chatbots; Bulbul is a system-wide input method.

## Privacy and trust model

- **BYOK**: User's Groq API key, user's Groq account. Audio goes directly machine → Groq.
- **No Bulbul servers**: The only network calls Bulbul makes are (a) to Groq with the user's key, and (b) to GitHub Releases for update checks. There is no Bulbul-operated backend.
- **Local-first storage**: SQLite on disk. Correction memory, snippets, settings, usage stats — all local.
- **Signed releases**: Every binary signed with minisign. Installer and auto-updater both verify signatures. Public key is shipped with the app and visible in the repo.
- **Opt-in telemetry**: Off by default. When on, anonymous counters only — no audio, no transcripts, no API payload content.
- **Open distribution**: Source at `github.com/codedpool/bulbul`. Reproducible via documented Tauri build flow.

## Technical architecture

For the credibility paragraph or a "How it's built" section:

- **Shell**: **Tauri 2** (Rust backend, React frontend, native OS WebView — not Electron). Installer ~5 MB (NSIS) or ~7 MB (MSI). Runtime memory ~50–100 MB.
- **Languages**: Rust (backend, ~10k lines) + React 18 + Vite (frontend).
- **OS integration**: Win32 APIs for global hotkeys (`GetAsyncKeyState` polling), foreground-window detection (`GetForegroundWindow`), focused-element text reading (UI Automation), and keystroke injection (`SendInput`).
- **Audio**: `cpal` (cross-platform; sets up Mac/Linux ports cleanly).
- **AI**: Groq API — Whisper-large-v3 for speech-to-text, 8B-class Llama for cleanup. User's API key, user's Groq account.
- **Local storage**: SQLite via `rusqlite`.
- **Auto-updates**: `tauri-plugin-updater` with minisign signatures. Updates install passively (progress bar, no clicks).
- **Single-instance**: `tauri-plugin-single-instance` — launching Bulbul again focuses the existing window instead of opening a second copy.

## Installation

**Manual (today)**: Download `Bulbul_1.0.0_x64-setup.exe` (or MSI) from GitHub Releases, run it.

**One-line install (coming, via new domain)**:
```powershell
irm https://<domain>/install.ps1 | iex
```
Downloads the installer, verifies the minisign signature against the embedded public key, installs passively, cleans up. Same pattern as `rustup`, `bun`, `deno`. The install script is open-source in the repo (`install.ps1` at root).

After install, Bulbul's own onboarding wizard runs on first launch (asks name + Groq API key + a couple of toggles).

## Platforms and roadmap

- **Today**: Windows 10/11 (x64 only).
- **Near-term roadmap**: macOS (~3 weeks of porting work — `core-graphics` + `objc2` replacements for the Win32 surface). Linux X11 (~1.5 weeks after macOS, since the Mac port forces a cross-platform abstraction first). Wayland later.
- **Microsoft Store**: technically possible after MSIX packaging, but $19 Partner Center fee. Lower priority than WinGet (free).
- **WinGet**: planned official package-manager presence. Free to publish via PR to `microsoft/winget-pkgs`.

## Brand voice

- **Name**: **Bulbul** — a South Asian songbird known for a rich melodic call. Single word, four letters, easy to spell after first encounter. Connects to the "voice" theme without being literal.
- **Common misspelling**: "Bubble." Important — the dev has flagged this explicitly. Always render as "Bulbul," never auto-correct to "Bubble," and never let an LLM suggest it.
- **Tone**: Direct, technical-but-friendly, zero marketing fluff. The app respects the user's time; copy should too.
  - Reference points: Linear, Raycast, Cursor — confident, specific, low-noise.
  - **Avoid**: "Revolutionize your workflow," "AI-powered," "Unleash," "10x," any "✨" emoji-as-decoration.
- **Specificity beats vagueness**: "Groq Whisper for transcription" beats "advanced AI." "Sub-second turnaround" beats "blazing fast." The audience prefers specifics; vagueness reads as evasion.

## Audience

Two distinct personas, both worth speaking to:

1. **The power user (primary)**: Developer, writer, or knowledge worker who types for a living. Comfortable with hotkeys. Comfortable with BYOK. Values privacy, speed, and depth. Will read the technical section. Will check the GitHub repo.
2. **The friend-of-friend (secondary)**: Less technical, has been frustrated by Windows' built-in dictation or subscription pricing on commercial apps. Needs the install to "just work" and the value prop to be obvious in three seconds. Won't read the technical section but will install if the hero feels clean.

The site should serve both: clean three-second pitch up top, with depth available below the fold for the first persona.

## Suggested landing-page outline

1. **Hero** — name, one-line pitch, install command (copy button), short loop showing dictation into VS Code or Gmail.
2. **Three-column "what it does"** — hold-to-talk in any app · auto-cleanup · learns your corrections.
3. **Live demo or animated GIF** — same hotkey, three different apps, identical result. Sells the system-wide angle.
4. **Privacy / trust section** — BYOK, local-first, signed releases, link to source. Short and direct.
5. **Comparison** — small table vs Windows built-in / MacWhisper / commercial alternatives. Optional but high-conversion for the power-user persona.
6. **Install** — repeat the one-liner, with GitHub Releases link as fallback.
7. **Footer** — GitHub, current version, "signed by minisign key `RWTLvdvs...`", contact email, MIT/license badge if applicable.

## Copy guardrails (things to NOT say)

- ❌ "Voice commands" / "Say 'delete that' to..." — Bulbul has zero spoken commands by design. False-positive risk outweighs the convenience.
- ❌ "Cloud-based" or "AI in the cloud" — frames the app wrong. It's local-first with user-authorized API calls.
- ❌ "Free trial," "subscription," "plan tiers" — there's no SaaS layer. The app is free; Groq usage is the user's own (and Groq has a generous free tier).
- ❌ Generic AI buzzwords ("AI-powered," "revolutionary AI") — be specific: name the models, name the providers.
- ❌ "Competes with ChatGPT" — wrong category. Bulbul is an input method, not a chatbot.
- ❌ Auto-substituting "Bubble" for "Bulbul" anywhere — never.

## Practical references

- **Repo**: `github.com/codedpool/bulbul`
- **Current version**: `1.0.0`
- **Bundle ID**: `com.bulbul.app`
- **Updater endpoint**: `github.com/codedpool/bulbul/releases/latest/download/latest.json`
- **Minisign public key**: `RWTLvdvsrlMNS4LQvsKO03T8kF+5jZ1s7KiyU4lKZmYPcd0+1qxm2gKt`
- **Install script (in repo)**: `install.ps1` at repo root
- **Contact**: `devshooked@gmail.com`
- **Platforms today**: Windows 10/11 x64
- **License**: (TBD — add when website goes live)
- **Tech stack one-liner**: Tauri 2 + Rust + React 18 + Groq (Whisper + 8B Llama)

## What's open / TBD

The website agent may want to know what's still being decided so it doesn't lock copy too early:

- **Domain**: not yet purchased at time of writing. Probably `.app` TLD.
- **License**: not yet declared in the repo.
- **Pricing**: free. No plans to monetize the app itself. Groq usage is the user's own.
- **macOS / Linux**: roadmap, not shipped. Don't promise dates on the site; if mentioned, frame as "Windows now, Mac and Linux coming."
- **Microsoft Store presence**: deferred.
- **Social proof**: no testimonials or user counts yet (early launch). Don't fabricate.
