# Spike: transcription worker (voice detection, Whisper, glossary, diarization)

Step 0, test 7 (ADR 0603, ADR 0007). Run on 2026-10-09.

Every statement below is marked **observed** (run and seen in this spike) or **not verified**. Statements read in a primary source name the source.

## Question

Which components can the transcription worker use so that it detects speech, transcribes English, Brazilian Portuguese and mixed speech with a glossary, separates unknown speakers in a shared-sound track, runs with no network and no telemetry, and has licences that Apache-2.0 software can distribute, with model files an administrator can get without an account?

## Result

| Gate | Result | Main number |
|---|---|---|
| T1 Component choice | PASS | faster-whisper 1.2.1 + Silero VAD v6 + sherpa-onnx 1.13.8 speaker embeddings with clustering; 6 candidates compared |
| T2 Voice detection removes invented text | PASS with the worker's own clips | 19 to 220 invented lines without voice detection; exactly the 2 sentences with it. The built-in filter of faster-whisper lost 1 of 2 sentences |
| T3 Transcription quality | PASS | Word error rate 0.3 to 1.5 % (English, Portuguese), 0.9 to 1.4 % (mixed, language per clip; 49 to 52 % in every built-in mode). GPU 21x (clips) to 78x (batched) real time; CPU 0.2 to 1.0x |
| T4 Glossary effect | PASS | Terms spelled right: 3/23 → 21/23 (English), 7/23 → 19/23 (Portuguese) with large-v3-turbo. Limit 223 tokens; a longer list is cut silently |
| T5 Diarization | PASS on synthetic voices | 4 of 4 and 3 of 3 speakers; DER 13.0 to 15.6 % (1.6 to 2.9 % with a 0.25 s collar); 96.5 to 97.2 % of speech time under the right speaker; unchanged after two Opus encodes; 17 to 25x real time on the CPU |
| T6 Offline and no telemetry | PASS | Runs with `--network none`; 0 socket or connect calls for an Internet address. ONNX Runtime 1.29 and later has telemetry on by default: pinned to 1.28.0 and switched off |
| T7 Resources | PASS (measured) | large-v3-turbo: 2.2 GB RAM, 3.3 GB video memory. large-v3: 3.7 GB RAM, 6.0 GB video memory. Without a GPU: 0.8 to 1.0x real time at best |

Recommended combination: **Silero VAD v6 → faster-whisper 1.2.1 (CTranslate2 4.8.2) with `large-v3-turbo` → sherpa-onnx 1.13.8 speaker embeddings (`nemo_en_titanet_large.onnx`) with agglomerative clustering for shared sound.** All code is MIT or Apache-2.0, the models are MIT and CC-BY-4.0, and every model file is a plain HTTPS download without an account. Details and reasons in T1.

Four results change what the worker must do, compared with the default behaviour of the libraries:

1. The worker must cut the speech clips itself and let Whisper detect the language of each clip. The built-in filters join the speech first, and then mixed English and Portuguese loses one language completely (T2, T3).
2. `onnxruntime` 1.29 and later contains a telemetry client that is on by default. It is a dependency of faster-whisper (T6).
3. The diarization that worked uses no pyannote model. The built-in pyannote-based pipeline of sherpa-onnx was less accurate on these files and ran at 1.2 to 7.0x real time on the CPU, against 17 to 25x (T5).
4. Without a GPU, large-v3-turbo transcribed at 0.8 to 1.0 times the speed of the speech at best. Other tests were loading the machine during that measurement (T7).

GPU: containers on the test machine already had GPU access (the `nvidia` runtime and CDI devices were configured before the spike). All GPU numbers are from that 8 GB GPU. Nothing was blocked.

## Test machine and method

- Machine: a Linux machine reached over SSH. 8 cores / 16 threads, 32 GB RAM, one NVIDIA GPU with 8 GB, driver 580.178.04. Docker 29.4.0 (**observed**).
- GPU from containers: already set up before the spike (`docker info` lists the `nvidia` runtime and the CDI devices `nvidia.com/gpu=0` and `nvidia.com/gpu=all`; `docker run --gpus all ubuntu:24.04 nvidia-smi` shows the GPU). Nothing was installed or changed on the host (**observed**).
- Every test ran in a container named `junto-spike-stt-*` with `--cpus 8 --memory 12g`. Every container that ran a model was started with `--network none`. Only the image build, the model download and one `pip download` check had network access.
- Other tests used the same machine at the same time and could not be stopped. The load average was between 12 and 21 on 16 threads for most of the spike. Speed numbers marked "shared machine" were measured while a second container of this spike also ran; numbers marked "quiet" were measured with only one container of this spike running. All CPU speeds are lower bounds for this reason.
- Two builds of the worker image were used. T2, T4 and the first T3 run used the first build (`onnxruntime` 1.30.0, no `ORT_DISABLE_TELEMETRY`, always `--network none`). The telemetry finding of T6 came after them. The quiet T3 run, the CPU runs, the chosen diarization method, T6 and T7 used the final build in `docker/Dockerfile.worker` (`onnxruntime` 1.28.0, the variable set). T2 and T4 were not repeated on the final build; the T6 run on the final build gave the same two lines and the same clip times for `silence_quiet` as T2.
- Test speech is synthetic. Piper 1.2.0 (`piper-tts==1.2.0`, MIT, the archived `rhasspy/piper`; the newer `piper-tts` 1.3 and later is GPL-3.0 and was not used) generated every sentence from the texts in `texts/`, so the reference text is exact. Files were mixed with `ffmpeg` (`adelay` + `amix`). No recording of a real person was used. Nothing was played.
- Limits of this method: synthetic speech is clean, has no accent variation, no hesitation and no room echo. Real recordings will have a higher word error rate and a higher diarization error rate than the numbers here. The numbers compare settings and components; they do not predict accuracy on real meetings.

### Commands

On the test machine, in the directory `/srv/junto-spike-stt/` with this spike directory copied to `src/` and `scripts/run.sh` copied next to it (`run.sh <name> <gpu|cpu> <net|nonet> <command>` starts one container `junto-spike-stt-<name>` with `--cpus 8 --memory 12g`, the models and audio mounted read-only):

```
docker build -t junto-spike-stt-tts:1 -f src/docker/Dockerfile.tts src/docker
docker build -t junto-spike-stt-worker:2 -f src/docker/Dockerfile.worker src/docker
docker run --rm --name junto-spike-stt-fetch --cpus 2 --memory 2g -v /srv/junto-spike-stt/models:/models \
  -v /srv/junto-spike-stt/src/scripts:/scripts:ro junto-spike-stt-tts:1 sh /scripts/fetch-models.sh /models
docker run --rm --name junto-spike-stt-gen --network none --cpus 6 --memory 4g -v /srv/junto-spike-stt/models/piper:/voices:ro \
  -v /srv/junto-spike-stt/src:/src:ro -v /srv/junto-spike-stt/audio:/audio junto-spike-stt-tts:1 \
  python /src/scripts/gen_audio.py /src/texts /voices /audio
./run.sh t2 gpu nonet python t2_vad.py --device cuda
./run.sh t3 gpu nonet python t3_quality.py --device cuda
./run.sh t4 gpu nonet python t4_glossary.py --device cuda
./run.sh t5a cpu nonet python t5_diar.py --file diar_en --file diar_pt --threshold 0.5 --threshold 0.7 --threshold 0.9
./run.sh t5s cpu nonet python t5_simple.py --window 2.5 --hop 1.25 --embedding nemo_en_titanet_large --threshold 0.6
./run.sh t6g gpu nonet sh t6_inside.sh cuda nemo_en_titanet_large 0.6
./run.sh t6c cpu nonet sh t6_inside.sh cpu nemo_en_titanet_large 0.6 notrace
sh src/scripts/chain_final.sh        # T7, the built-in pipeline after Opus, the CPU runs, whisper.cpp on the CPU
```

The whisper.cpp runs used `scripts/wcpp_run.sh` inside `ghcr.io/ggml-org/whisper.cpp:main-cuda` (digest `sha256:de0976fa94490d600f666f92dec927b22728a1982e0d4cbf366e902bd930eb95`) and `scripts/wcpp_score.py` for the scores. The threshold sweeps of T5 used `t5_simple.py` with `--threshold` from 0.3 to 0.8 and `--window 1.5 --hop 0.75`, `--window 1.0 --hop 0.5` and `--window 2.5 --hop 1.25`.

