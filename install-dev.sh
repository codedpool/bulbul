#!/usr/bin/env bash
# Bulbul macOS dev installer
# Usage:  curl -fsSL https://bulbultypes.xyz/install-dev.sh | sh
#
# Pulls the latest rolling dev build (the macos-port branch), verifies its
# minisign signature against the public key baked in below, installs to
# /Applications, strips the quarantine attribute so Gatekeeper doesn't
# intercept, and launches. Two external dependencies: minisign + brew
# (suggested only if minisign is missing). No Python, no jq — uses plutil
# which ships with macOS itself.
#
# Note: this is the *dev* installer for the macOS port in flight. Once
# v1.1.0 ships, install.sh (stable) takes over and this file is retired.

set -euo pipefail

# --- Config ----------------------------------------------------------------
MANIFEST_URL='https://github.com/codedpool/bulbul/releases/download/macos-dev/latest.json'
MINISIGN_KEY='RWTLvdvsrlMNS4LQvsKO03T8kF+5jZ1s7KiyU4lKZmYPcd0+1qxm2gKt'

# --- Pre-flight ------------------------------------------------------------
if [ "$(uname -s)" != 'Darwin' ]; then
  echo 'install-dev.sh is for macOS only. On Windows use install.ps1.' >&2
  exit 1
fi

case "$(uname -m)" in
  arm64)  PLATFORM='darwin-aarch64' ;;
  x86_64) PLATFORM='darwin-x86_64' ;;
  *)      echo "Unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

if ! command -v minisign >/dev/null 2>&1; then
  echo 'minisign is required for signature verification.'
  echo 'Install it with:  brew install minisign'
  exit 1
fi

echo
echo '  Bulbul dev installer'
echo '  --------------------'
echo

# --- Workspace -------------------------------------------------------------
TMPDIR="$(mktemp -d -t bulbul-dev.XXXXXX)"
trap 'rm -rf "$TMPDIR"' EXIT

# --- Fetch manifest --------------------------------------------------------
echo '  > Fetching latest dev build info...'
MANIFEST="$TMPDIR/latest.json"
curl -fsSL "$MANIFEST_URL" -o "$MANIFEST"

# plutil is built into macOS; no external dep. Reads the same JSON.
VERSION=$(plutil -extract version raw -o - -- "$MANIFEST")
URL=$(plutil -extract "platforms.${PLATFORM}.url" raw -o - -- "$MANIFEST")
SIG_B64=$(plutil -extract "platforms.${PLATFORM}.signature" raw -o - -- "$MANIFEST")

if [ -z "$URL" ] || [ -z "$SIG_B64" ]; then
  echo "  ! No build found for platform $PLATFORM in the release manifest." >&2
  exit 1
fi
echo "    Bulbul $VERSION ($PLATFORM)"

# --- Download bundle + signature ------------------------------------------
echo '  > Downloading bundle...'
BUNDLE="$TMPDIR/Bulbul.app.tar.gz"
SIG="$TMPDIR/Bulbul.app.tar.gz.sig"
curl -fsSL "$URL" -o "$BUNDLE"
# latest.json stores the .sig file content base64-encoded.
printf '%s' "$SIG_B64" | base64 --decode > "$SIG"

# --- Verify ----------------------------------------------------------------
echo '  > Verifying signature...'
if ! minisign -V -P "$MINISIGN_KEY" -m "$BUNDLE" -x "$SIG" >/dev/null; then
  echo '  ! Signature verification failed. Aborting.' >&2
  exit 1
fi
echo '    Signature verified.'

# --- Install ---------------------------------------------------------------
echo '  > Installing to /Applications...'
EXTRACT="$TMPDIR/extract"
mkdir -p "$EXTRACT"
tar -xzf "$BUNDLE" -C "$EXTRACT"

APP=$(find "$EXTRACT" -maxdepth 2 -name '*.app' -print -quit)
if [ -z "$APP" ]; then
  echo "  ! Couldn't locate a .app inside the bundle." >&2
  exit 1
fi

# Replace any previous install. sudo only if /Applications denies rm.
if [ -d '/Applications/Bulbul.app' ]; then
  rm -rf '/Applications/Bulbul.app' 2>/dev/null || sudo rm -rf '/Applications/Bulbul.app'
fi
mv "$APP" '/Applications/' 2>/dev/null || sudo mv "$APP" '/Applications/'

# Strip quarantine so Gatekeeper doesn't intercept on first launch.
xattr -dr com.apple.quarantine '/Applications/Bulbul.app' || true

# --- Launch ----------------------------------------------------------------
echo
echo "  Bulbul $VERSION installed."
echo '  Opening Bulbul...'
open -a '/Applications/Bulbul.app'
echo
