"""Install, start, update and recover real packages of two builds of one checkout."""
import contextlib
import hashlib
import os
import pathlib
import shutil
import signal
import stat
import subprocess
import sys
import tarfile
import threading
import time
import tomllib
import zipfile

import release_tools
from configuration import architecture
from programs import outcome

ROOT = release_tools.REPOSITORY
# The two builds of the check have a target directory of their own inside the target cache, so
# that the build of the commit in the shared target directory, which later steps start, stays
# as it is.
TARGET = pathlib.Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).resolve() / "lifecycle"
# The bound of one start until the first frame, and of --version. On Linux it also holds the
# start of the display and its window manager and, without FUSE, the extraction of the AppImage
# (ADR 0754).
START_BOUND = 300 if sys.platform.startswith("linux") else 120
# The bound of the update by the installed client: it downloads, verifies and extracts the new
# release and the installed one, two archives of a debug build; an AppImage without FUSE also
# extracts itself first. The checks print the time that each update and start took.
UPDATE_BOUND = 300
# How long the output of a command is read after its processes were ended.
DRAIN_BOUND = 30
# The check runs in a folder whose name has a space and a character outside ASCII.
FOLDER = "real client é"


if os.name == "nt":
    import ctypes
    from ctypes import wintypes

    class _BasicLimits(ctypes.Structure):
        _fields_ = [("PerProcessUserTimeLimit", ctypes.c_int64), ("PerJobUserTimeLimit", ctypes.c_int64),
                    ("LimitFlags", wintypes.DWORD), ("MinimumWorkingSetSize", ctypes.c_size_t),
                    ("MaximumWorkingSetSize", ctypes.c_size_t), ("ActiveProcessLimit", wintypes.DWORD),
                    ("Affinity", ctypes.c_size_t), ("PriorityClass", wintypes.DWORD),
                    ("SchedulingClass", wintypes.DWORD)]

    class _ExtendedLimits(ctypes.Structure):
        _fields_ = [("BasicLimitInformation", _BasicLimits), ("IoInfo", ctypes.c_uint64 * 6),
                    ("ProcessMemoryLimit", ctypes.c_size_t), ("JobMemoryLimit", ctypes.c_size_t),
                    ("PeakProcessMemoryUsed", ctypes.c_size_t), ("PeakJobMemoryUsed", ctypes.c_size_t)]

    _kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    _ntdll = ctypes.WinDLL("ntdll")
    _kernel32.CreateJobObjectW.argtypes = [wintypes.LPVOID, wintypes.LPCWSTR]
    _kernel32.CreateJobObjectW.restype = wintypes.HANDLE
    _kernel32.SetInformationJobObject.argtypes = [wintypes.HANDLE, ctypes.c_int, wintypes.LPVOID, wintypes.DWORD]
    _kernel32.SetInformationJobObject.restype = wintypes.BOOL
    _kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    _kernel32.OpenProcess.restype = wintypes.HANDLE
    _kernel32.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
    _kernel32.AssignProcessToJobObject.restype = wintypes.BOOL
    _kernel32.TerminateJobObject.argtypes = [wintypes.HANDLE, wintypes.UINT]
    _kernel32.TerminateJobObject.restype = wintypes.BOOL
    _kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
    _kernel32.CloseHandle.restype = wintypes.BOOL
    _ntdll.NtResumeProcess.argtypes = [wintypes.HANDLE]
    _ntdll.NtResumeProcess.restype = ctypes.c_long


class Processes:
    """The processes of one command and of every program that it starts: a process group on
    Unix, a job object that ends its processes when it closes on Windows."""

    def __init__(self, arguments, options):
        self.job = None
        if os.name == "nt":
            self.job = _kernel32.CreateJobObjectW(None, None)
            if not self.job:
                raise ctypes.WinError(ctypes.get_last_error())
            limits = _ExtendedLimits()
            limits.BasicLimitInformation.LimitFlags = 0x2000  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            if not _kernel32.SetInformationJobObject(self.job, 9, ctypes.byref(limits), ctypes.sizeof(limits)):
                self.close()
                raise ctypes.WinError(ctypes.get_last_error())
            # The program starts suspended, so that it is in the job before it can start another.
            options["creationflags"] = options.get("creationflags", 0) | 0x4  # CREATE_SUSPENDED
        else:
            options["start_new_session"] = True
        try:
            self.child = subprocess.Popen(arguments, stdout=subprocess.PIPE, stderr=subprocess.PIPE, **options)
        except BaseException:
            self.close()
            raise
        if os.name == "nt":
            self.join_job()

    def join_job(self):
        # PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_SUSPEND_RESUME
        process = _kernel32.OpenProcess(0x0100 | 0x0001 | 0x0800, False, self.child.pid)
        try:
            if not process or not _kernel32.AssignProcessToJobObject(self.job, process):
                error = ctypes.WinError(ctypes.get_last_error())
                self.child.kill()
                self.child.wait()
                self.close()
                raise error
            if _ntdll.NtResumeProcess(process) < 0:
                self.end()
                self.child.wait()
                raise AssertionError(f"the command did not resume: {self.child.args}")
        finally:
            if process:
                _kernel32.CloseHandle(process)

    def end(self):
        """Ends every process of the command; the direct child is not reaped here."""
        if os.name == "nt":
            if self.job:
                _kernel32.TerminateJobObject(self.job, 1)
        else:
            try:
                os.killpg(self.child.pid, signal.SIGKILL)
            # macOS refuses the signal when every process of the group has ended but the
            # direct child is not reaped yet.
            except (ProcessLookupError, PermissionError):
                pass

    def close(self):
        if self.job:
            _kernel32.CloseHandle(self.job)
            self.job = None


