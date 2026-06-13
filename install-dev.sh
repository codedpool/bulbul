#!/usr/bin/env bash
# Bulbul dev installer (macOS + Linux)
# Usage:  curl -fsSL https://bulbultypes.xyz/install-dev.sh | sh
#
# Detects the OS, pulls the latest rolling dev build for that platform
# from the v1.1-port branch, verifies its minisign signature against
# the public key baked in below, installs locally, and launches.
#
# Two dev releases drive this:
#   - macos-dev (built by .github/workflows/macos-dev.yml) — DMG +
#     .app.tar.gz, universal Apple Silicon + Intel.
#   - linux-dev (built by .github/workflows/linux-dev.yml) — .AppImage
#     primary, .deb and .rpm secondary.
#
# Dependencies the script expects to be present:
#   - minisign (verification). On macOS: brew install minisign. On
#     Linux: apt/dnf install minisign (most distros ship it).
#   - plutil (macOS only — built into the OS).
#   - python3 (Linux only — Ubuntu 20.04+, Fedora, Arch all ship it).
#
# This is the *dev* installer for the in-flight v1.1.0 port. Once
# v1.1.0 ships, install.sh (stable) takes over and this file retires.

set -euo pipefail

# --- Config ----------------------------------------------------------------
# Prehashed Ed25519 (algorithm byte "ED") — tauri-action moved to
# prehashed signing in a recent version. Same underlying Ed25519
# keypair as the legacy "Ed" public key shipped with v1.0.0; only the
# algorithm prefix differs (byte 2: 0x64 → 0x44, base64 char 2: W → U),
# so minisign verifies prehashed signatures using the BLAKE2b-of-file
# path instead of raw-file path. Key ID `cbbddbecae530d4b` matches.
MINISIGN_KEY='RUTLvdvsrlMNS4LQvsKO03T8kF+5jZ1s7KiyU4lKZmYPcd0+1qxm2gKt'

# --- Pre-flight: detect OS + arch -----------------------------------------
case "$(uname -s)" in
  Darwin) OS='macos' ;;
  Linux)  OS='linux' ;;
  *)      echo "install-dev.sh: unsupported OS $(uname -s). On Windows use install.ps1." >&2; exit 1 ;;
esac

ARCH_RAW="$(uname -m)"
case "$ARCH_RAW" in
  arm64|aarch64) ARCH='aarch64' ;;
  x86_64|amd64)  ARCH='x86_64' ;;
  *)             echo "install-dev.sh: unsupported architecture $ARCH_RAW" >&2; exit 1 ;;
esac

if ! command -v minisign >/dev/null 2>&1; then
  echo 'install-dev.sh: minisign is required for signature verification.'
  if [ "$OS" = 'macos' ]; then
    echo 'Install with:  brew install minisign'
  else
    echo 'Install with your package manager, e.g.:'
    echo '  Ubuntu/Debian:  sudo apt install minisign'
    echo '  Fedora:         sudo dnf install minisign'
    echo '  Arch:           sudo pacman -S minisign'
  fi
  exit 1
fi

if [ "$OS" = 'macos' ]; then
  MANIFEST_URL='https://github.com/codedpool/bulbul/releases/download/macos-dev/latest.json'
  PLATFORM_KEY="darwin-${ARCH}"
else
  MANIFEST_URL='https://github.com/codedpool/bulbul/releases/download/linux-dev/latest.json'
  PLATFORM_KEY="linux-${ARCH}"
  if ! command -v python3 >/dev/null 2>&1; then
    echo 'install-dev.sh: python3 is required on Linux to parse the release manifest.' >&2
    exit 1
  fi
fi

echo
echo "  Bulbul dev installer ($OS-$ARCH)"
echo '  --------------------'
echo

# --- Workspace -------------------------------------------------------------
TMPDIR="$(mktemp -d -t bulbul-dev.XXXXXX)"
trap 'rm -rf "$TMPDIR"' EXIT

# --- Fetch manifest --------------------------------------------------------
echo '  > Fetching latest dev build info...'
MANIFEST="$TMPDIR/latest.json"
curl -fsSL "$MANIFEST_URL" -o "$MANIFEST"

