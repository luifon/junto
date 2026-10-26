"""Writes the static files of the release site (ADR 0727)."""
import argparse
import base64
import dataclasses
import hashlib
import html
import os
import pathlib
import re
import shutil
import sys
import tempfile
import urllib.parse

VERSION = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)")
TARGET = r"(?:macos-bundle|linux-directory|linux-appimage|windows-directory)-(?:x86_64|aarch64)"
ARCHIVE = re.compile(rf"junto-(?P<version>[0-9.]+)-(?P<target>{TARGET})\.junto")
RELEASE = re.compile(rf"release-(?P<version>[0-9.]+)-(?P<target>{TARGET})\.json")
LATEST = re.compile(rf"latest-(?P<target>{TARGET})\.json")

SYSTEMS = (
    ("macos", "macOS", "Supported: macOS 26 on Apple Silicon. Older versions of macOS are not tested."),
    ("windows", "Windows", "Supported: Windows 11 on x86-64."),
    ("linux", "Linux", "Supported: Linux on x86-64 with PipeWire, under X11 or Wayland, with glibc 2.35 or newer."),
)

# System, kind, file name, label.
FIRST_INSTALL = (
    ("macos", "dmg", "Junto-{v}-macos-aarch64.dmg", "Disk image"),
    ("macos", "zip", "Junto-{v}-macos-aarch64.zip", "ZIP file"),
    ("windows", "setup", "Junto-{v}-windows-x86_64-setup.exe", "Setup program"),
    ("windows", "zip", "Junto-{v}-windows-x86_64.zip", "ZIP file"),
    ("linux", "appimage", "Junto-{v}-x86_64.AppImage", "AppImage"),
    ("linux", "tarball", "junto-{v}-linux-x86_64.tar.gz", "Tarball"),
    ("linux", "deb", "junto_{v}_amd64.deb", "Debian package"),
)
SYMBOLS = "Junto-{v}-windows-x86_64.pdb"
CHECKSUMS = "SHA256SUMS"
CHUNK = 1024 * 1024

STYLE = """
:root { color-scheme: light dark; --text: #1d1b18; --muted: #5d574f; --line: #d8d0c2; --back: #faf7f2;
  --panel: #ffffff; --accent: #9a4a2c; }
@media (prefers-color-scheme: dark) { :root { --text: #ece6dc; --muted: #b3aa9c; --line: #3e3933; --back: #1b1917;
  --panel: #24211e; --accent: #e09a77; } }
* { box-sizing: border-box; }
body { margin: 0; background: var(--back); color: var(--text); line-height: 1.5;
  font-family: system-ui, -apple-system, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif; }
main { max-width: 56rem; margin: 0 auto; padding: 1.5rem 1rem 3rem; }
a { color: var(--accent); }
code, pre { font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, "Liberation Mono", monospace; font-size: 0.9em; }
pre { background: var(--panel); border: 1px solid var(--line); padding: 0.75rem; overflow-x: auto; }
section.system, section.part { background: var(--panel); border: 1px solid var(--line); border-radius: 0.5rem;
  padding: 0.25rem 1rem 1rem; margin: 1rem 0; }
section.system.visitor { border: 2px solid var(--accent); }
.visitor-mark { color: var(--accent); font-weight: 600; margin: 0.5rem 0 0; }
.file { border-top: 1px solid var(--line); padding-top: 0.5rem; margin-top: 0.75rem; }
.file h3 { margin: 0.25rem 0; font-size: 1.05rem; }
.facts { color: var(--muted); margin: 0.25rem 0; }
.hash { word-break: break-all; }
"""

SCRIPT = """
(function () {
  var data = navigator.userAgentData;
  var text = data && data.platform ? data.platform : (navigator.platform || "") + " " + (navigator.userAgent || "");
  text = text.toLowerCase();
  var system = "";
  if (/android|iphone|ipad|ipod|cros|chrome os/.test(text)) {
    system = "";
  } else if (text.indexOf("mac") >= 0) {
    system = "macos";
  } else if (text.indexOf("win") >= 0) {
    system = "windows";
  } else if (text.indexOf("linux") >= 0) {
    system = "linux";
  }
  var list = document.getElementById("systems");
  var section = system ? document.getElementById("system-" + system) : null;
  if (!list || !section) {
    return;
  }
  list.insertBefore(section, list.firstElementChild);
  section.classList.add("visitor");
  var mark = section.querySelector(".visitor-mark");
  if (mark) {
    mark.hidden = false;
  }
})();
"""


