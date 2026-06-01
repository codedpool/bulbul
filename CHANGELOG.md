# Changelog

All notable changes to Bulbul are tracked here. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Planned

- Click-to-talk overlay — mouse-driven entry point with X / waveform / ✓ controls, alongside the existing hold-to-talk hotkey
- Per-app dictionary scoping — substitutions that only fire in specific apps
- macOS port

## [1.0.0] — 2026-06-02

The first public release. Everything below is in the box.

### Dictation

- **Hold-to-talk hotkey** — modifier-only chords supported (e.g. `Ctrl+Win`, `Alt+Win`) modifier-only, plus traditional `Ctrl+Shift+Space`-type combos
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