# Parse the manifest. macOS uses plutil (built in); Linux uses python3.
# Same JSON shape from tauri-action's latest.json.
if [ "$OS" = 'macos' ]; then
  VERSION=$(plutil -extract version raw -o - -- "$MANIFEST")
  URL=$(plutil -extract "platforms.${PLATFORM_KEY}.url" raw -o - -- "$MANIFEST")
  SIG_B64=$(plutil -extract "platforms.${PLATFORM_KEY}.signature" raw -o - -- "$MANIFEST")
else
  VERSION=$(python3 -c "import json,sys; print(json.load(open('$MANIFEST'))['version'])")
  URL=$(python3 -c "import json,sys; print(json.load(open('$MANIFEST'))['platforms']['$PLATFORM_KEY']['url'])")
  SIG_B64=$(python3 -c "import json,sys; print(json.load(open('$MANIFEST'))['platforms']['$PLATFORM_KEY']['signature'])")
fi

if [ -z "$URL" ] || [ -z "$SIG_B64" ]; then
  echo "  ! No build found for platform $PLATFORM_KEY in the release manifest." >&2
  exit 1
fi
echo "    Bulbul $VERSION ($PLATFORM_KEY)"

# --- Download bundle + signature ------------------------------------------
echo '  > Downloading bundle...'
if [ "$OS" = 'macos' ]; then
  BUNDLE="$TMPDIR/Bulbul.app.tar.gz"
else
  BUNDLE="$TMPDIR/Bulbul.AppImage"
fi
SIG="$BUNDLE.sig"

curl -fsSL "$URL" -o "$BUNDLE"
printf '%s' "$SIG_B64" | base64 --decode > "$SIG"

# --- Verify ----------------------------------------------------------------
echo '  > Verifying signature...'
if ! minisign -V -P "$MINISIGN_KEY" -m "$BUNDLE" -x "$SIG" >/dev/null; then
  echo '  ! Signature verification failed. Aborting.' >&2
  exit 1
fi
echo '    Signature verified.'

# --- Install ---------------------------------------------------------------
if [ "$OS" = 'macos' ]; then
  echo '  > Installing to /Applications...'
  EXTRACT="$TMPDIR/extract"
  mkdir -p "$EXTRACT"
  tar -xzf "$BUNDLE" -C "$EXTRACT"

  APP=$(find "$EXTRACT" -maxdepth 2 -name '*.app' -print -quit)
  if [ -z "$APP" ]; then
    echo "  ! Couldn't locate a .app inside the bundle." >&2
    exit 1
  fi

  if [ -d '/Applications/Bulbul.app' ]; then
    rm -rf '/Applications/Bulbul.app' 2>/dev/null || sudo rm -rf '/Applications/Bulbul.app'
  fi
  mv "$APP" '/Applications/' 2>/dev/null || sudo mv "$APP" '/Applications/'

  # Strip quarantine so Gatekeeper doesn't intercept on first launch.
  xattr -dr com.apple.quarantine '/Applications/Bulbul.app' || true

  INSTALL_PATH='/Applications/Bulbul.app'
  echo
  echo "  Bulbul $VERSION installed to /Applications/Bulbul.app"
  echo '  Opening Bulbul...'
  open -a "$INSTALL_PATH"
else
  echo '  > Installing to ~/.local/bin...'
  INSTALL_DIR="$HOME/.local/bin"
  mkdir -p "$INSTALL_DIR"
  INSTALL_PATH="$INSTALL_DIR/Bulbul.AppImage"
  mv "$BUNDLE" "$INSTALL_PATH"
  chmod +x "$INSTALL_PATH"

  # Write a .desktop launcher so Bulbul shows up in the user's app
  # menu. Keeps the dev install discoverable without polluting system
  # paths.
  DESKTOP_DIR="$HOME/.local/share/applications"
  mkdir -p "$DESKTOP_DIR"
  cat > "$DESKTOP_DIR/bulbul-dev.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Bulbul (dev)
Exec=$INSTALL_PATH
Icon=bulbul
Categories=Utility;
Terminal=false
EOF

  echo
  echo "  Bulbul $VERSION installed to $INSTALL_PATH"
  echo '  Launcher entry: ~/.local/share/applications/bulbul-dev.desktop'
  echo '  Launching...'
  # nohup + & so the AppImage detaches from this shell session.
  nohup "$INSTALL_PATH" >/dev/null 2>&1 &
fi
echo
