#!/usr/bin/env python3
"""Selects the CI jobs that a change needs, from the paths and lines that it changes (ADR 0905):

    changes.py <base> <commit>

run in a clone that has both commits, prints one line per job of tools/ci/ci.sh:

    <job> <tab> run|python|skip <tab> <reason>

"run" runs the whole job, "python" only the Python unit tests of a Rust job, "skip" not the job,
and "base+<part>+..." the Windows job with only the parts that it names (below).
The rules, for every path that the change adds, changes or removes between <base> and
<commit>:

- Documents (Markdown files and docs/, except the API documents docs/api/, which the Linux job
  compares with the code) need no job.
- A Python unit test file (test_*.py) outside tools/ci needs the Python unit tests of the Rust
  jobs of the three systems.
- recording-layout/ needs the layout page job and the deployment job, which builds the recorder
  image; workers/transcription/ the transcription worker job and the deployment job;
  deploy/ the deployment job, except deploy/livekit-common.yaml, which every test server of the
  Rust jobs reads.
- Every other path needs every job: the Rust workspace, Cargo files, vendor/, tools/ci itself,
  the Linux image, packaging/, the workflows, and any path that no rule names.

The Windows job runs only when the change can behave differently on Windows: a path of
tools/windows/, of the native LiveKit and FFmpeg of the Windows tests, packaging/, vendor/, a build script, a Windows source file, the storage and the
paths of the client core, a changed Cargo.toml line of a target, a cfg, the build dependencies or
the features, or an added line of Rust, Python or PowerShell outside tests that names a cfg of
the system, a file system or process call, or a path join, or a changed Rust file that holds
code under a cfg of the system; also a change of tools/ci, of the
Rust toolchain, of .cargo/ or of a Cargo file. Otherwise the run of main of each night tests it.

The Windows job has four parts that only some changes need; the Linux job runs each of them on
every change. Each part runs for a change of tools/ci, of a Cargo file, of .cargo/ or of the
Rust toolchain, and:

- release (the client in release form): of the client (crates/client/) or of the copies of gpui
  (vendor/gpui-pre*);
- server (the server tests): of the server, the protocol, the permissions or the setup tool, of
  tools/windows/, of the FFmpeg of the tests, or of vendor/;
- live (the window tests of calls): of the client, the client core, the media engine, the
  allocation counter, the server, the protocol or the permissions, of tools/windows/, of the LiveKit of the tests or its
  settings, or of vendor/;
- crates (the lint of each crate alone): of vendor/, of a Windows source file, of a Rust file
  that holds code under a cfg of the system before or after the change (an edit inside an
  existing Windows block names no cfg), or an added line of Rust outside tests with a cfg of
  the system or of a feature, which is the code that only a lint on Windows of a crate alone
  checks.

A change that needs every part runs the whole job ("run"); another runs "base" and the parts
that it needs. A run of every job, of named jobs and the run of each night run every part.
"""
import fnmatch
import re
import subprocess
import sys

JOBS = {
    "linux": ["rust-linux", "licences", "transcription-worker", "layout-page"],
    "macos": ["rust-macos", "deployment"],
    "windows": ["rust-windows"],
}
RUST = ("rust-linux", "rust-macos", "rust-windows")
ALL = [job for jobs in JOBS.values() for job in jobs]

WINDOWS_PATHS = re.compile(
    r"^(tools/windows/|tools/(with_native_(livekit|ffmpeg)|native_downloads)\.py$|packaging/|vendor/|tools/ci/|"
    r"\.cargo/|rust-toolchain\.toml$|Cargo\.lock$|(.*/)?Cargo\.toml$|"
    r".*build\.rs$|crates/.*/windows[^/]*\.rs$|crates/.*/src/storage/|"
    r"crates/client-core/src/(paths|data_dir|keys|releases)(/|\.rs$))")
