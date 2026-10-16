#!/usr/bin/env python3
"""Exercise the call library's TURN TLS client with temporary certificate chains."""

import os
from pathlib import Path
import socket
import ssl
import struct
import subprocess
import sys
import tempfile


def openssl(directory, *args):
    subprocess.run(["openssl", *args], cwd=directory, check=True,
                   stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)


def fixtures(directory):
    for name in ("trusted", "unknown"):
        openssl(directory, "req", "-x509", "-newkey", "rsa:2048", "-nodes",
                "-keyout", f"{name}-ca.key", "-out", f"{name}-ca.pem", "-days", "2",
                "-subj", f"/CN=Test {name} CA",
                "-addext", "basicConstraints=critical,CA:TRUE",
                "-addext", "keyUsage=critical,keyCertSign,cRLSign")
    openssl(directory, "req", "-newkey", "rsa:2048", "-nodes",
            "-keyout", "intermediate.key", "-out", "intermediate.csr",
            "-subj", "/CN=Test intermediate CA")
    (directory / "extensions").write_text(
        "basicConstraints=critical,CA:TRUE,pathlen:0\nkeyUsage=critical,keyCertSign,cRLSign\n")
    openssl(directory, "x509", "-req", "-in", "intermediate.csr",
            "-CA", "trusted-ca.pem", "-CAkey", "trusted-ca.key", "-CAcreateserial",
            "-out", "intermediate.pem", "-days", "1", "-extfile", "extensions")
    for name in ("trusted", "unknown", "wrong-name", "client-only"):
        ca = "unknown-ca" if name == "unknown" else "intermediate"
        host = "wrong.example.invalid" if name == "wrong-name" else "localhost"
        purpose = "clientAuth" if name == "client-only" else "serverAuth"
        openssl(directory, "req", "-newkey", "rsa:2048", "-nodes",
                "-keyout", f"{name}.key", "-out", f"{name}.csr", "-subj", f"/CN={host}")
        (directory / "extensions").write_text(
            f"subjectAltName=DNS:{host}\nbasicConstraints=critical,CA:FALSE\n"
            f"keyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage={purpose}\n")
        openssl(directory, "x509", "-req", "-in", f"{name}.csr",
                "-CA", f"{ca}.pem", "-CAkey", f"{ca}.key", "-CAcreateserial",
                "-out", f"{name}.pem", "-days", "1", "-extfile", "extensions")
        if ca == "intermediate":
            with (directory / f"{name}.pem").open("ab") as chain:
                chain.write((directory / "intermediate.pem").read_bytes())


def serve(directory, name):
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(directory / f"{name}.pem", directory / f"{name}.key")
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen()
        listener.settimeout(15)
        print(listener.getsockname()[1], flush=True)
        connection, _ = listener.accept()
        with connection:
            connection.settimeout(15)
            try:
                with context.wrap_socket(connection, server_side=True) as tls:
                    packet = b""
                    while len(packet) < 20:
                        data = tls.recv(20 - len(packet))
                        if not data:
                            break
                        packet += data
                    if not packet:
                        print("closed", flush=True)
                    else:
                        kind, _, cookie = struct.unpack("!HHI", packet[:8])
                        assert len(packet) == 20 and kind == 3 and cookie == 0x2112A442, packet
                        print("allocate", flush=True)
            except ssl.SSLError as error:
                if error.reason not in ("TLSV1_ALERT_UNKNOWN_CA", "SSLV3_ALERT_CERTIFICATE_UNKNOWN"):
                    raise
                print("untrusted", flush=True)


def run():
    if sys.platform != "linux":
        raise SystemExit("run this check through tools/linux-container.sh")
    with tempfile.TemporaryDirectory(prefix="junto-turn-tls-") as temporary:
        directory = Path(temporary)
        fixtures(directory)
        environment = dict(os.environ, SSL_CERT_FILE=str(directory / "trusted-ca.pem"),
                           SSL_CERT_DIR=str(directory / "empty"),
                           JUNTO_TEST_TLS_FIXTURES=str(directory))
        (directory / "empty").mkdir()
        command = ([sys.argv[2], "--ignored", "--exact",
                    "turn_tls_uses_system_roots_and_checks_the_peer", "--nocapture"]
                   if len(sys.argv) == 3 else
                   ["cargo", "test", "-p", "junto-media", "--test", "tls_trust",
                    "--", "--ignored", "--nocapture"])
        result = subprocess.run(command, env=environment)
        raise SystemExit(result.returncode)


if __name__ == "__main__":
    if len(sys.argv) in (2, 3) and sys.argv[1] == "run":
        run()
    elif len(sys.argv) == 4 and sys.argv[1] == "serve":
        serve(Path(sys.argv[2]), sys.argv[3])
    else:
        raise SystemExit("usage: test-turn-tls.py run | serve DIRECTORY CERTIFICATE")