class SiteError(Exception):
    pass


@dataclasses.dataclass
class Release:
    name: str
    directory: pathlib.Path
    first_install: dict = dataclasses.field(default_factory=dict)
    symbols: pathlib.Path | None = None
    archives: dict = dataclasses.field(default_factory=dict)
    releases: dict = dataclasses.field(default_factory=dict)
    latest: dict = dataclasses.field(default_factory=dict)

    @property
    def key(self):
        return version_key(self.name)

    def downloads(self):
        files = dict(self.first_install)
        if self.symbols is not None:
            files[self.symbols.name] = self.symbols
        return files


def version_key(name):
    match = VERSION.fullmatch(name)
    if match is None:
        raise SiteError(f"not a version: {name}")
    return tuple(int(part) for part in match.groups())


def first_install_names(version):
    return [template.format(v=version) for _, _, template, _ in FIRST_INSTALL]


def escape(text):
    return html.escape(text, quote=True)


def check_address(address, what, directory):
    if any(ord(character) <= 0x20 or ord(character) == 0x7F for character in address):
        raise SiteError(f"{what} holds a space or a control character: {address!r}")
    parts = urllib.parse.urlsplit(address)
    if parts.scheme != "https" or not parts.hostname:
        raise SiteError(f"{what} is not an HTTPS address: {address}")
    if parts.username is not None or parts.password is not None or parts.query or parts.fragment:
        raise SiteError(f"{what} holds credentials, a query or a fragment: {address}")
    if directory and not parts.path.endswith("/"):
        raise SiteError(f"{what} does not end in /: {address}")
    return address


def classify(release, path):
    """Puts one file of a version directory into its place in the release, or returns False."""
    name = path.name
    version = release.name
    if name in first_install_names(version):
        release.first_install[name] = path
        return True
    if name == SYMBOLS.format(v=version):
        release.symbols = path
        return True
    for pattern, place in ((ARCHIVE, release.archives), (RELEASE, release.releases)):
        match = pattern.fullmatch(name)
        if match is not None and match["version"] == version:
            place[match["target"]] = path
            return True
    match = LATEST.fullmatch(name)
    if match is not None:
        release.latest[match["target"]] = path
        return True
    return False


def read_releases(directory):
    """Reads the version directories of the releases directory, oldest first."""
    directory = pathlib.Path(directory)
    if not directory.is_dir():
        raise SiteError(f"the releases directory does not exist: {directory}")
    problems = []
    found = []
    for entry in sorted(directory.iterdir()):
        if not entry.is_dir() or entry.is_symlink() or VERSION.fullmatch(entry.name) is None:
            continue
        release = Release(entry.name, entry)
        for path in sorted(entry.iterdir()):
            if path.is_symlink() or not path.is_file():
                problems.append(f"not a regular file: {path}")
            elif not classify(release, path):
                problems.append(f"unknown file: {path}")
        problems.extend(incomplete_targets(release))
        found.append(release)
    if not found:
        raise SiteError(f"the releases directory holds no version directory: {directory}")
    found.sort(key=lambda release: release.key)
    problems.extend(missing_newest(found[-1]))
    if problems:
        raise SiteError("\n".join(problems))
    return found


def incomplete_targets(release):
    """Names the update files that a latest file, a release file or an archive needs beside it."""
    problems = []
    targets = set(release.archives) | set(release.releases) | set(release.latest)
    for target in sorted(targets):
        needed = [f"release-{release.name}-{target}.json", f"junto-{release.name}-{target}.junto"]
        for name in needed:
            if not (release.directory / name).is_file():
                problems.append(f"missing file: {release.directory / name}")
    return problems


def missing_newest(release):
    names = [*first_install_names(release.name), SYMBOLS.format(v=release.name)]
    problems = [f"missing file of the newest version: {release.directory / name}" for name in names
                if name not in release.downloads()]
    if not release.latest:
        problems.append(f"missing file of the newest version: {release.directory / 'latest-<target>.json'}")
    return problems


