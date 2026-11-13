#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
# Copyright (c) 2026 Romanch Roshan Singh
"""Generates src-tauri/crates/bulbul-local-stt/catalog.json.

Every size and sha256 in the catalog comes from the real file, never typed by
hand. Archives are downloaded once (cached), hashed, cross-checked against the
digest GitHub publishes when there is one, and opened to learn their contents.
Per-file sources take size and sha256 from Hugging Face's LFS metadata (small
non-LFS files are downloaded and hashed here), and are pinned to a commit.

Usage:
  python build_catalog.py --inspect   # list what's inside each archive
  python build_catalog.py             # write catalog.json

Standard library only. Set GITHUB_TOKEN if you hit GitHub's anonymous rate limit.
"""
import argparse
import datetime
import hashlib
import json
import os
import re
import sys
import tarfile
import tempfile
import urllib.request

OUT = os.path.join(os.path.dirname(__file__), "..", "..", "src-tauri", "crates",
                   "bulbul-local-stt", "catalog.json")
GH_RELEASE = "https://api.github.com/repos/k2-fsa/sherpa-onnx/releases/tags/asr-models"
GH_DOWNLOAD = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/"
HF = "https://huggingface.co"
SMALL_FILE = 5 * 1024 * 1024  # non-LFS files below this are hashed locally

# Licence texts shown next to a model. CC-BY needs the credit line.
LIC = {
    "mit": ("MIT", "https://opensource.org/license/mit"),
    "apache": ("Apache-2.0", "https://www.apache.org/licenses/LICENSE-2.0"),
    "ccby": ("CC-BY-4.0", "https://creativecommons.org/licenses/by/4.0/"),
}
NVIDIA_CREDIT = ("(c) NVIDIA, licensed under CC-BY-4.0 "
                 "(https://creativecommons.org/licenses/by/4.0/). Converted to ONNX and quantised "
                 "to int8 by third parties; NVIDIA does not endorse Bulbul.")

