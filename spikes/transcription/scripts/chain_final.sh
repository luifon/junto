#!/bin/sh
# Runs on the test machine, one container at a time.
# The offline runs (t6_inside.sh) and the quiet GPU speed run were started the same way before this list.
cd /srv/junto-spike-stt
EMB=nemo_en_titanet_large
THR=0.6

: > logs/t7.log
./run.sh t7 gpu nonet sh t7_inside.sh gpu_turbo_mic --kind mic --audio /audio/long_en.wav --reference /audio/long_en.ref.json --device cuda >> logs/t7.log 2>&1
./run.sh t7 gpu nonet sh t7_inside.sh gpu_turbo_shared --kind shared --audio /audio/long_diar_en_opus2.wav --reference /audio/long_diar_en_opus2.ref.json --device cuda --embedding $EMB --threshold $THR >> logs/t7.log 2>&1
./run.sh t7 gpu nonet sh t7_inside.sh gpu_turbo_int8f16_mic --kind mic --audio /audio/long_en.wav --reference /audio/long_en.ref.json --device cuda --compute-type int8_float16 >> logs/t7.log 2>&1
./run.sh t7 gpu nonet sh t7_inside.sh gpu_largev3_mic --kind mic --audio /audio/long_en.wav --reference /audio/long_en.ref.json --device cuda --model large-v3 >> logs/t7.log 2>&1
./run.sh t7 gpu nonet sh t7_inside.sh gpu_largev3_b4_mic --kind mic --audio /audio/long_en.wav --reference /audio/long_en.ref.json --device cuda --model large-v3 --batch 4 >> logs/t7.log 2>&1

./run.sh t5b cpu nonet python t5_diar.py --embedding 3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced --embedding nemo_en_titanet_large --threshold 0.7 --no-oracle --tag _builtin > logs/t5_builtin.log 2>&1

./run.sh t3c cpu nonet python t3_quality.py --device cpu --model large-v3-turbo --file en_kristin --file pt_faber --mode batched8 --tag _turbo_batched > logs/t3_cpu_turbo_batched.log 2>&1
./run.sh t3c cpu nonet python t3_quality.py --device cpu --model large-v3-turbo --file en_kristin --mode sequential --tag _turbo_sequential > logs/t3_cpu_turbo_sequential.log 2>&1
./run.sh t3c cpu nonet python t3_quality.py --device cpu --model large-v3 --file en_kristin --mode batched8 --tag _largev3_batched > logs/t3_cpu_largev3_batched.log 2>&1

docker run --rm --name junto-spike-stt-wcpp --gpus all --network none --cpus 8 --memory 12g \
  -v /srv/junto-spike-stt/models:/models:ro -v /srv/junto-spike-stt/audio:/audio:ro \
  -v /srv/junto-spike-stt/out:/out -v /srv/junto-spike-stt/src/scripts:/src:ro \
  --entrypoint sh ghcr.io/ggml-org/whisper.cpp:main-cuda /src/wcpp_run.sh cpu > logs/wcpp_cpu.tsv 2> logs/wcpp_cpu.err

./run.sh t3c cpu nonet python t3_quality.py --device cpu --model large-v3-turbo --file mix_a --mode clips8 --tag _turbo_clips > logs/t3_cpu_turbo_clips.log 2>&1
echo finished > logs/chain_final.done
