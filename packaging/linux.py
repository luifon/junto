#!/usr/bin/env python3
"""Assemble the Linux packages of a client build: the directory package as a tarball, and the
AppImage (ADR 0754). Each also gets the source directory of its update archive.

Runs in the Linux build image (tools/linux/Dockerfile), which holds the programs used here:
ldd, mksquashfs, and the AppImage runtime with its source."""
import argparse
import gzip
import io
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tarfile

import icons
import licences
from configuration import architecture as machine_architecture

APPLICATION_ID = "chat.junto.Junto"
# The paths of the build image are POSIX paths on every system: the Dockerfile names them, and
# the unit tests read them on Windows too. The image reads its files through pathlib.Path.
RUNTIME = pathlib.PurePosixPath("/opt/junto-linux/appimage-runtime")
# The source of the runtime, its build script, and the directory relink with every file of the
# link of the runtime and the script that links it (tools/linux/appimage-runtime), which the
# AppImage carries in licences/appimage-runtime (ADR 0801).
RUNTIME_SOURCE = pathlib.PurePosixPath("/opt/junto-linux/appimage-runtime-source")

# The libraries that a package carries: the X11 keyboard libraries and the XKB extension of
# the X11 connection library, which hosts without an X11 keyboard library often lack.
BUNDLED = {"libxkbcommon.so.0", "libxkbcommon-x11.so.0", "libxcb-xkb.so.1"}
# The libraries that a package takes from the host: the C library and its loader; the X11
# connection library and the libraries that it loads, because the graphics drivers of the
# host load its other extensions, which need the host's version of it; and the sound
# libraries, which load their plugins and configuration from the host.
HOST = {
    "linux-vdso.so.1", "libc.so.6", "libm.so.6", "libgcc_s.so.1", "ld-linux-x86-64.so.2",
    "ld-linux-aarch64.so.1", "libxcb.so.1", "libXau.so.6", "libXdmcp.so.6", "libbsd.so.0", "libmd.so.0",
    "libpipewire-0.3.so.0", "libasound.so.2",
}
# The Debian packages of the bundled libraries, whose copyright files the licence file holds.
LIBRARY_PACKAGES = ["libxkbcommon0", "libxkbcommon-x11-0", "libxcb-xkb1"]
# The components of the AppImage runtime: the source archive of each in the directory sources
# of RUNTIME_SOURCE, and the files of the archive that hold its licence.
RUNTIME_COMPONENTS = [
    ("the type 2 runtime of the AppImage project, commit dd6cebedcbddde9c82f89b011e8e1d40b6e43868 (release 20251108)",
     "type2-runtime-dd6cebedcbddde9c82f89b011e8e1d40b6e43868.tar.gz", ["LICENSE"]),
    ("libfuse 3.15.0, with the change of the AppImage project", "fuse-3.15.0.tar.xz", ["LICENSE", "LGPL2.txt"]),
    ("squashfuse 0.5.2", "squashfuse-0.5.2.tar.gz", ["LICENSE"]),
    ("zstd 1.5.6, under its BSD licence", "zstd-1.5.6.tar.gz", ["LICENSE"]),
    ("zlib 1.3.1", "zlib-1.3.1.tar.gz", ["LICENSE"]),
    ("mimalloc 2.1.7", "mimalloc-2.1.7.tar.gz", ["LICENSE"]),
]
# The parts of the runtime that come from Debian packages of the Linux build image, with the
# copyright file of each package.
RUNTIME_PACKAGES = [
    ("musl 1.2.3, the C library, from the Debian package musl-dev 1.2.3-1",
     pathlib.PurePosixPath("/usr/share/doc/musl/copyright")),
    ("the builtins and start files of compiler-rt of LLVM 22.1.8, from the Debian package libclang-rt-22-dev "
     "1:22.1.8-1~deb12u1", pathlib.PurePosixPath("/usr/share/doc/libclang-rt-22-dev/copyright")),
]
DESKTOP = f"""[Desktop Entry]
Type=Application
Name=Junto
Comment=Chat, calls and call recordings on your own server
Exec=junto
Icon={APPLICATION_ID}
Terminal=false
Categories=Network;Chat;InstantMessaging;
StartupWMClass={APPLICATION_ID}
"""