@contextlib.contextmanager
def interruptions(running):
    received = []
    previous = {}

    def interrupted(number, _frame):
        if not received:
            received.append(number)
        for processes in running:
            processes.end()

    if os.name != "nt":
        for number in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
            previous[number] = signal.signal(number, interrupted)
    try:
        yield received
    finally:
        for number, handler in previous.items():
            signal.signal(number, handler)


def lock_descriptors(environment):
    value = environment.get("JUNTO_LOCK_FD", "")
    if value.isdigit():
        try:
            os.fstat(int(value))
            return (int(value),)
        except OSError:
            pass
    return ()


def command(arguments, *, timeout=180, **options):
    """Runs a command and returns its output. Every process that the command started is ended
    when it returns, also when it fails, times out or this program receives TERM, INT or HUP."""
    running = []
    with interruptions(running) as received:
        return run_command(arguments, timeout, running, received, options)


def run_command(arguments, timeout, running, received, options):
    arguments = [str(value) for value in arguments]
    if os.name == "nt":
        ctypes.windll.kernel32.SetErrorMode(0x0001 | 0x0002 | 0x8000)
    else:
        options["pass_fds"] = lock_descriptors(options.get("env") or os.environ)
    processes = Processes(arguments, options)
    child = processes.child
    running.append(processes)
    readers = [Reader(child.stdout), Reader(child.stderr)]
    timed_out = False
    try:
        if received:
            processes.end()
        try:
            child.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
    finally:
        processes.end()
        if child.poll() is None:
            child.wait(timeout=DRAIN_BOUND)
        processes.close()
    complete = all([reader.finish() for reader in readers])
    if received:
        raise SystemExit(128 + received[0])
    result = subprocess.CompletedProcess(arguments, child.returncode, readers[0].text(), readers[1].text())
    if timed_out:
        raise AssertionError(f"command timed out after {timeout} seconds: " + outcome(result))
    if not complete:
        raise AssertionError("a program outside the processes of the command kept its output open: "
                             + outcome(result))
    if result.returncode:
        raise AssertionError(outcome(result))
    return result.stdout + result.stderr


class Reader:
    """Reads one pipe of a command on a thread of its own, so that a program outside the
    processes of the command that keeps the pipe open cannot hold the check: after
    DRAIN_BOUND the output read so far is used and the thread is left to end with the pipe."""

    def __init__(self, stream):
        self.stream = stream
        self.parts = []
        self.thread = threading.Thread(target=self.read, daemon=True)
        self.thread.start()

    def read(self):
        while chunk := os.read(self.stream.fileno(), 1 << 16):
            self.parts.append(chunk)

    def finish(self):
        """Waits at most DRAIN_BOUND for the end of the pipe; whether it ended."""
        self.thread.join(DRAIN_BOUND)
        return not self.thread.is_alive()

    def text(self):
        return b"".join(list(self.parts)).decode("utf-8", errors="replace")


def remove_tree(path, attempts=10, interval=3):
    """Removes a folder. On Windows a virus scanner can keep a new program file open for a short
    time after its process ended, so a removal that fails is tried again, a bounded number of
    times; the last failure names the files that are left."""
    path = pathlib.Path(path)
    for attempt in range(1, attempts + 1):
        try:
            shutil.rmtree(path)
            return
        except FileNotFoundError:
            return
        except OSError as error:
            if attempt == attempts:
                left = sorted(str(item.relative_to(path)) for item in path.rglob("*"))
                raise AssertionError(f"{path} was not removed after {attempts} attempts: {error}\n"
                                     f"left: {left[:50]}") from None
            time.sleep(interval)


