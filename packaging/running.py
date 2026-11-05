"""The applications that run on this Mac with the bundle identifier of Junto.

The checks that start a Junto bundle (check_update.py, check_lifecycle.py, check_start.py) refuse
to run while another application of that identifier runs: macOS registers each application with
LaunchServices by its bundle identifier, and with an installed Junto running, the client of the
check printed nothing within its time limit or ended without output. The bundles of the checks
keep the identifier of the release, because the update and the login item of the client check
it, so the checks cannot use an identifier of their own."""
import platform
import re
import subprocess

IDENTIFIER = "org.junto.app"
LSAPPINFO = "/usr/bin/lsappinfo"
# One application in the output of `lsappinfo find`: `ASN:0x0-0x1c01c-"Finder":`, its display
# name in quotes, which can hold spaces, or without a name.
ASN = re.compile(r'ASN:0x[0-9a-fA-F]+-0x[0-9a-fA-F]+(?:-"(?:[^"\\]|\\.)*")?:')


def running(identifier=IDENTIFIER, run=subprocess.run, system=platform.system):
    """The description of each application of `identifier` that runs, as lsappinfo states it:
    its name, process number and bundle path. Empty on other systems than macOS."""
    if system() != "Darwin":
        return []
    found = run([LSAPPINFO, "find", f"bundleid={identifier}"], capture_output=True, text=True, timeout=30)
    if found.returncode != 0:
        raise RuntimeError(f"lsappinfo could not list the applications of {identifier}: "
                           f"{found.stderr.strip() or found.returncode}")
    described = []
    for application in ASN.findall(found.stdout):
        info = run([LSAPPINFO, "info", "-only", "name", "-only", "pid", "-only", "bundlepath", application],
                   capture_output=True, text=True, timeout=30)
        described.append(" ".join(info.stdout.split()) or application)
    return described


def refuse_when_running(identifier=IDENTIFIER, run=subprocess.run, system=platform.system):
    """Ends the check with a message when an application of `identifier` runs. When lsappinfo
    fails, the check states it and runs."""
    try:
        applications = running(identifier, run, system)
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
        print(f"the check could not learn whether Junto runs: {error}", flush=True)
        return
    if applications:
        raise SystemExit(
            f"An application with the bundle identifier {identifier} runs: {'; '.join(applications)}. "
            "A check that starts a Junto bundle with the same identifier gets no answer from it while "
            "that application runs. Quit Junto, then run the check again.")
