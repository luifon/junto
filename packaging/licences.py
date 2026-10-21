#!/usr/bin/env python3
"""The licence file of a client package and of the server image (ADR 0801): every third-party
component in the programs, with the licence texts of each. The list of crates comes from the
dependency graph of the programs; the texts come from the packages of the crates, from the
WebRTC library of the build and from the repository."""
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


@dataclass
class Product:
    """The programs that a licence file covers: their Cargo packages, the sentence that names
    them, what holds the file, and whether the fonts of the client are compiled into them."""
    packages: list
    programs: str
    holder: str
    fonts: bool = False


CLIENT = Product(["junto-client", "junto-client-core"], "the programs junto and junto-recover", "this package",
                 fonts=True)
SERVER = Product(["junto-server"], "the program junto-server", "this image")


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
    repository: str | None = None


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


def graph(workspace, triple, packages=CLIENT.packages):
    """The name and version of each crate that the programs are built from for `triple`, with
    the crates of their build scripts and procedural macros, by the feature resolution of Cargo
    for these programs. Build dependencies are resolved for the machine that runs Cargo; each
    package is assembled on its own system."""
    selected = [argument for package in packages for argument in ["-p", package]]
    text = cargo(workspace, "tree", "--quiet", "--edges", "normal,build", "--target", triple, "--prefix", "none",
                 "--format", "{p}", *selected)
    found = set()
    for line in filter(None, text.splitlines()):
        match = re.match(r"(\S+) v(\S+)", line)
        if not match:
            raise ValueError(f"unexpected line of cargo tree: {line}")
        found.add(match.groups())
    return found


def crates(workspace, triple, selection=CLIENT.packages):
    """The crates of `graph`, with their manifest data, ordered by name and version."""
    metadata = json.loads(cargo(workspace, "metadata", "--format-version", "1"))
    members = set(metadata["workspace_members"])
    packages = {}
    for package in metadata["packages"]:
        packages.setdefault((package["name"], package["version"]), []).append(package)
    result = []
    for key in sorted(graph(workspace, triple, selection)):
        candidates = packages.get(key, [])
        if len(candidates) != 1:
            raise ValueError(f"{len(candidates)} packages of cargo metadata are {key[0]} {key[1]}")
        package = candidates[0]
        result.append(Crate(package["name"], package["version"], package["license"] or "",
                            package["source"] or "", pathlib.Path(package["manifest_path"]).parent,
                            package["license_file"], package["authors"], package["id"] in members,
                            package["repository"]))
    return result


def expression(text):
    """The tree of an SPDX licence expression: an identifier, ("WITH", licence, exception),
    ("AND", parts) or ("OR", parts). `/` between alternatives is the older form of OR."""
    tokens = re.findall(r"\(|\)|/|[^\s()/]+", text)
    position = 0

    def peek():
        return tokens[position] if position < len(tokens) else None

    def take():
        nonlocal position
        position += 1
        return tokens[position - 1]

    def alternatives():
        parts = [conjunction()]
        while peek() in ("OR", "/"):
            take()
            parts.append(conjunction())
        return parts[0] if len(parts) == 1 else ("OR", parts)

    def conjunction():
        parts = [term()]
        while peek() == "AND":
            take()
            parts.append(term())
        return parts[0] if len(parts) == 1 else ("AND", parts)

    def term():
        if peek() == "(":
            take()
            inner = alternatives()
            if peek() != ")":
                raise ValueError(f"the licence expression {text!r} has no closing parenthesis")
            take()
            return inner
        word = peek()
        if word is None or word in ("AND", "OR", "WITH", "/", ")"):
            raise ValueError(f"the licence expression {text!r} has {word or 'nothing'} where a licence is expected")
        take()
        if peek() == "WITH":
            take()
            exception = peek()
            if exception is None or exception in ("AND", "OR", "WITH", "/", "(", ")"):
                raise ValueError(f"the licence expression {text!r} names no exception after WITH")
            return ("WITH", word, take())
        return word

    tree = alternatives()
    if peek() is not None:
        raise ValueError(f"the licence expression {text!r} has {peek()} after its end")
    return tree


