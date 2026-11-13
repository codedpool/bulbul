# Third-party notices

Bulbul is licensed under GPL-3.0-only (see `LICENSE`). This file lists third-party software and models that Bulbul uses or downloads **for the optional on-device transcription feature**. It applies once that feature ships; entries marked *(pending)* still need their final licence text and exact copyright line copied from the release that is actually used.

Nothing in this file is bundled in the Bulbul installer: the on-device engine and the speech models are downloaded by the user, on request, from their original hosts.

## Software

### sherpa-onnx — Apache License 2.0 *(pending: licence text + notices)*
On-device speech recognition engine, used by the optional worker process.
Project: https://github.com/k2-fsa/sherpa-onnx — licence: Apache-2.0.

### ONNX Runtime — MIT License *(pending: licence text + copyright line)*
Inference runtime linked into the sherpa-onnx worker.
Project: https://github.com/microsoft/onnxruntime — copyright Microsoft Corporation.

## Speech models

Models are downloaded on demand and are **not distributed by Bulbul**. Some are converted (for example to ONNX / int8) by third parties; the licence of the original model applies to the converted files.

| Model | Original author | Licence |
|---|---|---|
| Whisper (small, large-v3-turbo) | OpenAI | MIT / Apache-2.0 as published for each model |
| Qwen3-ASR 0.6B | Qwen team (Alibaba Cloud) | Apache-2.0 |
| Omnilingual ASR 300M | Meta | Apache-2.0 |
| Moonshine tiny (English) | Moonshine AI | MIT |
| Parakeet TDT 0.6B v2, Parakeet TDT-CTC 110M | NVIDIA | **CC-BY-4.0** — attribution required, see below |

### Attribution required by CC-BY-4.0
- *Parakeet TDT 0.6B v2* and *Parakeet TDT-CTC 110M* — © NVIDIA, licensed under CC-BY-4.0 (https://creativecommons.org/licenses/by/4.0/). The versions downloaded by Bulbul are converted to ONNX (and quantised to int8) by third parties; those are changes to the original. NVIDIA does not endorse Bulbul.

The same credit is shown on each model's card inside Bulbul next to its download button.