# role -> regex that must match exactly one kept file (matched on the path inside the model folder).
MODELS = [
    dict(
        id="moonshine-tiny-en", name="Moonshine tiny (English)", kind="moonshine",
        summary="Tiny and quick. English only, good for low-end machines.",
        languages=["en"], multilingual=False, experimental=False,
        lic="mit", author="Moonshine AI", converted=True,
        archive="sherpa-onnx-moonshine-tiny-en-quantized-2026-02-27.tar.bz2",
        keep=r"\.(onnx|ort)$|tokens\.txt$",
        layout={"encoder": "encoder_model.ort", "merged_decoder": "decoder_model_merged.ort",
                "tokens": "tokens.txt"},
    ),
    dict(
        id="parakeet-tdt-110m-en", name="Parakeet TDT 110M (English)", kind="transducer",
        summary="Very fast English on an ordinary CPU.",
        languages=["en"], multilingual=False, experimental=False,
        lic="ccby", author="NVIDIA", converted=True, credit=NVIDIA_CREDIT,
        archive="sherpa-onnx-nemo-parakeet_tdt_transducer_110m-en-36000-int8.tar.bz2",
        keep=r"\.onnx$|tokens\.txt$",
        layout={"encoder": "encoder.int8.onnx", "decoder": "decoder.int8.onnx",
                "joiner": "joiner.int8.onnx", "tokens": "tokens.txt"},
    ),
    dict(
        id="whisper-tiny", name="Whisper tiny", kind="whisper",
        summary="Very small and multilingual. Accuracy is limited, especially outside English.",
        languages=[], multilingual=True, experimental=False,
        lic="apache", author="OpenAI", converted=True,
        hf_repo="csukuangfj/sherpa-onnx-whisper-tiny",
        hf_files=["tiny-encoder.int8.onnx", "tiny-decoder.int8.onnx", "tiny-tokens.txt"],
        layout={"encoder": "tiny-encoder.int8.onnx", "decoder": "tiny-decoder.int8.onnx",
                "tokens": "tiny-tokens.txt"},
    ),
    dict(
        id="whisper-base", name="Whisper base", kind="whisper",
        summary="Multilingual, a step up in accuracy from tiny.",
        languages=[], multilingual=True, experimental=False,
        lic="apache", author="OpenAI", converted=True,
        hf_repo="csukuangfj/sherpa-onnx-whisper-base",
        hf_files=["base-encoder.int8.onnx", "base-decoder.int8.onnx", "base-tokens.txt"],
        layout={"encoder": "base-encoder.int8.onnx", "decoder": "base-decoder.int8.onnx",
                "tokens": "base-tokens.txt"},
    ),
    dict(
        id="whisper-small", name="Whisper small", kind="whisper",
        summary="Small and multilingual, including Hindi.",
        languages=[], multilingual=True, experimental=False,
        lic="apache", author="OpenAI", converted=True,
        hf_repo="csukuangfj/sherpa-onnx-whisper-small",
        hf_files=["small-encoder.int8.onnx", "small-decoder.int8.onnx", "small-tokens.txt"],
        layout={"encoder": "small-encoder.int8.onnx", "decoder": "small-decoder.int8.onnx",
                "tokens": "small-tokens.txt"},
    ),
    dict(
        id="qwen3-asr-0.6b", name="Qwen3-ASR 0.6B", kind="qwen3_asr",
        summary="Strong on Hindi and about 30 languages in a third-party benchmark.",
        languages=[], multilingual=True, experimental=False,
        lic="apache", author="Qwen team, Alibaba Cloud", converted=True,
        archive="sherpa-onnx-qwen3-asr-0.6B-int8-2026-03-25.tar.bz2",
        keep=r".*",
        layout={"conv_frontend": "conv_frontend.onnx", "encoder": "encoder.int8.onnx",
                "decoder": "decoder.int8.onnx", "tokenizer": "tokenizer"},
    ),
    dict(
        id="parakeet-tdt-0.6b-v2-en", name="Parakeet TDT 0.6B v2 (English)", kind="transducer",
        summary="The most accurate English model offered here.",
        languages=["en"], multilingual=False, experimental=False,
        lic="ccby", author="NVIDIA", converted=True, credit=NVIDIA_CREDIT,
        archive="sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8.tar.bz2",
        keep=r"\.onnx$|tokens\.txt$",
        layout={"encoder": "encoder.int8.onnx", "decoder": "decoder.int8.onnx",
                "joiner": "joiner.int8.onnx", "tokens": "tokens.txt"},
    ),
    dict(
        id="whisper-large-v3-turbo", name="Whisper large-v3 turbo", kind="whisper",
        summary="Broad language coverage; the closest match to the cloud model.",
        languages=[], multilingual=True, experimental=False,
        lic="mit", author="OpenAI", converted=True,
        hf_repo="csukuangfj/sherpa-onnx-whisper-turbo",
        hf_files=["turbo-encoder.int8.onnx", "turbo-decoder.int8.onnx", "turbo-tokens.txt"],
        layout={"encoder": "turbo-encoder.int8.onnx", "decoder": "turbo-decoder.int8.onnx",
                "tokens": "turbo-tokens.txt"},
    ),
    dict(
        id="omnilingual-asr-300m", name="Omnilingual ASR 300M", kind="omnilingual_ctc",
        summary="Covers about 1,600 languages. Experimental: accuracy varies a lot by language.",
        languages=[], multilingual=True, experimental=True,
        lic="apache", author="Meta", converted=True,
        archive="sherpa-onnx-omnilingual-asr-1600-languages-300M-ctc-int8-2025-11-12.tar.bz2",
        keep=r"\.onnx$|tokens\.txt$",
        layout={"model": "model.int8.onnx", "tokens": "tokens.txt"},
    ),
]


def http_json(url):
    req = urllib.request.Request(url, headers={"User-Agent": "bulbul-stt-catalog"})
    tok = os.environ.get("GITHUB_TOKEN")
    if tok and "api.github.com" in url:
        req.add_header("Authorization", f"Bearer {tok}")
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.load(r)


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def download(url, dest):
    if os.path.exists(dest):
        return
    tmp = dest + ".part"
    print(f"  downloading {url}", file=sys.stderr)
    req = urllib.request.Request(url, headers={"User-Agent": "bulbul-stt-catalog"})
    with urllib.request.urlopen(req, timeout=120) as r, open(tmp, "wb") as f:
        while True:
            chunk = r.read(1 << 20)
            if not chunk:
                break
            f.write(chunk)
    os.replace(tmp, dest)


def github_digests():
    """Published sha256 per release asset (null for older assets)."""
    rel = http_json(GH_RELEASE)
    assets = {}
    page = 1
    while True:
        batch = http_json(
            f"https://api.github.com/repos/k2-fsa/sherpa-onnx/releases/{rel['id']}/assets"
            f"?per_page=100&page={page}")
        if not batch:
            break
        for a in batch:
            assets[a["name"]] = a.get("digest")
        page += 1
    return assets


