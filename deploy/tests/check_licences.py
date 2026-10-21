"""The licence files of the server image (ADR 0801): the licence of Junto, and a licence file
that lists every crate of junto-server for the system of the image, with texts that give the
licence of each, beside the licences of ffmpeg.

usage: check_licences.py <server image>
"""

import pathlib
import subprocess
import sys
import tomllib

REPOSITORY = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPOSITORY / "packaging"))
import licences  # noqa: E402

DIRECTORY = "/usr/share/licenses/junto-server"


def run(image, *command):
    result = subprocess.run(["docker", "run", "--rm", "--no-healthcheck", "--network", "none", "--entrypoint",
                             command[0], image, *command[1:]], capture_output=True, timeout=120)
    if result.returncode:
        raise AssertionError(f"{' '.join(command)} in {image} ended with code {result.returncode}: "
                             f"{result.stderr.decode(errors='replace').strip()}")
    return result.stdout


def main(image):
    failures = []

    def check(name, condition):
        print(f"{'ok  ' if condition else 'FAIL'}  {name}")
        if not condition:
            failures.append(name)

    triple = licences.target("linux", run(image, "uname", "-m").decode().strip())
    version = tomllib.loads((REPOSITORY / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
    names = run(image, "ls", DIRECTORY).decode().split()
    check(f"{DIRECTORY} holds the licence of Junto and the licence file", names == ["Junto-LICENSE.txt", licences.NAME])
    check("the licence of Junto is the LICENSE of the repository",
          run(image, "cat", f"{DIRECTORY}/Junto-LICENSE.txt") == (REPOSITORY / "LICENSE").read_bytes())
    text = run(image, "cat", f"{DIRECTORY}/{licences.NAME}").decode()
    check(f"the licence file is that of Junto {version} for {triple}",
          text.startswith(f"Junto {version} for {triple}: third-party licences\n"))
    expected = licences.graph(REPOSITORY, triple, licences.SERVER.packages)
    found = licences.listed(text)
    check(f"the licence file lists the {len(expected)} crates of junto-server for {triple}: "
          f"missing {sorted(expected - found)}, not in the graph {sorted(found - expected)}", found == expected)
    check(f"the texts of each crate give the licences of its expression: {licences.uncovered(text)}",
          not licences.uncovered(text))
    check("the licence file holds the Rust standard library", "\nThe Rust standard library\n" in text)
    check("the licence file names the licences of ffmpeg", "/usr/share/licenses/ffmpeg" in text)
    check("the licence texts of ffmpeg are in the image",
          licences.licences_in(run(image, "cat", "/usr/share/licenses/ffmpeg/COPYING.LGPLv2.1")) == {"LGPL-2.1-only"})
    return 1 if failures else 0


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(__doc__.strip())
    try:
        sys.exit(main(sys.argv[1]))
    except AssertionError as error:
        print(f"FAIL  {error}")
        sys.exit(1)
