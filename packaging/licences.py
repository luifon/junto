#!/usr/bin/env python3
"""The licence file of a client package (ADR 0801): every third-party component in the programs
of the package, with the licence texts of each. The list of crates comes from the dependency
graph of the programs; the texts come from the packages of the crates, from the WebRTC library
of the build and from the repository."""
import argparse
import json
import os
import pathlib
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass, field

ROOT = pathlib.Path(__file__).resolve().parent.parent
NAME = "third-party-licences.txt"
# The packages of the two programs of a client package: junto and junto-recover.
PROGRAMS = ["junto-client", "junto-client-core"]
# Standard texts for a crate whose package holds no licence file (licence-texts/README.md).
STANDARD_TEXTS = pathlib.Path(__file__).resolve().parent / "licence-texts"
SYSTEMS = {"macos": "apple-darwin", "windows": "pc-windows-msvc", "linux": "unknown-linux-gnu"}
LICENCE_FILE = re.compile(r"(un)?licen[cs]es?([-._].*)?|copying([-._].*)?", re.IGNORECASE)
NOTICE_FILE = re.compile(r"(copyright|notices?)([-._].*)?", re.IGNORECASE)
RULE = "=" * 78
# The directory of the programs of the debug build of this checkout, which the packaging tests
# and checks use.
DEBUG_BUILD = pathlib.Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target")) / "debug"
# The names that webrtc-sys-build gives the system and the architecture of a prebuilt library.
WEBRTC_SYSTEMS = {"apple-darwin": "mac", "pc-windows-msvc": "win", "unknown-linux-gnu": "linux"}
WEBRTC_ARCHITECTURES = {"aarch64": "arm64", "x86_64": "x64"}


def target(system, architecture):
    """The Rust target of a package."""
    if system not in SYSTEMS:
        raise ValueError(f"no Rust target for the system {system}")
    return f"{architecture}-{SYSTEMS[system]}"


@dataclass
class Crate:
    name: str
    version: str
    licence: str
    source: str
    directory: pathlib.Path
    licence_file: str | None = None
    authors: list = field(default_factory=list)
    member: bool = False


@dataclass
class Section:
    """A part of the file: its title, the sentences that describe it, and its components, each
    a pair of a label and the licence text that applies to it."""
    title: str
    description: list
    components: list = field(default_factory=list)


def cargo(workspace, *arguments):
    result = subprocess.run(["cargo", *arguments, "--locked", "--manifest-path", str(workspace / "Cargo.toml")],
                            capture_output=True, text=True, encoding="utf-8", timeout=600)
    if result.returncode:
        raise ValueError(f"cargo {' '.join(arguments)} ended with code {result.returncode}: {result.stderr.strip()}")
    return result.stdout


def graph(workspace, triple, programs=PROGRAMS):
    """The name and version of each crate that the programs are built from for `triple`, with
    the crates of their build scripts and procedural macros, by the feature resolution of Cargo
    for these programs. Build dependencies are resolved for the machine that runs Cargo; each
    package is assembled on its own system."""
    selected = [argument for program in programs for argument in ["-p", program]]
    text = cargo(workspace, "tree", "--quiet", "--edges", "normal,build", "--target", triple, "--prefix", "none",
                 "--format", "{p}", *selected)
    found = set()
    for line in filter(None, text.splitlines()):
        match = re.match(r"(\S+) v(\S+)", line)
        if not match:
            raise ValueError(f"unexpected line of cargo tree: {line}")
        found.add(match.groups())
    return found


def crates(workspace, triple, programs=PROGRAMS):
    """The crates of `graph`, with their manifest data, ordered by name and version."""
    metadata = json.loads(cargo(workspace, "metadata", "--format-version", "1"))
    members = set(metadata["workspace_members"])
    packages = {}
    for package in metadata["packages"]:
        packages.setdefault((package["name"], package["version"]), []).append(package)
    result = []
    for key in sorted(graph(workspace, triple, programs)):
        candidates = packages.get(key, [])
        if len(candidates) != 1:
            raise ValueError(f"{len(candidates)} packages of cargo metadata are {key[0]} {key[1]}")
        package = candidates[0]
        result.append(Crate(package["name"], package["version"], package["license"] or "",
                            package["source"] or "", pathlib.Path(package["manifest_path"]).parent,
                            package["license_file"], package["authors"], package["id"] in members))
    return result


def identifiers(expression):
    """The licence and exception identifiers of an SPDX expression, also of the older form with
    `/` between alternatives."""
    words = [word for word in re.split(r"[\s()/]+", expression) if word]
    return list(dict.fromkeys(word for word in words if word not in ("OR", "AND", "WITH")))


def standard_text(identifier):
    path = ROOT / "LICENSE" if identifier == "Apache-2.0" else STANDARD_TEXTS / f"{identifier}.txt"
    return path.read_bytes() if path.is_file() else None