def read_notes(directory, releases):
    directory = pathlib.Path(directory)
    notes = {}
    missing = []
    for release in releases:
        path = directory / f"{release.name}.md"
        if not path.is_file():
            missing.append(f"missing notes: {path}")
            continue
        try:
            notes[release.name] = path.read_text(encoding="utf-8")
        except UnicodeDecodeError as error:
            raise SiteError(f"the notes are not UTF-8: {path}: {error}") from error
    if missing:
        raise SiteError("\n".join(missing))
    return notes


def newest_latest(releases):
    """For each target, the latest file of the highest version directory that holds one."""
    chosen = {}
    for release in sorted(releases, key=lambda release: release.key):
        chosen.update(release.latest)
    return dict(sorted(chosen.items()))


def copy(source, destination):
    """Copies the bytes of a file and returns its size and SHA-256."""
    digest = hashlib.sha256()
    size = 0
    with open(source, "rb") as reader, open(destination, "xb") as writer:
        while chunk := reader.read(CHUNK):
            digest.update(chunk)
            writer.write(chunk)
            size += len(chunk)
    return size, digest.hexdigest()


def checksums(digests):
    return "".join(f"{digests[name][1]}  {name}\n" for name in sorted(digests))


def size_text(size):
    if size < 1_000_000:
        return f"{size:,} bytes"
    return f"{size / 1_000_000:.1f} MB ({size:,} bytes)"


# Markdown of the notes.

INLINE = re.compile(r"`([^`]+)`|\*\*(.+?)\*\*|\[([^\]]+)\]\(([^()\s]+)\)")
SCHEME = re.compile(r"[A-Za-z][A-Za-z0-9+.-]*:")
HEADING = re.compile(r"(#{1,3}) +(.*?) *#* *")


def link_allowed(target):
    if any(ord(character) <= 0x20 or ord(character) == 0x7F or character == "\\" for character in target):
        return False
    if SCHEME.match(target):
        parts = urllib.parse.urlsplit(target)
        return parts.scheme == "https" and bool(parts.hostname) and target.lower().startswith("https://")
    return not target.startswith("//")


def inline(text, links=True):
    parts = []
    position = 0
    for match in INLINE.finditer(text):
        parts.append(escape(text[position:match.start()]))
        code, strong, label, target = match.groups()
        if code is not None:
            parts.append(f"<code>{escape(code)}</code>")
        elif strong is not None:
            parts.append(f"<strong>{inline(strong, links)}</strong>")
        elif links and link_allowed(target):
            parts.append(f'<a href="{escape(target)}">{inline(label, False)}</a>')
        else:
            parts.append(escape(match.group(0)))
        position = match.end()
    parts.append(escape(text[position:]))
    return "".join(parts)


def markdown(text):
    """Renders the Markdown of a notes file: headings 1 to 3, paragraphs, one level of lists, fenced code."""
    output = []
    paragraph = []
    items = []
    code = None

    def flush():
        if paragraph:
            output.append(f"<p>{inline(' '.join(paragraph))}</p>")
            paragraph.clear()
        if items:
            output.append("<ul>\n" + "".join(f"<li>{inline(' '.join(item))}</li>\n" for item in items) + "</ul>")
            items.clear()

    for line in text.replace("\r\n", "\n").split("\n"):
        if code is not None:
            if line.strip() == "```":
                output.append("<pre><code>" + escape("\n".join(code)) + "</code></pre>")
                code = None
            else:
                code.append(line)
            continue
        stripped = line.strip()
        heading = HEADING.fullmatch(line)
        if line.startswith("```"):
            flush()
            code = []
        elif heading is not None:
            flush()
            level = len(heading[1])
            output.append(f"<h{level}>{inline(heading[2])}</h{level}>")
        elif line.startswith("- "):
            if paragraph:
                flush()
            items.append([line[2:].strip()])
        elif not stripped:
            flush()
        elif items and line.startswith(" "):
            items[-1].append(stripped)
        else:
            if items:
                flush()
            paragraph.append(stripped)
    if code is not None:
        output.append("<pre><code>" + escape("\n".join(code)) + "</code></pre>")
    flush()
    return "\n".join(output) + "\n"


