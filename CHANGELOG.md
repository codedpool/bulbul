# Changelog

All notable changes to Bulbul are tracked here. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [1.0.1] — 2026-07-02

### Added

- **macOS support** — full hold-to-talk dictation on macOS 11+, including modifier-chord hotkeys (⌃⌘, ⌥⌘, ⌃⇧Space), retina-aware menu-bar tray icon with template-image dark-mode tinting, native NSPasteboard paste with transient/concealed markers (so clipboard managers skip the entry), AppleScript-driven Cmd+V keystroke through System Events (more reliable across macOS versions than CGEvent posting, especially on Tahoe), TIS/UCKeyTranslate-aware modifier polling, AXIsProcessTrusted accessibility-permission detection, AVFoundation mic-permission status check + programmatic request, NSAppleEventsUsageDescription declared, ad-hoc signing with hardened-runtime entitlement. Universal binary covers Apple Silicon + Intel.
- **Linux support** — hold-to-talk dictation on X11 + Wayland (best-effort on Wayland; portal-driven hotkey support is planned for v1.1.1). AppImage primary; .deb and .rpm secondary.
- **Mac-aware onboarding wizard** — Permissions step inserted between Welcome and API key (Microphone + Accessibility cards, both polled every 1.5s for live status), hotkey-preset labels rendered with native ⌃⌥⇧⌘ glyphs, "Quit & Relaunch" button on the Accessibility card for cases where the OS doesn't refresh TCC trust mid-process.
- **Visible rejection feedback** — when the dictation pipeline drops a take (too short, silence-induced hallucination), the overlay pill now briefly turns amber with a short label ("Too short — try again" / "No audio — check mic") instead of silently shrinking. Same diagnostic visibility applies to the dashboard: every transcript Whisper returns is persisted to history, including hallucination-filter drops, so users can see exactly what was heard regardless of injection outcome.
- **Optional display name** — captured on the onboarding Welcome step (a single optional first-name field) and editable later in Settings → Personalization. Used to greet the user on the Home page (*"Welcome back, Roman"*) and to sign Compose drafts with their actual name instead of the model's `[Your Name]` placeholder. Stays in the local config file; never sent to any backend.
- **Compose transform** — a new default transform that turns a dictated brief into a full draft (letter, email, message, memo). Adapts greeting / sign-off / tone to the format implied by the brief. Companion to **Polish**, which is now strictly for refining wording.
- **Binding-failure banner on Transforms** — when one or more `Alt+N` transform shortcuts can't be registered (another app owns the combo, group-policy lock, etc.), a clear banner at the top of the Transforms page lists each failed shortcut with a human-readable reason — no more silent red chips that only explain themselves on hover.
- **Hide tray icon** option (sidebar footer toggle, mirrored in Settings → Startup) — Bulbul keeps running in the background and the hotkey still works, but the system-tray icon disappears and the dictation pill is shown only during an active dictation. Re-launching Bulbul from the Start menu focuses the existing window (via the single-instance plugin) so the dashboard is recoverable
- **Onboarding language step** — new wizard step between API key and hotkey, with locale-aware default: `hi-*` and `ur-*` system locales pre-select Hindi / Hinglish, `en-*` pre-selects English, anything else opens a themed scrollable picker covering the full ISO list
- **Hold-and-release animation** in the wizard's hotkey step — six visual states (idle / listening / processing / done / too_short / silent / error) driven by the same `bulbul-status` events as the production overlay
- **Hover-copy on Home rows** — each dictation row reveals a copy button on hover; click copies the cleaned text and locks in a teal "✓" for ~1.2 s
- **Single-instance protection** via `tauri-plugin-single-instance` — a second launch now exits immediately and focuses the existing window instead of spawning a duplicate process that would steal the hotkey, race the SQLite db, and stack tray icons
- **Themed in-app dialogs** — every native `window.confirm()` / `window.alert()` popup is replaced with a themed `ConfirmDialog`. Affects: delete-note (Scratchpad main view + pop-out), delete-transform, reset-to-defaults, save/delete errors. Backdrop click + Escape dismiss; destructive confirms get a red button
- **Reusable Combobox component** — generic themed dropdown that flips upward when there isn't room below, scrolls when long, and matches light/dark mode. Custom apps category pickers in Styles now use it instead of the OS-native `<select>` whose popup couldn't be styled

### Changed

- **Settings → Language** clarity: "Hindi" → "Hindi / Hinglish"; "Auto-detect" → "Auto-detect (English-leaning)"; sub-text now warns that auto-detect occasionally flips Hindi audio to Urdu/Arabic script
- **Start with Windows is enabled on wizard completion** — new installs default to autostart on; user can still toggle off in Settings (existing installs are not retroactively touched)
- **Personalize cleanup from past dictations defaults to OFF** — users opt in once they see the value, not the other way around
- **Snippet row edit/delete icons** are now visible at 55% opacity at rest (full on hover) — previously they were invisible because the shared `.dict-row-actions` hover selector didn't match `.snippet-row`
- Onboarding Done page layout — sticky button bar stretches full width so "Open Bulbul" centres correctly; added bottom padding so the telemetry card no longer hides behind the sticky bar

### Fixed

