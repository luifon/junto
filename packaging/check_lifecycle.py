"""Install, start, update and recover real packages of two builds of one checkout."""
import contextlib
import os
import pathlib
import shutil
import signal
import subprocess
import sys
import tarfile
import tomllib
import zipfile

import release_tools
from configuration import architecture
from programs import outcome

ROOT = release_tools.REPOSITORY
TARGET = pathlib.Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")).resolve() / "debug"


def stop(child):
    if os.name == "nt":
        subprocess.run(["taskkill", "/PID", str(child.pid), "/T", "/F"], capture_output=True, timeout=30)
    else:
        try:
            os.killpg(child.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass


@contextlib.contextmanager
def interruptions(children):
    received = []
    previous = {}

    def interrupted(number, _frame):
        if not received:
            received.append(number)
        for child in children:
            stop(child)

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
    children = []
    with interruptions(children) as received:
        return run_command(arguments, timeout, children, received, options)


def run_command(arguments, timeout, children, received, options):
    arguments = [str(value) for value in arguments]
    if os.name == "nt":
        import ctypes
        ctypes.windll.kernel32.SetErrorMode(0x0001 | 0x0002 | 0x8000)
    else:
        options["pass_fds"] = lock_descriptors(options.get("env") or os.environ)
    child = subprocess.Popen(arguments, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
                             start_new_session=os.name != "nt", **options)
    children.append(child)
    if received:
        stop(child)
    try:
        output, errors = child.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        stop(child)
        output, errors = child.communicate(timeout=30)
        raise AssertionError("command timed out: " + outcome(subprocess.CompletedProcess(
            arguments, child.returncode, output, errors))) from None
    finally:
        if child.poll() is None:
            stop(child)
            child.wait(timeout=30)
    if received:
        raise SystemExit(128 + received[0])
    result = subprocess.CompletedProcess(arguments, child.returncode, output, errors)
    if result.returncode:
        raise AssertionError(outcome(result))
    return output + errors


def start(arguments, version, data, *, environment=None):
    if data.exists() and any(data.iterdir()):
        raise AssertionError("the startup data directory must be empty")
    if os.name != "nt":
        data.mkdir(mode=0o700, parents=True, exist_ok=True)
        data.chmod(0o700)
    label = command([*arguments, "--version"], env=environment)
    if version + " (test configuration)" not in label.splitlines():
        raise AssertionError(f"wrong client version: {label}")
    output = command([*arguments, "--data-dir", data, "--no-activate", "--check-startup"],
                     env=environment, timeout=120)
    if "Junto first frame rendered" not in output.splitlines():
        raise AssertionError("the client did not render its first frame: " + output)
    print(f"    version {version}: first frame with an empty profile", flush=True)


def fuse_environment(root):
    environment = os.environ.copy()
    for name in ["APPIMAGE_EXTRACT_AND_RUN", "APPIMAGE", "APPDIR"]:
        environment.pop(name, None)
    environment["TMPDIR"] = str(root)
    if not pathlib.Path("/dev/fuse").exists():
        reason = "/dev/fuse is absent from the build container"
    elif not os.access("/dev/fuse", os.R_OK | os.W_OK):
        reason = "the build account cannot read and write /dev/fuse"
    elif not (shutil.which("fusermount") or shutil.which("fusermount3")):
        reason = "the build image has no fusermount or fusermount3"
    else:
        print("    AppImage startup through FUSE", flush=True)
        return environment
    print(f"    FUSE startup unavailable: {reason}; checking extract-and-run", flush=True)
    environment["APPIMAGE_EXTRACT_AND_RUN"] = "1"
    return environment


def check(root, system, files, *, signing=None):
    root.mkdir()
    current = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    major, minor, patch = map(int, current.split("."))
    versions = [current, f"{major}.{minor}.{patch + 1}"]
    environment = os.environ.copy()
    environment.pop("JUNTO_UPDATE_SOURCE", None)
    environment.update(JUNTO_TEST_UPDATE_KEYS_FILE=str(files["release"][1]),
                       JUNTO_TEST_FRESHNESS_KEYS_FILE=str(files["freshness"][1]))
    if signing:
        environment.update(signing)
    arch = architecture()
    suffix = ".exe" if system == "windows" else ""
    packages = []
    for index, version in enumerate(versions):
        print(f"    build real client {version}", flush=True)
        environment["JUNTO_TEST_BUILD_VERSION"] = version
        command(["cargo", "build", "--locked", "-p", "junto-client", "--bin", "junto", "-p",
                 "junto-client-core", "--bin", "junto-release", "--bin", "junto-recover",
                 "-p", "junto-icon", "--bin", "junto-icon"], cwd=ROOT, env=environment, timeout=7200)
        output = root / f"build-{index}"
        if system == "macos":
            from bundle import assemble
            bundle = assemble(TARGET / "junto", output / "package-bundle/Junto.app", version,
                              TARGET / "junto-recover", icon_tool=TARGET / "junto-icon", build=TARGET)
            command([sys.executable, ROOT / "packaging/sign.py", bundle], env=environment)
            command(["/usr/bin/ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", bundle,
                     output / "Junto.zip"])
        elif system == "windows":
            import windows
            windows.assemble(TARGET / "junto.exe", TARGET / "junto-recover.exe", version, output, build=TARGET)
        else:
            import linux
            linux.assemble(TARGET / "junto", TARGET / "junto-recover", version, output,
                           icon_tool=TARGET / "junto-icon", build=TARGET)
        packages.append(output)
        if index == 0:
            updater = root / ("updater" + suffix)
            shutil.copy2(TARGET / ("junto-release" + suffix), updater)

    kinds = ["bundle"] if system == "macos" else ["directory", "appimage"] if system == "linux" else ["directory"]
    for kind in kinds:
        print(f"    {system} {kind}: install, update, rollback", flush=True)
        work = root / kind
        work.mkdir()
        source = work / "source"
        source.mkdir()
        for index, version in enumerate(versions):
            published = work / f"release-{index}"
            release_tools.package(updater, files, packages[index] / f"package-{kind}", published,
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

        def started(stage, version):
            for backend in (["x11", "wayland"] if system == "linux" else [None]):
                prefix = [ROOT / "tools/linux/display.sh", backend] if backend else []
                start([*prefix, binary], version, work / f"data-{stage}-{backend}", environment=runtime)
            if system == "macos" and requirement(application) != expected:
                raise AssertionError("the signing requirement changed")

        started("installed", current)
        with release_tools.source(source) as (server, address):
            if kind == "appimage":
                test_environment = os.environ.copy()
                test_environment.update(JUNTO_TEST_PACKAGE_APPIMAGE=str(application),
                                        JUNTO_TEST_PACKAGE_VERSION=current, JUNTO_TEST_PACKAGE_SOURCE=address,
                                        JUNTO_TEST_PACKAGE_RELEASE_KEYS=str(files["release"][1]),
                                        JUNTO_TEST_PACKAGE_FRESHNESS_KEYS=str(files["freshness"][1]))
                command(["cargo", "test", "--locked", "-p", "junto-client-core", "--test", "linux_packages",
                         "real_appimage_package_update", "--", "--ignored", "--exact", "--nocapture"],
                        cwd=ROOT, env=test_environment, timeout=3600)
            else:
                command([updater, "update", address, application])
            expected_paths = [f"/latest-{system}-{kind}-{arch}.json",
                              f"/junto-{versions[1]}-{system}-{kind}-{arch}.junto",
                              f"/release-{current}-{system}-{kind}-{arch}.json",
                              f"/junto-{current}-{system}-{kind}-{arch}.junto"]
            if server.paths != expected_paths:
                raise AssertionError(f"unexpected release requests: {server.paths}")
        started("updated", versions[1])
        record = installed / f".{application.name}.junto-update/record"
        command([record / f"previous/recover{suffix}", "rollback", application])
        started("restored", current)
        if (record / "previous").exists():
            raise AssertionError("rollback kept a previous release")