# Pages.

def source_hash(text):
    return "'sha256-" + base64.b64encode(hashlib.sha256(text.encode("utf-8")).digest()).decode("ascii") + "'"


def policy(script):
    scripts = source_hash(script) if script is not None else "'none'"
    return (f"default-src 'self'; script-src {scripts}; style-src {source_hash(STYLE)}; "
            "object-src 'none'; base-uri 'self'; form-action 'self'")


def page(title, body, script=None):
    script_element = f"<script>{script}</script>\n" if script is not None else ""
    return ("<!doctype html>\n"
            '<html lang="en">\n'
            "<head>\n"
            '<meta charset="utf-8">\n'
            f'<meta http-equiv="Content-Security-Policy" content="{policy(script)}">\n'
            '<meta name="viewport" content="width=device-width, initial-scale=1">\n'
            '<meta name="referrer" content="no-referrer">\n'
            f"<title>{escape(title)}</title>\n"
            f"<style>{STYLE}</style>\n"
            "</head>\n"
            "<body>\n"
            "<main>\n"
            f"{body}"
            "</main>\n"
            f"{script_element}"
            "</body>\n"
            "</html>\n")


def steps(kind, name, version):
    file = f"<code>{escape(name)}</code>"
    deb = escape(f"junto_{version}_amd64.deb")
    if kind == ("macos", "dmg"):
        return ("<p>Open the disk image and drag <code>Junto</code> onto the <code>Applications</code> link. "
                "The folder that holds <code>Junto.app</code> must stay writable for updates.</p>")
    if kind == ("macos", "zip"):
        return (f"<p>Open {file} in Finder and move <code>Junto.app</code> into a writable folder of your account, "
                "such as your own Applications folder.</p>")
    if kind == ("windows", "setup"):
        return (f"<p>Run {file}. It installs Junto for your account without administrator rights, adds an entry "
                "to the Start menu and to the list of installed programs, and starts Junto.</p>")
    if kind == ("windows", "zip"):
        return (f"<p>Extract the folder <code>Junto</code> from {file} into a folder of your account, for example "
                "<code>%LOCALAPPDATA%\\Programs</code>, and start <code>Junto\\junto.exe</code>. "
                "The folder that holds <code>Junto</code> must stay writable for updates.</p>")
    if kind == ("linux", "appimage"):
        return ("<p>Put the file in a writable folder of your account, make it executable and start it:</p>\n"
                f"<pre><code>chmod +x {escape(name)}\n./{escape(name)}</code></pre>\n"
                "<p>The AppImage needs <code>fusermount</code> of FUSE 2 or 3. Without FUSE, start it with "
                f"<code>./{escape(name)} --appimage-extract-and-run</code>.</p>")
    if kind == ("linux", "tarball"):
        return ("<p>Extract the directory <code>junto</code> into a folder of your account and start it:</p>\n"
                f"<pre><code>mkdir -p ~/.local/opt\ntar -xzf {escape(name)} -C ~/.local/opt\n"
                "~/.local/opt/junto/junto</code></pre>\n"
                "<p><code>~/.local/opt/junto/install-menu-entry</code> adds Junto to the application menu.</p>")
    if kind == ("linux", "deb"):
        return ("<p>Install it with apt:</p>\n"
                f"<pre><code>sudo apt install ./{deb}</code></pre>\n"
                "<p>Junto cannot update a system package by itself. To update, install the <code>.deb</code> "
                "file of the newer version with the same command.</p>")
    raise SiteError(f"no install steps for {kind}")


SYSTEM_TEXT = {
    "macos": ("<p>First start: macOS blocks the first open of a downloaded copy, and the dialog has no button that "
              "opens Junto. Open System Settings, Privacy &amp; Security, choose &quot;Open Anyway&quot; for Junto, "
              "then confirm. Junto has no paid code signature.</p>\n"
              "<p>Junto updates itself from inside the application.</p>\n"),
    "windows": ("<p>First start: Microsoft Defender SmartScreen shows &quot;Windows protected your PC&quot;. Choose "
                "&quot;More info&quot;, then &quot;Run anyway&quot;. Smart App Control must be off; with it on, "
                "Windows does not run Junto. Junto has no paid code signature.</p>\n"
                "<p>Junto updates itself from inside the application.</p>\n"),
    "linux": "<p>The AppImage and the tarball update themselves from inside the application.</p>\n",
}


