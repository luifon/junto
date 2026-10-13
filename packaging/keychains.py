"""Serialize keychain search-list changes and restore them on handled signals."""
import contextlib
import os
import pathlib
import shlex
import signal
import subprocess


def run(*arguments):
    try:
        result = subprocess.run(arguments, capture_output=True, text=True, timeout=30)
    except subprocess.TimeoutExpired:
        raise RuntimeError(f"{arguments[0]} did not finish") from None
    if result.returncode:
        raise RuntimeError(f"{arguments[0]} failed: {result.stderr.strip()}")
    return result.stdout + result.stderr


def security_secret(*arguments):
    # Interactive command input keeps passwords out of the process arguments.
    try:
        result = subprocess.run(["security", "-i"], input=shlex.join(arguments) + "\n",
                                capture_output=True, text=True, timeout=30)
    except subprocess.TimeoutExpired:
        raise RuntimeError("security did not finish") from None
    if result.returncode or result.stderr.replace("security> ", "").strip():
        raise RuntimeError("security could not configure the private keychain")


@contextlib.contextmanager
def signals():
    def interrupted(_number, _frame):
        raise KeyboardInterrupt
    old = {number: signal.signal(number, interrupted) for number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)}
    try:
        yield
    finally:
        for number, handler in old.items():
            signal.signal(number, handler)


@contextlib.contextmanager
def session(original=None):
    import fcntl
    lock = pathlib.Path(os.environ.get("JUNTO_TEST_KEYCHAIN_LOCK", pathlib.Path.home() / "Library/Caches/Junto/keychain.lock"))
    lock.parent.mkdir(parents=True, exist_ok=True)
    with lock.open("a") as held, signals():
        fcntl.flock(held, fcntl.LOCK_EX)
        before = shlex.split(run("security", "list-keychains", "-d", "user"))
        try:
            yield before
        finally:
            old = {number: signal.signal(number, signal.SIG_IGN) for number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)}
            try:
                restore = before if original is None else shlex.split(original)
                run("security", "list-keychains", "-d", "user", "-s", *restore)
            finally:
                for number, handler in old.items():
                    signal.signal(number, handler)


def cleanup(paths, original=None):
    with session(original):
        failure = None
        for path in paths:
            try:
                if path.exists():
                    run("security", "delete-keychain", str(path))
            except (RuntimeError, subprocess.TimeoutExpired) as error:
                failure = error
        if failure:
            raise failure
