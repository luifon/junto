#!/usr/bin/env python3
"""Checks of the Linux packages in the Linux build image (ADR 0754): what the tarball and the
AppImage contain, that each starts and draws its first frame on a virtual display, and an
update and a rollback of the directory package with the recovery program.

Run it in the build image: tools/linux-container.sh python3 packaging/check_linux.py
It builds the client and the release tools in the target directory of the container."""
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tarfile
import tempfile

import icons
import licences
import linux
import release_tools
import check_lifecycle
from configuration import architecture as machine_architecture
from programs import outcome

DISPLAY = release_tools.REPOSITORY / "tools/linux/display.sh"
TARGET = pathlib.Path(os.environ.get("CARGO_TARGET_DIR", release_tools.REPOSITORY / "target")) / "debug"
# The AppImage runtime of the build image and its source, as files of this system.
RUNTIME, RUNTIME_SOURCE = pathlib.Path(linux.RUNTIME), pathlib.Path(linux.RUNTIME_SOURCE)
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


def started(program, backend, data, environment=None):
    """Starts a program of a package with the client's start check on a virtual display."""
    result = ran([DISPLAY, backend, program, "--data-dir", data, "--no-activate", "--check-startup"],
                 env=environment or os.environ.copy(), timeout=300)
    expect(result.returncode == 0 and "Junto first frame rendered" in (result.stdout + result.stderr).splitlines(),
           f"{program} did not draw its first frame on {backend}", result)


