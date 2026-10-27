#!/usr/bin/env python3
"""Checks of the Windows package on a Windows machine (ADR 0755): the ZIP is extracted into a
folder of the account, its program starts and draws its first frame, its window acts as a window
of Windows with its title bar and registers the identity of its notifications, a running
installation is updated from a local source, and its recovery program rolls the update back.

Each step prints its name; a failure prints the step, the command, its exit code and both of
its outputs. It builds the client and the release tools in the target directory of the
repository."""
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import zipfile

import icons
import licences
import release_tools
import check_lifecycle
import window_frame
import windows
from programs import outcome

TARGET = pathlib.Path(os.environ.get("CARGO_TARGET_DIR", release_tools.REPOSITORY / "target")) / "debug"
STEPS = []


def step(name):
    def decorate(function):
        STEPS.append((name, function))
        return function
    return decorate


def ran(arguments, **options):
    """Runs a command; a command that does not end in time is a failure with its output so far."""
    arguments = [str(argument) for argument in arguments]
    try:
        return subprocess.run(arguments, capture_output=True, text=True, timeout=options.pop("timeout", 180),
                              **options)
    except subprocess.TimeoutExpired as expired:
        raise AssertionError(outcome(subprocess.CompletedProcess(arguments, -1, expired.stdout, expired.stderr))
                             + f"\nthe command did not end within {expired.timeout} seconds") from None


def expect(condition, message, result=None):
    if not condition:
        raise AssertionError(message + ("\n" + outcome(result) if result is not None else ""))


def listing(directory):
    return sorted(path.relative_to(directory).as_posix() for path in directory.rglob("*"))