WINDOWS_LINES = re.compile(
    r"cfg\((not\()?(windows|unix|target_os|target_family)|cfg!\(|std::fs::|"
    r"fs::(write|rename|remove|create|copy|hard_link|OpenOptions)|PathBuf::from|"
    r"\.join\(\"[^\"]*[/\\\\]|std::process::|Command::new|signal|os\.path|subprocess|shutil")
WINDOWS_CARGO = re.compile(r"target\.|cfg\(|build-dependencies|\[features\]|^[+-]\s*default\s*=")
EVERY_PART = r"tools/ci/|\.cargo/|rust-toolchain\.toml$|Cargo\.lock$|(.*/)?Cargo\.toml$"
PART_PATHS = {
    "release": re.compile(rf"^({EVERY_PART}|crates/client/|vendor/gpui-pre)"),
    "server": re.compile(rf"^({EVERY_PART}|crates/(server|protocol|authz|setup)/|vendor/|tools/windows/|"
                         r"tools/(with_native_ffmpeg|native_downloads)\.py$)"),
    "live": re.compile(rf"^({EVERY_PART}|crates/(client|client-core|media|alloc-count|server|protocol|authz)/|vendor/|"
                       r"tools/windows/|tools/(with_native_livekit|native_downloads)\.py$|deploy/livekit-common\.yaml$)"),
    "crates": re.compile(rf"^({EVERY_PART}|vendor/|crates/.*/windows[^/]*\.rs$|crates/.*/windows/)"),
}
PARTS = list(PART_PATHS)
# A condition of the system anywhere in the text of a file, also one written over several
# lines: a key of the system, `windows` or `unix` in a cfg of one line, or a line that is only
# `windows` or `unix`, an item of a list of `any(`, `all(` or `not(`. `git grep -E` reads it.
SYSTEM_CODE = (r"target_os|target_family|cfg!?\(.*(windows|unix)"
               r"|^[[:space:]]*(not\()?(windows|unix)[[:space:]]*[,)]*[[:space:]]*$")
CRATE_LINES = re.compile(r"cfg\((not\()?(any\(|all\()?(windows|unix|target_os|target_family|feature)|cfg!\(")
TEST_PATH = re.compile(r"(^|/)tests?/|tests?\.rs$|/test_[^/]*\.py$|^test_[^/]*\.py$|_tests\.rs$")


def documents(path):
    return (path.endswith(".md") or path.startswith("docs/")) and not path.startswith("docs/api/")


def needs(path):
    """The jobs that a changed path needs, as {job: "run"|"python"}, and the reason."""
    if documents(path):
        return {}, "documents"
    name = path.rsplit("/", 1)[-1]
    if fnmatch.fnmatch(name, "test_*.py") and not path.startswith("tools/ci/"):
        return dict.fromkeys(RUST, "python"), "Python unit tests"
    if path.startswith("recording-layout/"):
        return {"layout-page": "run", "deployment": "run"}, "the layout page"
    if path.startswith("workers/transcription/"):
        return {"transcription-worker": "run", "deployment": "run"}, "the transcription worker"
    if path.startswith("deploy/") and path != "deploy/livekit-common.yaml":
        return {"deployment": "run"}, "the deployment"
    return dict.fromkeys(ALL, "run"), "code"


def windows_dependent(paths, cargo_lines, code_lines, system_files=()):
    """Why a change can behave differently on Windows, or an empty string."""
    found = [path for path in paths if WINDOWS_PATHS.match(path)]
    if found:
        return "Windows-dependent files: " + " ".join(found[:5])
    if system_files:
        return "code under a cfg of the system in " + " ".join(system_files[:5])
    cargo = [line for line in cargo_lines if WINDOWS_CARGO.search(line)]
    if cargo:
        return "Cargo.toml: " + cargo[0][:90]
    lines = [line for line in code_lines if WINDOWS_LINES.search(line)]
    if lines:
        return "a line that can differ on Windows: " + lines[0][:90]
    return ""