class Check:
    def __init__(self, root):
        self.root = root
        self.architecture = machine_architecture()

    @step("build the client and the release tools")
    def build(self):
        release_tools.run("cargo", "build", "--locked", "-p", "junto-client", "--bin", "junto", "-p",
                          "junto-client-core", "--bin", "junto-release", "-p", "junto-icon", "--bin", "junto-icon",
                          cwd=release_tools.REPOSITORY, timeout=7200)
        self.icon_tool = TARGET / "junto-icon"
        self.client = self.root / "junto"
        shutil.copyfile(TARGET / "junto", self.client)
        self.client.chmod(0o755)
        self.files = release_tools.keys(TARGET / "junto-release", self.root)
        self.tool, self.recover = release_tools.build(self.files)

    @step("assemble the tarball and the AppImage")
    def assemble(self):
        self.packages = linux.assemble(self.client, self.recover, "1.0.0", self.root / "packages",
                                       icon_tool=self.icon_tool, build=TARGET)
        self.tarball = self.packages / f"junto-1.0.0-linux-{self.architecture}.tar.gz"
        self.appimage = self.packages / f"Junto-1.0.0-{self.architecture}.AppImage"
        self.deb = self.packages / linux.deb_name("1.0.0", self.architecture)
        expect(self.tarball.is_file() and self.appimage.is_file() and self.deb.is_file(),
               "a package file is missing")

    @step("the tarball holds the program, its libraries, the menu entry, the icons and the licences")
    def tarball_contents(self):
        with tarfile.open(self.tarball) as archive:
            members = archive.getmembers()
            names = {member.name for member in members}
            expect(all(member.uid == 0 and member.gid == 0 for member in members), "an entry is not owned by root")
            expect(not any(member.issym() or member.islnk() for member in members), "the tarball holds a link")
            installed = self.root / "installed"
            # The filter of extraction exists from Python 3.11.4; the build image has 3.11.2.
            archive.extractall(installed, **({"filter": "data"} if hasattr(tarfile, "data_filter") else {}))
        expected = {"junto", "junto/junto", "junto/junto-recover", "junto/junto-package.json", "junto/install-menu-entry",
                    "junto/lib", f"junto/share/applications/{linux.APPLICATION_ID}.desktop"}
        expect(expected <= names, f"missing entries: {sorted(expected - names)}")
        libraries = {name.split("/")[-1] for name in names if name.startswith("junto/lib/")}
        expect(libraries == linux.BUNDLED, f"bundled libraries: {sorted(libraries)}")
        expect(not libraries & linux.HOST, "a library of the host is bundled")
        found = {name.split("/")[-1] for name in names if name.startswith("junto/licences/")}
        expect(found == {"Junto-LICENSE.txt", licences.NAME}, f"licences: {sorted(found)}")
        self.licence_file(installed / "junto", appimage=False)
        description = json.loads((installed / "junto/junto-package.json").read_text())
        expect(description == {"format": 1, "version": "1.0.0", "system": "linux", "package": "directory",
                               "architecture": self.architecture}, f"description: {description}")
        self.installed = installed / "junto"
        self.icon_theme(self.installed)

    def licence_file(self, directory, appimage):
        """The licence file of a package lists every crate of the programs and holds the copyright
        files of the bundled libraries; the AppImage's also states the licence of its runtime."""
        triple = licences.target("linux", self.architecture)
        text = (directory / "licences" / licences.NAME).read_text(encoding="utf-8")
        expect(text.startswith(f"Junto 1.0.0 for {triple}: third-party licences\n"), text[:200])
        missing = licences.graph(release_tools.REPOSITORY, triple) - licences.listed(text)
        expect(not missing, f"crates that the licence file does not list: {sorted(missing)}")
        expect("Copyright (c) 2011, The WebRTC project authors" in text, "the licence file has no WebRTC licence")
        for package in linux.LIBRARY_PACKAGES:
            statement = licences.normal(pathlib.Path(f"/usr/share/doc/{package}/copyright").read_bytes())
            expect(statement in text, f"the licence file has no copyright file of {package}")
        expect(("The AppImage runtime" in text) == appimage, "the statement of the AppImage runtime")
        if appimage:
            for component, archive, names in linux.RUNTIME_COMPONENTS:
                for name in names:
                    licence = licences.normal(linux.archive_member(RUNTIME_SOURCE / "sources" / archive, name))
                    expect(licence in text, f"the licence file has no {name} of {component}")
            lgpl = linux.archive_member(RUNTIME_SOURCE / "sources/fuse-3.15.0.tar.xz", "LGPL2.txt")
            expect(licences.licences_in(lgpl) == {"LGPL-2.1-only"}, "LGPL2.txt of libfuse is not the LGPL 2.1")
            for label, path in linux.RUNTIME_PACKAGES:
                expect(licences.normal(pathlib.Path(path).read_bytes()) in text,
                       f"the licence file has no copyright file of {label}")

    def icon_theme(self, directory):
        """The icon theme of a package: the SVG source and its rendering at each size."""
        theme = directory / "share/icons/hicolor"
        expected = {f"{size}x{size}" for size in linux.ICON_SIZES} | {"scalable"}
        found = {path.name for path in theme.iterdir()}
        expect(found == expected, f"the icon theme has the sizes {sorted(found)}")
        expect((theme / f"scalable/apps/{linux.APPLICATION_ID}.svg").read_bytes() == icons.SOURCE.read_bytes(),
               "the scalable icon is not the SVG source")
        for size in linux.ICON_SIZES:
            picture = (theme / f"{size}x{size}/apps/{linux.APPLICATION_ID}.png").read_bytes()
            expect(picture == icons.rendering(self.icon_tool, size), f"the icon of {size} pixels is not the rendering")

    @step("the program loads the bundled libraries from the package and the others from the host")
    def tarball_libraries(self):
        result = ran(["readelf", "-d", self.installed / "junto"])
        expect("Library rpath: [$ORIGIN/lib]" in result.stdout, "the program has no search path of the package",
               result)
        found = linux.libraries(self.installed / "junto")
        for name in linux.BUNDLED:
            expect(pathlib.Path(found[name]).parent == self.installed / "lib",
                   f"{name} is loaded from {found[name]}")
        for name, path in found.items():
            if name not in linux.BUNDLED:
                expect(not str(path).startswith(str(self.installed)), f"{name} of the host is loaded from the package")

    @step("the tarball's program draws its first frame under X11 and Wayland")
    def tarball_start(self):
        for backend in ["x11", "wayland"]:
            started(self.installed / "junto", backend, self.root / f"data-tarball-{backend}")

    @step("the menu entry names this copy, and its icons are installed")
    def menu_entry(self):
        home = self.root / "home"
        result = ran([self.installed / "install-menu-entry"], env=dict(os.environ, HOME=str(home), XDG_DATA_HOME=""))
        expect(result.returncode == 0, "the menu entry was not written", result)
        entry = (home / f".local/share/applications/{linux.APPLICATION_ID}.desktop").read_text()
        expect(f'Exec="{self.installed}/junto"' in entry and f"Icon={linux.APPLICATION_ID}" in entry, entry)
        self.icon_theme(home / ".local")

    @step("the Debian package holds the program of the tarball, names its dependencies, and its program starts")
    def debian_package(self):
        result = ran(["dpkg-deb", "--field", self.deb, "Package", "Version", "Architecture", "Depends"])
        expect(result.returncode == 0, "dpkg-deb cannot read the package", result)
        fields = dict(line.split(": ", 1) for line in result.stdout.splitlines())
        expect(fields.get("Package") == "junto" and fields.get("Version") == "1.0.0"
               and fields.get("Architecture") == linux.DEB_ARCHITECTURES[self.architecture], f"fields: {fields}")
        # Each dependency names a package that this Debian release has: the first name of each
        # alternative is installed in the build image, which runs the program.
        for dependency in fields["Depends"].split(", "):
            name = dependency.split(" | ")[0].split(" ")[0]
            found = ran(["dpkg-query", "-W", "-f", "${Status}", name])
            expect("install ok installed" in found.stdout, f"the dependency {name} is not a package of the image",
                   found)
        extracted = self.root / "deb"
        result = ran(["dpkg-deb", "-x", self.deb, extracted])
        expect(result.returncode == 0, "dpkg-deb cannot extract the package", result)
        program = extracted / "usr/lib/junto"
        description = json.loads((program / "junto-package.json").read_text())
        expect(description == {"format": 1, "version": "1.0.0", "system": "linux", "package": "deb",
                               "architecture": self.architecture}, f"description: {description}")
        expect(os.readlink(extracted / "usr/bin/junto") == "../lib/junto/junto", "the link of the program")
        expect((program / "junto").read_bytes() == (self.installed / "junto").read_bytes(),
               "the program of the Debian package differs from the one of the tarball")
        libraries = {path.name for path in (program / "lib").iterdir()}
        expect(libraries == linux.BUNDLED, f"bundled libraries: {sorted(libraries)}")
        entry = (extracted / f"usr/share/applications/{linux.APPLICATION_ID}.desktop").read_text()
        expect("Exec=junto" in entry, entry)
        self.icon_theme(extracted / "usr")
        expect((extracted / "usr/share/doc/junto/copyright").is_file(), "the copyright file is missing")
        expect(not (program / "junto-recover").exists() and not (program / "install-menu-entry").exists(),
               "the Debian package holds the update tools of the directory package")
        for backend in ["x11", "wayland"]:
            started(extracted / "usr/bin/junto", backend, self.root / f"data-deb-{backend}")

    @step("the AppImage holds the same program and libraries, the menu entry and the icon")
    def appimage_contents(self):
        start = self.appimage.read_bytes()[:11]
        expect(start[:4] == b"\x7fELF" and start[8:] == b"AI\x02", "the file has no AppImage mark")
        extracted = self.root / "extracted"
        extracted.mkdir()
        result = ran([self.appimage, "--appimage-extract"], cwd=extracted)
        expect(result.returncode == 0, "the AppImage was not extracted", result)
        squashfs = extracted / "squashfs-root"
        expect(os.readlink(squashfs / "AppRun") == "junto", "AppRun does not start the program")
        for name in [f"{linux.APPLICATION_ID}.desktop", f"{linux.APPLICATION_ID}.svg", ".DirIcon", "junto-recover",
                     "licences/Junto-LICENSE.txt"]:
            expect((squashfs / name).is_file(), f"the AppImage has no {name}")
        self.licence_file(squashfs, appimage=True)
        carried = squashfs / "licences/appimage-runtime"
        expected = sorted(path.relative_to(RUNTIME_SOURCE) for path in RUNTIME_SOURCE.rglob("*"))
        expect(sorted(path.relative_to(carried) for path in carried.rglob("*")) == expected,
               f"the source of the runtime in the AppImage: {sorted(carried.rglob('*'))}")
        for name in expected:
            path = carried / name
            expect(path.is_dir() or path.read_bytes() == (RUNTIME_SOURCE / name).read_bytes(),
                   f"licences/appimage-runtime/{name} differs from the source of the runtime")
        relink = carried / "relink"
        named = linux.link_inputs((relink / "link.sh").read_text())
        held = sorted(path.name for path in relink.iterdir())
        expect(held == sorted([*named, "link.sh"]),
               f"the directory relink holds {held}, and link.sh links {named}")
        self.runtime_source = carried
        expect({path.name for path in (squashfs / "lib").iterdir()} == linux.BUNDLED, "bundled libraries differ")
        self.icon_theme(squashfs)
        expect((squashfs / ".DirIcon").read_bytes() == icons.rendering(self.icon_tool, 256),
               "the icon of the AppImage is not the rendering of 256 pixels")
        expect((squashfs / f"{linux.APPLICATION_ID}.svg").read_bytes() == icons.SOURCE.read_bytes(),
               "the SVG icon of the AppImage is not the source")
        expect((squashfs / "junto").read_bytes() == self.client.read_bytes(), "the program differs from the build")
        update = self.packages / "package-appimage"
        expect(sorted(path.name for path in update.iterdir()) == ["Junto.AppImage", "junto-package.json",
                                                                    "junto-recover"], "update source of the AppImage")

    @step("the files of relink in the AppImage link its runtime again, byte for byte")
    def runtime_from_relink(self):
        runtime = RUNTIME.read_bytes()
        expect(self.appimage.read_bytes()[:len(runtime)] == runtime, "the AppImage does not start with the runtime")
        linked = self.root / "runtime-linked"
        result = ran(["sh", self.runtime_source / "relink/link.sh", linked], cwd=self.root)
        expect(result.returncode == 0, "link.sh of the AppImage failed", result)
        expect(linked.read_bytes() == runtime,
               "the runtime that link.sh links differs from the runtime of the AppImage")

    @step("the source that the AppImage carries builds its runtime and the files of relink again, byte for byte")
    def runtime_from_source(self):
        rebuilt, relink = self.root / "runtime-rebuilt", self.root / "relink-rebuilt"
        result = ran(["sh", self.runtime_source / "build.sh", rebuilt, relink], cwd=self.root, timeout=1800)
        expect(result.returncode == 0, "the build script of the runtime failed", result)
        expect(rebuilt.read_bytes() == RUNTIME.read_bytes(),
               "the runtime built from the source of the AppImage differs from the runtime of the AppImage")
        carried = self.runtime_source / "relink"
        for path in sorted(carried.iterdir()):
            expect((relink / path.name).read_bytes() == path.read_bytes(),
                   f"relink/{path.name} built from the source differs from the one of the AppImage")
        expect(sorted(path.name for path in relink.iterdir()) == sorted(path.name for path in carried.iterdir()),
               "the rebuilt relink directory holds other files")

    @step("the AppImage's program draws its first frame under X11")
    def appimage_start(self):
        # The build container has no FUSE: the runtime extracts the image and runs it.
        started(self.appimage, "x11", self.root / "data-appimage",
                dict(os.environ, APPIMAGE_EXTRACT_AND_RUN="1", TMPDIR=str(self.root)))

    @step("a directory package with a running program is updated from a local source and rolled back")
    def update_and_rollback(self):
        def tree(directory, version, status):
            program = directory / "junto/junto"
            program.parent.mkdir(parents=True)
            program.write_text(f"#!/bin/sh\nif [ \"${{1:-}}\" = --wait ]; then exec sleep 600; fi\nexit {status}\n")
            program.chmod(0o755)
            shutil.copyfile(self.recover, directory / "junto/junto-recover")
            (directory / "junto/junto-recover").chmod(0o755)
            (directory / "junto/junto-package.json").write_text(linux.description(version, "directory",
                                                                                   self.architecture))
        source = self.root / "release"
        for version, status in [("1.0.0", 0), ("2.0.0", 1)]:
            tree(self.root / f"tree-{version}", version, status)
            release_tools.package(self.tool, self.files, self.root / f"tree-{version}", self.root / f"out-{version}",
                                  version, "linux", "directory", self.architecture)
        source.mkdir()
        for path in (self.root / "out-1.0.0").iterdir():
            if not path.name.startswith("latest-"):
                shutil.copyfile(path, source / path.name)
        for path in (self.root / "out-2.0.0").iterdir():
            shutil.copyfile(path, source / path.name)
        place = self.root / "place"
        tree(place, "1.0.0", 0)
        application = place / "junto"
        running = subprocess.Popen([application / "junto", "--wait"])
        try:
            with release_tools.source(source) as (server, address):
                update = [self.tool, "update", address, application]
                place.chmod(0o555)
                try:
                    result = ran(update)
                finally:
                    place.chmod(0o755)
                expect(result.returncode != 0 and "move Junto" in result.stderr, "a read-only place was updated",
                       result)
                expect(server.paths == [], f"a read-only place asked the source: {server.paths}")
                result = ran(update)
                expect(result.returncode == 0 and "version 1.0.0 is kept for a rollback" in result.stdout,
                       "the update failed", result)
                arch = self.architecture
                expect(server.paths == [f"/latest-linux-directory-{arch}.json",
                                        f"/junto-2.0.0-linux-directory-{arch}.junto",
                                        f"/release-1.0.0-linux-directory-{arch}.json",
                                        f"/junto-1.0.0-linux-directory-{arch}.junto"], f"requests: {server.paths}")
            expect(running.poll() is None, "the running program stopped during the update")
            result = ran([application / "junto"])
            expect(result.returncode == 1, "the new program is not in place", result)
            record = place / ".junto.junto-update/record"
            recovery = [record / "previous/recover", "rollback", application]
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
            result = ran([application / "junto"])
            expect(result.returncode == 0, "the previous program is not in place", result)
            # The record now names the restored release as installed and has no older one.
            expect(not (record / "previous").exists(), "the record still holds a previous release")
            result = ran([record / "installed/recover", "rollback", application])
            expect(result.returncode != 0, "a second rollback was accepted", result)
        finally:
            running.kill()
            running.wait()

    @step("real client packages: install, first frame, update, first frame, rollback, first frame")
    def real_client_lifecycle(self):
        check_lifecycle.check(self.root, "linux", self.files)


def main():
    with tempfile.TemporaryDirectory(prefix="junto-linux-packages-") as temporary:
        check = Check(pathlib.Path(temporary))
        for number, (name, function) in enumerate(STEPS, 1):
            print(f"[{number}/{len(STEPS)}] {name}", flush=True)
            try:
                function(check)
            except Exception as error:
                print(f"FAILED at step {number}: {name}\n{type(error).__name__}: {error}", flush=True)
                return 1
        print(f"all {len(STEPS)} steps of the Linux package check passed", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
