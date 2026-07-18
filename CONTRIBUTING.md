# Contributing to Bulbul

Bulbul is a small open-source project maintained by one person. Contributions are welcome, and so is just hanging out in [Discussions](https://github.com/codedpool/bulbul/discussions) telling me what you wish it did differently.

## TL;DR

- **Found a bug?** Open an [issue](https://github.com/codedpool/bulbul/issues/new/choose) using the **Bug report** template.
- **Have a feature idea?** Start a thread in [Discussions → Ideas](https://github.com/codedpool/bulbul/discussions/categories/ideas) first. If we land on a concrete design, that becomes an issue + a PR.
- **Just want help?** [Discussions → Q&A](https://github.com/codedpool/bulbul/discussions/categories/q-a).
- **Sending a PR?** Open an issue first for anything bigger than a typo fix, so we don't both build the wrong thing.

## Setup

Bulbul is a Tauri 2 app — Rust backend, React/Vite frontend. You need:

- **Node 18+**
- **Rust stable** (`rustup install stable`)
- **Microsoft C++ Build Tools** ([download](https://visualstudio.microsoft.com/visual-cpp-build-tools/) — pick "Desktop development with C++")
- **WebView2 Runtime** (already installed on Windows 11; on Windows 10, get it from [Microsoft](https://developer.microsoft.com/en-us/microsoft-edge/webview2/))

Then:

```bash
git clone https://github.com/codedpool/bulbul.git
cd bulbul
npm install
npm run tauri dev
```

That'll spin up a dev build with hot reload on the frontend and rebuild-on-save on the Rust side.

Production build:

```bash
npm run tauri build
```

Installer lands in `src-tauri/target/release/bundle/nsis/` (NSIS `-setup.exe`) and `…/msi/` (MSI).

## Project layout

```text
src/                          # React frontend (Vite)
  App.jsx                     # Dashboard shell + routing
  views/                      # Top-level pages (Home, Insights, Dictionary, …)
  onboarding/                 # First-run wizard
  assets/                     # Static images
src-tauri/                    # Rust backend (Tauri 2)
  src/
    lib.rs                    # Orchestrator — pipeline, tray, windows, IPC
    audio.rs                  # Mic capture (cpal + WAV encoding)
    hotkey.rs                 # Modifier-chord watcher + global shortcuts
    groq.rs                   # Whisper STT + Llama cleanup HTTP client
    db.rs                     # SQLite — dictation log, dictionary, snippets, transforms
    config.rs                 # Config schema, persistence, defaults
    telemetry.rs              # Opt-out anonymous usage stats → Supabase
    inject.rs                 # Win32 SendInput-based text injection
    correction.rs             # V3.1 correction memory (watch fields for edits)
    window_info.rs            # Win32 GetForegroundWindow + process name lookup
    uia.rs                    # UI Automation helpers
  tauri.conf.json             # Window setup, bundle, updater config
  capabilities/               # Tauri 2 permission boundaries
.github/workflows/release.yml # Signed-build pipeline on tag push
docs/                         # End-user docs (SmartScreen FAQ, etc.)
```

## Code style

### Rust

- **Format with `rustfmt`** before committing. `cargo fmt` is the truth.
- **Comments explain *why*, not *what*.** A function named `start_recording` doesn't need a comment saying "starts recording". A non-obvious workaround, a subtle invariant, a hard-won bug fix — those get comments.
- **Use `tracing` for logging**, not `println!`. Pick a level: `error!` for things that hurt the user, `warn!` for things to investigate, `info!` for boot/lifecycle and per-dictation summary, `debug!` for noisy detail.
- **Errors via `anyhow::Result`** at boundaries; only invent new error types when you have a reason to match on them in caller code.
- **Don't add features speculatively.** If the bug fix is three lines, ship three lines. Surrounding cleanup belongs in its own PR.

### Frontend (React)

- **Functional components + hooks** throughout. No classes.
- **Inline styles only for runtime-computed values.** Static styles go in `App.css` or a feature-specific stylesheet.
- **Tauri commands via `invoke("name", { args })`** — keep names snake_case to match the Rust side.
- **Listen for events with the cleanup pattern** from `useEffect` — `un.then(f => f())` on unmount, otherwise listeners leak across navigations.
- **No prop-drilling for global app state.** Lift to `App.jsx` and pass down — there are only a handful of leaf views.

### Commit messages

Look at recent commits for the house style. Short imperative subject (`Add X`, `Drop Y`, `Make Z better`), under ~70 chars. If the *why* needs explaining, add a blank line + a body that fits in a terminal.

```text
Make cleanup app-aware: bullet lists, venue hints, custom apps

- Cleanup modes (Clean + Polished) now emit a markdown bullet list when
  the speaker enumerates items ("first ... second ...", "one ... two ..."),
  dropping the enumerator words. Single-sentence prose is untouched.
- ...
```

Don't @-mention the maintainer in the commit; the PR/issue thread is where conversation belongs.

## Testing

- **Rust unit tests** live in the file they're testing under `#[cfg(test)]` modules. Run `cargo test` from `src-tauri/`.
- **No JS unit tests yet** — the React surface is thin and most of it is glue code. If you change the dictation/cleanup pipeline, the meaningful test is `npm run tauri dev` and *actually dictating*.
- **Manual smoke test before submitting a PR**: hold the hotkey, dictate one sentence, confirm it lands cleanly in Notepad. If you touched cleanup, also try one Polish-hotkey dictation.

## Things that need fixing right now

Look at:

- [Issues tagged `good first issue`](https://github.com/codedpool/bulbul/issues?q=is%3Aopen+label%3A%22good+first+issue%22) — small, well-scoped, no architecture decisions involved
- [Issues tagged `help wanted`](https://github.com/codedpool/bulbul/issues?q=is%3Aopen+label%3A%22help+wanted%22) — bigger, but the design is clear
- [The Unreleased section in CHANGELOG.md](CHANGELOG.md#unreleased) — what's on deck

## What probably won't get merged

- **Telemetry payload expansions** without a clear question they'd answer
- **Code-style sweeps** across the whole repo (one file at a time is fine)
- **New dependencies** that solve something we can solve in 30 lines
- **Cross-platform abstractions** before the macOS port lands — Bulbul is Windows-only today and pretending otherwise just adds dead code
- **Breaking changes** to the on-disk config or SQLite schema without a documented migration path

## Reporting security issues

Please don't open public issues for anything that looks like a credential leak, an injection vector, or a way to deliver a malicious update. Email `devshooked@gmail.com` instead. I'll respond within a few days and credit you in the fix unless you'd rather stay anonymous.

## License

By contributing, you agree your changes are released under the [GPL-3.0 license](LICENSE) that covers the rest of the project. There's no separate CLA.

## Thanks

If you got this far, you're already helping. Feel free to drop into [Discussions](https://github.com/codedpool/bulbul/discussions) and say hi.
