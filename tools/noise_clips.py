"""Makes the clips of the measurement of the noise suppressor (ADR 0733,
crates/media/tests/noise_quality.rs): speech of one voice with pauses, and four noises mixed
with it, all 48 kHz mono 16-bit WAV. Test data only: the clips are not kept in the repository.

Runs in the image of workers/transcription/tests/speech/Dockerfile, with the voices that
workers/transcription/tests/speech/fetch-voices.sh downloads:

    python3 noise_clips.py <voices> <texts of workers/transcription/tests/speech/texts> <out>

The speech is six sentences of en.txt in the voice of the English model tests, with 1 s of
silence before and 1.5 s between them, at -26 dBFS over its active samples. The noises:
typing (keys pressed and released, 10 dB below the speech), clicks of a mouse (15 dB), a fan
with a hum of 100 Hz (10 dB), and a second voice reading mix.txt (10 dB). The random numbers
have a fixed seed, and the seed of ONNX Runtime is set for each sentence, so the clips are
the same at each run.
"""

import sys
import wave
from pathlib import Path

import numpy as np
import onnxruntime
import soundfile
from piper import PiperVoice

RATE = 48_000
voices, texts, out = (Path(p) for p in sys.argv[1:4])
out.mkdir(parents=True, exist_ok=True)
rng = np.random.default_rng(19)


def resample(signal: np.ndarray, rate: int) -> np.ndarray:
    """Band-limited resampling to 48 kHz through the spectrum of the whole signal."""
    length = int(round(len(signal) * RATE / rate))
    spectrum = np.fft.rfft(signal)
    target = np.zeros(length // 2 + 1, dtype=complex)
    keep = min(len(spectrum), len(target))
    target[:keep] = spectrum[:keep]
    return np.fft.irfft(target, length) * (length / len(signal))


def synth(voice_name: str, text: str, seed: int) -> np.ndarray:
    onnxruntime.set_seed(seed)
    voice = PiperVoice.load(str(voices / f"{voice_name}.onnx"))
    raw = out / "tmp.wav"
    with wave.open(str(raw), "wb") as handle:
        voice.synthesize(text, handle)
    data, rate = soundfile.read(str(raw), dtype="float64")
    raw.unlink()
    return resample(data, rate)


sentences = [line.strip() for line in (texts / "en.txt").read_text().splitlines() if line.strip()]
second = [line.strip() for line in (texts / "mix.txt").read_text().splitlines() if line.strip()]

# The speech: six sentences of voice A, 1 s of silence at the start and 1.5 s between them.
parts = [np.zeros(RATE)]
for index, text in enumerate(sentences[:6]):
    parts.append(synth("en_US-kristin-medium", text, 100 + index))
    parts.append(np.zeros(int(1.5 * RATE)))
speech = np.concatenate(parts).astype(np.float64)
length = len(speech)
active = np.abs(speech) > 1e-4


def rms(signal: np.ndarray, where=None) -> float:
    values = signal if where is None else signal[where]
    return float(np.sqrt(np.mean(values ** 2)))


# The speech at -26 dBFS over its active samples.
speech *= 10 ** (-26 / 20) / rms(speech, active)


def keystrokes(per_second: float, attack_ms: float, decay_ms: float, band: tuple[float, float]) -> np.ndarray:
    """Short noise bursts at random times: a press and a release for each key."""
    signal = np.zeros(length)
    times = np.cumsum(rng.exponential(1 / per_second, int(per_second * length / RATE * 1.2)))
    burst_len = int((attack_ms + 5 * decay_ms) / 1000 * RATE)
    t = np.arange(burst_len) / RATE
    envelope = np.minimum(t / (attack_ms / 1000), 1) * np.exp(-np.maximum(t - attack_ms / 1000, 0) / (decay_ms / 1000))
    spectrum_filter = None
    for time in times:
        for offset in (0.0, rng.uniform(0.06, 0.12)):
            start = int((time + offset) * RATE)
            if start + burst_len >= length:
                continue
            noise = rng.standard_normal(burst_len)
            spectrum = np.fft.rfft(noise)
            freqs = np.fft.rfftfreq(burst_len, 1 / RATE)
            spectrum[(freqs < band[0]) | (freqs > band[1])] = 0
            burst = np.fft.irfft(spectrum, burst_len) * envelope * rng.uniform(0.6, 1.0)
            signal[start:start + burst_len] += burst
    return signal


def fan() -> np.ndarray:
    """A steady fan: pink noise below 4 kHz and a hum of 100 Hz with harmonics."""
    white = rng.standard_normal(length)
    spectrum = np.fft.rfft(white)
    freqs = np.fft.rfftfreq(length, 1 / RATE)
    spectrum[1:] /= np.sqrt(freqs[1:])
    spectrum[freqs > 4000] *= 0.1
    noise = np.fft.irfft(spectrum, length)
    t = np.arange(length) / RATE
    hum = sum(np.sin(2 * np.pi * 100 * k * t) / k for k in (1, 2, 3))
    noise = noise / rms(noise) + 0.3 * hum / rms(hum)
    return noise


def second_voice() -> np.ndarray:
    parts = []
    index = 0
    while sum(len(p) for p in parts) < length:
        parts.append(synth("en_US-norman-medium", second[index % len(second)], 200 + index))
        parts.append(np.zeros(int(0.3 * RATE)))
        index += 1
    return np.concatenate(parts)[:length]


noises = {
    "typing": (keystrokes(7, 0.5, 4, (800, 8000)), 10),
    "clicks": (keystrokes(1.5, 0.2, 1.5, (200, 12000)), 15),
    "fan": (fan(), 10),
    "voice": (second_voice(), 10),
}


def write(name: str, signal: np.ndarray) -> None:
    pcm = np.clip(signal * 32767, -32768, 32767).astype("<i2")
    with wave.open(str(out / f"{name}.wav"), "wb") as handle:
        handle.setnchannels(1)
        handle.setsampwidth(2)
        handle.setframerate(RATE)
        handle.writeframes(pcm.tobytes())


write("speech", speech)
np.save(out / "active.npy", active)
for name, (noise, snr_db) in noises.items():
    # The noise level relative to the speech: the RMS of the speech over its active samples
    # is snr_db above the RMS of the noise over the whole clip.
    noise = noise * rms(speech, active) / rms(noise) * 10 ** (-snr_db / 20)
    write(f"noise-{name}", noise)
    write(f"mix-{name}", speech + noise)
print(f"{length / RATE:.1f} s, speech active {active.mean():.2f}")
