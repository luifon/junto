#!/usr/bin/env python3
"""Calls the LiveKit server API (Twirp over HTTP) with the key in .env.local.

  lkapi.py Egress ListEgress '{"room_name": "demo"}'
  lkapi.py RoomService ListParticipants '{"room": "demo"}'

Prints the JSON response. Also usable as a module: call(service, method, body).
"""
import base64
import hashlib
import hmac
import json
import os
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path


def load_env():
    env = dict(os.environ)
    path = Path(__file__).resolve().parent.parent / ".env.local"
    if path.exists():
        for line in path.read_text().splitlines():
            if "=" in line and not line.startswith("#"):
                key, value = line.split("=", 1)
                env.setdefault(key.strip(), value.strip())
    return env


ENV = load_env()
HTTP_URL = ENV.get("LIVEKIT_URL", "ws://localhost:7880").replace("ws", "http", 1)


def _b64(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=")


def api_token(room="", ttl=600):
    now = int(time.time())
    claims = {
        "iss": ENV["LIVEKIT_API_KEY"],
        "nbf": now - 5,
        "exp": now + ttl,
        "video": {"roomCreate": True, "roomList": True, "roomAdmin": True, "roomRecord": True, "room": room},
    }
    signing_input = _b64(json.dumps({"alg": "HS256", "typ": "JWT"}).encode()) + b"." + _b64(json.dumps(claims).encode())
    signature = hmac.new(ENV["LIVEKIT_API_SECRET"].encode(), signing_input, hashlib.sha256).digest()
    return (signing_input + b"." + _b64(signature)).decode()


def call(service, method, body=None, timeout=20):
    """Returns (http_status, parsed_json). The token is limited to the room named in the body."""
    body = body or {}
    room = body.get("room") or body.get("room_name") or ""
    request = urllib.request.Request(
        f"{HTTP_URL}/twirp/livekit.{service}/{method}",
        data=json.dumps(body).encode(),
        headers={"Authorization": f"Bearer {api_token(room)}", "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.status, json.loads(response.read() or b"{}")
    except urllib.error.HTTPError as error:
        text = error.read().decode(errors="replace")
        try:
            return error.code, json.loads(text)
        except ValueError:
            return error.code, {"raw": text}
    except OSError as error:
        return 0, {"error": str(error)}


if __name__ == "__main__":
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    status, reply = call(sys.argv[1], sys.argv[2], json.loads(sys.argv[3]) if len(sys.argv) > 3 else {})
    print(json.dumps(reply, indent=1))
    sys.exit(0 if status == 200 else 1)
