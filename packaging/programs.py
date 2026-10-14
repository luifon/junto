"""The programs that the packaging checks put into test bundles, and the report of a finished command."""
import pathlib
import shlex
import subprocess

# Copies of system programs are not used: for Apple silicon they hold only arm64e code, which
# macOS 15 does not run without an Apple signature. The oldest macOS for Apple silicon is the
# minimum, so that the SDK of a newer Xcode does not raise it above the macOS of the machine.
COMPILER = "/usr/bin/cc"
MINIMUM_MACOS = "11.0"


def outcome(result):
    def text(output):
        if isinstance(output, bytes):
            return output.decode(errors="replace")
        return output or ""
    arguments = result.args if isinstance(result.args, (list, tuple)) else [result.args]
    code = f"code {result.returncode}"
    if result.returncode < 0:
        code += f" (signal {-result.returncode})"
    return (f"{shlex.join(str(argument) for argument in arguments)} ended with {code}\n"
            f"stdout:\n{text(result.stdout)}\nstderr:\n{text(result.stderr)}")


def program(path, status):
    """Builds a program for this machine that ends with `status`."""
    path = pathlib.Path(path)
    source = f"int main(void) {{ return {int(status)}; }}\n"
    result = subprocess.run([COMPILER, f"-mmacosx-version-min={MINIMUM_MACOS}", "-x", "c", "-o", str(path), "-"], input=source,
                            capture_output=True, text=True, timeout=120)
    if result.returncode:
        raise RuntimeError("the test program was not built: " + outcome(result))
    return path
