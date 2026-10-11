"""A client of the Junto API for the tests of the deployment.

It holds one device key, proves it with a signed challenge (ADR 0108) and sends requests with
the session token. It uses the Python standard library only, so the Ed25519 signature is
computed here with the arithmetic of RFC 8032. The keys are test keys of one run; this code is
not for any other use.
"""

from __future__ import annotations

import base64
import datetime
import hashlib
import json
import os
import urllib.error
import urllib.request
from typing import Any

_P = 2**255 - 19
_L = 2**252 + 27742317777372353535851937790883648493
_D = -121665 * pow(121666, _P - 2, _P) % _P
_GY = 4 * pow(5, _P - 2, _P) % _P
_GX = 15112221349535400772501151409588531511454012693041857206046113283949847762202
_G = (_GX, _GY, 1, _GX * _GY % _P)

_Point = tuple[int, int, int, int]


def _add(a: _Point, b: _Point) -> _Point:
    first = (a[1] - a[0]) * (b[1] - b[0]) % _P
    second = (a[1] + a[0]) * (b[1] + b[0]) % _P
    third = 2 * a[3] * b[3] * _D % _P
    fourth = 2 * a[2] * b[2] % _P
    e, f, g, h = second - first, fourth - third, fourth + third, second + first
    return (e * f % _P, g * h % _P, f * g % _P, e * h % _P)


def _multiply(scalar: int, point: _Point) -> _Point:
    result: _Point = (0, 1, 1, 0)
    while scalar > 0:
        if scalar & 1:
            result = _add(result, point)
        point = _add(point, point)
        scalar >>= 1
    return result


def _compress(point: _Point) -> bytes:
    inverse = pow(point[2], _P - 2, _P)
    x, y = point[0] * inverse % _P, point[1] * inverse % _P
    return (y | ((x & 1) << 255)).to_bytes(32, "little")


def _expand(secret: bytes) -> tuple[int, bytes]:
    digest = hashlib.sha512(secret).digest()
    scalar = int.from_bytes(digest[:32], "little")
    scalar &= (1 << 254) - 8
    scalar |= 1 << 254
    return scalar, digest[32:]


def public_key(secret: bytes) -> bytes:
    scalar, _ = _expand(secret)
    return _compress(_multiply(scalar, _G))


def sign(secret: bytes, message: bytes) -> bytes:
    scalar, prefix = _expand(secret)
    public = _compress(_multiply(scalar, _G))
    r = int.from_bytes(hashlib.sha512(prefix + message).digest(), "little") % _L
    commitment = _compress(_multiply(r, _G))
    h = int.from_bytes(hashlib.sha512(commitment + public + message).digest(), "little") % _L
    return commitment + ((r + h * scalar) % _L).to_bytes(32, "little")


def _text(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


class Answer:
    """The answer to one request: the status, the headers and the body."""

    def __init__(self, status: int, headers: Any, body: bytes) -> None:
        self.status = status
        self.headers = headers
        self.body = body

    def json(self) -> Any:
        return json.loads(self.body) if self.body else None

    def error_code(self) -> str | None:
        try:
            return str(self.json()["error"]["code"])
        except (ValueError, KeyError, TypeError):
            return None


class Unexpected(Exception):
    """A request got another status than the caller stated."""


def request(
    method: str,
    url: str,
    body: Any = None,
    token: str | None = None,
    headers: dict[str, str] | None = None,
    expect: int | tuple[int, ...] | None = None,
    timeout: float = 30,
) -> Answer:
    data = None
    sent = dict(headers or {})
    if body is not None:
        data = json.dumps(body).encode()
        sent["content-type"] = "application/json"
    if token is not None:
        sent["authorization"] = f"Bearer {token}"
    call = urllib.request.Request(url, data=data, method=method, headers=sent)
    try:
        with urllib.request.urlopen(call, timeout=timeout) as response:
            answer = Answer(response.status, response.headers, response.read())
    except urllib.error.HTTPError as error:
        answer = Answer(error.code, error.headers, error.read())
    if expect is not None:
        allowed = (expect,) if isinstance(expect, int) else expect
        if answer.status not in allowed:
            # Only the code of an error is shown: a body of another form can hold a token.
            raise Unexpected(f"{method} {url.split('?')[0]}: status {answer.status}, error code {answer.error_code()}")
    return answer


class Device:
    """One device with its key at one server, and the session of its account."""

    def __init__(self, origin: str) -> None:
        self.origin = origin.rstrip("/")
        self.secret = os.urandom(32)
        self.public = _text(public_key(self.secret))
        self.token: str | None = None
        self.account_id: str | None = None

    def _proof(self, operation: str) -> dict[str, str]:
        challenge = request(
            "POST",
            f"{self.origin}/api/v1/auth/challenges",
            {"public_key": self.public, "operation": operation},
            expect=(200, 201),
        ).json()
        expires = datetime.datetime.fromisoformat(challenge["expires_at"].replace("Z", "+00:00"))
        lines = [
            "junto-challenge-v2",
            f"server:{challenge['server_id']}",
            f"origin:{self.origin}",
            f"operation:{challenge['operation']}",
            f"account:{challenge.get('account_id') or 'none'}",
            f"device:{challenge['public_key']}",
            f"challenge:{challenge['id']}",
            f"nonce:{challenge['nonce']}",
            f"expires:{int(expires.timestamp())}",
        ]
        signature = sign(self.secret, "\n".join(lines).encode())
        return {"challenge_id": challenge["id"], "origin": self.origin, "signature": _text(signature)}

    def _session(self, path: str, operation: str, fields: dict[str, Any]) -> None:
        body = {"proof": self._proof(operation), **fields}
        created = request("POST", f"{self.origin}{path}", body, expect=(200, 201)).json()
        self.token = created["token"]
        self.account_id = created["account_id"]

    def claim_owner(self, owner_code: str, name: str) -> None:
        self._session("/api/v1/owner/claim", "claim_owner", {"owner_code": owner_code, "name": name})

    def join(self, invite_code: str, name: str) -> None:
        self._session("/api/v1/join", "join", {"invite_code": invite_code, "name": name})

    def login(self) -> None:
        self._session("/api/v1/auth/sessions", "login", {})

    def call(
        self,
        method: str,
        path: str,
        body: Any = None,
        headers: dict[str, str] | None = None,
        expect: int | tuple[int, ...] | None = None,
        timeout: float = 30,
    ) -> Answer:
        return request(method, f"{self.origin}{path}", body, self.token, headers, expect, timeout)

    def get(self, path: str) -> Any:
        return self.call("GET", path, expect=200).json()