def file_block(kind, name, size, digest, href, version):
    return (f'<div class="file" id="file-{escape(name)}">\n'
            f'<h3><a href="{escape(href)}">{escape(name)}</a></h3>\n'
            f'<p class="facts">{escape(size_text(size))}<br>SHA-256: <code class="hash">{digest}</code></p>\n'
            f"{steps(kind, name, version)}\n"
            "</div>\n")


def index_page(newest, digests, releases, base, server_install):
    version = newest.name
    labels = {template.format(v=version): (system, kind) for system, kind, template, _ in FIRST_INSTALL}
    sections = []
    for system, title, supported in SYSTEMS:
        files = "".join(file_block(labels[name], name, *digests[name], f"download/{version}/{name}", version)
                        for name in first_install_names(version) if labels[name][0] == system)
        sections.append(f'<section class="system" id="system-{system}">\n'
                        '<p class="visitor-mark" hidden>Your system</p>\n'
                        f"<h2>{title}</h2>\n"
                        f"<p>{escape(supported)}</p>\n"
                        f"{files}"
                        f"{SYSTEM_TEXT[system]}"
                        "</section>\n")
    symbols = SYMBOLS.format(v=version)
    size, digest = digests[symbols]
    sums = f"download/{version}/{CHECKSUMS}"
    dmg = escape(f"Junto-{version}-macos-aarch64.dmg")
    setup = escape(f"Junto-{version}-windows-x86_64-setup.exe")
    versions = "".join(
        f'<li>{escape(release.name)}{" (newest)" if release is newest else ""}: '
        f'<a href="notes/{escape(release.name)}.html">release notes</a>, '
        f'<a href="notes/{escape(release.name)}.html#files">files</a></li>\n'
        for release in sorted(releases, key=lambda release: release.key, reverse=True))
    body = (f"<h1>Junto {escape(version)}</h1>\n"
            "<p>Junto is a self-hosted application for team text chat, voice and video calls, screen sharing, "
            "and call recording with transcripts. This page offers the desktop client.</p>\n"
            f'<p><a href="notes/{escape(version)}.html">Release notes of {escape(version)}</a>. '
            f'The server is installed with Docker Compose: <a href="{escape(server_install)}">server installation</a>.'
            "</p>\n"
            f'<div id="systems">\n{"".join(sections)}</div>\n'
            '<section class="part" id="checksums">\n'
            "<h2>Check a download</h2>\n"
            f'<p><a href="{escape(sums)}">{CHECKSUMS}</a> lists the SHA-256 of every file of this version. '
            "Compare the result of the command with the SHA-256 on this page.</p>\n"
            f"<p>macOS, in Terminal:</p>\n<pre><code>shasum -a 256 {dmg}</code></pre>\n"
            f"<p>Windows, in PowerShell:</p>\n<pre><code>Get-FileHash -Algorithm SHA256 .\\{setup}</code></pre>\n"
            f"<p>Linux, in the folder of the download, with {CHECKSUMS} beside the file:</p>\n"
            f"<pre><code>curl -fLO {escape(base)}{escape(sums)}\n"
            f"sha256sum -c --ignore-missing {CHECKSUMS}</code></pre>\n"
            "</section>\n"
            '<section class="part" id="other-files">\n'
            "<h2>Other files</h2>\n"
            f'<p><a href="download/{escape(version)}/{escape(symbols)}">{escape(symbols)}</a>, '
            f"{escape(size_text(size))}, SHA-256 <code class=\"hash\">{digest}</code>: the symbols of "
            "<code>junto.exe</code>. They resolve the frames of an error report of the Windows client. "
            "Junto does not need them to run.</p>\n"
            "</section>\n"
            '<section class="part" id="versions">\n'
            f"<h2>Versions</h2>\n<ul>\n{versions}</ul>\n"
            "</section>\n")
    return page(f"Junto {version}", body, SCRIPT)