def archive_info(spec, cache, digests):
    name = spec["archive"]
    path = os.path.join(cache, name)
    download(GH_DOWNLOAD + name, path)
    got = sha256_file(path)
    pub = digests.get(name)
    if pub:
        want = pub.split(":", 1)[-1]
        if want != got:
            sys.exit(f"{name}: sha256 {got} doesn't match GitHub's published {want}")

    norm = lambda n: n[2:] if n.startswith("./") else n
    keep = re.compile(spec["keep"])
    top = None
    all_files, kept = [], []
    # One streaming pass: hash every file we keep as it goes by.
    with tarfile.open(path, "r|bz2") as tf:
        for m in tf:
            if not m.isfile():
                continue
            full = norm(m.name)
            first = full.split("/", 1)[0]
            if top is None:
                top = first
            elif first != top:
                sys.exit(f"{name}: members don't share one top-level folder ({top!r} vs {first!r})")
            rel = full.split("/", 1)[1] if "/" in full else full
            all_files.append((full, m.size))
            if "test_wavs" in rel or rel.lower().endswith((".md", ".wav")) or rel.startswith("."):
                continue
            if not keep.search(rel):
                continue
            h = hashlib.sha256()
            f = tf.extractfile(m)
            for chunk in iter(lambda: f.read(1 << 20), b""):
                h.update(chunk)
            kept.append((rel, m.size, h.hexdigest()))
    return dict(path=path, size=os.path.getsize(path), sha256=got, top=top, kept=kept, all=all_files)


def hf_files(spec):
    repo = spec["hf_repo"]
    meta = http_json(f"{HF}/api/models/{repo}")
    rev = meta["sha"]
    tree = {t["path"]: t for t in http_json(f"{HF}/api/models/{repo}/tree/{rev}?recursive=true")
            if t["type"] == "file"}
    out = []
    for p in spec["hf_files"]:
        t = tree.get(p)
        if not t:
            sys.exit(f"{repo}: {p} not found at {rev}")
        sha = (t.get("lfs") or {}).get("oid")
        if not sha:
            if t["size"] > SMALL_FILE:
                sys.exit(f"{repo}/{p}: large file without LFS metadata")
            tmp = os.path.join(tempfile.gettempdir(), "bulbul-stt-small.bin")
            urllib.request.urlretrieve(f"{HF}/{repo}/resolve/{rev}/{p}", tmp)
            sha = sha256_file(tmp)
        out.append(dict(path=p, size=t["size"], sha256=sha))
    return f"{HF}/{repo}/resolve/{rev}", out


def build(inspect, cache):
    digests = github_digests()
    models = []
    for spec in MODELS:
        print(f"{spec['id']}", file=sys.stderr)
        if "archive" in spec:
            info = archive_info(spec, cache, digests)
            if inspect:
                print(f"\n== {spec['id']}  ({info['size']:,} bytes, top={info['top']})")
                for n, s in info["all"]:
                    print(f"   {s:>13,}  {n}")
                continue
            if not spec["layout"]:
                sys.exit(f"{spec['id']}: fill in `layout` from --inspect first")
            kept_names = {r for r, _, _ in info["kept"]}
            for role, p in spec["layout"].items():
                is_dir = any(n.startswith(p + "/") for n in kept_names)
                if p not in kept_names and not is_dir:
                    sys.exit(f"{spec['id']}: layout {role} -> {p} isn't a kept file ({sorted(kept_names)})")
            source = dict(type="archive", url=GH_DOWNLOAD + spec["archive"], size=info["size"],
                          sha256=info["sha256"], format="tar_bz2", strip_prefix=info["top"],
                          files=[dict(path=r, size=sz, sha256=h) for r, sz, h in sorted(info["kept"])])
            download_bytes = info["size"]
            disk_bytes = sum(sz for _, sz, _ in info["kept"])
        else:
            if inspect:
                continue
            base, files = hf_files(spec)
            source = dict(type="files", base_url=base, files=files)
            download_bytes = disk_bytes = sum(f["size"] for f in files)
        spdx, lic_url = LIC[spec["lic"]]
        models.append(dict(
            id=spec["id"], name=spec["name"], summary=spec["summary"], kind=spec["kind"],
            languages=spec["languages"], multilingual=spec["multilingual"],
            experimental=spec["experimental"],
            license=dict(spdx=spdx, author=spec["author"], url=lic_url,
                         attribution=spec.get("credit"), converted=spec["converted"]),
            source=source, layout=spec["layout"],
            download_bytes=download_bytes, disk_bytes=disk_bytes,
        ))
    if inspect:
        return None
    return dict(schema=1, generated=datetime.date.today().isoformat(), models=models)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--inspect", action="store_true")
    ap.add_argument("--cache", default=os.path.join(tempfile.gettempdir(), "bulbul-stt-catalog"))
    args = ap.parse_args()
    os.makedirs(args.cache, exist_ok=True)
    cat = build(args.inspect, args.cache)
    if cat:
        os.makedirs(os.path.dirname(OUT), exist_ok=True)
        with open(OUT, "w", encoding="utf-8", newline="\n") as f:
            json.dump(cat, f, indent=2, ensure_ascii=False)
            f.write("\n")
        print(f"wrote {os.path.normpath(OUT)} ({len(cat['models'])} models)")


if __name__ == "__main__":
    main()
