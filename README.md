<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" alt="Bulbul" width="120" height="120" />
</p>

<h1 align="center">Bulbul</h1>

<p align="center">
  <b>Free, open-source voice dictation for Windows, macOS, Linux, and Android.</b><br/>
  Hold a hotkey, talk anywhere, watch the text appear.
</p>

<p align="center">
  <a href="https://github.com/codedpool/bulbul/releases/latest"><img alt="Latest release" src="https://img.shields.io/github/v/release/codedpool/bulbul?color=5ec8c0&label=release" /></a>
  <img alt="Platforms" src="https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux%20%7C%20Android-4c8bf5" />
  <a href="LICENSE"><img alt="License" src="https://img.shields.io/github/license/codedpool/bulbul?color=44b268" /></a>
  <a href="https://github.com/codedpool/bulbul/releases"><img alt="Downloads" src="https://img.shields.io/github/downloads/codedpool/bulbul/total?color=e8873a" /></a>
  <img alt="Powered by Groq" src="https://img.shields.io/badge/powered%20by-Groq-f55036" />
</p>

Bulbul talks directly to [Groq](https://groq.com) using your own API key — no Bulbul-owned server in between, no subscription, no usage caps beyond Groq's own free tier. Your dictation history, dictionary, snippets, and settings all live in a local SQLite file on your machine.

---

## Get started in 60 seconds

**Windows** — PowerShell:

```powershell
irm https://bulbultypes.xyz/install.ps1 | iex
```

**macOS / Linux** — Terminal:

```bash
curl -fsSL https://bulbultypes.xyz/install.sh | sh
```

**Android** — download the `.apk` from the [latest release](https://github.com/codedpool/bulbul/releases/latest) and sideload it.

Each installer pulls the latest release and verifies its minisign signature against the embedded public key before installing. Prefer to click? Grab any installer straight from [Releases](https://github.com/codedpool/bulbul/releases/latest) (Windows SmartScreen warns on first run — see the [SmartScreen FAQ](docs/SMARTSCREEN.md)).

After install:

1. **Open Bulbul** → paste your free [Groq API key](https://console.groq.com/keys) → pick a hotkey
2. **Hold the hotkey anywhere. Speak. Release. Done.**

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

- **Windows** 10/11 (x64), **macOS** 11+ (Apple Silicon or Intel), **Linux** (X11 or Wayland; `.deb` / `.rpm` / AppImage), or **Android** (arm64)
- A **free Groq API key** — [console.groq.com](https://console.groq.com)
- An internet connection (Groq is cloud-hosted) and a microphone

---

## Installation & platform notes

### 🪟 Windows

```powershell
irm https://bulbultypes.xyz/install.ps1 | iex
```

Or download `Bulbul_x.y.z_x64-setup.exe` from [Releases](https://github.com/codedpool/bulbul/releases/latest) and run it (SmartScreen warns on first launch — see the [SmartScreen FAQ](docs/SMARTSCREEN.md)). Typing and "start with Windows" work out of the box.

### 🍎 macOS

```bash
curl -fsSL https://bulbultypes.xyz/install.sh | sh
```

Universal build (Apple Silicon + Intel), macOS 11+. On first launch, grant two permissions in **System Settings → Privacy & Security**:

- **Microphone** — to record your dictation.
- **Accessibility** — so Bulbul can type into other apps and read which app is focused.

If Accessibility stays greyed-out after you enable it, use the **Quit & Relaunch** button on the permission card (works around a stale macOS permission cache on ad-hoc-signed builds).

### 🐧 Linux

```bash
curl -fsSL https://bulbultypes.xyz/install.sh | sh
```

The installer picks the right package for your distro: **`.deb`** (Debian / Ubuntu / Mint), **`.rpm`** (Fedora / openSUSE), or **AppImage** everywhere else.

- **Typing and hotkeys work on both X11 and Wayland.** Bulbul types through a kernel `uinput` virtual keyboard and reads the hotkey directly from the keyboard (evdev) — the one path GNOME Wayland can't block. The `.deb` and `.rpm` grant that access on install via a udev `uaccess` rule, so it works **immediately, with no logout/login**.
- **AppImage** (other distros) has no install step, so on Wayland it falls back to clipboard-paste until you grant `uinput` access yourself — the `.deb` / `.rpm` are the frictionless path.
- **GNOME hides tray icons.** Install the *"AppIndicator and KStatusNotifierItem"* Shell extension to see Bulbul (and its teal "listening" tint) in the top bar — dictation works without it. KDE, XFCE, and Cinnamon show the tray natively.
- Verified on **Linux Mint** (X11), **Ubuntu** (Wayland), and **Fedora** (Wayland).

### 🤖 Android

Download the arm64 **`.apk`** from the [latest release](https://github.com/codedpool/bulbul/releases/latest) and sideload it (allow "install unknown apps" for your browser or file manager when prompted).

- A floating **bubble** rides above your keyboard — **hold or tap it** to dictate into any app.
- Grant **Microphone**, and enable Bulbul's **Accessibility service** so it can type into other apps — the app walks you through both on first run.
- The transcript is injected straight into the focused text field — no clipboard round-trip, no paste toast.

---

## How it works under the hood

```text
   ┌──────────────┐  hold hotkey   ┌──────────┐    audio    ┌────────────┐
   │  Anywhere    │ ─────────────▶│  Bulbul  │ ──────────▶│   Groq     │
   │  you type    │                │ overlay  │             │  Whisper   │
   └──────────────┘ ◀── inject text└────┬─────┘ ◀── text  ──┴────────────┘
                                         │
                                         ▼
                                   ┌──────────┐    text     ┌────────────┐
                                   │  Cleanup │ ──────────▶│   Groq     │
                                   │  module  │             │  Llama 3.1 │
                                   └────┬─────┘ ◀── cleaned─┴────────────┘
                                         │
                                         ▼ apply dictionary + snippets
                              inject into the focused app
```

The whole loop runs in 400–900ms on a decent connection. Groq is fast.

---

## Build from source

```bash
# Prerequisites: Node 18+, Rust stable, and your platform's C toolchain
# (MSVC Build Tools on Windows, Xcode CLT on macOS, webkit2gtk/gtk dev on Linux)
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
- [ ] **Editable transform-slot hotkeys** — rebind the `Alt`/`⌘`+`1..9` transform slots
- [ ] **Per-app dictionary scoping** — substitutions that only fire in certain apps
- [ ] **Wayland app-detection** — per-app style on GNOME/wlroots (X11 already works)

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
- [Tauri](https://tauri.app) for making native cross-platform apps feel light