def identifiers(text):
    """The licence and exception identifiers of an SPDX expression, in their order."""
    found = []

    def walk(node):
        if isinstance(node, str):
            found.append(node)
        elif node[0] == "WITH":
            found.extend(node[1:])
        else:
            for part in node[1]:
                walk(part)

    walk(expression(text))
    return list(dict.fromkeys(found))


def satisfied(node, present):
    """Whether the licence texts of the identifiers in `present` give each licence that the
    expression `node` requires: one alternative of OR, each part of AND, a licence and its
    exception."""
    if isinstance(node, str):
        return node in present
    if node[0] == "WITH":
        return node[1] in present and node[2] in present
    parts = [satisfied(part, present) for part in node[1]]
    return any(parts) if node[0] == "OR" else all(parts)


def completion(node, present, available):
    """The identifiers whose standard texts make the texts of `present` satisfy the expression
    `node`, choosing for each OR the alternative that needs the fewest added texts (the first of
    them in the expression); only identifiers in `available` can be added. None when no choice
    satisfies it."""
    if isinstance(node, str):
        return [] if node in present else [node] if node in available else None
    if node[0] == "WITH":
        parts = [completion(identifier, present, available) for identifier in node[1:]]
        return None if None in parts else [identifier for part in parts for identifier in part]
    parts = [completion(part, present, available) for part in node[1]]
    if node[0] == "OR":
        possible = [part for part in parts if part is not None]
        return min(possible, key=len) if possible else None
    if None in parts:
        return None
    return list(dict.fromkeys(identifier for part in parts for identifier in part))


_MIT_GRANT = r"permission is hereby granted free of charge to any person obtaining a copy"
_MIT_CONDITION = (r"the above copyright notice and this permission notice (including the next paragraph )?shall be "
                  r"included in all copies or substantial portions of the software")
_AS_IS = r"the software is provided as is without warranty of any kind express or implied"
_ISC_GRANT = r"permission to use copy modify and or distribute this software for any purpose with or without fee is " \
             r"hereby granted"
_ISC_DISCLAIMER = (r"the software is provided as is and the authors? disclaims? all warranties with regard to this "
                   r"software")
_BSD = r"redistribution and use in source and binary forms with or without modification are permitted"
_BSD_SOURCE = r"redistributions of source code must retain the above copyright notice"
_BSD_BINARY = r"redistributions in binary form must reproduce the above copyright notice"
_BSD_DISCLAIMER = r"as is and any express or implied warranties"
_ZLIB = [r"this software is provided as is without any express or implied warranty",
         r"the origin of this software must not be misrepresented",
         r"altered source versions must be plainly marked as such",
         r"this notice may not be removed or altered from any source distribution"]