# The sizes of the hicolor icon theme that a package holds as PNG pictures, beside the SVG
# source in `scalable` (ADR 0754, ADR 0756).
ICON_SIZES = [16, 22, 24, 32, 48, 64, 128, 256, 512]

# Writes the menu entry and the icons of a directory package for the person who runs it, with
# the absolute path of this copy. Run it again after moving the directory.
INSTALL_ENTRY = f"""#!/bin/sh
set -eu
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
data=${{XDG_DATA_HOME:-$HOME/.local/share}}
mkdir -p "$data/applications"
for icon in "$here"/share/icons/hicolor/*/apps/{APPLICATION_ID}.*; do
    size=$(basename "$(dirname "$(dirname "$icon")")")
    mkdir -p "$data/icons/hicolor/$size/apps"
    cp "$icon" "$data/icons/hicolor/$size/apps/"
done
sed "s|^Exec=junto$|Exec=\\"$here/junto\\"|" "$here/share/applications/{APPLICATION_ID}.desktop" \\
    > "$data/applications/{APPLICATION_ID}.desktop"
echo "Junto is in the application menu: $data/applications/{APPLICATION_ID}.desktop"
"""


def libraries(binary):
    """The shared libraries that the loader resolves for `binary`, with their paths."""
    result = subprocess.run(["ldd", str(binary)], capture_output=True, text=True, check=True,
                            env=dict(os.environ, LD_LIBRARY_PATH=""))
    return parse_ldd(result.stdout)


def parse_ldd(text):
    found = {}
    for line in text.splitlines():
        missing = re.match(r"\s*(\S+) => not found$", line)
        if missing:
            found[missing.group(1)] = None
            continue
        match = re.match(r"\s*(\S+)(?: => (\S+))? \(0x[0-9a-f]+\)$", line)
        if not match:
            raise ValueError(f"unexpected ldd line: {line}")
        name, path = match.groups()
        found[pathlib.PurePosixPath(name).name] = path or name
    return found


def classify(found):
    """Splits the libraries into those to bundle and those of the host. A library in neither
    list stops the build until a person decides where it belongs."""
    missing = sorted(name for name in found if found[name] is None)
    if missing:
        raise ValueError("libraries that the loader does not find: " + ", ".join(missing))
    unknown = sorted(set(found) - BUNDLED - HOST)
    if unknown:
        raise ValueError("libraries that are neither bundled nor taken from the host: " + ", ".join(unknown))
    return {name: found[name] for name in found if name in BUNDLED}


def description(version, package, architecture):
    return json.dumps({"format": 1, "version": version, "system": "linux", "package": package,
                       "architecture": architecture}) + "\n"


def write(path, content, mode=0o644):
    path.parent.mkdir(parents=True, exist_ok=True)
    if isinstance(content, str):
        content = content.encode()
    path.write_bytes(content)
    path.chmod(mode)


def copy(source, destination, mode):
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)
    destination.chmod(mode)


def icon_theme(directory, icon_tool):
    """The icons of the program in `directory/share/icons/hicolor`: the SVG source, and a
    rendering by `icon_tool` (the program `junto-icon`) for each size of ICON_SIZES."""
    theme = directory / "share/icons/hicolor"
    copy(icons.SOURCE, theme / f"scalable/apps/{APPLICATION_ID}.svg", 0o644)
    for size in ICON_SIZES:
        picture = theme / f"{size}x{size}/apps/{APPLICATION_ID}.png"
        picture.parent.mkdir(parents=True, exist_ok=True)
        icons.run(icon_tool, "png", size, picture)
        picture.chmod(0o644)


def tree(directory, client, recover, version, architecture, licence_texts, icon_tool):
    """The directory of the program: the AppDir of the AppImage has the same files."""
    copy(client, directory / "junto", 0o755)
    copy(recover, directory / "junto-recover", 0o755)
    for name, path in sorted(classify(libraries(client)).items()):
        copy(path, directory / "lib" / name, 0o644)
    write(directory / f"share/applications/{APPLICATION_ID}.desktop", DESKTOP)
    icon_theme(directory, icon_tool)
    for name, text in licence_texts.items():
        write(directory / "licences" / name, text)


