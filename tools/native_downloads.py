"""The download of a pinned program for the tests: once into a folder of the account, checked
by its SHA-256, and the extraction of single files from its archive. tools/with_native_livekit.py
and tools/with_native_ffmpeg.py use it (ADR 0905)."""

import hashlib
import pathlib
import shutil
import tarfile
import time
import urllib.request
import zipfile


class Failure(Exception):
    pass


def sha256_of(path):
    digest = hashlib.sha256()
    with open(path, "rb") as file:
        for block in iter(lambda: file.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def download(url, path):
    with urllib.request.urlopen(url, timeout=300) as response, open(path, "wb") as file:
        shutil.copyfileobj(response, file, 1 << 20)


def fetch(label, url, expected, downloads, get=download):
    """The archive of `url` in `downloads`, downloaded when it is absent or has another SHA-256."""
    downloads.mkdir(parents=True, exist_ok=True)
    path = downloads / url.rsplit("/", 1)[1]
    if path.exists() and sha256_of(path) == expected:
        print(f"{label}: the kept download has the pinned SHA-256", flush=True)
        return path
    partial = path.with_name(path.name + ".part")
    started = time.monotonic()
    get(url, partial)
    found = sha256_of(partial)
    if found != expected:
        partial.unlink()
        raise Failure(f"the download of {url} has the SHA-256 {found}, not the pinned {expected}")
    partial.replace(path)
    print(f"{label}: downloaded in {time.monotonic() - started:.0f} s, SHA-256 checked", flush=True)
    return path


def extract(archive, members, destination):
    """Extracts the files `members` (paths inside the archive, with /) of a ZIP or a gzipped tar
    archive into `destination`, each under its last name, and returns their paths in order."""
    destination.mkdir(parents=True, exist_ok=True)
    found = {}
    if archive.name.endswith(".zip"):
        with zipfile.ZipFile(archive) as opened:
            for name in members:
                try:
                    source = opened.open(name)
                except KeyError:
                    continue
                target = destination / pathlib.PurePosixPath(name).name
                with source, open(target, "wb") as file:
                    shutil.copyfileobj(source, file, 1 << 20)
                found[name] = target
    else:
        with tarfile.open(archive, "r:gz") as opened:
            for name in members:
                try:
                    member = opened.getmember(name)
                except KeyError:
                    continue
                source = opened.extractfile(member) if member.isfile() else None
                if source is None:
                    continue
                target = destination / pathlib.PurePosixPath(name).name
                with source, open(target, "wb") as file:
                    shutil.copyfileobj(source, file, 1 << 20)
                target.chmod(0o755)
                found[name] = target
    missing = [name for name in members if name not in found]
    if missing:
        raise Failure(f"{archive.name} holds no {', '.join(missing)}")
    return [found[name] for name in members]