# The licences whose text this file identifies in a licence file: for each SPDX identifier,
# the phrases that its text holds and the phrases that it does not hold, on the text in lower
# case with each run of other characters than letters and digits as one space. The phrases
# come from the terms of each licence in the SPDX License List 3.27.0, not only from its
# heading, so that a file with a heading or a reference only is no licence text. The texts in
# licence-texts/ and LICENSE are identified by them (test_licences.py).
LICENCE_PHRASES = {
    "Apache-2.0": ([r"apache license version 2 0 january 2004",
                    r"2 grant of copyright license subject to the terms and conditions of this license each "
                    r"contributor hereby grants",
                    r"4 redistribution you may reproduce and distribute copies of the work",
                    r"7 disclaimer of warranty", r"8 limitation of liability"], []),
    "MIT": ([_MIT_GRANT, _MIT_CONDITION, _AS_IS], []),
    "MIT-0": ([_MIT_GRANT, r"to deal in the software without restriction", _AS_IS], [_MIT_CONDITION, r"unicode"]),
    "ISC": ([_ISC_GRANT + r" provided that the above copyright notice and this permission notice appear in all "
                          r"copies", _ISC_DISCLAIMER], []),
    "0BSD": ([_ISC_GRANT, _ISC_DISCLAIMER], [_ISC_GRANT + r" provided that"]),
    "BSD-2-Clause": ([_BSD, _BSD_SOURCE, _BSD_BINARY, _BSD_DISCLAIMER], [r"neither the name", r"bzip2"]),
    "BSD-3-Clause": ([_BSD, _BSD_SOURCE, _BSD_BINARY, r"neither the name", _BSD_DISCLAIMER], []),
    "Zlib": (_ZLIB, [r"bzip2"]),
    "bzip2-1.0.6": ([r"bzip2", _BSD, _BSD_SOURCE, r"altered source versions must be plainly marked as such",
                     r"the name of the author may not be used to endorse or promote products"], []),
    "Unicode-3.0": ([r"unicode license v3",
                     r"permission is hereby granted free of charge to any person obtaining a copy of data files",
                     r"this copyright and permission notice appear with all copies of the data files or software",
                     r"the data files and software are provided as is without warranty of any kind"], []),
    "Unlicense": ([r"this is free and unencumbered software released into the public domain",
                   r"anyone is free to copy modify publish use compile sell or distribute this software",
                   r"the software is provided as is without warranty of any kind"], []),
    "MPL-2.0": ([r"mozilla public license version 2 0", r"2 license grants and conditions",
                 r"3 responsibilities", r"6 disclaimer of warranty", r"10 versions of the license"], []),
    "CC0-1.0": ([r"cc0 1 0 universal", r"statement of purpose", r"2 waiver",
                 r"4 limitations and disclaimers"], []),
    "BSL-1.0": ([r"boost software license version 1 0 august 17th 2003",
                 r"permission is hereby granted free of charge to any person or organization obtaining a copy of the "
                 r"software and accompanying documentation covered by this license",
                 r"the software is provided as is without warranty of any kind"], []),
    "LLVM-exception": ([r"llvm exceptions to the apache 2 0 license",
                        r"as an exception if as a result of your compiling your source code portions of this software "
                        r"are embedded into an object form of such source code"], []),
    "OFL-1.1": ([r"sil open font license version 1 1",
                 r"permission is hereby granted free of charge to any person obtaining a copy of the font software",
                 r"termination", r"disclaimer"], []),
    "LGPL-2.1-only": ([r"gnu lesser general public license version 2 1 february 1999",
                       r"terms and conditions for copying distribution and modification",
                       r"0 this license agreement applies to any software library",
                       r"6 as an exception to the sections above you may also combine or link a work that uses the "
                       r"library"], []),
    "GPL-2.0-only": ([r"gnu general public license version 2 june 1991",
                      r"terms and conditions for copying distribution and modification",
                      r"0 this license applies to any program or other work"], []),
}


def licences_in(text):
    """The SPDX identifiers of the licence texts that `text` holds, by LICENCE_PHRASES."""
    words = " " + " ".join(re.findall(r"[a-z0-9]+", text.decode("utf-8", errors="replace").lower())) + " "
    return {identifier for identifier, (required, excluded) in LICENCE_PHRASES.items()
            if all(re.search(f" {phrase} ", words) for phrase in required)
            and not any(re.search(f" {phrase} ", words) for phrase in excluded)}


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


@dataclass
class CrateTexts:
    """The licence texts of a crate, each a pair of a label and the text; the identifiers whose
    standard text was added because the licence files of its package do not hold it; and the
    identifiers whose texts its licence files hold although its manifest does not declare them."""
    entries: list
    licence_files: int
    standard: list = field(default_factory=list)
    undeclared: list = field(default_factory=list)


def crate_texts(crate):
    """The licence texts of a crate: the licence files of its package, then the standard text of
    each identifier of its licence when the licence files do not hold the texts that its
    licence expression requires, and its notice files."""
    licence_files = files(crate.directory, LICENCE_FILE)
    if crate.licence_file:
        named = (crate.directory / crate.licence_file).resolve()
        if named not in [path.resolve() for path in licence_files]:
            licence_files.append(named)
    entries = [(path.relative_to(crate.directory).as_posix() if path.is_relative_to(crate.directory) else path.name,
                path.read_bytes()) for path in licence_files]
    if not crate.licence and not entries:
        raise ValueError(f"{crate.name} {crate.version} states no licence and holds no licence file")
    present = set().union(*(licences_in(text) for _, text in entries))
    result = CrateTexts(entries, len(entries))
    if crate.licence:
        declared = identifiers(crate.licence)
        result.undeclared = sorted(present - set(declared))
        tree = expression(crate.licence)
        if not satisfied(tree, present):
            available = {identifier for identifier in declared if standard_text(identifier) is not None}
            added = completion(tree, present, available)
            if added is None:
                missing = [identifier for identifier in declared if identifier not in present | available]
                raise ValueError(f"the licence files of {crate.name} {crate.version} do not hold the texts that "
                                 f"{crate.licence} requires, and there is no standard text of {', '.join(missing)} in "
                                 f"{STANDARD_TEXTS.relative_to(ROOT).as_posix()}")
            for identifier in added:
                entries.append((f"standard text of {identifier}", standard_text(identifier)))
                result.standard.append(identifier)
    entries.extend((path.relative_to(crate.directory).as_posix(), path.read_bytes())
                   for path in files(crate.directory, NOTICE_FILE) if path not in licence_files)
    return result