class Check:
    def __init__(self, root):
        self.root = root

    @step("build the client and the release tools")
    def build(self):
        release_tools.run("cargo", "build", "--locked", "-p", "junto-client", "--bin", "junto", "-p",
                          "junto-client-core", "--bin", "junto-release", "--bin", "junto-installer",
                          "-p", "junto-icon", "--bin", "junto-icon",
                          cwd=release_tools.REPOSITORY, timeout=7200)
        self.files = release_tools.keys(TARGET / "junto-release.exe", self.root)
        self.tool, self.recover = release_tools.build(self.files)
        for name, status in [("exit-0", 0), ("exit-1", 1)]:
            source = self.root / f"{name}.rs"
            source.write_text("fn main() {\n"
                              "    if std::env::args().nth(1).as_deref() == Some(\"--wait\") {\n"
                              "        std::thread::sleep(std::time::Duration::from_secs(600));\n"
                              "    }\n"
                              f"    std::process::exit({status});\n"
                              "}\n")
            release_tools.run("rustc", "--edition", "2021", "-o", self.root / f"{name}.exe", source)

    @step("assemble the ZIP of the client build")
    def assemble(self):
        self.packages = windows.assemble(TARGET / "junto.exe", self.recover, "1.0.0", self.root / "packages",
                                         build=TARGET, installer=TARGET / "junto-installer.exe",
                                         release_tool=TARGET / "junto-release.exe")
        self.setup = self.packages / "Junto-1.0.0-windows-x86_64-setup.exe"
        expect(self.setup.is_file(), "the release tool wrote no setup program")
        self.zip = self.packages / "Junto-1.0.0-windows-x86_64.zip"
        with zipfile.ZipFile(self.zip) as archive:
            names = sorted(archive.namelist())
        expect(not any(name.endswith(".pdb") for name in names), f"the ZIP holds symbols: {names}")
        expect((self.packages / "Junto-1.0.0-windows-x86_64.pdb").is_file(), "no file of the symbols")
        expect({"Junto/junto.exe", "Junto/junto-recover.exe", "Junto/uninstall.exe", "Junto/junto-package.json",
                "Junto/licences/Junto-LICENSE.txt", f"Junto/licences/{licences.NAME}"} <= set(names),
               f"ZIP entries: {names}")
        with zipfile.ZipFile(self.zip) as archive:
            text = archive.read(f"Junto/licences/{licences.NAME}").decode()
        triple = licences.target("windows", "x86_64")
        expect(text.startswith(f"Junto 1.0.0 for {triple}: third-party licences\n"), text[:200])
        missing = licences.graph(release_tools.REPOSITORY, triple) - licences.listed(text)
        expect(not missing, f"crates that the licence file does not list: {sorted(missing)}")
        expect("Copyright (c) 2011, The WebRTC project authors" in text, "the licence file has no WebRTC licence")
        expect(not any(name.lower().endswith(".dll") for name in names), f"the ZIP holds a library: {names}")

    @step("each program holds the icon group 1 with the rendering of the source at each size")
    def program_icon(self):
        for program in ["junto.exe", "junto-recover.exe", "uninstall.exe"]:
            with zipfile.ZipFile(self.zip) as archive:
                pictures = icons.program_icon(archive.read(f"Junto/{program}"))
            expect(sorted(pictures) == [16, 24, 32, 48, 64, 256],
                   f"the icon of {program} has the sizes {sorted(pictures)}")
            for size, picture in pictures.items():
                expect(picture == icons.png(icons.rendering(TARGET / "junto-icon.exe", size)),
                       f"the icon of {size} pixels of {program} is not the rendering of the source")

    @step("the client is a program of the Windows subsystem and the recovery program a console program")
    def subsystems(self):
        # Windows opens a console beside a program of the console subsystem that Explorer starts.
        for program, expected in [("junto.exe", windows.WINDOWS_SUBSYSTEM),
                                  ("junto-recover.exe", windows.CONSOLE_SUBSYSTEM),
                                  ("uninstall.exe", windows.WINDOWS_SUBSYSTEM)]:
            with zipfile.ZipFile(self.zip) as archive:
                found = windows.subsystem(archive.read(f"Junto/{program}"))
            expect(found == expected, f"{program} has the subsystem {found}, not {expected}")

    @step("install: extract the ZIP into a folder of the account")
    def install(self):
        programs = self.root / "Programs"
        with zipfile.ZipFile(self.zip) as archive:
            archive.extractall(programs)
        self.installed = programs / "Junto"
        description = json.loads((self.installed / "junto-package.json").read_text())
        expect(description == {"format": 1, "version": "1.0.0", "system": "windows", "package": "directory",
                               "architecture": "x86_64"}, f"description: {description}")

    @step("start: the installed program states its version")
    def version(self):
        result = ran([self.installed / "junto.exe", "--version"])
        expect(result.returncode == 0 and result.stdout.strip(), "the program did not start", result)
        self.stated = result.stdout.strip()
        print(f"    the program states: {self.stated}", flush=True)

    @step("start: the installed program draws its first frame")
    def first_frame(self):
        result = ran([self.installed / "junto.exe", "--data-dir", self.root / "data", "--no-activate",
                      "--check-startup"], timeout=300)
        expect(result.returncode == 0 and "Junto first frame rendered" in (result.stdout + result.stderr).splitlines(),
               "the program did not draw its first frame", result)

    @step("the window: its strip acts as the title bar, and its notifications have their identity")
    def window(self):
        data = self.root / "window-data"
        process = subprocess.Popen([str(self.installed / "junto.exe"), "--data-dir", str(data), "--no-activate"],
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            window_frame.check_frame(process.pid, expect)
            try:
                stdout, stderr = process.communicate(timeout=60)
            except subprocess.TimeoutExpired:
                raise AssertionError("the close button of the window did not end the program") from None
            expect(process.returncode == 0, "the program did not end without an error after its close",
                   subprocess.CompletedProcess(process.args, process.returncode, stdout, stderr))
        finally:
            if process.poll() is None:
                process.kill()
                process.communicate()
        values = window_frame.identity()
        expect(values is not None, "the program registered no identity of its notifications")
        picture = data / "notification-icon.png"
        expect(values.get("DisplayName", (None,))[0] == "Junto", f"the identity names {values}")
        expect(pathlib.Path(values.get("IconUri", ("",))[0]) == picture, f"the identity names {values}")
        expect(icons.png(picture.read_bytes()) == icons.png(icons.rendering(TARGET / "junto-icon.exe", 256)),
               "the picture of the notifications is not the rendering of the icon at 256 pixels")
        result = ran([self.installed / "junto-recover.exe", "unregister"])
        expect(result.returncode == 0 and "removed" in result.stdout, "the recovery program did not unregister",
               result)
        expect(window_frame.identity() is None, "the key of the identity is still there")
        result = ran([self.installed / "junto-recover.exe", "unregister"])
        expect(result.returncode == 0 and "no registration" in result.stdout,
               "a second removal of the identity did not succeed", result)

    @step("update a running installation from a local source")
    def update(self):
        def tree(directory, version, program):
            folder = directory / "Junto"
            folder.mkdir(parents=True)
            shutil.copyfile(program, folder / "junto.exe")
            shutil.copyfile(self.recover, folder / "junto-recover.exe")
            (folder / "junto-package.json").write_text(windows.description(version, "x86_64"), newline="\n")
        source = self.root / "release"
        source.mkdir()
        for version, program in [("1.0.0", "exit-0.exe"), ("2.0.0", "exit-1.exe")]:
            tree(self.root / f"tree-{version}", version, self.root / program)
            release_tools.package(self.tool, self.files, self.root / f"tree-{version}", self.root / f"out-{version}",
                                  version, "windows", "directory", "x86_64")
            for path in (self.root / f"out-{version}").iterdir():
                if version == "2.0.0" or not path.name.startswith("latest-"):
                    shutil.copyfile(path, source / path.name)
        self.place = self.root / "Place"
        tree(self.place, "1.0.0", self.root / "exit-0.exe")
        self.application = self.place / "Junto"
        self.running = subprocess.Popen([str(self.application / "junto.exe"), "--wait"])
        with release_tools.source(source) as (server, address):
            result = ran([self.tool, "update", address, self.application])
            expect(result.returncode == 0 and "version 1.0.0 is kept for a rollback" in result.stdout,
                   "the update failed", result)
            expect(server.paths == ["/latest-windows-directory-x86_64.json",
                                    "/junto-2.0.0-windows-directory-x86_64.junto",
                                    "/release-1.0.0-windows-directory-x86_64.json",
                                    "/junto-1.0.0-windows-directory-x86_64.junto"], f"requests: {server.paths}")
        expect(self.running.poll() is None, "the running program stopped during the update")
        result = ran([self.application / "junto.exe"])
        expect(result.returncode == 1, "the new program is not in place", result)
        update = self.place / ".Junto.junto-update"
        print(f"    the update directory holds: {listing(update)}", flush=True)
        # A staging directory that still holds the replaced running program has a mark, so that
        # the next operation removes it; one without a mark could hold the only recovery copy.
        names = {path.name for path in update.iterdir()}
        unmarked = [name for name in names if name.startswith("stage-") and f"finished-{name}" not in names]
        expect(not unmarked, f"staging directories without a mark: {unmarked}")

    @step("roll back with the recovery program of the installation")
    def rollback(self):
        self.running.kill()
        self.running.wait()
        record = self.place / ".Junto.junto-update/record"
        recovery = [record / "previous/recover.exe", "rollback", self.application]
        archive = record / "previous/archive.junto"
        backup = archive.read_bytes()
        archive.unlink()
        archive.write_bytes(b"changed archive")
        result = ran(recovery)
        expect(result.returncode != 0, "a changed archive was restored", result)
        archive.unlink()
        archive.write_bytes(backup)
        result = ran(recovery)
        expect(result.returncode == 0 and "restored version 1.0.0" in result.stdout, "the rollback failed", result)
        result = ran([self.application / "junto.exe"])
        expect(result.returncode == 0, "the previous program is not in place", result)
        # The record now names the restored release as installed and has no older one.
        expect(not (record / "previous").exists(), "the record still holds a previous release")
        result = ran([record / "installed/recover.exe", "rollback", self.application])
        expect(result.returncode != 0, "a second rollback was accepted", result)
        update = self.place / ".Junto.junto-update"
        left = [path.name for path in update.iterdir() if path.name.startswith(("stage-", "finished-"))]
        expect(not left, f"staging directories were left: {left}")

    @step("damaged setup programs refuse removal and preserve their containing folder")
    def damaged_setup(self):
        directory = self.root / "downloads"
        directory.mkdir()
        sentinel = directory / "personal.txt"
        sentinel.write_text("keep")
        for name, content in [("truncated.exe", self.setup.read_bytes()[:-10]),
                              ("missing.exe", (TARGET / "junto-installer.exe").read_bytes())]:
            program = directory / name
            program.write_bytes(content)
            for arguments in (["--quiet"], ["--uninstall", "--quiet"]):
                result = ran([program, *arguments])
                expect(result.returncode != 0, "a damaged setup program accepted removal", result)
                expect(sentinel.read_text() == "keep", "the containing folder was removed")

    @step("the setup program installs for the account, and its uninstall program removes it (ADR 0728)")
    def setup_program(self):
        expect(windows.subsystem(self.setup.read_bytes()) == windows.WINDOWS_SUBSYSTEM,
               "the setup program is not a program of the Windows subsystem")
        pictures = icons.program_icon(self.setup.read_bytes())
        expect(sorted(pictures) == [16, 24, 32, 48, 64, 256], f"the icon of the setup program: {sorted(pictures)}")
        local, roaming, temporary = (self.root / "setup-local", self.root / "setup-roaming", self.root / "setup-temp")
        for directory in (local, roaming, temporary):
            directory.mkdir()
        environment = dict(os.environ, LOCALAPPDATA=str(local), APPDATA=str(roaming), TEMP=str(temporary),
                           TMP=str(temporary))
        folder = local / "Programs/Junto"
        shortcut = roaming / "Microsoft/Windows/Start Menu/Programs/Junto.lnk"
        expected = self.packages / "package-directory/Junto"
        with window_frame.kept_uninstall_entry():
            for remove_data in (False, True):
                result = ran([self.setup, "--quiet", "--no-start"], env=environment, timeout=300)
                expect(result.returncode == 0 and "installed version 1.0.0" in result.stdout,
                       "the setup program did not install", result)
                check_lifecycle.compare(folder.parent, folder, expected)
                expect(shortcut.is_file(), "the setup program made no Start menu entry")
                entry = window_frame.uninstall_entry()
                expect(entry is not None, "the setup program wrote no entry of the installed programs")
                expect(entry["DisplayVersion"][0] == "1.0.0", f"the entry states {entry}")
                expect(entry["UninstallString"][0] == f'"{folder / "uninstall.exe"}" --uninstall', f"the entry states {entry}")
                expect(entry["InstallLocation"][0] == str(folder), f"the entry states {entry}")
                # The program states the version of its build, not the version of the test package.
                result = ran([folder / "junto.exe", "--version"], env=environment)
                expect(result.returncode == 0 and result.stdout.strip() == self.stated,
                       "the installed program did not start", result)
                # A second run of the setup program replaces the folder.
                result = ran([self.setup, "--quiet", "--no-start"], env=environment, timeout=300)
                expect(result.returncode == 0, "a second run of the setup program failed", result)
                check_lifecycle.compare(folder.parent, folder, expected)
                data = local / "Junto"
                data.mkdir(exist_ok=True)
                (data / "kept.txt").write_text("data")
                arguments = [folder / "uninstall.exe", "--uninstall", "--quiet"] + (["--remove-data"] if remove_data else [])
                result = ran(arguments, env=environment, timeout=60)
                expect(result.returncode == 0, "the uninstall program failed", result)
                # A copy outside the folder removes it after the uninstall program ended, then
                # removes itself.
                removed = check_lifecycle.wait_until(
                    lambda: not folder.exists() and window_frame.uninstall_entry() is None
                    and not any(temporary.iterdir()), 60)
                expect(removed, f"the removal did not end: folder {folder.exists()}, entry "
                                f"{window_frame.uninstall_entry()}, left {listing(temporary)}")
                expect(not shortcut.exists(), "the Start menu entry is still there")
                expect(not (local / "Programs/.Junto.junto-update").exists(), "the update directory is still there")
                expect((data / "kept.txt").exists() != remove_data,
                       f"the data directory {'was not' if remove_data else 'was'} removed")
        print("    the setup program installed and the uninstall program removed Junto twice", flush=True)

    @step("real client packages: install, first frame, update, first frame, rollback, first frame")
    def real_client_lifecycle(self):
        check_lifecycle.check(self.root, "windows", self.files)


def main():
    import ctypes
    ctypes.windll.kernel32.SetErrorMode(0x0001 | 0x0002 | 0x8000)
    temporary = pathlib.Path(tempfile.mkdtemp(prefix="junto-windows-package-"))
    check = Check(temporary)
    try:
        # Each start of a client writes the key of the identity of the notifications.
        with window_frame.kept_identity():
            for number, (name, function) in enumerate(STEPS, 1):
                print(f"[{number}/{len(STEPS)}] {name}", flush=True)
                try:
                    function(check)
                except Exception as error:
                    print(f"FAILED at step {number}: {name}\n{type(error).__name__}: {error}", flush=True)
                    return 1
        print(f"all {len(STEPS)} steps of the Windows package check passed", flush=True)
        return 0
    finally:
        running = getattr(check, "running", None)
        if running is not None and running.poll() is None:
            running.kill()
            running.wait()
        check_lifecycle.remove_tree(temporary)


if __name__ == "__main__":
    sys.exit(main())
