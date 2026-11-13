# Third-party notices

Bulbul is licensed under GPL-3.0-only (see `LICENSE`). This file lists third-party software and models used by the optional **on-device transcription** feature.

Nothing here is bundled in the Bulbul installer. The speech engine (a small separate program) and the speech models are downloaded by the user, on request, and are checked against published hashes before they are used.

## Software in the speech engine

The speech engine is a separate program, `bulbul-stt-worker`, built from this repository and published as a release asset. It links the two libraries below, whose licence texts are kept in [`licenses/`](licenses/).

### sherpa-onnx — Apache License 2.0
On-device speech recognition. Version 2.0.1.
Project: https://github.com/k2-fsa/sherpa-onnx · licence text: [`licenses/sherpa-onnx-LICENSE.txt`](licenses/sherpa-onnx-LICENSE.txt)

### ONNX Runtime — MIT License
Inference runtime, included in the sherpa-onnx static libraries. Copyright (c) Microsoft Corporation.
Project: https://github.com/microsoft/onnxruntime · licence text: [`licenses/onnxruntime-LICENSE.txt`](licenses/onnxruntime-LICENSE.txt)

The worker also depends on ordinary Rust crates (listed in `src-tauri/crates/bulbul-stt-worker/Cargo.lock`), each under its own permissive licence.

## Speech models

Models are downloaded on demand and are **not distributed by Bulbul**. Some are converted (for example to ONNX or int8) by third parties; the licence of the original model applies to the converted files. The licence and credit shown on each model's card inside Bulbul come from the same catalogue (`src-tauri/crates/bulbul-local-stt/catalog.json`).

| Model | Original author | Licence |
|---|---|---|
| Whisper tiny, base, small | OpenAI | Apache-2.0 |
| Whisper large-v3 turbo | OpenAI | MIT |
| Qwen3-ASR 0.6B | Qwen team (Alibaba Cloud) | Apache-2.0 |
| Omnilingual ASR 300M | Meta | Apache-2.0 |
| Moonshine tiny (English) | Moonshine AI | MIT |
| Parakeet TDT 0.6B v2 (English) | NVIDIA | **CC-BY-4.0**: attribution required, see below |
| Parakeet TDT 110M (English) | NVIDIA | **CC-BY-4.0**: attribution required, see below |

### Attribution required by CC-BY-4.0
- *Parakeet TDT 0.6B v2* and *Parakeet TDT 110M*: © NVIDIA, licensed under CC-BY-4.0 (https://creativecommons.org/licenses/by/4.0/). The versions downloaded by Bulbul are converted to ONNX and quantised to int8 by third parties; those are changes to the original. NVIDIA does not endorse Bulbul.

The same credit is shown on each of these models' cards in Bulbul, next to its download button.