def listing(items):
    return items[0] if len(items) == 1 else ", ".join(items[:-1]) + " and " + items[-1]


def standard_statement(crate, texts, numbers):
    """The sentence of a crate line that names its standard texts and their copyright holders."""
    names = texts.standard
    if texts.licence_files:
        reason = "the licence files of the package do not hold the texts that its licence expression requires"
    else:
        reason = "the package holds no licence file"
    which = (f"text {numbers[0]} is the standard text of {names[0]}, a licence" if len(names) == 1 else
             f"texts {listing([str(number) for number in numbers])} are the standard texts of {listing(names)}, "
             "licences")
    if crate.authors:
        holders = f"the copyright holders are the authors that its manifest declares: {', '.join(crate.authors)}"
    elif crate.repository:
        holders = ("its manifest declares no authors, and the copyright holders are the authors of the crate in its "
                   f"repository {crate.repository}")
    else:
        holders = ("its manifest declares neither authors nor a repository, and the copyright holders are the "
                   "authors of the crate")
    return f"{reason}: {which} that its manifest declares, and {holders}"


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


def rust_version(workspace):
    """The version of the Rust compiler that builds `workspace`: the toolchain of its
    rust-toolchain.toml, or the compiler of the build image."""
    result = subprocess.run(["rustc", "--version"], cwd=workspace, capture_output=True, text=True, encoding="utf-8",
                            timeout=600)
    match = re.match(r"rustc (\d+\.\d+\.\d+) ", result.stdout)
    if result.returncode or not match:
        raise ValueError(f"rustc --version gave no version: {result.stdout.strip()} {result.stderr.strip()}")
    return match.group(1)


def rust_section(version):
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


def document(version, triple, build, extra=(), workspace=ROOT, product=CLIENT):
    """The text of the licence file of `product` of `version` for `triple`. `build` is the
    directory that holds the programs of the build; `extra` are sections for the parts that only
    one package holds. `workspace` names the Cargo workspace."""
    workspace = pathlib.Path(workspace).resolve()
    found = crates(workspace, triple, product.packages)
    numbers, texts = {}, []

    def number(label, text):
        key = normal(text)
        if key not in numbers:
            numbers[key] = len(texts) + 1
            texts.append((key, []))
        texts[numbers[key] - 1][1].append(label)
        return numbers[key]

    pictures = "; the icons and the other pictures of the programs are part of Junto" if product.fonts else ""
    lines = [f"Junto {version} for {triple}: third-party licences", "",
             f"This file names the third-party code in {product.holder} and gives its licence texts. Junto is "
             f"under the Apache License 2.0, in the file Junto-LICENSE.txt beside this file{pictures}. The licence "
             "texts are at the end of this file, each text once, with the components that it applies to.", "",
             RULE, "Rust crates", RULE, "",
             f"The crates that {product.programs} {'is' if len(product.packages) == 1 else 'are'} built from for "
             f"{triple}, with the crates that their build scripts and procedural macros use. Each line names the "
             "crate, its version, the licence that its manifest declares, where it comes from, and its licence "
             "texts: the licence files of its package, and its notice files. When the licence files do not hold "
             "the texts that the expression of the manifest requires, or when the package holds no licence file, "
             "the line says so, and the file adds the standard texts of declared licences that complete the "
             "expression, for each OR the alternative that needs the fewest texts, from the SPDX License List and "
             "the Apache Software Foundation; the copyright holders are then the authors that the manifest "
             "declares. When the licence files hold the text of a licence that "
             "the manifest does not declare, the line says so too.", ""]
    for crate in found:
        where = source(crate, workspace)
        if crate.member:
            reference = "Junto-LICENSE.txt"
        else:
            result = crate_texts(crate)
            labels = [number(f"{crate.name} {crate.version}, {name}", text) for name, text in result.entries]
            reference = "texts " + ", ".join(map(str, labels))
            if result.standard:
                standard = [labels[index] for index, (name, _) in enumerate(result.entries)
                            if name.startswith("standard text of ")]
                reference += "; " + standard_statement(crate, result, standard)
            if result.undeclared:
                reference += (f"; its licence files also hold the text of {listing(result.undeclared)}, which its "
                              "manifest does not declare")
        lines.append(f"{crate.name} {crate.version} | {crate.licence} | {where} | {reference}")
    sections = [rust_section(rust_version(workspace))]
    for crate in found:
        if crate.name == "webrtc-sys-build":
            sections.append(webrtc_section(build, triple, webrtc_tag(crate.directory)))
    if product.fonts:
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