def files(directory, pattern):
    found = []
    for path in sorted(directory.iterdir()):
        if not pattern.fullmatch(path.name):
            continue
        if path.is_dir():
            found.extend(sorted(item for item in path.iterdir() if item.is_file()))
        elif path.is_file():
            found.append(path)
    return found


def source(crate, workspace):
    if crate.member:
        return "Junto"
    if crate.source == "registry+https://github.com/rust-lang/crates.io-index":
        return "crates.io"
    if crate.source.startswith("git+"):
        return crate.source[len("git+"):]
    if not crate.source and crate.directory.is_relative_to(workspace / "vendor"):
        return (f"changed copy in vendor/{crate.directory.relative_to(workspace / 'vendor').as_posix()} of the "
                "Junto source; each changed file states its change")
    raise ValueError(f"{crate.name} {crate.version} comes from {crate.source or crate.directory}, which is "
                     "neither crates.io, a git repository, the workspace nor vendor/")


def crate_texts(crate):
    """The licence texts of a crate: the licence files of its package, or the standard text of
    each identifier of its licence when it has none; and its notice files."""
    licence_files = files(crate.directory, LICENCE_FILE)
    if crate.licence_file:
        named = (crate.directory / crate.licence_file).resolve()
        if named not in [path.resolve() for path in licence_files]:
            licence_files.append(named)
    texts = [(path.relative_to(crate.directory).as_posix() if path.is_relative_to(crate.directory) else path.name,
              path.read_bytes()) for path in licence_files]
    if not texts:
        if not crate.licence:
            raise ValueError(f"{crate.name} {crate.version} states no licence and holds no licence file")
        for identifier in identifiers(crate.licence):
            text = standard_text(identifier)
            if text is None:
                raise ValueError(f"{crate.name} {crate.version} holds no licence file, and there is no standard "
                                 f"text of {identifier} in {STANDARD_TEXTS.relative_to(ROOT).as_posix()}")
            texts.append((f"standard text of {identifier}", text))
    texts.extend((path.relative_to(crate.directory).as_posix(), path.read_bytes())
                 for path in files(crate.directory, NOTICE_FILE) if path not in licence_files)
    return texts


def webrtc_tag(directory):
    """The release of the prebuilt WebRTC library that the crate webrtc-sys-build in `directory`
    downloads."""
    source = (directory / "src/lib.rs").read_text(encoding="utf-8")
    match = re.search(r'pub const WEBRTC_TAG: &str = "([^"]+)";', source)
    if not match:
        raise ValueError(f"no WEBRTC_TAG in {directory / 'src/lib.rs'}")
    return match.group(1)


def webrtc_platform(triple):
    """The name that webrtc-sys-build gives the system and the architecture of `triple`."""
    architecture, _, system = triple.partition("-")
    if system not in WEBRTC_SYSTEMS or architecture not in WEBRTC_ARCHITECTURES:
        raise ValueError(f"no prebuilt WebRTC library is known for {triple}")
    return f"{WEBRTC_SYSTEMS[system]}-{WEBRTC_ARCHITECTURES[architecture]}"


def webrtc_section(build, triple, tag):
    """The licence file of the WebRTC library that the build in `build` (the directory of the
    programs, such as target/release) linked into the client."""
    custom = os.environ.get("LK_CUSTOM_WEBRTC")
    if custom:
        path = pathlib.Path(custom) / "LICENSE.md"
        if not path.is_file():
            raise ValueError(f"the WebRTC library of LK_CUSTOM_WEBRTC has no licence file {path}")
        return Section("The WebRTC library", [
            "A WebRTC library built from source, linked into junto. Its licence file names WebRTC and each "
            "project that it contains, with its licence."], [("libwebrtc, LICENSE.md", path.read_bytes())])
    found = sorted(pathlib.Path(build).glob(
        f"build/scratch-*/out/livekit_webrtc/livekit/{webrtc_platform(triple)}-*-{tag}/*/LICENSE.md"))
    if not found:
        raise ValueError(f"the licence file of the WebRTC library {tag} for {triple} is not under {build}/build: "
                         "build the client there first")
    texts = {path.read_bytes() for path in found}
    if len(texts) != 1:
        raise ValueError("the WebRTC licence files under the build differ: " + ", ".join(map(str, found)))
    return Section("The WebRTC library", [
        f"The prebuilt WebRTC library of the LiveKit Rust SDK release {tag} "
        f"(https://github.com/livekit/rust-sdks/releases/tag/{tag}), linked into junto. Its licence file names "
        "WebRTC and each project that it contains, with its licence."], [(f"libwebrtc {tag}, LICENSE.md", texts.pop())])


def rust_section():
    version = tomllib.loads((ROOT / "rust-toolchain.toml").read_text(encoding="utf-8"))["toolchain"]["channel"]
    return Section("The Rust standard library", [
        f"The standard library of Rust {version} (core, alloc, std and the crates that the Rust project builds "
        "them from) is compiled into each program. The Rust project publishes it under the MIT licence or the "
        "Apache License 2.0; the copyright belongs to its contributors, as the file COPYRIGHT of the Rust source "
        f"states: https://github.com/rust-lang/rust/blob/{version}/COPYRIGHT"],
        [(f"Rust {version}, standard text of {identifier}", standard_text(identifier))
         for identifier in ["MIT", "Apache-2.0"]])


