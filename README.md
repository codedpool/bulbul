# Bulbul

**Free, open-source Windows voice dictation. Hold a hotkey, talk anywhere, watch the text appear.**

Bulbul talks directly to [Groq](https://groq.com) using your own API key — no Bulbul-owned server in between, no subscription, no usage caps beyond Groq's own free tier. Your dictation history, dictionary, snippets, and settings all live in a local SQLite file on your machine.

---

## Get started in 60 seconds

**One-line install (PowerShell):**

```powershell
irm https://bulbultypes.xyz/install.ps1 | iex
```

Downloads the latest release, verifies the minisign signature against the embedded public key, installs Bulbul passively. No clicks.

**Or download manually:** [Releases](https://github.com/codedpool/bulbul/releases/latest) → `Bulbul_x.y.z_x64-setup.exe` → run it. Windows SmartScreen will warn you the first time (see [SmartScreen FAQ](docs/SMARTSCREEN.md)).

After install:

1. **Open Bulbul** → paste your free [Groq API key](https://console.groq.com/keys) → pick a hotkey
2. **Hold the hotkey anywhere on Windows. Speak. Release. Done.**

The transcript types itself into whatever app has focus — your browser, VS Code, Word, a terminal, Slack, Cursor, anywhere.

---

## Why Bulbul

| | Bulbul | Commercial alternatives |
|---|---|---|
| **Cost** | Free + your Groq key | Subscription |
| **Open source** | Yes (MIT) | No |
| **Where your audio goes** | Groq's API, with your key | Vendor's servers |
| **Source of truth for your data** | Your machine | Vendor |
| **Latency** | ~600ms typical | similar |
| **Custom hotkeys** | Any combo, including modifier-only chords | Limited |
| **Dictionary / snippets / transforms** | Yes, stored locally | Some, gated by tier |

---

## What's in the box

- **Two-step pipeline**: Whisper Large v3 Turbo (STT) → Llama 3.1 8B (cleanup) — both via Groq, both fast.
- **Cleanup modes**: Raw (just fix obvious errors), Clean (remove fillers, fix punctuation — default), Polished (rewrite for clarity).
- **Polish hotkey**: a second hold-to-talk shortcut that forces Polished mode regardless of your global setting. Default `Shift+Alt+P`.
- **Modifier-only hotkeys**: hold `Ctrl+Win` or `Alt+Win`. No letter needed.
- **Per-app style**: pick a tone (Formal / Casual / Very Casual) per app category. The cleanup model adapts.
- **Venue-aware**: Bulbul tells the cleanup model which app you're dictating into ("Windows Terminal" vs. "Outlook") so output formatting fits.
- **Bullet-list detection**: enumerate items aloud, get a markdown bullet list out.
- **Dictionary**: word substitutions (e.g. "groq" → "Groq") that fire after cleanup.
- **Snippets**: triggers that expand (e.g. "my email" → real email address).
- **Transforms**: per-text rewrites — Polish, Make Formal, Translate, Bulletize, or your own custom prompts. Bind to `Alt+1` through `Alt+9`.
- **Insights**: dictation history, voice profile, peak times, most-used words.
- **Scratchpad**: a standalone notes window with transforms applied to selections.
- **Auto-update**: signed releases. The app silently downloads new versions in the background and applies on next quit.

---

## Privacy posture

- **Your audio is sent to Groq** — that's how transcription works. Audio is processed under Groq's privacy policy ([review it before dictating sensitive content](https://groq.com/privacy-policy)).
- **No Bulbul-owned server.** There is no backend storing your transcripts, hotkeys, or dictionary.
- **All your dictation history lives locally** in `%APPDATA%\Bulbul\bulbul.db` (SQLite). Delete it anytime; back it up if you care.
- **Anonymous usage stats** (counts, durations, error categories) ship to a Supabase endpoint by default so the maintainer can see what's used and what breaks. **Never** your transcripts, audio, dictionary, or which app you're typing into. Off-toggle in **Settings → Privacy**.

---

## Requirements

- **Windows 10 or 11** (x64)
- A **free Groq API key** — sign up at [console.groq.com](https://console.groq.com)
- An internet connection (Groq is cloud-hosted)
- A microphone

macOS and Linux support is in development on the `v1.1-port` branch (rolling dev builds via `curl -fsSL https://bulbultypes.xyz/install-dev.sh | sh`). Stable releases are Windows-only for now.

### Linux dev-build notes

- **X11 sessions**: hotkey + paste work out of the box, same as Windows.
- **Wayland sessions**: Bulbul uses xdg-desktop-portal — no external tools required. On first launch it asks for two permissions via system dialogs (approve once, the grant is remembered):
  - **Global shortcuts** — for the dictation hotkey.
  - **Remote control** — for typing text into the focused app.
- **If the hotkey doesn't fire** (some GNOME versions decline the shortcuts portal for non-sandboxed apps), bind any system keyboard shortcut to `bulbul --toggle-dictation` — press once to start, again to stop. `SIGUSR2` does the same for compositor keybindings.
- **If pasting doesn't work** (you declined the Remote-control prompt, or your compositor lacks the portal), install a keystroke tool as a fallback: `wtype` on most desktops, `ydotool` on GNOME (whose compositor blocks wtype). The in-app banner tells you which.
- **GNOME tray**: install the "AppIndicator and KStatusNotifierItem" Shell extension to see Bulbul's tray icon; dictation works without it.

---

## How it works under the hood

```text
   ┌──────────────┐  hold hotkey   ┌──────────┐    audio    ┌────────────┐
   │  Anywhere    │ ─────────────▶│  Bulbul  │ ──────────▶│   Groq     │
   │  on Windows  │                │ overlay  │             │  Whisper   │
   └──────────────┘ ◀── inject text└────┬─────┘ ◀── text  ──┴────────────┘
                                         │
                                         ▼
                                   ┌──────────┐    text     ┌────────────┐
                                   │  Cleanup │ ──────────▶│   Groq     │
                                   │  module  │             │  Llama 3.1 │
                                   └────┬─────┘ ◀── cleaned─┴────────────┘
                                         │
                                         ▼ apply dictionary + snippets
                                    inject via Win32 SendInput
```

The whole loop runs in 400–900ms on a decent connection. Groq is fast.

---

## Build from source

```bash
# Prerequisites: Node 18+, Rust stable, Microsoft C++ Build Tools
git clone https://github.com/codedpool/bulbul.git
cd bulbul
npm install
npm run tauri dev     # dev build with hot reload
npm run tauri build   # production installer in src-tauri/target/release/bundle/
```

---

## Auto-update

Once you're on v1.0.0 or later, Bulbul checks GitHub Releases every 6 hours, downloads new versions in the background, and shows a small banner inviting you to restart. Quitting from the tray installs immediately. Every update is signed with the maintainer's [minisign](https://jedisct1.github.io/minisign/) key — installers that don't match the embedded public key are rejected.

If you forked Bulbul and ship your own builds, generate your own key with `npx tauri signer generate` and replace the `pubkey` in `src-tauri/tauri.conf.json`.

---

## Roadmap

- [ ] **Click-to-talk overlay** — mouse-driven entry point (X / waveform / ✓) alongside the hotkey
- [ ] **macOS port**
- [ ] **Per-app dictionary scoping** — substitutions that only fire in certain apps
- [ ] **Bullet-list detection refinement** — fewer false positives

See [CHANGELOG.md](CHANGELOG.md) for shipped versions.

---

## Contributing

Bulbul is a solo project, but contributions are welcome. Open an issue first to discuss anything bigger than a small fix.

---

## License

[MIT](LICENSE) — do whatever you want, just don't blame me.

---

## Thanks

- [Groq](https://groq.com) for the absurdly fast inference API
- [Tauri](https://tauri.app) for making native Windows apps feel light