def start(arguments, version, data, *, display=(), environment=None):
    """Checks the version of the client, then starts it with an empty profile until its first
    frame; `display` is the command that runs the start on a virtual display."""
    if data.exists() and any(data.iterdir()):
        raise AssertionError("the startup data directory must be empty")
    if os.name != "nt":
        data.mkdir(mode=0o700, parents=True, exist_ok=True)
        data.chmod(0o700)
    label = command([*arguments, "--version"], env=environment, timeout=START_BOUND)
    if version + " (test configuration)" not in label.splitlines():
        raise AssertionError(f"wrong client version: {label}")
    began = time.monotonic()
    output = command([*display, *arguments, "--data-dir", data, "--no-activate", "--check-startup"],
                     env=environment, timeout=START_BOUND)
    if "Junto first frame rendered" not in output.splitlines():
        raise AssertionError("the client did not render its first frame: " + output)
    print(f"    version {version}: first frame with an empty profile in {time.monotonic() - began:.1f} s",
          flush=True)


def fuse_environment(root, device=pathlib.Path("/dev/fuse"), which=shutil.which):
    """The environment of an AppImage: FUSE when its device and fusermount can be used, else
    extract-and-run into `root`. The choice and its reason are printed."""
    environment = os.environ.copy()
    for name in ["APPIMAGE_EXTRACT_AND_RUN", "APPIMAGE", "APPDIR"]:
        environment.pop(name, None)
    environment["TMPDIR"] = str(root)
    if not device.exists():
        reason = f"{device} is absent from the build container"
    elif not os.access(device, os.R_OK | os.W_OK):
        reason = f"the build account cannot read and write {device}"
    elif not (which("fusermount") or which("fusermount3")):
        reason = "the build image has no fusermount or fusermount3"
    else:
        print("    AppImage startup through FUSE", flush=True)
        return environment
    print(f"    FUSE startup unavailable: {reason}; checking extract-and-run", flush=True)
    environment["APPIMAGE_EXTRACT_AND_RUN"] = "1"
    return environment


def versions(manifest):
    """The version of the workspace manifest and the next patch version."""
    current = tomllib.loads(manifest)["workspace"]["package"]["version"]
    major, minor, patch = map(int, current.split("."))
    return [current, f"{major}.{minor}.{patch + 1}"]


def requests(system, kind, arch, versions):
    """The requests of an update from the first version to the second: the latest metadata, the
    new archive, then the metadata and archive of the installed version for the rollback."""
    current, new = versions
    return [f"/latest-{system}-{kind}-{arch}.json",
            f"/junto-{new}-{system}-{kind}-{arch}.junto",
            f"/release-{current}-{system}-{kind}-{arch}.json",
            f"/junto-{current}-{system}-{kind}-{arch}.junto"]


def contents(path):
    """Each entry at and below `path`, by its path relative to `path`: a link with its target,
    a folder, or a file with its SHA-256 and, on Unix, whether it can be run."""
    path = pathlib.Path(path)
    found = {}
    for item in [path, *sorted(path.rglob("*"))] if path.is_dir() else [path]:
        name = item.relative_to(path).as_posix()
        status = item.lstat()
        if stat.S_ISLNK(status.st_mode):
            found[name] = ("link", os.readlink(item))
        elif stat.S_ISDIR(status.st_mode):
            found[name] = ("folder",)
        else:
            digest = hashlib.sha256()
            with open(item, "rb") as file:
                for block in iter(lambda: file.read(1 << 20), b""):
                    digest.update(block)
            runnable = os.name != "nt" and bool(status.st_mode & 0o111)
            found[name] = ("file", digest.hexdigest(), runnable)
    return found


def compare(installed, application, expected):
    """The folder of the installation holds the application, the same as `expected`, and at most
    the update record of the application."""
    record = f".{application.name}.junto-update"
    extra = sorted(item.name for item in installed.iterdir() if item.name not in (application.name, record))
    if extra:
        raise AssertionError(f"unexpected entries beside the application: {extra}")
    have, want = contents(application), contents(expected)
    if have != want:
        differ = sorted(name for name in have.keys() | want.keys() if have.get(name) != want.get(name))
        raise AssertionError(f"{application} differs from {expected} in {len(differ)} entries: {differ[:20]}")


def verify_signature(application):
    result = subprocess.run(["/usr/bin/codesign", "--verify", "--deep", "--strict", str(application)],
                            capture_output=True, text=True, timeout=120)
    if result.returncode:
        raise AssertionError("the signature of the installed bundle is not valid: " + outcome(result))


