#!/usr/bin/env python3
"""Records the sha256 of the sherpa-onnx native archives the speech worker is
built against, so CI can refuse anything that doesn't match.

    python scripts/stt-engine/pin_sherpa.py            # rewrite sherpa-archives.json
    python scripts/stt-engine/pin_sherpa.py --check    # fail if the file is stale

The hashes come from the digests GitHub publishes for each release asset.
Re-run after bumping the pinned sherpa-onnx version.
"""
import json
import sys
import urllib.request
from pathlib import Path

HERE = Path(__file__).parent
VERSION = "2.0.1"  # must equal the `=` pin in crates/bulbul-stt-worker/Cargo.toml
REPO = "k2-fsa/sherpa-onnx"

# platform key (as used in the engine manifest) -> archive the crate downloads
ARCHIVES = {
    "windows-x86_64": f"sherpa-onnx-v{VERSION}-win-x64-static-MT-Release-lib.tar.bz2",
    "windows-aarch64": f"sherpa-onnx-v{VERSION}-win-arm64-static-MT-Release-lib.tar.bz2",
    "macos-aarch64": f"sherpa-onnx-v{VERSION}-osx-arm64-static-lib.tar.bz2",
    "macos-x86_64": f"sherpa-onnx-v{VERSION}-osx-x64-static-lib.tar.bz2",
    "linux-x86_64": f"sherpa-onnx-v{VERSION}-linux-x64-static-lib.tar.bz2",
    "linux-aarch64": f"sherpa-onnx-v{VERSION}-linux-aarch64-static-lib.tar.bz2",
    # The Android app loads these through JNI (jniLibs/<abi>/).
    "android": f"sherpa-onnx-v{VERSION}-android.tar.bz2",
}


def release_digests():
    req = urllib.request.Request(
        f"https://api.github.com/repos/{REPO}/releases/tags/v{VERSION}",
        headers={"Accept": "application/vnd.github+json", "User-Agent": "bulbul-pin"},
    )
    with urllib.request.urlopen(req, timeout=60) as r:
        data = json.load(r)
    return {a["name"]: (a.get("digest") or "", a["size"]) for a in data["assets"]}


def main():
    digests = release_digests()
    out = {"sherpa_onnx_version": VERSION, "archives": {}}
    for key, name in ARCHIVES.items():
        digest, size = digests.get(name, ("", 0))
        if not digest.startswith("sha256:"):
            sys.exit(f"no sha256 digest published for {name}")
        out["archives"][key] = {"name": name, "size": size, "sha256": digest[len("sha256:"):]}
    text = json.dumps(out, indent=2) + "\n"
    target = HERE / "sherpa-archives.json"
    if "--check" in sys.argv:
        if not target.exists() or target.read_text() != text:
            sys.exit("sherpa-archives.json is stale; run pin_sherpa.py")
        print("up to date")
    else:
        target.write_text(text)
        print(f"wrote {target}")


if __name__ == "__main__":
    main()