def archive_member(archive, name):
    """The content of the file `name` in the top directory of a source archive."""
    with tarfile.open(archive) as source:
        [member] = [member for member in source.getmembers()
                    if member.isfile() and pathlib.PurePosixPath(member.name).parts[1:] == (name,)]
        return source.extractfile(member).read()


def link_inputs(script):
    """The files that the ld command of a link script of the runtime (link.sh) links, in order."""
    command = re.search(r"^ld (.*?[^\\])$", script.replace("\\\n", " "), re.MULTILINE)
    if not command:
        raise ValueError("the link script has no ld command")
    words = command.group(1).split()
    return [word for word in words if re.fullmatch(r"[\w.+-]+\.(o|a|ld)", word)]


def runtime_section(runtime_source):
    """The section of the licence file on the AppImage runtime, with the licence texts of each of
    its components, from their source archives in `runtime_source`."""
    components = [(f"{component}: {name}", archive_member(runtime_source / "sources" / archive, name))
                  for component, archive, names in RUNTIME_COMPONENTS for name in names]
    components.extend((f"{label}: {path}", pathlib.Path(path).read_bytes()) for label, path in RUNTIME_PACKAGES)
    return licences.Section("The AppImage runtime", [
        "The AppImage file starts with its runtime, the program that mounts the SquashFS image that follows it and "
        "starts junto. The runtime is the type 2 runtime of the AppImage project under the MIT licence, linked "
        "statically with libfuse under the GNU Lesser General Public License 2.1, squashfuse, zstd, zlib, "
        "mimalloc, the musl C library, and the builtins and start files of compiler-rt of LLVM. The directory "
        "licences/appimage-runtime of the AppImage holds the complete source of the runtime and of libfuse, "
        "squashfuse, zstd, zlib and mimalloc, the script that built the runtime from it, and the directory relink "
        "with every file of the link of the runtime (among them musl's libc.a and the builtins of compiler-rt) and "
        "the script link.sh that links them. Its README.md gives the steps to link the runtime with a changed "
        "libfuse and to put it into the AppImage."], components)


def licence_texts(version, architecture, build, appimage=False, runtime_source=RUNTIME_SOURCE):
    """The files of the directory `licences`: the licence of Junto and the licence file, which
    also holds the copyright files of the bundled libraries; in the AppImage also the licences of
    its runtime, whose source `runtime_source` holds."""
    copyrights = {package: pathlib.PurePosixPath(f"/usr/share/doc/{package}/copyright") for package in LIBRARY_PACKAGES}
    libraries = licences.Section("Libraries in lib", [
        "The libraries in the directory lib, from the Debian packages of the Linux build image, with the "
        "copyright file of each package."],
        [(f"{package}: {path}", pathlib.Path(path).read_bytes()) for package, path in copyrights.items()])
    extra = [libraries, runtime_section(pathlib.Path(runtime_source))] if appimage else [libraries]
    return licences.package_files(version, licences.target("linux", architecture), build, extra)


def tarball(directory, output, epoch):
    """A tarball of `directory` under the name `junto`, owned by root, with one time."""
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w", format=tarfile.PAX_FORMAT) as archive:
        for path in [directory, *sorted(directory.rglob("*"))]:
            info = archive.gettarinfo(str(path), "junto/" + str(path.relative_to(directory)) if path != directory else "junto")
            info.uid = info.gid = 0
            info.uname = info.gname = "root"
            info.mtime = epoch
            if info.isfile():
                with path.open("rb") as content:
                    archive.addfile(info, content)
            else:
                archive.addfile(info)
    with gzip.GzipFile(output, "wb", mtime=epoch) as compressed:
        compressed.write(buffer.getvalue())