def check(parent, system, files, *, signing=None):
    root = parent / FOLDER
    root.mkdir()
    built = versions((ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    current = built[0]
    environment = os.environ.copy()
    environment.pop("JUNTO_UPDATE_SOURCE", None)
    environment.update(JUNTO_TEST_UPDATE_KEYS_FILE=str(files["release"][1]),
                       JUNTO_TEST_FRESHNESS_KEYS_FILE=str(files["freshness"][1]),
                       CARGO_TARGET_DIR=str(TARGET))
    if signing:
        environment.update(signing)
    build = TARGET / "debug"
    arch = architecture()
    suffix = ".exe" if system == "windows" else ""
    packages = []
    for index, version in enumerate(built):
        print(f"    build real client {version}", flush=True)
        environment["JUNTO_TEST_BUILD_VERSION"] = version
        command(["cargo", "build", "--locked", "-p", "junto-client", "--bin", "junto", "-p",
                 "junto-client-core", "--bin", "junto-release", "--bin", "junto-recover",
                 "-p", "junto-icon", "--bin", "junto-icon"], cwd=ROOT, env=environment, timeout=7200)
        output = root / f"build-{index}"
        if system == "macos":
            from bundle import assemble
            bundle = assemble(build / "junto", output / "package-bundle/Junto.app", version,
                              build / "junto-recover", icon_tool=build / "junto-icon", build=build)
            command([sys.executable, ROOT / "packaging/sign.py", bundle], env=environment)
            command(["/usr/bin/ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", bundle,
                     output / "Junto.zip"])
        elif system == "windows":
            import windows
            windows.assemble(build / "junto.exe", build / "junto-recover.exe", version, output, build=build)
        else:
            import linux
            linux.assemble(build / "junto", build / "junto-recover", version, output,
                           icon_tool=build / "junto-icon", build=build)
        packages.append(output)
        if index == 0:
            packager = root / ("junto-release" + suffix)
            shutil.copy2(build / ("junto-release" + suffix), packager)

    kinds = ["bundle"] if system == "macos" else ["directory", "appimage"] if system == "linux" else ["directory"]
    for kind in kinds:
        print(f"    {system} {kind}: install, update, rollback", flush=True)
        work = root / kind
        work.mkdir()
        source = work / "source"
        source.mkdir()
        for index, version in enumerate(built):
            published = work / f"release-{index}"
            release_tools.package(packager, files, packages[index] / f"package-{kind}", published,
                                  version, system, kind, arch)
            for path in published.iterdir():
                if index == 1 or not path.name.startswith("latest-"):
                    shutil.copy2(path, source / path.name)
        installed = work / "installed"
        installed.mkdir()
        if system == "macos":
            command(["/usr/bin/ditto", "-x", "-k", packages[0] / "Junto.zip", installed])
            application = installed / "Junto.app"
            binary = application / "Contents/MacOS/junto"
            from check_signing import requirement
            expected = requirement(application)
        elif system == "windows":
            with zipfile.ZipFile(packages[0] / f"Junto-{current}-windows-{arch}.zip") as archive:
                archive.extractall(installed)
            application = installed / "Junto"
            binary = application / "junto.exe"
        elif kind == "directory":
            with tarfile.open(packages[0] / f"junto-{current}-linux-{arch}.tar.gz") as archive:
                archive.extractall(installed, **({"filter": "data"} if hasattr(tarfile, "data_filter") else {}))
            application = installed / "junto"
            binary = application / "junto"
        else:
            application = installed / "Junto.AppImage"
            shutil.copy2(packages[0] / f"Junto-{current}-{arch}.AppImage", application)
            binary = application
        runtime = fuse_environment(work) if kind == "appimage" else os.environ.copy()

        def started(stage, index):
            compare(installed, application, packages[index] / f"package-{kind}" / application.name)
            if system == "macos":
                verify_signature(application)
                if requirement(application) != expected:
                    raise AssertionError("the signing requirement changed")
            for backend in (["x11", "wayland"] if system == "linux" else [None]):
                display = [ROOT / "tools/linux/display.sh", backend] if backend else []
                start([binary], built[index], work / f"data-{stage}-{backend}", display=display,
                      environment=runtime)

        started("installed", 0)
        with release_tools.source(source) as (server, address):
            # The installed client updates itself, as the update action of its window does.
            began = time.monotonic()
            output = command([binary, "--update-from", address], env=runtime, timeout=UPDATE_BOUND)
            if f"installed version {built[1]}" not in output.splitlines():
                raise AssertionError("the client did not report the update: " + output)
            print(f"    updated to {built[1]} by the installed client in {time.monotonic() - began:.1f} s", flush=True)
            if server.paths != requests(system, kind, arch, built):
                raise AssertionError(f"unexpected release requests: {server.paths}")
        started("updated", 1)
        record = installed / f".{application.name}.junto-update/record"
        command([record / f"previous/recover{suffix}", "rollback", application])
        started("restored", 0)
        if (record / "previous").exists():
            raise AssertionError("rollback kept a previous release")
