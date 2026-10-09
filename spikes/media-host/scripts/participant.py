"""Test participant built on the LiveKit Python SDK (libwebrtc through the Rust FFI).

Prints one JSON object per line. Tokens are written to files, never printed.
"""

import argparse
import asyncio
import base64
import json
import math
import os
import struct
import sys
import time
from datetime import timedelta

from google.protobuf.json_format import MessageToDict
from livekit import api, rtc

START = time.monotonic()


def emit(event, **fields):
    record = {"t": round(time.monotonic() - START, 2), "event": event}
    record.update(fields)
    print(json.dumps(record), flush=True)


def claims_of(token):
    payload = token.split(".")[1]
    payload += "=" * (-len(payload) % 4)
    claims = json.loads(base64.urlsafe_b64decode(payload))
    now = int(time.time())
    return {
        "identity": claims.get("sub"),
        "room": claims.get("video", {}).get("room"),
        "room_join": claims.get("video", {}).get("roomJoin"),
        "seconds_until_exp": claims.get("exp", 0) - now,
        "lifetime_seconds": claims.get("exp", 0) - claims.get("nbf", claims.get("iat", 0)),
    }


def mint_token(room, identity, ttl_seconds):
    return (
        api.AccessToken(os.environ["LIVEKIT_API_KEY"], os.environ["LIVEKIT_API_SECRET"])
        .with_identity(identity)
        .with_grants(api.VideoGrants(room_join=True, room=room))
        .with_ttl(timedelta(seconds=ttl_seconds))
        .to_jwt()
    )


def selected_pairs(stat_list):
    stats = [MessageToDict(s, preserving_proto_field_name=True) for s in stat_list]
    by_id = {}
    for s in stats:
        for body in s.values():
            if isinstance(body, dict) and "rtc" in body:
                by_id[body["rtc"].get("id")] = body
    result = {"transport": None, "inbound": [], "outbound": []}
    for s in stats:
        if "candidate_pair" in s:
            pair = s["candidate_pair"]["candidate_pair"]
            if not pair.get("nominated"):
                continue
            local = by_id.get(pair.get("local_candidate_id"), {}).get("candidate", {})
            remote = by_id.get(pair.get("remote_candidate_id"), {}).get("candidate", {})
            result["transport"] = {
                "state": pair.get("state"),
                "local_protocol": local.get("protocol"),
                "local_type": local.get("candidate_type"),
                "local_tcp_type": local.get("tcp_type"),
                "remote_protocol": remote.get("protocol"),
                "remote_type": remote.get("candidate_type"),
                "remote_port": remote.get("port"),
                "bytes_sent": pair.get("bytes_sent"),
                "bytes_received": pair.get("bytes_received"),
                "rtt": pair.get("current_round_trip_time"),
            }
        if "inbound_rtp" in s:
            body = s["inbound_rtp"]
            result["inbound"].append(
                {
                    "kind": body.get("stream", {}).get("kind"),
                    "packets_received": body.get("received", {}).get("packets_received"),
                    "packets_lost": body.get("received", {}).get("packets_lost"),
                    "bytes_received": body.get("inbound", {}).get("bytes_received"),
                    "frames_decoded": body.get("inbound", {}).get("frames_decoded"),
                }
            )
        if "outbound_rtp" in s:
            body = s["outbound_rtp"]
            result["outbound"].append(
                {
                    "kind": body.get("stream", {}).get("kind"),
                    "packets_sent": body.get("sent", {}).get("packets_sent"),
                    "bytes_sent": body.get("sent", {}).get("bytes_sent"),
                    "rid": body.get("outbound", {}).get("rid"),
                }
            )
    return result


async def publish_video(room, width, height, fps, stop):
    source = rtc.VideoSource(width, height)
    track = rtc.LocalVideoTrack.create_video_track("test-video", source)
    options = rtc.TrackPublishOptions(source=rtc.TrackSource.SOURCE_CAMERA, simulcast=False)
    await room.local_participant.publish_track(track, options)
    emit("published", kind="video", width=width, height=height, fps=fps)
    frame_bytes = bytearray(width * height * 4)
    n = 0
    while not stop.is_set():
        shade = (n * 7) % 256
        band = (n * 4) % height
        frame_bytes[:] = bytes([shade, 255 - shade, 128, 255]) * (width * height)
        row = bytes([255, 255, 255, 255]) * width
        frame_bytes[band * width * 4 : (band + 1) * width * 4] = row
        source.capture_frame(rtc.VideoFrame(width, height, rtc.VideoBufferType.RGBA, bytes(frame_bytes)))
        n += 1
        await asyncio.sleep(1 / fps)