### Voices used (test data only, not distributed)

| Voice | Licence stated in its `MODEL_CARD` (read 2026-10-09) |
|---|---|
| `en_US-kristin-medium`, `en_US-norman-medium`, `en_GB-cori-medium` | Dataset: public domain (LibriVox), trained from scratch |
| `en_US-joe-medium`, `pt_BR-faber-medium`, `pt_BR-cadu-medium`, `pt_BR-jeff-medium` | Dataset: CC0; fine-tuned from the `lessac` voice, whose dataset has its own research licence |

Piper links `espeak-ng` (GPL-3.0) for phonemes. Piper and the voices are tools of this test only. No code was copied from them and none of them is a dependency of the worker.

### Files

| Path | Content |
|---|---|
| `docker/Dockerfile.worker` | The worker image used for T2 to T7 |
| `docker/Dockerfile.tts` | The image that generated the test audio |
| `scripts/fetch-models.sh` | Downloads every model with plain HTTPS `GET`, no account, no token; writes `SHA256SUMS` |
| `scripts/gen_audio.py` | Generates all test audio and the reference timelines |
| `scripts/common.py`, `scripts/diar.py` | Shared functions: text normalisation, word error rate, speech clips, diarization, diarization error rate |
| `scripts/t2_vad.py`, `t3_quality.py`, `t4_glossary.py`, `t5_diar.py`, `t5_simple.py` | One script per gate |
| `scripts/worker.py` | The whole chain for one track (T6, T7) |
| `scripts/run.sh`, `scripts/chain_final.sh` | Container start line and the last list of runs |
| `scripts/t6_inside.sh`, `t7_inside.sh`, `telemetry_scan.sh`, `ortcheck.sh`, `vad_only.py` | Offline, resource and telemetry checks |
| `scripts/wcpp_run.sh`, `wcpp_score.py` | The whisper.cpp comparison |
| `texts/` | Reference texts |

## T1. Component choice — PASS

### Comparison

Sources were read on 2026-10-09. "Gated" is the `gated` field of the Hugging Face API (`https://huggingface.co/api/models/<id>`); `auto` means an account must accept conditions before any file can be downloaded.

