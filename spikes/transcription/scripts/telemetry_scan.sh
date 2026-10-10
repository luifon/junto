#!/bin/sh
cd /usr/local/lib/python3.11/site-packages
echo "== huggingface_hub constants"
grep -n -E "HF_HUB_OFFLINE|HF_HUB_DISABLE_TELEMETRY|DO_NOT_TRACK|DISABLE_TELEMETRY" huggingface_hub/constants.py | cut -c1-160
grep -rn "def send_telemetry" huggingface_hub/utils/_telemetry.py | head -3
echo "== faster_whisper imports of huggingface_hub / network"
grep -rn -E "huggingface_hub\.|import huggingface_hub|requests|urllib|http" faster_whisper/*.py | grep -v "^faster_whisper/utils.py.*#" | cut -c1-150 | head -12
echo "== sherpa-onnx bundled onnxruntime version"
grep -a -o -E "onnxruntime-[0-9]+\.[0-9]+\.[0-9]+|rel-[0-9]+\.[0-9]+\.[0-9]+|ORT_API_VERSION|1\.(1[0-9]|2[0-9]|3[0-9])\.[0-9]" sherpa_onnx/lib/libonnxruntime.so | sort | uniq -c | sort -rn | head -5
python - <<'PY'
import ctypes, glob
lib = ctypes.CDLL(glob.glob("/usr/local/lib/python3.11/site-packages/sherpa_onnx/lib/libonnxruntime.so")[0])
lib.OrtGetApiBase.restype = ctypes.c_void_p
class Base(ctypes.Structure):
    _fields_ = [("GetApi", ctypes.c_void_p), ("GetVersionString", ctypes.CFUNCTYPE(ctypes.c_char_p))]
base = Base.from_address(lib.OrtGetApiBase())
print("sherpa-onnx bundled onnxruntime version:", base.GetVersionString().decode())
PY
echo "== strings that name an http endpoint or telemetry in native libraries"
for f in sherpa_onnx/lib/*.so* ctranslate2/*.so ctranslate2.libs/*.so* tokenizers/*.so; do
  [ -f "$f" ] || continue
  n=$(grep -a -o -E "https?://[a-zA-Z0-9./_-]+" "$f" | sort -u | grep -v -E "w3\.org|apache\.org|gnu\.org|github\.com|openssl\.org|unicode\.org|microsoft\.com/fwlink|schemas|json-schema|mozilla\.org|intel\.com|llvm\.org|bugs\.|gcc\.|sourceware|stackoverflow|wikipedia|arxiv|onnx\.ai|khronos|boost\.org|rust-lang|docs\.rs|huggingface\.co/docs" | head -5 | tr '\n' ' ')
  t=$(grep -a -c -i -E "telemetry|OneCollector" "$f")
  echo "$f: telemetry-strings=$t urls=[$n]"
done