async def publish_audio(room, stop):
    rate = 48000
    source = rtc.AudioSource(rate, 1)
    track = rtc.LocalAudioTrack.create_audio_track("test-audio", source)
    options = rtc.TrackPublishOptions(source=rtc.TrackSource.SOURCE_MICROPHONE)
    await room.local_participant.publish_track(track, options)
    emit("published", kind="audio")
    samples = rate // 100
    phase = 0
    while not stop.is_set():
        data = b"".join(
            struct.pack("<h", int(8000 * math.sin(2 * math.pi * 440 * (phase + i) / rate))) for i in range(samples)
        )
        phase += samples
        await source.capture_frame(rtc.AudioFrame(data, rate, 1, samples))


async def count_frames(track, counters, key):
    stream = rtc.VideoStream(track) if track.kind == rtc.TrackKind.KIND_VIDEO else rtc.AudioStream(track)
    async for _ in stream:
        counters[key] = counters.get(key, 0) + 1


async def run(args):
    if args.token_file:
        with open(args.token_file) as f:
            token = f.read().strip()
        emit("token_loaded", source=os.path.basename(args.token_file), **claims_of(token))
    else:
        token = mint_token(args.room, args.identity, args.ttl)
        emit("token_minted", **claims_of(token))
    if args.save_token:
        with open(args.save_token, "w") as f:
            f.write(token)

    if args.wait_before_connect > 0:
        emit("waiting_before_connect", seconds=args.wait_before_connect)
        await asyncio.sleep(args.wait_before_connect)
        emit("token_state_at_connect", **claims_of(token))

    room = rtc.Room()
    stop = asyncio.Event()
    counters = {}
    tasks = []
    refreshed = {"n": 0}

    @room.on("participant_connected")
    def _(participant):
        emit("participant_connected", identity=participant.identity)

    @room.on("participant_disconnected")
    def _(participant):
        emit("participant_disconnected", identity=participant.identity)

    @room.on("track_subscribed")
    def _(track, publication, participant):
        key = f"{participant.identity}/{'video' if track.kind == rtc.TrackKind.KIND_VIDEO else 'audio'}"
        emit("track_subscribed", track=key)
        tasks.append(asyncio.ensure_future(count_frames(track, counters, key)))

    @room.on("reconnecting")
    def _():
        emit("reconnecting")

    @room.on("reconnected")
    def _():
        emit("reconnected")

    @room.on("connection_state_changed")
    def _(state):
        emit("connection_state", state=rtc.ConnectionState.Name(state))

    @room.on("disconnected")
    def _(reason):
        emit("disconnected", reason=rtc.DisconnectReason.Name(reason))
        stop.set()

    @room.on("token_refreshed")
    def _():
        refreshed["n"] += 1
        new_token = room._token
        emit("token_refreshed", n=refreshed["n"], **claims_of(new_token))
        if args.save_refreshed:
            with open(args.save_refreshed, "w") as f:
                f.write(new_token)

    try:
        await room.connect(os.environ["LIVEKIT_URL"], token, options=rtc.RoomOptions(auto_subscribe=True))
    except Exception as error:
        emit("connect_failed", error=str(error)[:300])
        return 2
    emit(
        "connected",
        room=room.name,
        identity=room.local_participant.identity,
        others=[p.identity for p in room.remote_participants.values()],
    )

    if args.publish in ("video", "both"):
        tasks.append(asyncio.ensure_future(publish_video(room, args.width, args.height, args.fps, stop)))
    if args.publish in ("audio", "both"):
        tasks.append(asyncio.ensure_future(publish_audio(room, stop)))

    deadline = time.monotonic() + args.duration
    while not stop.is_set() and time.monotonic() < deadline:
        try:
            await asyncio.wait_for(stop.wait(), timeout=min(args.stats_every, max(0.1, deadline - time.monotonic())))
        except asyncio.TimeoutError:
            pass
        if stop.is_set():
            break
        try:
            stats = await room.get_rtc_stats()
            emit(
                "stats",
                frames_received=dict(counters),
                publisher=selected_pairs(stats.publisher_stats),
                subscriber=selected_pairs(stats.subscriber_stats),
            )
        except Exception as error:
            emit("stats_failed", error=str(error)[:200])

    was_removed = stop.is_set()
    stop.set()
    for task in tasks:
        task.cancel()
    if not was_removed:
        await room.disconnect()
        emit("left")
    return 0


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--room", required=True)
    parser.add_argument("--identity", required=True)
    parser.add_argument("--ttl", type=int, default=300)
    parser.add_argument("--token-file")
    parser.add_argument("--save-token")
    parser.add_argument("--save-refreshed")
    parser.add_argument("--wait-before-connect", type=float, default=0)
    parser.add_argument("--publish", choices=["none", "video", "audio", "both"], default="none")
    parser.add_argument("--width", type=int, default=640)
    parser.add_argument("--height", type=int, default=360)
    parser.add_argument("--fps", type=int, default=10)
    parser.add_argument("--duration", type=float, default=30)
    parser.add_argument("--stats-every", type=float, default=5)
    args = parser.parse_args()
    sys.exit(asyncio.run(run(args)))


if __name__ == "__main__":
    main()