def notes_page(release, notes, digests):
    version = release.name
    if digests:
        rows = "".join(f'<li><a href="../download/{escape(version)}/{escape(name)}">{escape(name)}</a>, '
                       f'{escape(size_text(size))}, SHA-256 <code class="hash">{digest}</code></li>\n'
                       for name, (size, digest) in sorted(digests.items()))
        files = (f"<ul>\n{rows}</ul>\n"
                 f'<p><a href="../download/{escape(version)}/{CHECKSUMS}">{CHECKSUMS}</a></p>\n')
    else:
        files = "<p>The site keeps no first-install file of this version.</p>\n"
    body = ('<p><a href="../index.html">Download page</a></p>\n'
            f"<article>\n{markdown(notes)}</article>\n"
            '<section class="part" id="files">\n'
            f"<h2>Files of {escape(version)}</h2>\n{files}"
            "</section>\n")
    return page(f"Junto {version}: release notes", body)


# The site.

def write_text(path, text):
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "x", encoding="utf-8", newline="\n") as file:
        file.write(text)


def write_site(releases, notes, directory, base, server_install):
    digests = {}
    for release in releases:
        target = directory / "download" / release.name
        files = release.downloads()
        digests[release.name] = {}
        if files:
            target.mkdir(parents=True)
            for name, path in sorted(files.items()):
                digests[release.name][name] = copy(path, target / name)
            write_text(target / CHECKSUMS, checksums(digests[release.name]))
    updates = directory / "updates"
    updates.mkdir()
    for release in releases:
        for path in [*release.archives.values(), *release.releases.values()]:
            copy(path, updates / path.name)
    for target, path in newest_latest(releases).items():
        copy(path, updates / f"latest-{target}.json")
    newest = releases[-1]
    write_text(directory / "index.html",
               index_page(newest, digests[newest.name], releases, base, server_install))
    for release in releases:
        write_text(directory / "notes" / f"{release.name}.html",
                   notes_page(release, notes[release.name], digests[release.name]))


def build(releases_directory, notes_directory, output, base, server_install):
    """Writes the site into the new directory OUTPUT and returns its files with their sizes."""
    base = check_address(base, "--base", True)
    server_install = check_address(server_install, "--server-install", False)
    output = pathlib.Path(output).absolute()
    if output.exists() or output.is_symlink():
        raise SiteError(f"the output directory exists: {output}")
    if not output.parent.is_dir():
        raise SiteError(f"the parent of the output directory does not exist: {output.parent}")
    releases = read_releases(releases_directory)
    notes = read_notes(notes_directory, releases)
    temporary = pathlib.Path(tempfile.mkdtemp(prefix=f".{output.name}.", dir=output.parent))
    try:
        write_site(releases, notes, temporary, base, server_install)
        temporary.chmod(0o755)
        if output.exists() or output.is_symlink():
            raise SiteError(f"the output directory exists: {output}")
        os.rename(temporary, output)
    except BaseException:
        shutil.rmtree(temporary, ignore_errors=True)
        raise
    return [(path.relative_to(output).as_posix(), path.stat().st_size)
            for path in sorted(output.rglob("*")) if path.is_file()]


def upload_order():
    return ["Upload order:",
            "1. every file except updates/latest-*.json",
            "2. the files updates/latest-*.json, after every file of step 1 is present on the site"]


def main(arguments=None):
    parser = argparse.ArgumentParser(description="Writes the static files of the release site (ADR 0727).")
    parser.add_argument("releases", help="the directory with one directory of release files per version")
    parser.add_argument("notes", help="the directory with the notes <version>.md of each version")
    parser.add_argument("output", help="the new output directory")
    parser.add_argument("--base", required=True, help="the HTTPS address of the site, ending in /")
    parser.add_argument("--server-install", required=True, help="the HTTPS address of the server install document")
    options = parser.parse_args(arguments)
    try:
        files = build(options.releases, options.notes, options.output, options.base, options.server_install)
    except (SiteError, OSError) as error:
        print(f"site.py: {error}", file=sys.stderr)
        return 1
    for name, size in files:
        print(f"{size:>14,}  {name}")
    print(f"Update source of the clients (JUNTO_UPDATE_SOURCE): {options.base}updates/")
    for line in upload_order():
        print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
