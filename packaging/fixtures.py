"""Files that the packaging tests put in place of the output of a build."""
import pathlib
import subprocess

import licences

WEBRTC_LICENCE = b"# webrtc\nA licence text of a test WebRTC library.\n"


def build_directory(directory, triple, text=WEBRTC_LICENCE, profile="release"):
    """A directory of programs with the licence file of the prebuilt WebRTC library for `triple`
    where webrtc-sys-build puts it."""
    directory = pathlib.Path(directory)
    [crate] = sorted((licences.ROOT / "vendor").glob("webrtc-sys-build-*"))
    tag, platform = licences.webrtc_tag(crate), licences.webrtc_platform(triple)
    path = (directory / "build/scratch-0123456789abcdef/out/livekit_webrtc/livekit"
            / f"{platform}-{profile}-{tag}/{platform}-{profile}/LICENSE.md")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(text)
    return directory


def client_graph(triple):
    """The crates of junto and junto-recover for `triple`, as cargo tree lists them."""
    result = subprocess.run(["cargo", "tree", "--locked", "--quiet", "-e", "normal,build", "--target", triple,
                             "--prefix", "none", "-f", "{p}", "-p", "junto-client", "-p", "junto-client-core",
                             "--manifest-path", str(licences.ROOT / "Cargo.toml")],
                            capture_output=True, text=True, encoding="utf-8", check=True,
                            timeout=600)
    return {(line.split(" ")[0], line.split(" ")[1].removeprefix("v")) for line in result.stdout.splitlines() if line}
