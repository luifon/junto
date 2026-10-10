# Third-party software and models of the transcription worker

Junto is licensed under Apache-2.0. This file lists every model and every package that the worker uses at run time, with its licence, version and source. Versions are the ones in `uv.lock`. The licence column states what the package declares; the notes state what was found inside the binary packages.

The project distributes source code and the image definitions in `docker/`. It does not distribute a built image or a model file. An operator builds the image and downloads the models (`junto-transcribe models fetch`).

## Models

| Model | Use | Version | Licence | Source |
|---|---|---|---|---|
| Whisper large-v3-turbo, CTranslate2 conversion | Transcription (default) | revision `0a363e9161cbc7ed1431c9597a8ceaf0c4f78fcf` | MIT | <https://huggingface.co/dropbox-dash/faster-whisper-large-v3-turbo>; original model <https://huggingface.co/openai/whisper-large-v3-turbo> |
| Whisper small, CTranslate2 conversion | Development test tier only | revision `536b0662742c02347bc0e980a01041f333bce120` | MIT (the conversion's repository); the original model card states Apache-2.0 | <https://huggingface.co/Systran/faster-whisper-small> |
| Silero VAD v6 | Voice detection | the file inside faster-whisper 1.2.1 | MIT | <https://github.com/snakers4/silero-vad> |
| TitaNet-Large (`nemo_en_titanet_large.onnx`) | Speaker embeddings for shared audio | ONNX export in the sherpa-onnx release `speaker-recongition-models` | CC-BY-4.0 | <https://huggingface.co/nvidia/speakerverification_en_titanet_large>; file from <https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-recongition-models> |

The SHA-256 checksum and the download address of every model file are in `src/junto_transcribe/manifest.json`; `junto-transcribe models list` prints them.

### Attribution required by CC-BY-4.0

The worker separates speakers with **TitaNet-Large (`speakerverification_en_titanet_large`) by NVIDIA**, licensed under the Creative Commons Attribution 4.0 International licence (<https://creativecommons.org/licenses/by/4.0/>). The file is the ONNX export that the sherpa-onnx project distributes. Junto uses it unchanged. NVIDIA does not endorse Junto.

A distribution of Junto that includes this model file must keep this attribution. The transcript header names the model and its licence for every transcript that used it.

### Other attributions

- Whisper: Copyright (c) 2022 OpenAI, MIT licence. The CTranslate2 conversions are published by their repositories under MIT.
- Silero VAD: Copyright (c) 2020-present Silero Team, MIT licence.

## Python packages

Direct dependencies:

| Package | Version | Licence | Source |
|---|---|---|---|
| faster-whisper | 1.2.1 | MIT | <https://github.com/SYSTRAN/faster-whisper> |
| ctranslate2 | 4.8.2 | MIT | <https://github.com/OpenNMT/CTranslate2> |
| onnxruntime | 1.28.0 | MIT | <https://github.com/microsoft/onnxruntime> |
| sherpa-onnx, sherpa-onnx-core | 1.13.8 | Apache-2.0 | <https://github.com/k2-fsa/sherpa-onnx> |
| numpy | 2.5.3 | BSD-3-Clause AND 0BSD AND MIT AND Zlib AND CC0-1.0 | <https://numpy.org> |
| jsonschema | 4.26.0 | MIT | <https://github.com/python-jsonschema/jsonschema> |

Dependencies of those:

| Package | Version | Licence | Source |
|---|---|---|---|
| av (PyAV) | 19.0.1 | BSD-3-Clause | <https://github.com/PyAV-Org/PyAV> |
| tokenizers | 0.23.2 | Apache-2.0 | <https://github.com/huggingface/tokenizers> |
| huggingface-hub | 1.33.0 | Apache-2.0 | <https://github.com/huggingface/huggingface_hub> |
| hf-xet | 1.7.0 | Apache-2.0 | <https://github.com/huggingface/xet-core> |
| httpx | 0.28.1 | BSD-3-Clause | <https://github.com/encode/httpx> |
| httpcore | 1.0.9 | BSD-3-Clause | <https://github.com/encode/httpcore> |
| h11 | 0.16.0 | MIT | <https://github.com/python-hyper/h11> |
| anyio | 4.15.1 | MIT | <https://github.com/agronholm/anyio> |
| idna | 3.20 | BSD-3-Clause | <https://github.com/kjd/idna> |
| certifi | 2026.7.22 | MPL-2.0 | <https://github.com/certifi/python-certifi> |
| click | 8.5.0 | BSD-3-Clause | <https://github.com/pallets/click> |
| filelock | 4.0.12 | MIT | <https://github.com/tox-dev/py-filelock> |
| fsspec | 2026.9.0 | BSD-3-Clause | <https://github.com/fsspec/filesystem_spec> |
| tqdm | 4.70.1 | MPL-2.0 AND MIT | <https://github.com/tqdm/tqdm> |
| pyyaml | 6.0.3 | MIT | <https://pyyaml.org> |
| packaging | 26.3 | Apache-2.0 OR BSD-2-Clause | <https://github.com/pypa/packaging> |
| protobuf | 7.36.2 | BSD-3-Clause | <https://github.com/protocolbuffers/protobuf> |
| flatbuffers | 25.12.19 | Apache-2.0 | <https://github.com/google/flatbuffers> |
| attrs | 26.1.0 | MIT | <https://github.com/python-attrs/attrs> |
| referencing | 0.37.0 | MIT | <https://github.com/python-jsonschema/referencing> |
| jsonschema-specifications | 2025.9.1 | MIT | <https://github.com/python-jsonschema/jsonschema-specifications> |
| rpds-py | 2026.9.1 | MIT | <https://github.com/crate-py/rpds> |
| typing-extensions | 4.16.0 | PSF-2.0 | <https://github.com/python/typing_extensions> |

The worker never calls `huggingface-hub`, `httpx`, `httpcore`, `h11`, `anyio`, `idna`, `certifi`, `hf-xet`, `fsspec` or `filelock`. They are installed because `faster-whisper` and `tokenizers` declare them.

## Native libraries inside the images

Observed in the image built from `docker/Dockerfile` on x86-64:

| Library | Where | Licence | Note |
|---|---|---|---|
| FFmpeg 9.0.2 (`libavcodec`, `libavformat`, `libavutil`, `libavfilter`, `libavdevice`, `libswresample`, `libswscale`) | `/opt/ffmpeg/lib`, built by the image definition | LGPL-2.1-or-later | Source: <https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz>, SHA-256 `8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e`. Built without `--enable-gpl`, `--enable-version3` and `--enable-nonfree`, with no external codec library; the build fails if one of the three is on. Shared libraries, linked dynamically. Decoders: Opus, Vorbis, FLAC, PCM. Containers: Ogg, WAV, FLAC, Matroska. The licence texts are in `/opt/ffmpeg/share/licenses/ffmpeg/` of the image. The configure line is in `docker/Dockerfile`. |
| Intel oneAPI Math Kernel Library | compiled into `libctranslate2` of the `ctranslate2` package | Intel Simplified Software License | Not open source. The licence allows redistribution without modification. |
| oneDNN | compiled into `libctranslate2` | Apache-2.0 | |
| libgomp | `ctranslate2.libs` | GPL-3.0-or-later with the GCC Runtime Library Exception | The exception allows use with software under any licence. |
| OpenBLAS | `numpy.libs` | BSD-3-Clause | |
| libgfortran | `numpy.libs` | GPL-3.0-or-later with the GCC Runtime Library Exception | |
| libquadmath | `numpy.libs` | LGPL-2.1-or-later | Shared library. |
| ONNX Runtime 1.28.2 | `sherpa_onnx/lib/libonnxruntime.so` | MIT | Contains no telemetry client (checked by a test). |
| ALSA library (`libasound`) | `sherpa_onnx.libs` | LGPL-2.1-or-later | Shared library. The worker does not use it. |
| Python 3.12 and Debian 12 base image | `python:3.12-slim-bookworm`, pinned by digest | PSF-2.0; Debian packages under their own licences | |

### PyAV is compiled from source in the images

The binary package of PyAV 19.0.1 on PyPI contains FFmpeg together with `libx264` and `libx265`, which are GPL-2.0-or-later (observed in the macOS package). The image definition does not install that package: it compiles PyAV from its source archive (BSD-3-Clause, the hash is in `uv.lock`) against the FFmpeg build in the table above.

A developer who runs `uv sync` outside a container gets the binary package from PyPI on the development machine. The project does not distribute it.

### GPU image only

| Package | Version | Licence | Source |
|---|---|---|---|
| nvidia-cublas-cu12 | 12.9.2.10 | NVIDIA proprietary (`LicenseRef-NVIDIA-Proprietary`) | <https://pypi.org/project/nvidia-cublas-cu12/> |
| nvidia-cuda-nvrtc-cu12 | 12.9.86 | NVIDIA proprietary | <https://pypi.org/project/nvidia-cuda-nvrtc-cu12/> |
| nvidia-cudnn-cu12 | 9.27.0.42 | NVIDIA proprietary | <https://pypi.org/project/nvidia-cudnn-cu12/> |

These three packages are not open source. CTranslate2 needs cuBLAS and cuDNN to run on an NVIDIA GPU, and no open replacement exists. The project does not distribute them: the GPU image definition downloads them from PyPI when an operator builds the image. Whether a built GPU image may be passed on to others was not verified against NVIDIA's terms, so the project publishes no GPU image. The CPU image contains none of them.

## Tools that are not part of the worker

| Tool | Use | Licence |
|---|---|---|
| uv 0.11.13 | Installs the locked packages while an image is built; not in the final image | Apache-2.0 OR MIT |
| hatchling 1.32.4, setuptools 84.0.0, Cython 3.3.0 | Build the worker package and PyAV while an image is built; not in the final image | MIT, MIT, Apache-2.0 |
| pytest, ruff, mypy, types-jsonschema, jiwer, scipy | Development and tests | MIT, MIT, MIT, Apache-2.0, Apache-2.0, BSD-3-Clause |
| Piper 1.2.0 (`piper-tts`) with espeak-ng | Generates the speech of the model tests, in its own image (`tests/speech/Dockerfile`) | MIT; espeak-ng is GPL-3.0-or-later |
| Piper voices `en_US-kristin-medium`, `en_US-norman-medium`, `en_GB-cori-medium`, `pt_BR-faber-medium` | Test speech | Stated in each voice's `MODEL_CARD`: the three English voices are trained from public-domain LibriVox data; the Portuguese voice has a CC0 dataset and is fine-tuned from the `lessac` voice, whose dataset has its own research licence |

The test voices and the generated speech are downloaded or generated when the model tests are prepared. They are not in the repository and are not distributed.