def appimage(directory, output, runtime, epoch):
    """An AppImage of type 2: the runtime followed by a SquashFS image of the AppDir."""
    appdir = output.with_name(output.name + ".AppDir")
    shutil.copytree(directory, appdir, symlinks=True)
    try:
        (appdir / "AppRun").symlink_to("junto")
        shutil.copyfile(appdir / f"share/applications/{APPLICATION_ID}.desktop", appdir / f"{APPLICATION_ID}.desktop")
        shutil.copyfile(appdir / f"share/icons/hicolor/scalable/apps/{APPLICATION_ID}.svg", appdir / f"{APPLICATION_ID}.svg")
        shutil.copyfile(appdir / f"share/icons/hicolor/256x256/apps/{APPLICATION_ID}.png", appdir / ".DirIcon")
        image = output.with_name(output.name + ".squashfs")
        subprocess.run(["mksquashfs", str(appdir), str(image), "-root-owned", "-noappend", "-no-xattrs",
                        "-comp", "zstd", "-all-time", str(epoch), "-quiet", "-no-progress"], check=True)
        with output.open("wb") as file:
            file.write(runtime.read_bytes())
            file.write(image.read_bytes())
        image.unlink()
        output.chmod(0o755)
    finally:
        shutil.rmtree(appdir)


def assemble(client, recover, version, output, runtime=RUNTIME, epoch=None, *, icon_tool, build,
             runtime_source=RUNTIME_SOURCE):
    """Writes the packages into the new directory `output`; `icon_tool` is the program
    `junto-icon` that renders the icons, `build` the directory of the programs of the build,
    whose licences the packages carry, and `runtime_source` the source of `runtime`, which the
    AppImage carries."""
    client, recover, output = pathlib.Path(client), pathlib.Path(recover), pathlib.Path(output)
    runtime, runtime_source = pathlib.Path(runtime), pathlib.Path(runtime_source)
    if not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", version):
        raise ValueError("a release version has three decimal numbers")
    architecture = machine_architecture()
    epoch = int(os.environ.get("SOURCE_DATE_EPOCH", "0")) if epoch is None else epoch
    output.parent.mkdir(parents=True, exist_ok=True)
    output.mkdir(exist_ok=False)
    try:
        directory = output / "package-directory/junto"
        tree(directory, client, recover, version, architecture, licence_texts(version, architecture, build), icon_tool)
        write(directory / "junto-package.json", description(version, "directory", architecture))
        write(directory / "install-menu-entry", INSTALL_ENTRY, 0o755)
        tarball(directory, output / f"junto-{version}-linux-{architecture}.tar.gz", epoch)
        image = output / "package-appimage"
        image.mkdir()
        appimage_tree = output / ".appimage-tree"
        tree(appimage_tree, client, recover, version, architecture,
             licence_texts(version, architecture, build, appimage=True, runtime_source=runtime_source), icon_tool)
        shutil.copytree(runtime_source, appimage_tree / "licences/appimage-runtime")
        appimage(appimage_tree, image / "Junto.AppImage", runtime, epoch)
        shutil.rmtree(appimage_tree)
        copy(recover, image / "junto-recover", 0o755)
        write(image / "junto-package.json", description(version, "appimage", architecture))
        shutil.copyfile(image / "Junto.AppImage", output / f"Junto-{version}-{architecture}.AppImage")
        (output / f"Junto-{version}-{architecture}.AppImage").chmod(0o755)
    except BaseException:
        shutil.rmtree(output)
        raise
    return output


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Assemble the Linux packages of a client build.")
    parser.add_argument("client")
    parser.add_argument("recover")
    parser.add_argument("version")
    parser.add_argument("output", help="a new directory")
    parser.add_argument("--runtime", default=str(RUNTIME))
    parser.add_argument("--runtime-source", default=str(RUNTIME_SOURCE), help="the source of the runtime")
    parser.add_argument("--icon-tool", required=True, help="the program junto-icon of the same build")
    arguments = parser.parse_args()
    try:
        assemble(arguments.client, arguments.recover, arguments.version, arguments.output, arguments.runtime,
                 icon_tool=arguments.icon_tool, build=pathlib.Path(arguments.client).parent,
                 runtime_source=pathlib.Path(arguments.runtime_source))
    except (ValueError, subprocess.CalledProcessError, OSError) as error:
        sys.exit(str(error))