def parts(paths, rust_lines, system_files=()):
    """The parts of the Windows job that a change needs, with the reason of each."""
    needed = {}
    if system_files:
        needed["crates"] = f"code under a cfg of the system in {system_files[0]}"
    for name, pattern in PART_PATHS.items():
        found = [path for path in paths if pattern.match(path)]
        if found:
            needed[name] = found[0]
    if "crates" not in needed:
        line = next((line for line in rust_lines if CRATE_LINES.search(line)), None)
        if line is not None:
            needed["crates"] = line.strip()[:60]
    return needed


def select(paths, cargo_lines=(), code_lines=(), rust_lines=(), system_files=()):
    """{job: (decision, reason)} for every job."""
    decided = {}
    for path in paths:
        jobs, reason = needs(path)
        for job, decision in jobs.items():
            if decided.get(job, ("skip",))[0] != "run":
                decided[job] = (decision, f"{reason}: {path}")
    if not paths:
        decided = {job: ("run", "the change has no path") for job in ALL}
    result = {job: decided.get(job, ("skip", "the change does not touch it")) for job in ALL}
    if result["rust-windows"][0] != "skip":
        why = windows_dependent(paths, cargo_lines, code_lines, system_files)
        if not why:
            result["rust-windows"] = ("skip", "nothing that can behave differently on Windows; the run of main tests it")
        elif result["rust-windows"][0] == "run":
            needed = parts(paths, rust_lines, system_files)
            if len(needed) < len(PARTS):
                chosen = "+".join(["base", *(name for name in PARTS if name in needed)])
                because = "; ".join(f"{name}: {needed[name]}" for name in PARTS if name in needed)
                result["rust-windows"] = (chosen, f"{why}; parts {because or 'none'}")
            else:
                result["rust-windows"] = ("run", why)
    return result


def git(*arguments):
    return subprocess.run(["git", *arguments], capture_output=True, text=True, check=True).stdout


def added(diff):
    return [line[1:] for line in diff.splitlines() if line.startswith("+") and not line.startswith("+++")]


def changed(diff):
    return [line for line in diff.splitlines()
            if line[:1] in "+-" and not line.startswith(("+++", "---"))]


def from_git(base, commit):
    paths = [path for path in git("diff", "--no-renames", "--name-only", base, commit).splitlines() if path]
    cargo = changed(git("diff", "-U0", base, commit, "--", "*Cargo.toml"))
    code = [path for path in paths if path.endswith((".rs", ".py", ".ps1")) and not TEST_PATH.search(path)]
    lines = added(git("diff", "-U0", base, commit, "--", *code)) if code else []
    rust = [path for path in code if path.endswith(".rs")]
    rust_lines = added(git("diff", "-U0", base, commit, "--", *rust)) if rust else []
    every_rust = [path for path in paths if path.endswith(".rs")]
    return select(paths, cargo, lines, rust_lines, system_files(base, commit, every_rust))


def system_files(base, commit, paths):
    """The files of `paths` that hold code under a cfg of the system at `base` or at `commit`."""
    found = set()
    for revision in (base, commit):
        if not paths:
            break
        listed = subprocess.run(["git", "grep", "-l", "-E", SYSTEM_CODE, revision, "--", *paths],
                                capture_output=True, text=True)
        if listed.returncode not in (0, 1):
            raise subprocess.CalledProcessError(listed.returncode, listed.args, listed.stdout, listed.stderr)
        found.update(line.split(":", 1)[1] for line in listed.stdout.splitlines() if ":" in line)
    return sorted(found)


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print("usage: changes.py <base> <commit>", file=sys.stderr)
        sys.exit(2)
    try:
        for job, (decision, reason) in from_git(sys.argv[1], sys.argv[2]).items():
            print(f"{job}\t{decision}\t{reason}")
    except subprocess.CalledProcessError as error:
        print(f"git failed: {error.stderr.strip()}", file=sys.stderr)
        sys.exit(2)