def font_section():
    fonts = ROOT / "crates/client/assets/fonts"
    section = Section("Fonts", ["The fonts that are compiled into junto, under the SIL Open Font License 1.1."])
    for licence in sorted(fonts.glob("*-OFL.txt")):
        family = licence.name.removesuffix("-OFL.txt")
        members = sorted(path.name for path in fonts.glob(f"{family}-*.ttf"))
        if not members:
            raise ValueError(f"{licence.name} names no font file")
        section.components.append((f"{', '.join(members)}: {licence.name}", licence.read_bytes()))
    if not section.components:
        raise ValueError(f"no font licence in {fonts}")
    return section


def normal(text):
    lines = text.decode("utf-8", errors="replace").replace("\r\n", "\n").replace("\r", "\n").split("\n")
    return "\n".join(line.rstrip() for line in lines).strip("\n")


def document(version, triple, build, extra=(), workspace=ROOT, programs=PROGRAMS):
    """The text of the licence file of a package of `version` for `triple`. `build` is the
    directory that holds the programs of the build; `extra` are sections for the parts that only
    one package holds. `workspace` and `programs` name the Cargo workspace and its packages."""
    workspace = pathlib.Path(workspace).resolve()
    found = crates(workspace, triple, programs)
    numbers, texts = {}, []

    def number(label, text):
        key = normal(text)
        if key not in numbers:
            numbers[key] = len(texts) + 1
            texts.append((key, []))
        texts[numbers[key] - 1][1].append(label)
        return numbers[key]

    lines = [f"Junto {version} for {triple}: third-party licences", "",
             "This file names the third-party code in this package and gives its licence texts. Junto is under "
             "the Apache License 2.0, in the file Junto-LICENSE.txt beside this file; the icons and the other "
             "pictures of the programs are part of Junto. The licence texts are at the end of this file, each "
             "text once, with the components that it applies to.", "",
             RULE, "Rust crates", RULE, "",
             f"The crates that the programs junto and junto-recover are built from for {triple}, with the crates "
             "that their build scripts and procedural macros use. Each line names the crate, its version, the "
             "licence that its manifest states, where it comes from, and its licence texts. A crate whose package "
             "holds no licence file has the standard text of each licence that its manifest names; its authors "
             "are the copyright holders.", ""]
    for crate in found:
        where = source(crate, workspace)
        if crate.member:
            reference = "Junto-LICENSE.txt"
        else:
            entries = crate_texts(crate)
            reference = "texts " + ", ".join(
                str(number(f"{crate.name} {crate.version}, {name}", text)) for name, text in entries)
            if any(name.startswith("standard text of ") for name, _ in entries) and crate.authors:
                reference += "; authors: " + ", ".join(crate.authors)
        lines.append(f"{crate.name} {crate.version} | {crate.licence} | {where} | {reference}")
    sections = [rust_section()]
    for crate in found:
        if crate.name == "webrtc-sys-build":
            sections.append(webrtc_section(build, triple, webrtc_tag(crate.directory)))
    sections.append(font_section())
    sections.extend(extra)
    for section in sections:
        lines += ["", RULE, section.title, RULE, "", *section.description]
        if section.components:
            lines.append("")
        for label, text in section.components:
            lines.append(f"{label} | text {number(label, text)}")
    lines += ["", RULE, "Licence texts", RULE]
    for index, (text, labels) in enumerate(texts, start=1):
        lines += ["", RULE, f"Text {index}, for:", *(f"  {label}" for label in labels), "-" * 78, text]
    return "\n".join(lines) + "\n"


def package_files(version, triple, build, extra=()):
    """The files of the directory `licences` of a package: the licence of Junto and the licence
    file of the package."""
    return {"Junto-LICENSE.txt": (ROOT / "LICENSE").read_bytes(),
            NAME: document(version, triple, build, extra).encode()}


def listed(text):
    """The name and version of each crate in the section "Rust crates" of a licence file."""
    section = text.split(f"{RULE}\nRust crates\n{RULE}\n", 1)[1].split(f"\n{RULE}\n", 1)[0]
    return {tuple(match.groups()) for match in re.finditer(r"^(\S+) (\S+) \| ", section, re.MULTILINE)}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Write the licence file of a client package.")
    parser.add_argument("system", choices=sorted(SYSTEMS))
    parser.add_argument("architecture")
    parser.add_argument("build", help="the directory of the programs of the build, such as target/release")
    parser.add_argument("output", help="a new file")
    arguments = parser.parse_args()
    version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
    try:
        text = document(version, target(arguments.system, arguments.architecture), arguments.build)
        with open(arguments.output, "x", encoding="utf-8", newline="\n") as output:
            output.write(text)
    except (ValueError, OSError) as error:
        sys.exit(str(error))