| Candidate | Code licence, version | Model weights: licence and download | Runtime and dependencies | GPU / CPU | Offline | Telemetry | Tested here |
|---|---|---|---|---|---|---|---|
| (a) **whisper.cpp** | MIT, v1.9.5 (2026-10-06). [repo](https://github.com/ggml-org/whisper.cpp) | Whisper in ggml format: MIT, **not gated**, [ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp). Silero voice detector in ggml format: MIT, not gated, [ggml-org/whisper-vad](https://huggingface.co/ggml-org/whisper-vad) | C/C++, no Python. CUDA build links cuBLAS | CUDA, Vulkan, Metal, OpenVINO, CPU (README) | Yes (**observed**, `--network none`) | No network code in `src/whisper.cpp` (read). Not traced | Yes: large-v3-turbo on the GPU, image `ghcr.io/ggml-org/whisper.cpp:main-cuda` |
| (b) **faster-whisper** on CTranslate2 | MIT, v1.2.1; CTranslate2 MIT, 4.8.2. [repo](https://github.com/SYSTRAN/faster-whisper), [CTranslate2](https://github.com/OpenNMT/CTranslate2) | MIT, **not gated**: [Systran/faster-whisper-large-v3](https://huggingface.co/Systran/faster-whisper-large-v3), [dropbox-dash/faster-whisper-large-v3-turbo](https://huggingface.co/dropbox-dash/faster-whisper-large-v3-turbo). Silero voice detector v6 (MIT) is inside the Python package | Python. `ctranslate2`, `tokenizers` (Apache-2.0), `huggingface_hub` (Apache-2.0), `onnxruntime` (MIT), `av` (BSD-3; bundles FFmpeg), `numpy`. GPU needs cuBLAS and cuDNN 9 for CUDA 12 (NVIDIA proprietary licence) | CUDA or CPU | Yes (**observed**) | `huggingface_hub` has a telemetry function, off with `HF_HUB_DISABLE_TELEMETRY=1`. **`onnxruntime` 1.29 and later contains a telemetry client that is on by default** (see T6) | Yes: large-v3 and large-v3-turbo, GPU and CPU |
| (c) **sherpa-onnx** | Apache-2.0, v1.13.8. [repo](https://github.com/k2-fsa/sherpa-onnx) | Files on GitHub releases, no account. Whisper ONNX exports. Silero VAD (MIT). Segmentation: ONNX export of `pyannote/segmentation-3.0` with its MIT `LICENSE` file in the archive. Speaker embeddings: 3D-Speaker (Apache-2.0), WeSpeaker VoxCeleb (CC-BY-4.0), NeMo TitaNet (CC-BY-4.0) | C++ with bindings (Python, C, Rust, Go and others). Bundles ONNX Runtime 1.28.2 (**observed**) | CPU in the default wheel; a CUDA wheel exists (documentation, not tested) | Yes (**observed**) | None found: no telemetry strings in its libraries, bundled ONNX Runtime is older than the telemetry client (**observed**) | Yes: speaker embeddings and the built-in diarization pipeline. Its Whisper was **not** tested because `OfflineWhisperModelConfig` has no prompt field (`sherpa-onnx/csrc/offline-whisper-model-config.h`), so it cannot take a glossary |
| (d) **pyannote.audio** pipelines | MIT, 4.0.7. [repo](https://github.com/pyannote/pyannote-audio) | `speaker-diarization-3.1` MIT, **gated: auto**. `segmentation-3.0` MIT, **gated: auto**. `speaker-diarization-community-1` CC-BY-4.0, **gated: auto**. The README requires a Hugging Face account, acceptance of the conditions and an access token. A request without a token returns "Access to model … is restricted" (**observed**) | Python, PyTorch 2.8 or later, `torchaudio`, `torchcodec`, `lightning`, OpenTelemetry packages | CUDA or CPU | Yes after the files are present (README; not tested) | **On by default** in 4.x: `telemetry/config.yaml` has `metrics_enabled: true` and sends to `https://otel.pyannote.ai/v1/traces`. Off with `PYANNOTE_METRICS_ENABLED=0` | No: the model files cannot be obtained without an account |
| (e1) **NVIDIA NeMo** diarization | Apache-2.0. [repo](https://github.com/NVIDIA-NeMo/Speech) | Not gated. `diar_sortformer_4spk-v1` is **CC-BY-NC-4.0** (not usable). `diar_streaming_sortformer_4spk-v2` CC-BY-4.0 (its name states 4 speakers). `speakerverification_en_titanet_large` CC-BY-4.0 | Python 3.12 or later, PyTorch 2.7 or later | CUDA or CPU | Not tested | Not checked | Only the TitaNet embedding, through its ONNX export in sherpa-onnx |
| (e2) **Speaker embeddings + clustering**, no segmentation model | sherpa-onnx `SpeakerEmbeddingExtractor` (Apache-2.0) with the Silero voice detector and agglomerative clustering (about 60 lines, `scripts/diar.py`) | 3D-Speaker models Apache-2.0 ([ModelScope](https://modelscope.cn/models/iic/speech_campplus_sv_zh_en_16k-common_advanced) states "Apache License 2.0"); TitaNet CC-BY-4.0. ONNX files on the sherpa-onnx `speaker-recongition-models` release, no account | Same as (c) | CPU (tested); GPU not tested | Yes (**observed**) | Same as (c) | Yes |

Other facts read in the sources:

- The faster-whisper alias `large-v3-turbo` points to `mobiuslabsgmbh/faster-whisper-large-v3-turbo`. That address now answers HTTP 307 to `dropbox-dash/faster-whisper-large-v3-turbo` (**observed**). The product must not depend on an alias: it names the repository and revision, and checks SHA-256.
- TEN VAD (offered by sherpa-onnx) has a non-standard licence (GitHub reports `NOASSERTION`). It was not used.
- The `reverb-diarization` models offered by sherpa-onnx come from a gated repository with licence `other`. They were not used.
- WeSpeaker and 3D-Speaker code is Apache-2.0. The WeSpeaker VoxCeleb models are CC-BY-4.0.

### Recommended combination

| Part | Component | Version | Licence | Model file and location |
|---|---|---|---|---|
| Voice detection | Silero VAD v6, run by `faster_whisper.vad` on ONNX Runtime | faster-whisper 1.2.1, `onnxruntime==1.28.0` | MIT | `silero_vad_v6.onnx`, inside the `faster-whisper` package (no download) |
| Transcription | faster-whisper `BatchedInferencePipeline` on CTranslate2, called with the worker's own speech clips | faster-whisper 1.2.1, CTranslate2 4.8.2 | MIT | Default: `dropbox-dash/faster-whisper-large-v3-turbo`, revision `0a363e9161cbc7ed1431c9597a8ceaf0c4f78fcf` (1.6 GB). Option: `Systran/faster-whisper-large-v3`, revision `edaa852ec7e145841d8ffdb056a99866b5f0a478` (2.9 GB). Both from `https://huggingface.co/<repo>/resolve/<revision>/<file>`, files `config.json`, `model.bin`, `preprocessor_config.json`, `tokenizer.json`, `vocabulary.json` |
| Glossary | The `hotwords` parameter of faster-whisper, at most 200 tokens | same | same | none |
| Diarization (shared sound only) | sherpa-onnx `SpeakerEmbeddingExtractor` on 2.5 s windows of detected speech, then agglomerative clustering (average linkage, cosine distance, threshold 0.6) | sherpa-onnx 1.13.8 | Apache-2.0 (code) | Default: `nemo_en_titanet_large.onnx` (101 MB, CC-BY-4.0, attribution to NVIDIA). All-Apache option: `3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx` (28 MB, Apache-2.0) with threshold 0.5. Both from `https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/<file>` |

SHA-256 of the files used (**observed**, `SHA256SUMS` written by `scripts/fetch-models.sh`):

```
aa3cfc16963a10586a9393f5035d6d6b57e98d358b347f80c2a30bf4f00ceba2  ./diar/3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx
d51abcf31717ef28162f26acb9d44dd4127c3d44c9b8624f699f3425daca8e77  ./diar/nemo_en_titanet_large.onnx
e76620f83d5f5b69efd3d87e3dc180c1bd21df9fbebacfd4335e5e1efcc018da  ./faster-whisper-large-v3-turbo/model.bin
69f74147e3334731bc3a76048724833325d2ec74642fb52620eda87352e3d4f1  ./faster-whisper-large-v3/model.bin
4cbf549b8326f60f80f2536d9eefeb450a9abe83365a098031c89719f1be17d2  faster_whisper/assets/silero_vad_v6.onnx (inside the faster-whisper 1.2.1 package)
```

Reasons:

1. **Glossary.** faster-whisper and whisper.cpp accept a prompt. The Whisper of sherpa-onnx does not, so sherpa-onnx alone cannot meet ADR 0603.
2. **Mixed languages.** Every built-in mode of faster-whisper and of the whisper.cpp command line lost all English sentences of the mixed files (word error rate 49 to 52 %, T3). The only setting that transcribed them (0.9 to 1.4 %) gives each speech clip its own language detection. faster-whisper allows this in one batched call (`clip_timestamps` + `multilingual=True`). With whisper.cpp the worker would have to call the library once per clip through its C API; that was not tested.
3. **Speed on the GPU.** For the same weights (large-v3-turbo) on a 9.7-minute file, measured while a second container of this spike used 8 CPU threads: faster-whisper batched 58.5x real time, faster-whisper per clip 17.5x, whisper.cpp 29.0x. On a quieter machine faster-whisper reached 77.7x and 21.2x (T3); whisper.cpp was not measured again.
4. **Model files without an account.** Every file of the combination is a plain HTTPS download. No pyannote file and no file derived from a gated repository is needed, because the diarization that worked best here uses no segmentation model.
5. **Diarization.** Embeddings plus clustering assigned 96.4 to 97.2 % of speech time to the right speaker and found the right number of speakers on all 8 test files, also after two Opus encodes (T5). The built-in sherpa-onnx pipeline (pyannote segmentation) had no single setting that found the right number of speakers on both the English and the Portuguese file, and ran at 1.2 to 7.0x real time on the CPU against 17 to 25x.

Fallback if a Python worker is not wanted: whisper.cpp (MIT, C++) for transcription with the worker cutting clips itself, and the same sherpa-onnx diarization through its C or Rust API. Measured here for whisper.cpp: same accuracy on single-language files (0.3 % on the short files), the same loss on mixed files with the command line, glossary effect 2/23 to 16/23 terms.

## T2. Voice detection removes invented text — PASS with the worker's own clips

Files: 10 minutes of noise with two sentences, at 150 s and at 430 s.

- `silence_quiet`: brown noise at −62 dBFS, one English and one Portuguese sentence.
- `silence_room`: pink noise at −45 dBFS, the same two sentences.
- `silence_en`: brown noise at −62 dBFS, two English sentences.

Command: `./run.sh t2 gpu nonet python t2_vad.py --device cuda` (GPU, `float16`, `--network none`). All numbers **observed**; the transcripts are in `out/t2_cuda.json`.

The voice detector alone (Silero v6, defaults of faster-whisper, 400 ms padding) found exactly two spans in each file, for example `[149.94, 152.27]` and `[429.90, 432.72]` against the reference `[150.06, 151.84]` and `[430.04, 432.28]`. It processed 600 s in 1.5 s on the CPU (about 400x real time).

Lines in the transcript ("exact" = lines equal to a reference sentence, "other" = every other line):

| Setting | Model | `silence_quiet` exact / other | `silence_room` exact / other | `silence_en` exact / other |
|---|---|---|---|---|
| No voice detection | large-v3-turbo | 2 / **19** | 1 / **20** | 0 / **220** |
| No voice detection | large-v3 | 0 / **34** | 1 / **37** | 0 / **34** |
| faster-whisper `vad_filter=True` | large-v3-turbo | 1 / 0 | 0 / 1 | 2 / 0 |
| faster-whisper `vad_filter=True` | large-v3 | 1 / 0 | 1 / 0 | 2 / 0 |
| faster-whisper batched, `vad_filter=True`, `multilingual=True` | large-v3-turbo | 1 / 0 | 0 / 1 | 0 / 1 |
| faster-whisper batched, `vad_filter=True`, `multilingual=True` | large-v3 | 1 / 0 | 1 / 0 | 0 / 1 |
| **Worker's own clips, language per clip** | large-v3-turbo | **2 / 0** | **2 / 0** | **2 / 0** |
| **Worker's own clips, language per clip** | large-v3 | **2 / 0** | **2 / 0** | **2 / 0** |

Invented lines without voice detection (examples): "Amen.", "Thank you.", "Diolch." (detected language Welsh), "Undertexter från Amara.org-gemenskapen", "Takk for ating med.", and 220 lines that contain only ".".

Findings:

1. Voice detection removes every invented line. PASS for the gate as stated needs the worker's own clips: with them the transcript is exactly the two sentences, for both models and all three files.
2. **The built-in filter of faster-whisper is not enough.** It joins all detected speech into one piece of audio before Whisper reads it. With one English and one Portuguese sentence, the language detected for the joined audio was Portuguese and the English sentence disappeared from the transcript (1 of 2 sentences). In `silence_room` the turbo model also dropped the word "Acho". In the batched mode the two English sentences came out as one line with the time range 149.94 to 432.66 s, which is wrong for both.
3. **Padding matters.** With 200 ms of padding around each detected span the turbo model lost the first word ("everyone see my screen now"), because the detector reports the start of speech about 0.3 s late on these voices. With 400 ms the sentence is complete. whisper.cpp pads 30 ms by default (`-vp`); with that default it lost the English sentence, and with `-vp 400` it kept both in `silence_quiet` and lost one in `silence_room`.
4. whisper.cpp without voice detection invented 19 lines in each file ("Thank you.").

The worker's method (function `transcribe_clips` in `scripts/common.py`): run the detector (`min_silence_duration_ms=300`, `speech_pad_ms=400`, `max_speech_duration_s=29`), pass the detected spans as `clip_timestamps` to `BatchedInferencePipeline.transcribe(..., vad_filter=False, multilingual=True)`. Each clip is one batch element with its own language detection and its own start and end time.

## T3. Transcription quality — PASS

Files (synthetic): `en_*` 328 words, `pt_*` 343 words, `mix_*` 219 words in 18 sentences that alternate English and Portuguese (the voice changes with the language, because no Piper voice speaks both), `*_opus2` the same audio after two Opus encodes at 32 kbit/s, `long_en` and `long_pt` five of those files joined (9.7 and 10.7 minutes).

Word error rate: lower case, punctuation removed, hyphens to spaces, digits 0 to 20 written as words (`normalize` in `scripts/common.py`), then `jiwer` 4.0.0.

Modes:

- **sequential**: `WhisperModel.transcribe(vad_filter=True)`, 30-second windows, each conditioned on the text before it.
- **batched**: `BatchedInferencePipeline.transcribe(batch_size=8)`, speech joined into pieces of at most 30 s.
- **clips**: the worker's own clips with a language per clip (T2), batch size 8.

### Word error rate on the GPU (`float16`, beam 5), **observed**, `out/t3_cuda.json`

| File | turbo sequential | turbo batched | turbo clips | large-v3 sequential | large-v3 batched | large-v3 clips |
|---|---|---|---|---|---|---|
| `en_kristin` | 0.6 % | 1.5 % | 0.9 % | 2.4 % | 0.6 % | 0.6 % |
| `en_norman` | 0.6 % | 0.9 % | 1.5 % | 2.7 % | 0.6 % | 1.2 % |
| `pt_faber` | 0.6 % | 4.1 % | 0.9 % | 1.2 % | 1.2 % | 0.3 % |
| `pt_cadu` | 0.6 % | 0.6 % | 0.6 % | 4.4 % | 0.3 % | 0.9 % |
| `en_kristin_opus2` | 0.9 % | 0.6 % | 0.9 % | 0.6 % | 0.9 % | 0.3 % |
| `pt_faber_opus2` | 0.9 % | 0.9 % | 0.6 % | 0.6 % | 0.3 % | 0.6 % |
| `mix_a` | **51.6 %** | **49.8 %** | 0.9 % | **49.3 %** | **49.3 %** | 1.4 % |
| `mix_b` | **49.3 %** | **49.8 %** | 1.4 % | **49.3 %** | **49.3 %** | 1.4 % |
| `long_en` | 1.3 % | 1.0 % | 1.1 % | 7.9 % | 0.7 % | 0.9 % |
| `long_pt` | 3.4 % | 1.2 % | 0.4 % | 2.6 % | 0.6 % | 0.5 % |

`multilingual=True` did not change the mixed files in the sequential and batched modes (49.3 to 51.6 %): the language is detected once per piece of up to 30 s, the piece was detected as Portuguese, and every English sentence was left out (108 or 109 of 219 words deleted).

Findings:

1. English and Portuguese: 0.3 to 1.5 % in the clips mode for both models. Two Opus encodes at 32 kbit/s did not raise the error.
2. Mixed speech works only with a language per clip. This is true when the speaker pauses between sentences, as in the test files. A change of language inside one sentence was not tested and cannot be solved by cutting at pauses.
3. The sequential mode invented or repeated text on the long files (large-v3 on `long_en`: 76 inserted words in the first run, 116 in the second; turbo on `long_pt`: 44 deleted words). The batched mode dropped 13 words of one sentence in `pt_faber` and `long_pt` with turbo. The clips mode had none of these.
4. large-v3 is not more accurate than large-v3-turbo on this synthetic speech. Whether it is on real speech was not tested.

### Speed, seconds of audio per second (model already loaded), **observed**

| Model | Mode | GPU `float16`, `long_en` (585 s) | CPU `int8`, 8 threads, short files (76 to 119 s) |
|---|---|---|---|
| large-v3-turbo | clips | 21.2x | 0.2x (`mix_a`: 76 s of audio in 440 s; word error rate 0.9 %) |
| large-v3-turbo | batched | 77.7x | 1.0x (`en_kristin`, 1.5 %), 0.8x (`pt_faber`, 4.4 %) |
| large-v3-turbo | sequential | 38.9x | 0.8x (`en_kristin`, 0.3 %) |
| large-v3 | clips | 14.1x | not run |
| large-v3 | batched | 43.6x | 0.6x (`en_kristin`, 0.6 %) |
| large-v3 | sequential | 9.1x | not run |

- GPU numbers: `out/t3_cuda_quiet.json`, with only one container of this spike running. The same runs while a second container of this spike used 8 CPU threads gave 17.5x, 58.5x and 30.7x for turbo (`out/t3_cuda.json`).
- CPU numbers: `out/t3_cpu_*.json`. **They are a lower bound.** During the CPU runs the load average of the machine was 17 to 21 on 16 threads (8 cores), caused by the other tests on it, which could not be stopped. A quiet measurement was not possible.
- The clips mode is slow because every clip is padded to a 30-second window: `long_en` has 150 clips, so 150 encoder passes instead of about 20. On the GPU that makes it 3 to 4 times slower. On the CPU one 30-second window took 21 to 31 s, so the clips mode ran at 0.2x real time for dense speech.
- For a track whose language was set beforehand (ADR 0603), and for a server without a GPU, the batched mode is the faster choice; its disadvantages are the dropped words (finding 3) and the loss of the other language on mixed speech.

### whisper.cpp v1.9.5, same weights (large-v3-turbo, ggml), GPU, `--vad -vp 400`, **observed** (shared machine)

| File | Word error rate | Speed |
|---|---|---|
| `en_kristin` | 0.3 % | 23.2x |
| `pt_faber` | 0.3 % | 23.6x |
| `mix_a`, `mix_b` | 49.3 %, 49.3 % (all English sentences missing) | 35.3x, 38.2x |
| `long_en` | 2.9 % (35 deleted words) | 29.0x |
| `long_pt` | 1.3 % | 25.8x |
| `en_kristin` on the CPU (`-ng -t 8`, same loaded machine) | 0.6 % | 0.3x |

## T4. Glossary effect — PASS

Files: `gloss_en` (128 words) and `gloss_pt` (145 words), 11 sentences each, with 12 invented names and product terms (`texts/glossary_terms.txt`) that occur 23 times in each file. The glossary is the 12 entries in one line, separated by commas. Spelling accuracy = occurrences written exactly as in the glossary (case and punctuation ignored, accents kept) out of 23. GPU, **observed**, `out/t4_cuda.json`.

| Model, mode | No glossary (en / pt) | Glossary as `initial_prompt` | Glossary as `hotwords` |
|---|---|---|---|
| turbo, clips | 3/23, 7/23 | 21/23, 19/23 | 21/23, 19/23 |
| turbo, batched | 3/23, 7/23 | 21/23, 17/23 | 19/23, 17/23 |
| turbo, sequential | 5/23, 9/23 | 16/23, 16/23 | 16/23, 16/23 |
| large-v3, clips | 1/23, 6/23 | 21/23, 23/23 | 21/23, 23/23 |
| large-v3, batched | 2/23, 8/23 | 22/23, 23/23 | 21/23, 23/23 |
| large-v3, sequential | 2/23, 11/23 | 23/23, 23/23 | 23/23, 23/23 |
| whisper.cpp turbo, `--prompt` | 2/23, 9/23 | 16/23, 14/23 | (`--carry-initial-prompt`: 16/23, 14/23) |

Word error rate of the whole file in the clips mode with turbo: 25.0 % and 15.2 % without the glossary, 1.6 % and 3.4 % with it.

Without the glossary: "Oswin Vacaby", "Isild Livarkonnen", "Corvex", "Lumetrax", "TarnBook". With it, the remaining errors of turbo were "Oswald Varkonen" for one occurrence of "Ysolde Varkonen", "Joandira" for "Jurandira", and "Corvex" for "Quorvex" in the Portuguese file (3 of 3). The glossary influences the spelling and does not guarantee it, as ADR 0603 states.

Side effects of the glossary on files that contain none of its terms (clips mode): `en_kristin` 0.9 % → 1.2 % (turbo), 0.6 % → 1.2 % (large-v3); `pt_faber` 0.9 % → 0.6 %, 0.3 % → 0.3 %; `silence_quiet` still exactly the two sentences, no glossary term appeared in the output.

### The limit

Read in the source of faster-whisper 1.2.1 (`transcribe.py`, `get_prompt`): `max_length = 448`; the text before a window, which includes `initial_prompt`, is cut to its **last** `448 // 2 - 1 = 223` tokens; `hotwords` are cut to their **first** 223 tokens. whisper.cpp states "initial prompt (max n_text_ctx/2 tokens)", which is 224, and its source keeps the last tokens.

Measured: the 12-entry glossary is 67 tokens (an invented name uses about 5 tokens; a common word uses 1 or 2). A glossary of 529 tokens was built from the 12 entries plus 462 tokens of ordinary words:

| Long glossary (529 tokens), turbo | clips (en / pt) | batched | sequential |
|---|---|---|---|
| `initial_prompt`, terms first | 3/23, 7/23 (= no glossary) | 5/23, 7/23 | 5/23, 9/23 |
| `initial_prompt`, terms last | 17/23, 18/23 | 20/23, 16/23 | 19/23, 16/23 |
| `hotwords`, terms first | 12/23, 16/23 | 16/23, 16/23 | 14/23, 7/23; word error rate 15.6 % and **44.8 %** |
| `hotwords`, terms last | 3/23, 7/23 (= no glossary) | 3/23, 7/23 | 5/23, 6/23; word error rate **39.1 %** and **48.3 %** |

Findings:

1. A glossary over the limit is cut without any error or warning. Which end is cut depends on the parameter: `initial_prompt` loses its beginning, `hotwords` loses its end. With `hotwords`, the order of priority of ADR 0603 maps directly to the order of the list: the first entries survive.
2. In the sequential mode a `hotwords` list at the limit damaged the transcript (the Portuguese file ended in a repeated word, "planejamos planejamos pl…"), because the list and the previous text together fill the whole context of the model.
3. With large-v3, batch size 8 and the 529-token glossary, the clips mode failed with `CUDA failed with error out of memory` on the 8 GB GPU, and the next call in the same process failed with `cudaErrorInvalidDevice`. The same model with the 67-token glossary ran.
4. In the sequential mode with `condition_on_previous_text`, `initial_prompt` reaches later windows only through the previous text. In the batched and clips modes the prompt is built once and applied to every piece (source: `generate_segment_batched`), so every clip gets the glossary.

Product rule that follows: the worker counts glossary tokens with the tokenizer of the model, keeps entries in priority order until 200 tokens, passes them as `hotwords`, and tells the editor of the glossary which entries are in use.

## T5. Diarization — PASS on synthetic voices

Files:

- `diar_en`: 93 s, 21 turns, 4 voices (two female, two male), pauses of 0.3 s between turns, two overlaps of about 1.5 s.
- `diar_pt`: 109 s, 21 turns, 3 voices (all male; Piper has no female Brazilian voice), two overlaps.
- `*_opus1`: after one Opus encode at 32 kbit/s (48 kHz). `*_opus2`: decoded and encoded a second time.
- `long_diar_en`, `long_diar_en_opus2`: `diar_en` six times in a row (9.3 minutes), for speed and for stability on a longer file.

Reference timeline: each sentence was synthesised alone; its speech span is the part of its own clean file above −45 dBFS, placed at the known offset (`voiced_spans` in `scripts/gen_audio.py`).

Diarization error rate (DER) = (missed + false alarm + confusion) / reference speech time, on 10 ms frames with the best one-to-one mapping of speakers (`error_rate` in `scripts/diar.py`). Overlapped speech is scored. "Collar 0.25" leaves out 0.25 s on each side of every reference boundary. "Right speaker" = share of reference speech time that carries the mapped speaker.

### Chosen method: speech windows, embeddings, clustering (**observed**)

Steps (`diarize_simple` in `scripts/diar.py`): Silero voice detector (`min_silence_duration_ms=300`, `speech_pad_ms=200`); each speech span cut into windows of 2.5 s with a step of 1.25 s; one embedding per window with sherpa-onnx `SpeakerEmbeddingExtractor`; agglomerative clustering (average linkage, cosine distance, cut at the threshold); clusters with fewer than 3 windows go to the nearest larger cluster; each 10 ms frame takes the label of the nearest window that covers it.

Command: `./run.sh check cpu nonet python t5_simple.py --embedding nemo_en_titanet_large --threshold 0.6 --window 2.5 --hop 1.25` (CPU, 8 threads, `--network none`).

| File | Speakers found | DER | missed / false alarm / confusion | DER, collar 0.25 | Right speaker | Speed (CPU) |
|---|---|---|---|---|---|---|
| `diar_en` | 4 of 4 | 13.4 % | 3.1 / 9.9 / 0.5 | 2.9 % | 96.5 % | 18.0x |
| `diar_en_opus1` | 4 of 4 | 13.2 % | 3.1 / 9.6 / 0.5 | 2.7 % | 96.5 % | 16.8x |
| `diar_en_opus2` | 4 of 4 | 13.0 % | 3.1 / 9.5 / 0.5 | 2.6 % | 96.5 % | 18.6x |
| `diar_pt` | 3 of 3 | 15.6 % | 1.9 / 12.7 / 0.9 | 1.6 % | 97.2 % | 20.6x |
| `diar_pt_opus1` | 3 of 3 | 15.3 % | 1.9 / 12.5 / 0.9 | 1.6 % | 97.2 % | 22.6x |
| `diar_pt_opus2` | 3 of 3 | 15.3 % | 1.9 / 12.5 / 0.9 | 1.6 % | 97.2 % | 24.8x |
| `long_diar_en` (9.3 min) | 4 of 4 | 13.4 % | 3.1 / 9.9 / 0.5 | 2.9 % | 96.4 % | 19.0x |
| `long_diar_en_opus2` | 4 of 4 | 13.2 % | 3.1 / 9.7 / 0.5 | 2.7 % | 96.4 % | 17.6x |

(The two long rows are from the threshold sweep, `out/t5_simple_w25all.json`.)

What the errors are:

- False alarm (9.5 to 12.7 %) is the 200 ms of padding that the voice detector adds on each side of a turn. The reference spans have no padding. It does not put text under a wrong speaker.
- Missed speech (1.9 to 3.1 %) is the overlaps: this method gives one speaker to each moment, so the second voice of an overlap is not labelled.
- Confusion (0.5 to 0.9 %) is the real speaker error.
- Two Opus encodes changed no result by more than 0.4 points.

Sensitivity to the threshold (2.5 s windows, all 8 files; "all right" = the number of speakers is right on every file):

| Embedding model | Licence | Thresholds where all 8 files are right | Outside that range | Speed, 9.3-minute file |
|---|---|---|---|---|
| `nemo_en_titanet_large` | CC-BY-4.0 | 0.55 to 0.65 (6 short files: 0.35 to 0.65) | 0.70: 2 or 3 of 4 speakers, DER 36 to 60 %. 0.30: up to 10 speakers | 19.0x |
| `3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced` | Apache-2.0 | 0.45 to 0.55 | 0.60: 2 or 3 of 4 speakers, DER 36 to 59 % | 29.5x |
| `3dspeaker_speech_eres2net_sv_en_voxceleb_16k` | Apache-2.0 | 0.50 to 0.55 | 0.60: 3 of 4 speakers, DER 37 % | 18.6x |

With 1.5 s windows TitaNet was right on the 6 short files from 0.50 to 0.70, and found 5 or more speakers on the long files at every threshold up to 0.70 (`out/t5_simple_fine.json`). With `wespeaker_en_voxceleb_resnet34_LM` the same thresholds gave 1 or 2 speakers; its distances have another scale and it was not tuned.

The result fails abruptly when the threshold is too high: two voices become one speaker and the DER goes from 13 % to 36 % or more. The thresholds above were chosen on these 7 synthetic voices. **Not verified**: that the same threshold is right for real voices, for more than 4 speakers, and for recordings longer than 10 minutes. This needs a check on real recordings before release, and the product needs the rename and correction functions of ADR 0603 in any case.

### Compared: the built-in pipeline of sherpa-onnx (pyannote segmentation-3.0 + embeddings + its clustering), **observed**

Command: `./run.sh t5a cpu nonet python t5_diar.py --file diar_en --file diar_pt --threshold 0.5 --threshold 0.7 --threshold 0.9` (CPU, 8 threads, shared machine).

| Embedding model | Setting | `diar_en`: speakers, DER, right speaker | `diar_pt`: speakers, DER, right speaker | Speed |
|---|---|---|---|---|
| `3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k` | threshold 0.5 | 10/4, 53.2 %, 51.3 % | 6/3, 42.4 %, 61.0 % | 2.8x |
| `3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k` | threshold 0.7 | 6/4, 29.6 %, 74.9 % | 5/3, 42.5 %, 61.0 % | 2.8x |
| `3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k` | threshold 0.9 | 5/4, 25.4 %, 79.2 % | 3/3, 23.8 %, 79.8 % | 2.8x |
| `3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k` | count given (4 / 3) | 4/4, 17.3 %, 87.3 % | 3/3, 23.8 %, 79.8 % | 2.7x |
| `3dspeaker_speech_eres2netv2_sv_zh-cn_16k-common` | threshold 0.5 | 4/4, 19.4 %, 85.4 % | 5/3, 35.2 %, 68.4 % | 1.3x |
| `3dspeaker_speech_eres2netv2_sv_zh-cn_16k-common` | threshold 0.7 | 4/4, 20.6 %, 84.5 % | 3/3, 14.6 %, 89.2 % | 1.3x |
| `3dspeaker_speech_eres2netv2_sv_zh-cn_16k-common` | threshold 0.9 | 3/4, 38.2 %, 66.9 % | 2/3, 42.4 %, 61.6 % | 1.2x |
| `3dspeaker_speech_eres2netv2_sv_zh-cn_16k-common` | count given (4 / 3) | 3/4, 38.2 %, 66.9 % | 2/3, 42.4 %, 61.6 % | 1.2x |
| `3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced` | threshold 0.5 | 6/4, 13.2 %, 91.2 % | 7/3, 40.6 %, 61.9 % | 6.1x |
| `3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced` | threshold 0.7 | 4/4, 10.6 %, 94.1 % | 5/3, 33.9 %, 69.7 % | 6.1x |
| `3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced` | threshold 0.9 | 2/4, 53.9 %, 50.7 % | 3/3, 26.2 %, 77.3 % | 5.9x |
| `3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced` | count given (4 / 3) | 2/4, 53.9 %, 50.7 % | 2/3, 38.5 %, 65.5 % | 6.5x |
| `3dspeaker_speech_eres2net_sv_en_voxceleb_16k` | threshold 0.5 | 6/4, 25.1 %, 79.5 % | 5/3, 27.9 %, 74.5 % | 3.1x |
| `3dspeaker_speech_eres2net_sv_en_voxceleb_16k` | threshold 0.7 | 5/4, 23.7 %, 81.1 % | 4/3, 26.8 %, 75.7 % | 3.1x |
| `3dspeaker_speech_eres2net_sv_en_voxceleb_16k` | threshold 0.9 | 3/4, 38.3 %, 66.3 % | 3/3, 14.9 %, 88.2 % | 3.0x |
| `3dspeaker_speech_eres2net_sv_en_voxceleb_16k` | count given (4 / 3) | 4/4, 23.1 %, 81.6 % | 3/3, 14.9 %, 88.2 % | 3.1x |
| `wespeaker_en_voxceleb_resnet34_LM` | threshold 0.5 | 5/4, 17.2 %, 87.5 % | 3/3, 36.0 %, 67.0 % | 3.6x |
| `wespeaker_en_voxceleb_resnet34_LM` | threshold 0.7 | 2/4, 54.1 %, 50.7 % | 2/3, 38.4 %, 64.9 % | 3.9x |
| `wespeaker_en_voxceleb_resnet34_LM` | threshold 0.9 | 1/4, 84.3 %, 26.2 % | 2/3, 66.3 %, 35.9 % | 3.8x |
| `wespeaker_en_voxceleb_resnet34_LM` | count given (4 / 3) | 2/4, 54.1 %, 50.7 % | 2/3, 38.4 %, 64.9 % | 4.0x |
| `nemo_en_titanet_large` | threshold 0.5 | 7/4, 34.3 %, 70.3 % | 7/3, 35.7 %, 66.8 % | 3.6x |
| `nemo_en_titanet_large` | threshold 0.7 | 4/4, 17.3 %, 87.9 % | 5/3, 29.9 %, 72.6 % | 3.7x |
| `nemo_en_titanet_large` | threshold 0.9 | 3/4, 36.1 %, 69.0 % | 3/3, 23.1 %, 81.0 % | 3.4x |
| `nemo_en_titanet_large` | count given (4 / 3) | 4/4, 17.3 %, 87.9 % | 3/3, 23.1 %, 81.0 % | 3.3x |

After Opus (threshold 0.7; `./run.sh t5b cpu nonet python t5_diar.py --embedding … --threshold 0.7 --no-oracle`; the machine was loaded by other tests):

| Embedding model | File | Speakers found | DER | Confusion | Right speaker | Speed |
|---|---|---|---|---|---|---|
| `3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced` | `diar_en` | 4 of 4 | 10.6 % | 5.7 % | 94.1 % | 6.1x |
| `3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced` | `diar_en_opus1` | 4 of 4 | 11.6 % | 7.2 % | 92.6 % | 5.7x |
| `3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced` | `diar_en_opus2` | 4 of 4 | 12.4 % | 7.2 % | 92.5 % | 5.1x |
| `3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced` | `diar_pt` | 5 of 3 | 33.9 % | 29.5 % | 69.7 % | 5.5x |
| `3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced` | `diar_pt_opus1` | 5 of 3 | 37.0 % | 33.7 % | 65.8 % | 5.9x |
| `3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced` | `diar_pt_opus2` | 4 of 3 | 25.6 % | 21.1 % | 78.3 % | 7.0x |
| `nemo_en_titanet_large` | `diar_en` | 4 of 4 | 17.3 % | 11.9 % | 87.9 % | 2.6x |
| `nemo_en_titanet_large` | `diar_en_opus1` | 5 of 4 | 28.1 % | 23.3 % | 76.5 % | 2.8x |
| `nemo_en_titanet_large` | `diar_en_opus2` | 4 of 4 | 18.1 % | 13.1 % | 86.7 % | 2.7x |
| `nemo_en_titanet_large` | `diar_pt` | 5 of 3 | 29.9 % | 26.7 % | 72.6 % | 1.8x |
| `nemo_en_titanet_large` | `diar_pt_opus1` | 4 of 3 | 36.5 % | 33.5 % | 66.1 % | 2.6x |
| `nemo_en_titanet_large` | `diar_pt_opus2` | 4 of 3 | 32.6 % | 27.9 % | 71.5 % | 2.3x |

Findings for the built-in pipeline:

- Its segmentation is accurate: turn boundaries within 0.1 s and both overlaps detected (false alarm 2.4 to 5.2 %, missed 0.2 to 1.0 %).
- Its speaker assignment was worse: confusion 5.7 % at best and above 20 % in most rows. Among the thresholds 0.5, 0.7 and 0.9, only `3dspeaker_speech_eres2netv2_sv_zh-cn_16k-common` at 0.7 found the right number of speakers on both files (DER 20.6 % and 14.6 %), at 1.3x real time.
- Giving the number of speakers (`num_clusters`) did not always return that number (for example 2 speakers when 4 were given).
- Speed on the CPU: 1.2x to 7.0x real time, against 17 to 30x for the chosen method.

Not tested: pyannote.audio pipelines (gated), NeMo Sortformer (PyTorch stack; the non-streaming model is non-commercial), the GPU build of sherpa-onnx.

### Row for `docs/step-0/README.md`, test 7

| Component | Version | Licence | Runs without network | Result on several voices |
|---|---|---|---|---|
| sherpa-onnx `SpeakerEmbeddingExtractor` with `nemo_en_titanet_large.onnx`, Silero VAD v6, agglomerative clustering | sherpa-onnx 1.13.8 | Apache-2.0 (code), CC-BY-4.0 (model, attribution), MIT (VAD) | Yes (observed, `--network none`) | Synthetic voices: right number of speakers on 8 of 8 files, 96 to 97 % of speech time under the right speaker, DER 13 to 16 % (2 to 3 % with a 0.25 s collar), same after two Opus encodes. Overlapped speech gets one speaker. Threshold not verified on real voices |

## T6. Offline and no telemetry — PASS

### The chain runs with networking disabled (**observed**)

Image: `docker/Dockerfile.worker` (`junto-spike-stt-worker:2`). Started with `docker run --network none --gpus all --cpus 8 --memory 12g`, models mounted read-only. Script: `scripts/t6_inside.sh`, which runs `scripts/worker.py` for one microphone track and one shared-sound track with the glossary, under `strace -f -e trace=socket,connect`.

Output of the GPU run (`out/t6_cuda.log`, shortened):

```
## environment
DO_NOT_TRACK=1
HF_HUB_DISABLE_TELEMETRY=1
HF_HUB_OFFLINE=1
ORT_DISABLE_TELEMETRY=1
## network interfaces: lo
connection to ('1.1.1.1', 443) failed: [Errno 101] Network is unreachable
connection to ('huggingface.co', 443) failed: [Errno -3] Temporary failure in name resolution
## microphone track (cuda)
[ 149.94- 152.27] Member A: Can everyone see my screen now?
[ 429.90- 432.72] Member A: Acho que a conexão caiu por alguns segundos.
{"kind": "mic", "audio_seconds": 600.0, "voiced_seconds": 5.2, "clips": 2, "lines": 2, "stage_seconds": {"decode": 0.22, "voice_detection": 2.08, "model_load": 2.33, "whisper": 1.12}, "gpu_peak_mb": 2312, "speed": 175.2, "wer": 0.0}
## shared-sound track, compressed twice (cuda)
[   0.18-   5.06] Speaker 1: Thanks for joining. I would like to start with the customer feedback from last month.
[   5.06-  10.46] Speaker 2: Most of the comments were about the export function and people want more file formats.
[  10.46-  15.90] Speaker 3: I read those comments too. The request for spreadsheet export appeared more than twenty times.
[  15.90-  19.23] Speaker 1: That matches what the sales team told me on Tuesday.
[  19.23-  24.98] Speaker 4: From the engineering side, spreadsheet export is not difficult, but we need a clear list of columns.
 ... 19 more lines ...
{"kind": "shared", "audio_seconds": 92.7, "clips": 24, "lines": 24, "stage_seconds": {"voice_detection": 0.31, "model_load": 1.62, "whisper": 5.09, "diarization": 5.62}, "gpu_peak_mb": 3272, "wer": 0.0483, "der": 0.1302, "speakers_found": 4, "speakers_reference": 4}
## socket and connect system calls during both runs
/tmp/mic.trace: total 3, AF_INET or AF_INET6 0, AF_UNIX 3, AF_NETLINK 0
/tmp/shared.trace: total 3, AF_INET or AF_INET6 0, AF_UNIX 3, AF_NETLINK 0
## files created outside /out and /tmp
## done
```

- The process made no `socket` or `connect` call for an Internet address. The 3 calls are Unix sockets.
- It wrote no file outside its output directory.
- The same script on the CPU (`out/t6_cpu.log`, `int8`, without `strace`) gave the same words and the same speaker labels on all 26 lines (3 lines differ in one punctuation mark) and the same scores (word error rate 4.8 %, DER 13.0 %, 4 of 4 speakers).
- In the shared-sound transcript all 24 lines carry the speaker who spoke most of the line. The method does not handle the two overlaps: one line joined the end of one speaker's sentence with the next speaker's sentence under one label, and the words "A video is fine" were not transcribed. The word error rate of this file (4.8 %) is almost all from the two overlaps.

### Telemetry found and how it is turned off

| Component | What was found | Setting |
|---|---|---|
| **ONNX Runtime** (dependency of faster-whisper for the voice detector) | **Versions 1.29.0 and 1.30.0 contain a telemetry client that is on by default on Linux** (1.31.0, published on the day of the test, was not checked). Read: `docs/Privacy.md` at tags v1.29.0 and v1.30.0 ("Telemetry is turned ON by default in the official builds", events go to Microsoft over HTTPS). **Observed** in the wheels: 1.29.0 and 1.30.0 contain the strings `ORT_DISABLE_TELEMETRY` and `mobile.events.data.microsoft.com/OneCollector/1.0`; 1.28.0 contains neither. **Observed** at run time with 1.30.0 and no setting, in a container without network: after one voice-detection call the process had created `/root/.cache/Microsoft/DeveloperTools/.onnxruntime/deviceid`, `.../onnxruntime.db`, `/tmp/mat-debug-*.log` and `/tmp/.ses`. With `ORT_DISABLE_TELEMETRY=1` none of these files appeared. Not observed: an actual upload, because no container that ran a model had network access | Pin `onnxruntime==1.28.0` **and** set `ORT_DISABLE_TELEMETRY=1`. Check the wheel for the two strings at every upgrade (`scripts/ortcheck.sh`) |
| ONNX Runtime inside sherpa-onnx 1.13.8 | Version 1.28.2 (**observed** through `OrtGetApiBase`), no `OneCollector` string | None needed now. The same check at every sherpa-onnx upgrade; `ORT_DISABLE_TELEMETRY=1` is already set |
| `huggingface_hub` (dependency of faster-whisper) | Has `send_telemetry`; `constants.py` turns it off for `HF_HUB_DISABLE_TELEMETRY`, `DISABLE_TELEMETRY` or `DO_NOT_TRACK`. faster-whisper calls the hub only in `download_model`, which is not reached when the model is a local directory | `HF_HUB_DISABLE_TELEMETRY=1`, `DO_NOT_TRACK=1`, `HF_HUB_OFFLINE=1`; load with a directory path and `local_files_only=True` |
| CTranslate2 4.8.2, tokenizers, sherpa-onnx 1.13.8 | No telemetry string and no reporting address in their native libraries (`scripts/telemetry_scan.sh`, **observed**) | None |
| Silero VAD | README: "no telemetry, no keys, no registration". It is a model file run by ONNX Runtime | None |
| pyannote.audio 4.x (not used) | Telemetry on by default, endpoint `https://otel.pyannote.ai/v1/traces` (read) | `PYANNOTE_METRICS_ENABLED=0` if it is ever used |

Commands for the ONNX Runtime observation (first build of the image, `onnxruntime` 1.30.0; that build is the final `Dockerfile.worker` without the `onnxruntime` pin and without `ORT_DISABLE_TELEMETRY`):

```
docker run --rm --name junto-spike-stt-strace --network none --cpus 2 --memory 4g \
  -v /srv/junto-spike-stt/audio:/audio:ro -v /srv/junto-spike-stt/src/scripts:/src:ro -w /src junto-spike-stt-worker:1 \
  sh -c "strace -f -qq -e trace=socket,connect,sendto,sendmsg -o /tmp/net.trace python vad_only.py /audio/gloss_en.wav 8; \
         find / -xdev -newer /src/vad_only.py -type f"
# second run: the same with "env ORT_DISABLE_TELEMETRY=1 strace ..."
docker run --rm --name junto-spike-stt-ortcheck -v /srv/junto-spike-stt/src/scripts:/src:ro junto-spike-stt-worker:1 sh /src/ortcheck.sh
```

In the 8 seconds of the first run the process made no `socket` or `connect` call for an Internet address (the container had no network interface except `lo`).

Complete list for the worker container:

```
network_mode: none                  # compose; the worker gets files through mounted directories
ORT_DISABLE_TELEMETRY=1
HF_HUB_DISABLE_TELEMETRY=1
HF_HUB_OFFLINE=1
DO_NOT_TRACK=1
```

plus the pin `onnxruntime==1.28.0`. With `network_mode: none` a library that tried to report could not reach any address; the variables stop the attempt itself and stop the local files.

## T7. Resources — PASS (measured); a server without a GPU is slow

Command: `./run.sh t7 gpu nonet sh t7_inside.sh <label> <worker.py arguments>`, one container per row, so the peak values belong to one run. Memory = `Maximum resident set size` of `/usr/bin/time -v` and `memory.peak` of the container's cgroup. Video memory = highest `memory.used` of `nvidia-smi`, sampled every 0.2 s, minus the value before the run. All **observed** (`out/t7_*.json`, `out/t7.log`). Speed is for the whole chain of one track (decode, voice detection, Whisper in the clips mode, diarization), model load not counted.

| Run | Audio | Speed | Peak memory (RSS / cgroup) | Peak video memory |
|---|---|---|---|---|
| GPU, large-v3-turbo `float16`, batch 8, microphone track | `long_en`, 585 s, 150 clips | 18.9x | 2.1 GB / 1.8 GB | 3 271 MiB |
| GPU, large-v3-turbo `float16`, batch 8, shared sound with diarization | `long_diar_en_opus2`, 556 s, 149 clips | 9.3x (Whisper 26 s on the GPU, diarization 32 s on the CPU) | 2.2 GB / 1.9 GB | 3 271 MiB |
| GPU, large-v3-turbo `int8_float16`, batch 8 | `long_en` | 22.3x | 2.2 GB / 2.0 GB | 2 311 MiB |
| GPU, large-v3 `float16`, batch 8 | `long_en` | 14.7x | 3.6 GB / 3.7 GB | 5 991 MiB |
| GPU, large-v3 `float16`, batch 4 | `long_en` | 13.5x | 3.6 GB / 3.4 GB | 4 935 MiB |
| CPU, large-v3-turbo `int8`, 8 threads, microphone track | `silence_quiet`, 600 s with 5 s of speech, 2 clips | 9.2x | 2.2 GB / 2.2 GB | none |
| CPU, large-v3-turbo `int8`, 8 threads, batch 8, shared sound with diarization | `diar_en_opus2`, 93 s, 24 clips | **0.2x** (Whisper 517 s, diarization 6 s) | 3.6 GB / 3.6 GB | none |

Model load: 1.2 to 2.7 s (turbo) and 3.2 to 3.7 s (large-v3) on the GPU, 2.5 to 6 s on the CPU, from a local NVMe disk.

### What an administrator without a GPU gets

**Observed** on this machine (8 cores / 16 threads), `int8`, 8 threads, while other tests kept the load average at 17 to 21, so these are lower bounds:

- Voice detection: 400x real time. A microphone track that is mostly silence takes little time: 600 s with 5 s of speech took 65 s in total (9.2x).
- large-v3-turbo, speech joined into 30-second pieces (batched or sequential mode): 0.8 to 1.0x real time of speech. One hour of speech takes about one hour.
- large-v3-turbo in the clips mode (language per clip): 0.2x. One hour of dense speech takes about five hours. The mixed-language file was still transcribed correctly (0.9 %).
- large-v3, batched: 0.6x.
- whisper.cpp, large-v3-turbo, CPU: 0.3x.
- Diarization: 17 to 25x; it always ran on the CPU.
- Memory: 2.2 GB for a microphone track, 3.6 GB with batch size 8 and diarization.

So a server without a GPU can transcribe with large-v3-turbo at about the speed of the speech itself, only with joined pieces, which loses mixed-language speech (T3). A smaller Whisper model for such servers was not tested. ADR 0603 leaves "what a server without a GPU does" open; these are the numbers for that decision.

## What the product's transcription worker needs

Container:

- Base `python:3.11-slim-bookworm` (used here). Python packages, pinned: `faster-whisper==1.2.1`, `ctranslate2==4.8.2`, `onnxruntime==1.28.0`, `sherpa-onnx==1.13.8`, `numpy`. The spike image also has `ffmpeg`, `strace`, `scipy`, `soundfile` and `jiwer` for the tests; the worker needs none of them if it writes its own clustering (average-linkage agglomerative clustering on at most a few thousand vectors).
- GPU variant: cuBLAS and cuDNN 9 for CUDA 12 on the library path (here the wheels `nvidia-cublas-cu12==12.9.2.10` and `nvidia-cudnn-cu12==9.27.0.42`; image size 5.46 GB with them). The host needs the NVIDIA driver and the NVIDIA Container Toolkit; the compose service reserves the GPU device. A CPU-only image was not built or measured for size.
- `network_mode: none` and the four environment variables of T6.
- Audio input: `faster_whisper.audio.decode_audio` (PyAV) read the Ogg/Opus file of the test directly and gave the same samples as the WAV file (**observed**, correlation 1.0). The worker needs 16 kHz mono float samples in memory: 115 MB per hour of one track.

Models, installed by the administrator before the worker starts:

- A setup step downloads the files by HTTPS `GET` from the addresses in T1 into one directory, checks SHA-256 against a list that is distributed with Junto, and the worker mounts the directory read-only. No account, no token (`scripts/fetch-models.sh` did exactly this). An administrator without Internet access on the server copies the same files by hand.
- Sizes: large-v3-turbo 1.6 GB (default), large-v3 2.9 GB (option), TitaNet 101 MB or CAM++ 28 MB. The voice detector is inside the Python package.
- The worker loads models by directory path with `local_files_only=True` and refuses to start when a file is missing or its checksum differs. It never names a model by alias.

Processing rules that the measurements require:

1. Run the voice detector on every track with 400 ms of padding. Send only detected clips to Whisper (T2).
2. Do not use `vad_filter=True` of faster-whisper for the final result. Pass the clips as `clip_timestamps` with `multilingual=True`, so each clip has its own language and its own times (T2, T3). ADR 0603 says "Whisper detects the language of each recording"; the measured need is a language per clip.
3. When the language was set beforehand, or on a server without a GPU, join neighbouring clips into pieces of up to 30 s to save encoder passes (2 to 4 times faster on the GPU, T3). On mixed speech this loses the sentences of the other language.
4. Do not use the sequential mode (`WhisperModel.transcribe`) for long tracks: it inserted and repeated text (T3).
5. Glossary: count tokens with the model's tokenizer, keep entries in priority order up to 200 tokens (the hard limit is 223), pass them as `hotwords` (T4). ADR 0603 says "about 220 tokens"; the measured and read value is 223.
6. Batch size 8 with large-v3-turbo fits in 3.3 GB of video memory. With large-v3 use batch size 4 on an 8 GB card (4.9 GB) and treat `CUDA out of memory` as fatal for the process: the next call failed too (T4).
7. Diarization of shared sound: 2.5 s windows, TitaNet embeddings, threshold 0.6 (T5). One speaker label per transcript line (the speaker with most time in the line). Splitting a line where the speaker changes inside it was not tested.

Expected speed and memory (this machine, T3 and T7):

| | With the 8 GB GPU | CPU only, 8 threads |
|---|---|---|
| Microphone track, large-v3-turbo, clips mode | 19 to 25x real time of dense speech; silence is processed at 400x (voice detector only) | 0.2x of dense speech (lower bound, loaded machine) |
| Same, clips joined into 30 s pieces | 76 to 78x | 0.8 to 1.0x (lower bound, loaded machine) |
| large-v3 | 14 to 17x (clips), 34 to 44x (pieces) | 0.6x (pieces); clips not run |
| Diarization of shared sound | 17 to 19x on the CPU (not run on the GPU) | 17 to 25x |
| Memory | 2.2 GB RAM and 3.3 GB video memory (turbo); 3.7 GB RAM and 6.0 GB video memory (large-v3, batch 8) | 2.2 to 3.6 GB RAM |

A one-hour meeting with four microphone tracks contains about one hour of speech in total, because each track is mostly silence. At 19x that is about 3 minutes of GPU time with large-v3-turbo in the clips mode (computed, not measured as one job).

Open points for the design (none of them was decided here):

- With `network_mode: none` the worker cannot call the server. Jobs and results must pass through a mounted directory or a Unix socket.
- Licences to check before an image is distributed: the FFmpeg build inside the PyAV wheel reports "LGPL version 3 or later" and its configuration lists `--enable-libx264` and `--enable-libx265` (**observed**); cuBLAS and cuDNN are under NVIDIA's proprietary licence (**not verified** whether Junto may redistribute them in an image; building the GPU image on the administrator's machine avoids the question). Attribution texts are needed for: OpenAI Whisper weights (MIT), faster-whisper and CTranslate2 (MIT), Silero VAD (MIT), ONNX Runtime (MIT), sherpa-onnx (Apache-2.0), tokenizers and huggingface_hub (Apache-2.0), PyAV (BSD-3-Clause), NumPy (BSD-3-Clause), NVIDIA TitaNet-Large (CC-BY-4.0).
- The diarization threshold and the word error rates come from synthetic voices. A check with real recordings (consented, several accents, real overlap, a remote-desktop audio path) is needed before release.
- A language change inside one sentence, more than 4 speakers, and tracks longer than 11 minutes were not tested.

## What was left in place

On the test machine (**observed** after the removal):

- Removed: every container named `junto-spike-stt-*`, the images `junto-spike-stt-worker:1`, `junto-spike-stt-worker:2`, `junto-spike-stt-tts:1` and `ghcr.io/ggml-org/whisper.cpp:main-cuda`, and the directory `/srv/junto-spike-stt/` with all models and audio. `ls` reports that the directory does not exist and `docker ps -a` lists no container of this spike.
- Left: the Docker build cache entries of the three image builds. The build cache of that machine is shared with other work, and `docker builder prune` would remove the cache of that work too. `docker system df` shows 37.6 GB of build cache in total, of which 29.9 GB is reclaimable; the part that belongs to this spike was not measured.
- Not touched: host packages, host configuration, other containers and services, the directory of the other test.

On the development machine:

- `spikes/transcription/out/` (2 MB, ignored by git): the JSON results and logs that this file cites. No audio and no model file was copied there.
- No build output and no container.
