#!/bin/sh
for v in 1.28.0 1.29.0; do
  mkdir -p /tmp/w$v && cd /tmp/w$v
  pip download -q onnxruntime==$v --no-deps -d . >/dev/null 2>&1
  python -c 'import zipfile,glob; zipfile.ZipFile(glob.glob("*.whl")[0]).extractall("x")'
  echo "== onnxruntime $v: $(ls *.whl)"
  for f in x/onnxruntime/capi/*.so*; do
    echo "   $(basename $f): ORT_DISABLE_TELEMETRY=$(grep -a -c ORT_DISABLE_TELEMETRY $f) OneCollector=$(grep -a -c OneCollector $f)"
  done
done
