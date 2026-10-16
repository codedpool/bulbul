# Contributing to Bulbul

Bulbul is a small open-source project maintained by one person. Contributions are welcome, and so is just hanging out in [Discussions](https://github.com/codedpool/bulbul/discussions) telling me what you wish it did differently.

## TL;DR

- **Found a bug?** Open an [issue](https://github.com/codedpool/bulbul/issues/new/choose) using the **Bug report** template.
- **Have a feature idea?** Start a thread in [Discussions -> Ideas](https://github.com/codedpool/bulbul/discussions/categories/ideas) first. If we land on a concrete design, that becomes an issue + a PR.
- **Just want help?** [Discussions -> Q&A](https://github.com/codedpool/bulbul/discussions/categories/q-a).
- **Sending a PR?** Open an issue first for anything bigger than a typo fix, so we don't both build the wrong thing.

## Setup

Bulbul is a Tauri 2 app - Rust backend, React/Vite frontend, plus a Kotlin layer on Android. It ships on Windows, macOS, Linux, and Android.

Everyone needs:

- **Node 20.19+** (Vite 7 requires it; Node 18 can't run the dev server)
- **Rust stable** (`rustup install stable`)

Plus, per platform:

- **Windows:** [Microsoft C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) (pick "Desktop development with C++") and the [WebView2 Runtime](https://developer.microsoft.com/en-us/microsoft-edge/webview2/) (already installed on Windows 11).
- **macOS:** Xcode Command Line Tools (`xcode-select --install`).
- **Linux (Debian/Ubuntu):** `sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev libasound2-dev patchelf` - the same list the release workflow installs.
- **Android:** JDK 17, the Android SDK + NDK, and the arm64 Rust target (`rustup target add aarch64-linux-android`).

Then:

```bash
git clone https://github.com/codedpool/bulbul.git
cd bulbul
npm install
npm run tauri dev
```

That'll spin up a desktop dev build with hot reload on the frontend and rebuild-on-save on the Rust side.

Production build:

```bash
npm run tauri build
```

Installers land in `src-tauri/target/release/bundle/` - NSIS `-setup.exe` + MSI on Windows, a universal DMG on macOS, `.deb` / `.rpm` / AppImage on Linux.

Android APK (arm64):

```bash
npx tauri android build --apk --target aarch64
```

The APK lands under `src-tauri/gen/android/app/build/outputs/apk/`.

**Linux, running from source:** typing and the hotkey go straight to the kernel - `/dev/uinput` for typing, `/dev/input/event*` (evdev) for the hotkey. The `.deb` / `.rpm` installers grant that access on install via a udev `uaccess` rule (see `src-tauri/deb/postinst.sh`), no logout needed. A dev build skips the installer, so on Wayland set that up by hand first or typing and the hotkey will silently not work.

## Project layout

```text
src/                          # React frontend (Vite, React 19)
  App.jsx                     # Dashboard shell + routing
  Overlay.jsx                 # Floating recording pill
  ScratchpadWindow.jsx        # Standalone notes window
  views/                      # Top-level pages (Home, Insights, Dictionary, Snippets,
                              #   Style, Transforms, Scratchpad, Settings)
  onboarding/                 # First-run wizard
  components/                 # Shared UI bits (Combobox, ConfirmDialog, tooltips, ...)
  platform.js                 # Desktop vs Android capability checks
  assets/                     # Static images
src-tauri/                    # Rust backend (Tauri 2)
  src/
    lib.rs                    # Entry point - cfg-includes desktop.rs or mobile.rs
    desktop.rs                # Desktop orchestrator - pipeline, tray, windows, IPC
    mobile.rs                 # Android entry (mobile_entry_point) + JNI surface
    audio.rs                  # Desktop mic capture (cpal + WAV encoding)
    hotkey/                   # Hotkey registration + per-OS backends (evdev/portal on Linux)
    inject/                   # Text injection per OS (clipboard+paste on Win/Mac,
                              #   uinput/portal on Linux)
    groq.rs                   # Groq Whisper STT + cleanup LLM client
    model_config.rs           # Remote cleanup-model chain, fetched from
                              #   bulbultypes.xyz and cached locally
    db.rs                     # SQLite - dictation log, dictionary, snippets, transforms
    config.rs                 # Config schema, persistence, defaults
    telemetry.rs              # Opt-out anonymous usage stats -> Supabase
    correction.rs             # Learns from the edits you make by hand after dictating
    window_info/              # Focused-window/process lookup, per OS
    uia/                      # Focused-element text reader, per OS (correction memory)
    keyboard_hook.rs          # Low-level keyboard hook (Windows)
    mouse_hook.rs             # Mouse-button hotkey hook (Windows)
    linux_env.rs              # Linux session introspection (Wayland vs X11, DE quirks)
  gen/android/                # Android app in Kotlin - AudioRecord capture,
                              #   Accessibility Service injection, overlay bubble
  capabilities/               # Tauri 2 permission boundaries
  tauri.conf.json             # Window setup, bundle, updater config (plus per-OS
                              #   tauri.linux/macos/android.conf.json overrides)
  deb/, rpm/                  # Linux post-install scripts (the uinput/udev setup)
.github/workflows/release.yml # Tag push -> signed installers for all four platforms
                              #   + version-less "stable alias" copies
.github/workflows/*-dev.yml   # Manual rolling dev builds per platform
docs/                         # End-user docs (SmartScreen FAQ)
install.ps1                   # Windows install script served from bulbultypes.xyz
```

## Code style

### Rust

- **Format with `rustfmt`** before committing. `cargo fmt` is the truth.
- **Comments explain *why*, not *what*.** A function named `start_recording` doesn't need a comment saying "starts recording". A non-obvious workaround, a subtle invariant, a hard-won bug fix - those get comments.
- **Use `tracing` for logging**, not `println!`. Pick a level: `error!` for things that hurt the user, `warn!` for things to investigate, `info!` for boot/lifecycle and per-dictation summary, `debug!` for noisy detail.
- **Errors via `anyhow::Result`** at boundaries; only invent new error types when you have a reason to match on them in caller code.
- **Don't add features speculatively.** If the bug fix is three lines, ship three lines. Surrounding cleanup belongs in its own PR.

### Frontend (React)

- **Functional components + hooks** throughout. No classes.
- **Inline styles only for runtime-computed values.** Static styles go in `App.css` or a feature-specific stylesheet.
- **Tauri commands via `invoke("name", { args })`** - keep names snake_case to match the Rust side.
- **Listen for events with the cleanup pattern** from `useEffect` - `un.then(f => f())` on unmount, otherwise listeners leak across navigations.
- **No prop-drilling for global app state.** Lift to `App.jsx` and pass down - there are only a handful of leaf views.

### Commit messages

Look at recent commits for the house style. Short imperative subject (`Add X`, `Drop Y`, `Make Z better`), under ~70 chars. If the *why* needs explaining, add a blank line + a body that fits in a terminal.

Don't @-mention the maintainer in the commit; the PR/issue thread is where conversation belongs.

## Testing

- **Rust unit tests** live in the file they're testing under `#[cfg(test)]` modules. Run `cargo test` from `src-tauri/`.
- **No JS unit tests yet** - the React surface is thin and most of it is glue code. If you change the dictation/cleanup pipeline, the meaningful test is `npm run tauri dev` and *actually dictating*.
- **Manual smoke test before submitting a PR**: hold the hotkey, dictate one sentence, confirm it lands cleanly in a plain text field (Notepad on Windows, TextEdit on macOS, any editor on Linux). If you touched cleanup, also try one Polish-hotkey dictation. On Android, hold the floating bubble and dictate into any app's text field.

## Things that need fixing right now

Look at:

- [Issues tagged `good first issue`](https://github.com/codedpool/bulbul/issues?q=is%3Aopen+label%3A%22good+first+issue%22) - small, well-scoped, no architecture decisions involved
- [Issues tagged `help wanted`](https://github.com/codedpool/bulbul/issues?q=is%3Aopen+label%3A%22help+wanted%22) - bigger, but the design is clear
- [The Unreleased section in CHANGELOG.md](CHANGELOG.md#unreleased) - what's on deck

## What probably won't get merged

- **Telemetry payload expansions** without a clear question they'd answer
- **Code-style sweeps** across the whole repo (one file at a time is fine)
- **New dependencies** that solve something we can solve in 30 lines
- **iOS abstractions** before an iOS port is actually underway - desktop and Android share very little today, and speculative shared layers just add dead code
- **Breaking changes** to the on-disk config or SQLite schema without a documented migration path

## Reporting security issues

Please don't open public issues for anything that looks like a credential leak, an injection vector, or a way to deliver a malicious update. Email `devshooked@gmail.com` instead. I'll respond within a few days and credit you in the fix unless you'd rather stay anonymous.

## License

By contributing, you agree your changes are released under the [GPL-3.0 license](LICENSE) that covers the rest of the project. There's no separate CLA.

## Thanks

If you got this far, you're already helping. Feel free to drop into [Discussions](https://github.com/codedpool/bulbul/discussions) and say hi.
