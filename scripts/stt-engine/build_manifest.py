#!/usr/bin/env python3
"""Packs the per-platform speech worker builds into release assets and writes
the manifest the app reads.

    python build_manifest.py <dir-with-one-folder-per-platform> <out-dir> <tag-url-base>

`<dir>/<platform>/bulbul-stt-worker[.exe]` -> `<out>/bulbul-stt-worker-<version>-<platform>.gz`
plus `<out>/stt-engine-manifest.json` (sign it afterwards).
"""
import gzip
import hashlib
import json
import re
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CRATE = ROOT / "src-tauri" / "crates" / "bulbul-stt-worker"
PROTOCOL_SRC = ROOT / "src-tauri" / "crates" / "bulbul-local-stt" / "src" / "protocol.rs"


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def worker_version() -> str:
    m = re.search(r'^version\s*=\s*"([^"]+)"', (CRATE / "Cargo.toml").read_text(encoding="utf-8"), re.M)
    return m.group(1)


def protocol_version() -> int:
    m = re.search(r"PROTOCOL_VERSION:\s*u32\s*=\s*(\d+)", PROTOCOL_SRC.read_text(encoding="utf-8"))
    return int(m.group(1))


def main():
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    src, out, base = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3].rstrip("/")
    version = worker_version()
    out.mkdir(parents=True, exist_ok=True)
    assets = {}
    for plat in sorted(p for p in src.iterdir() if p.is_dir()):
        exe = next((f for f in plat.iterdir() if f.name.startswith("bulbul-stt-worker")), None)
        if exe is None:
            sys.exit(f"no worker program in {plat}")
        gz = out / f"bulbul-stt-worker-{version}-{plat.name}.gz"
        with open(exe, "rb") as i, gzip.GzipFile(gz, "wb", compresslevel=9, mtime=0) as o:
            shutil.copyfileobj(i, o)
        assets[plat.name] = {
            "url": f"{base}/{gz.name}",
            "size": gz.stat().st_size,
            "sha256": sha256(gz),
            "exe_size": exe.stat().st_size,
            "exe_sha256": sha256(exe),
        }
        print(f"{plat.name}: {exe.stat().st_size} -> {gz.stat().st_size} bytes")
    manifest = {"schema": 1, "version": version, "protocol": protocol_version(), "assets": assets}
    (out / "stt-engine-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    main()
