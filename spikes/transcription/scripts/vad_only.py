import sys
import time

from faster_whisper.audio import decode_audio
from faster_whisper.vad import VadOptions, get_speech_timestamps

samples = decode_audio(sys.argv[1], sampling_rate=16000)
stamps = get_speech_timestamps(samples, VadOptions())
print("speech spans:", len(stamps))
time.sleep(float(sys.argv[2]) if len(sys.argv) > 2 else 0)
