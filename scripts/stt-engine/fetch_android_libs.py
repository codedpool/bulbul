#!/usr/bin/env python3
"""Puts sherpa-onnx's Android libraries where the app build picks them up.

    python scripts/stt-engine/fetch_android_libs.py

Downloads the Android archive named in sherpa-archives.json, refuses it unless
its sha256 matches the pinned one, and extracts the arm64 libraries into
src-tauri/gen/android/app/src/main/jniLibs/arm64-v8a/ (git-ignored). Run it
before building the Android app; CI does the same.
"""
import hashlib
import json
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path

HERE = Path(__file__).parent
ROOT = HERE.parents[1]
ABI = "arm64-v8a"
LIBS = ["libsherpa-onnx-jni.so", "libonnxruntime.so"]
DEST = ROOT / "src-tauri" / "gen" / "android" / "app" / "src" / "main" / "jniLibs" / ABI


def main():
    pins = json.loads((HERE / "sherpa-archives.json").read_text())
    pin = pins["archives"]["android"]
    url = f"https://github.com/k2-fsa/sherpa-onnx/releases/download/v{pins['sherpa_onnx_version']}/{pin['name']}"
    with tempfile.TemporaryDirectory() as tmp:
        archive = Path(tmp) / pin["name"]
        print("downloading", url)
        urllib.request.urlretrieve(url, archive)
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        if digest != pin["sha256"]:
            sys.exit(f"{pin['name']}: sha256 {digest} does not match the pinned {pin['sha256']}")
        DEST.mkdir(parents=True, exist_ok=True)
        with tarfile.open(archive, "r:bz2") as tar:
            wanted = {f"jniLibs/{ABI}/{n}": n for n in LIBS}
            found = set()
            for member in tar.getmembers():
                # Match by suffix: the archive may or may not have a top folder.
                for suffix, name in wanted.items():
                    if member.isfile() and member.name.replace("\\", "/").endswith(suffix):
                        (DEST / name).write_bytes(tar.extractfile(member).read())
                        found.add(name)
            missing = set(LIBS) - found
            if missing:
                sys.exit(f"missing from the archive: {', '.join(sorted(missing))}")
    for n in LIBS:
        print(f"  {n}: {(DEST / n).stat().st_size // 1024} KB")


if __name__ == "__main__":
    main()