def package_files(version, triple, build, extra=(), product=CLIENT):
    """The files of the directory of licences of a package or an image: the licence of Junto and
    the licence file."""
    return {"Junto-LICENSE.txt": (ROOT / "LICENSE").read_bytes(),
            NAME: document(version, triple, build, extra, product=product).encode()}


def texts_of(text):
    """The licence texts at the end of a licence file, in their order."""
    return [part.split("\n" + "-" * 78 + "\n", 1)[1].rstrip("\n") for part in text.split(f"{RULE}\nText ")[1:]]


def uncovered(text):
    """The crates of a licence file whose texts do not give the licences that the expression of
    their manifest requires."""
    found = texts_of(text)
    section = text.split(f"{RULE}\nRust crates\n{RULE}\n", 1)[1].split(f"\n{RULE}\n", 1)[0]
    result = []
    for line in section.splitlines():
        parts = line.split(" | ")
        if len(parts) != 4 or parts[3] == "Junto-LICENSE.txt":
            continue
        numbers = parts[3].split(";")[0].removeprefix("texts ").split(", ")
        present = set().union(*(licences_in(found[int(number) - 1].encode()) for number in numbers))
        if not satisfied(expression(parts[1]), present):
            result.append(parts[0])
    return result


def listed(text):
    """The name and version of each crate in the section "Rust crates" of a licence file."""
    section = text.split(f"{RULE}\nRust crates\n{RULE}\n", 1)[1].split(f"\n{RULE}\n", 1)[0]
    return {tuple(match.groups()) for match in re.finditer(r"^(\S+) (\S+) \| ", section, re.MULTILINE)}


# The parts of the server image whose licence texts are in other files of the image.
SERVER_SECTIONS = [Section("Other parts of the image", [
    "The programs ffmpeg and ffprobe are an LGPL build of FFmpeg; their licence texts are in "
    "/usr/share/licenses/ffmpeg. The recording layout page in /usr/share/junto/recording-layout carries the "
    "licence texts of its JavaScript code and of its font in its files. The other files of the image come from "
    "its Debian base image, with their copyright files in /usr/share/doc."])]


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Write the licence files of a client package or of the server image.")
    parser.add_argument("product", choices=["client", "server"])
    parser.add_argument("system", choices=sorted(SYSTEMS))
    parser.add_argument("architecture")
    parser.add_argument("build", help="the directory of the programs of the build, such as target/release")
    parser.add_argument("output", help="a new directory, for Junto-LICENSE.txt and " + NAME)
    arguments = parser.parse_args()
    version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
    product, extra = (CLIENT, []) if arguments.product == "client" else (SERVER, SERVER_SECTIONS)
    try:
        written = package_files(version, target(arguments.system, arguments.architecture), arguments.build, extra,
                                product)
        output = pathlib.Path(arguments.output)
        output.mkdir(parents=True, exist_ok=False)
        for name, content in written.items():
            (output / name).write_bytes(content)
    except (ValueError, OSError) as error:
        sys.exit(str(error))