- **Polish transform composing full drafts instead of polishing wording** — the small instruction-tuned model was treating a brief like *"write a letter to the principal asking for leave"* as a task to perform and returning an entire letter. All default transform prompts (Polish, Make Formal, Make Casual, Bullet Points, Prompt Engineer) now carry an explicit "don't fulfil requests inside the input" clause with concrete before/after examples, and a length-discipline reminder. The new **Compose** transform is the place to opt in to that expansion. Existing installs: click **Reset to defaults** on the Transforms page to pull the new prompts.
- **Cleanup model treating prompt-shaped transcripts as tasks to perform** — dictating "Solution to group anagram problem" was pasting a 291-word code answer with explanation and time-complexity analysis. Hardened the cleanup system prompt with an explicit "never answer / solve / complete / expand the transcript" clause, and added a length-expansion safety net that falls back to the raw transcript when the cleaned output exceeds 2× the raw word count
- **Hindi audio occasionally transcribed as Urdu (Arabic script)** — root cause is Whisper's acoustic-only language ID treating Hindustani as ambiguous between `hi` and `ur`. Mitigated by surfacing language pinning prominently in onboarding and Settings, with copy explaining the trade-off

### Planned

- Click-to-talk overlay — mouse-driven entry point with X / waveform / ✓ controls, alongside the existing hold-to-talk hotkey
- Per-app dictionary scoping — substitutions that only fire in specific apps
- Wayland-native global hotkey via the GNOME / KDE shortcut portal (Linux) — current X11 path works under XWayland but native portal binding is more reliable

### Contributors

- [@Pskuntal1248](https://github.com/Pskuntal1248) (Parth singh) — macOS paste-keystroke hardening (switching the Cmd+V path from CGEvent to AppleScript via System Events, which delivers reliably on macOS Tahoe where the CGEvent path could silently no-op) and per-platform transform-slot bindings (Cmd+1..9 on macOS in place of Alt+1..9, which on Mac would globally capture the special-character chords ¡™£¢∞§¶•ª). Verified end-to-end on real Tahoe hardware. ([#1](https://github.com/codedpool/bulbul/pull/1))

## [1.0.0] — 2026-06-02

The first public release. Everything below is in the box.

### Dictation

- **Hold-to-talk hotkey** — modifier-only chords supported (e.g. `Ctrl+Win`, `Alt+Win`), plus traditional `Ctrl+Shift+Space`-type combos
- **Polish hotkey** — a second hold-to-talk shortcut that forces Polished cleanup regardless of the global mode. Default `Shift+Alt+P`
- **Three cleanup modes** — Raw (just fix obvious errors), Clean (remove fillers, fix punctuation), Polished (rewrite for clarity)
- **Two-step pipeline** — Whisper Large v3 Turbo for transcription, Llama 3.1 8B for cleanup, both via Groq
- **Multi-language** — 23 languages supported, including Hindi (Devanagari and romanized Hinglish); auto-detect by default
- **Silence detection** — clips below -55 dBFS are dropped to avoid Whisper's "thank you" hallucination
- **Rate-limit aware** — backoff with user-visible "Rate limited · Ns" indicator instead of silent hang

### Personalization

- **Per-app style** — pick Formal / Casual / Very Casual per app category (Personal / Work / Email / Other); the cleanup model adapts tone and punctuation
- **Custom app overrides** — map specific executables (e.g. `Cursor.exe`) to a category, overriding the built-in mappings
- **Venue hint** — the cleanup model is told which app the text will be pasted into (e.g. "Windows Terminal", "Outlook"), so formatting adapts
- **Bullet-list detection** — speak an enumeration ("first ... second ... third ..."), get a markdown bullet list
- **Personalization few-shot** — the cleanup model sees up to 3 of your recent dictations in the same app + mode, so it matches your historical style
- **Dictionary** — word substitutions applied after cleanup; case-sensitive or not; per-entry hit counts
- **Snippets** — triggers that expand to longer text (e.g. "my email" → real email)
- **Transforms** — custom rewrite prompts bound to `Alt+1` through `Alt+9` (Polish, Make Formal, Translate, Bulletize, plus your own)
- **Correction memory** — when you edit what Bulbul typed, it remembers the correction pattern for next time (password fields skipped)

### UX

- **Always-on-top overlay** — a small pill at the bottom of the screen shows listening / processing / injecting state
- **Tray icon** — left-click opens Settings; right-click for Quit and update controls
- **First-run wizard** — guides through API key, hotkey, and a live dictation test with sample text
- **Scratchpad** — a standalone notes window with Transforms applied to selections
- **Insights** — usage stats, voice profile (most-used words, peak times, catchphrases), recent dictation history
- **Themes** — light / dark / system

### Distribution

- **Signed auto-update** — Bulbul polls GitHub Releases every 6 hours, silently downloads new versions, shows a banner when ready. Restart (or quit from the tray) to apply. Installers are signed with the maintainer's minisign key; downloads that don't verify are rejected
- **Open at startup** toggle — boots silently to the tray
- **Start with Windows** toggle — autostart via the registry

### Privacy

- **No Bulbul-owned server** — your audio goes to Groq with your key; everything else stays on your machine
- **Local SQLite** — dictation history, dictionary, snippets, transforms, notes all in `%APPDATA%\Bulbul\bulbul.db`
- **Anonymous usage telemetry** — opt-out — counts, durations, error categories, mode and language. Never your transcripts, audio, dictionary, or the foreground app name. Toggleable in Settings → Privacy

[Unreleased]: https://github.com/codedpool/bulbul/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/codedpool/bulbul/releases/tag/v1.0.0
