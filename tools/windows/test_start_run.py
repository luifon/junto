"""Unit tests of tools/windows/start-run.ps1. They need Windows PowerShell, so they run on Windows
only, in the Windows CI job."""

import os
import pathlib
import subprocess
import sys
import tempfile
import unittest
import uuid

SCRIPT = pathlib.Path(__file__).resolve().with_name("start-run.ps1")
# The run of these tests. Its name is new in each process of the tests, so that a scheduled task
# \junto-ci\<run> that another copy or an earlier version of these tests left cannot change a
# result, and these tests change no task of another run.
RUN = f"sample-run-{uuid.uuid4().hex[:12]}"


def powershell(*arguments):
    return subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
                           *arguments], capture_output=True, text=True, timeout=120)


def script(directory, *arguments):
    return powershell("-File", str(SCRIPT), "-Directory", str(directory), "-Run", RUN, *arguments)


def functions(directory, commands):
    """Runs PowerShell commands after the functions of start-run.ps1 for the run RUN of
    `directory`. A function defined in `commands` replaces a command of the same name. The
    module of the scheduled-task commands is loaded first: PowerShell loads a module at the
    first use of one of its commands, and that load replaces the functions of the same names."""
    return powershell("-Command", f"Import-Module ScheduledTasks; "
                      f". '{SCRIPT}' -Directory '{directory}' -Run {RUN} -FunctionsOnly; {commands}")


def removed_task():
    """Removes the scheduled task of the run RUN, and states whether it existed."""
    result = powershell("-Command", f"$task = Get-ScheduledTask -TaskPath '\\junto-ci\\' -TaskName '{RUN}' "
                        "-ErrorAction SilentlyContinue; "
                        "if ($task) { $task | Stop-ScheduledTask; $task | Unregister-ScheduledTask -Confirm:$false; "
                        "'removed' }")
    return result.stdout.strip() == "removed"


def creation_time(number):
    """The creation time of a process as Windows stores it."""
    import ctypes
    from ctypes import wintypes

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.OpenProcess.restype = wintypes.HANDLE
    handle = kernel32.OpenProcess(0x1000, False, number)  # PROCESS_QUERY_LIMITED_INFORMATION
    times = [ctypes.c_uint64() for _ in range(4)]
    if not handle or not kernel32.GetProcessTimes(wintypes.HANDLE(handle), *[ctypes.byref(value) for value in times]):
        raise ctypes.WinError(ctypes.get_last_error())
    kernel32.CloseHandle(wintypes.HANDLE(handle))
    return times[0].value


def run_folder(temporary):
    here = pathlib.Path(temporary) / "runs" / RUN
    here.mkdir(parents=True)
    return here


def waiting_process():
    """A process that runs until a line is written to its standard input."""
    return subprocess.Popen([sys.executable, "-c", "import sys; sys.stdin.readline()"], stdin=subprocess.PIPE,
                            text=True)


def removal(temporary):
    """Starts the removal copy of start-run.ps1 for the run RUN."""
    return subprocess.Popen(["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File",
                             str(SCRIPT), "-Directory", temporary, "-Run", RUN, "-Remove"],
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)


@unittest.skipIf(os.name != "nt", "start-run.ps1 runs on Windows")
class Session(unittest.TestCase):
    def test_an_account_without_a_desktop_session_cannot_start_a_run(self):
        with tempfile.TemporaryDirectory(prefix="junto-start-") as temporary:
            result = script(temporary, "-Check", "-User", "NO-SUCH-MACHINE\\no-desktop")
            self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
            self.assertIn("NO-SUCH-MACHINE\\no-desktop is not logged on to the desktop", result.stdout)

    def test_a_disconnected_or_locked_session_is_refused_with_its_state(self):
        sessions = {
            "@()": "is not logged on to the desktop",
            "@([pscustomobject]@{ Connected = $false; Lock = 'unlocked' })": "is disconnected",
            "@([pscustomobject]@{ Connected = $true; Lock = 'locked' })": "is locked",
            "@([pscustomobject]@{ Connected = $false; Lock = 'locked' },"
            " [pscustomobject]@{ Connected = $true; Lock = 'locked' })": "is locked",
            "@([pscustomobject]@{ Connected = $true; Lock = 'unknown' })": "does not state whether the screen",
            "@([pscustomobject]@{ Connected = $true; Lock = 'unlocked' })": "",
            "@([pscustomobject]@{ Connected = $false; Lock = 'unlocked' },"
            " [pscustomobject]@{ Connected = $true; Lock = 'unlocked' })": "",
        }
        for given, expected in sessions.items():
            with self.subTest(sessions=given):
                result = functions("x", f"Get-DesktopRefusal 'MACHINE\\account' {given}")
                self.assertEqual(result.returncode, 0, result.stderr)
                if expected:
                    self.assertIn(expected, result.stdout)
                else:
                    self.assertEqual(result.stdout.strip(), "")

    # A session whose lock state Windows does not know is not taken as unlocked.
    def test_the_read_session_values_give_the_connect_and_lock_state(self):
        values = {(0, 1): "True unlocked", (0, 0): "True locked", (0, -1): "True unknown", (0, 2): "True unknown",
                  (4, 1): "False unlocked"}
        for (state, flags), expected in values.items():
            with self.subTest(state=state, flags=flags):
                result = functions("x", f"$s = Get-SessionState {state} {flags}; \"$($s.Connected) $($s.Lock)\"")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), expected)

    # The tests of the Windows job run in the desktop session without elevation.
    def test_the_sessions_are_read_without_elevation(self):
        own = powershell("-Command", "[Security.Principal.WindowsIdentity]::GetCurrent().Name; "
                         "[Diagnostics.Process]::GetCurrentProcess().SessionId").stdout.split("\n")
        account, session = own[0].strip(), int(own[1])
        result = functions("x", "Get-Session | ForEach-Object { \"$($_.Id) $($_.Account)\" }")
        self.assertEqual(result.returncode, 0, result.stderr)
        if session > 0:
            self.assertIn(f"{session} {account}".lower(), result.stdout.lower())


@unittest.skipIf(os.name != "nt", "start-run.ps1 runs on Windows")
class Ended(unittest.TestCase):
    # tools/ci/ci.sh reports such a run as failed, and "ci.sh forget" removes it.
    def test_a_start_that_is_refused_records_its_end_and_removes_the_bundle(self):
        with tempfile.TemporaryDirectory(prefix="junto-start-") as temporary:
            here = run_folder(temporary)
            (here / "commit.bundle").write_bytes(b"bundle")
            result = functions(temporary, f"Stop-Start 2 'the run {RUN} cannot start: the screen is locked'")
            self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
            self.assertEqual((here / "finished").read_text(), "2\n")
            self.assertEqual((here / "runner.log").read_text().strip(),
                             f"the run {RUN} cannot start: the screen is locked")
            self.assertFalse((here / "commit.bundle").exists())

    # The second copy of start-run.ps1 waits for the process recorded in the file "process" of the
    # run, then removes the task (the run of these tests has none, so it records that the task was
    # not removed).
    def test_the_removal_waits_until_the_process_of_the_run_has_ended(self):
        with tempfile.TemporaryDirectory(prefix="junto-start-") as temporary:
            here = run_folder(temporary)
            child = waiting_process()
            try:
                (here / "process").write_text(f"{child.pid} {creation_time(child.pid)}\n")
                remover = removal(temporary)
                try:
                    with self.assertRaises(subprocess.TimeoutExpired):
                        remover.wait(timeout=10)
                finally:
                    child.communicate("end\n", timeout=60)
                output, _ = remover.communicate(timeout=120)
            finally:
                child.kill()
            self.assertEqual(remover.returncode, 1, output)
            self.assertIn("the task of the run was not removed", (here / "task.log").read_text())
            self.assertNotIn("the wait for the process of the run failed", (here / "task.log").read_text())

    # After the console host of the run has ended, Windows can give its number to another
    # process. The removal does not wait for that process.
    def test_the_removal_does_not_wait_for_a_later_process_with_the_number_of_the_run(self):
        with tempfile.TemporaryDirectory(prefix="junto-start-") as temporary:
            here = run_folder(temporary)
            child = waiting_process()
            try:
                (here / "process").write_text(f"{child.pid} {creation_time(child.pid) - 1}\n")
                remover = removal(temporary)
                try:
                    output, _ = remover.communicate(timeout=60)
                except subprocess.TimeoutExpired:
                    remover.kill()
                    raise AssertionError("the removal waits for a process that is not the process of the run")
                self.assertIsNone(child.poll(), "the process ended before the removal")
            finally:
                child.communicate("end\n", timeout=60)
            self.assertEqual(remover.returncode, 1, output)
            self.assertIn("the task of the run was not removed", (here / "task.log").read_text())

    def test_a_run_ends_when_its_recorded_process_no_longer_runs(self):
        with tempfile.TemporaryDirectory(prefix="junto-start-") as temporary:
            here = run_folder(temporary)
            child = waiting_process()
            try:
                start = creation_time(child.pid)
                (here / "process").write_text(f"{child.pid} {start}\n")
                self.assertEqual(script(temporary, "-Ended").returncode, 1, "the process still runs")
                # The same number with another creation time is another process.
                (here / "process").write_text(f"{child.pid} {start - 1}\n")
                self.assertEqual(script(temporary, "-Ended").returncode, 0)
                (here / "process").write_text(f"{child.pid} {start}\n")
                # The creation time does not depend on the time zone of the reader. The local
                # zone of .NET is replaced, as after a change of the zone of Windows.
                zone = ("$cached = [TimeZoneInfo].GetField('s_cachedData', 'NonPublic,Static').GetValue($null); "
                        "$cached.GetType().GetField('m_localTimeZone', 'NonPublic,Public,Instance').SetValue("
                        "$cached, [TimeZoneInfo]::FindSystemTimeZoneById('{}')); ")
                for name in ("Tokyo Standard Time", "Pacific Standard Time"):
                    with self.subTest(zone=name):
                        result = functions(temporary, zone.format(name) + "[TimeZoneInfo]::Local.Id; "
                                           f"Test-RunEnded $here {RUN}")
                        self.assertEqual(result.returncode, 0, result.stderr)
                        self.assertEqual(result.stdout.split(), [*name.split(), "False"])
            finally:
                child.communicate("end\n", timeout=60)
            result = script(temporary, "-Ended")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual(result.stdout.strip(), "ended")

    # Without the file "process" (the console host was not found, or the start ended before it
    # wrote the file) the end of the run is read from its start, recorded in the file "start",
    # and its task.
    def test_a_run_without_its_process_ends_when_neither_its_start_nor_its_task_runs(self):
        with tempfile.TemporaryDirectory(prefix="junto-start-") as temporary:
            here = run_folder(temporary)
            (here / "started").write_text("")
            # The run of these tests has no task, and no start of it is recorded.
            result = script(temporary, "-Ended")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            child = waiting_process()
            try:
                running = f"{child.pid} {creation_time(child.pid)}\n"
                # The same number with another creation time is another process.
                ended = f"{child.pid} {creation_time(child.pid) - 1}\n"
                cases = [
                    ("function Get-RunTask { [pscustomobject]@{ State = 'Running' } }", None, "False"),
                    ("function Get-RunTask { [pscustomobject]@{ State = 'Ready' } }", None, "True"),
                    ("function Get-RunTask { }", running, "False"),
                    ("function Get-RunTask { }", ended, "True"),
                    ("function Get-RunTask { [pscustomobject]@{ State = 'Running' } }", ended, "False"),
                    ("function Get-RunTask { }", None, "True"),
                ]
                for task, start, expected in cases:
                    with self.subTest(task=task, start=start):
                        (here / "start").unlink(missing_ok=True)
                        if start is not None:
                            (here / "start").write_text(start)
                        result = functions(temporary, f"{task}; Test-RunEnded $here {RUN}")
                        self.assertEqual(result.returncode, 0, result.stderr)
                        self.assertEqual(result.stdout.strip(), expected)
            finally:
                child.communicate("end\n", timeout=60)

    # A start that has not yet written the file "start" (during the start of PowerShell and the
    # compile of its native functions), or a start of an earlier version of the script, is found
    # by its command line.
    def test_a_run_whose_start_has_not_recorded_itself_is_found_by_its_command_line(self):
        with tempfile.TemporaryDirectory(prefix="junto-start-") as temporary:
            here = run_folder(temporary)
            # A process with the command line of the start of the run RUN, which runs until a line
            # is written to its input. The script is named without its folder, so that its path
            # has no quotes in the command line.
            (here / "start-run.ps1").write_text("[Console]::In.ReadLine() | Out-Null\n")
            start = subprocess.Popen(["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
                                      "-File", "start-run.ps1", "-Directory", temporary, "-Run", RUN],
                                     cwd=here, stdin=subprocess.PIPE, text=True)
            try:
                result = functions(temporary, f"function Get-RunTask {{ }}; Test-RunEnded $here {RUN}")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout.strip(), "False")
                result = script(temporary, "-Ended")
                self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            finally:
                start.communicate("end\n", timeout=60)
            result = script(temporary, "-Ended")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)


# The start with the commands that change scheduled tasks replaced, so that the tests create none.
# The replaced registration and start write their call to tasks.log of the run.
TASKS = ("function Get-DesktopSession { [pscustomobject]@{ Connected = $true; Lock = 'unlocked' } }; "
         "function Get-ScheduledTask { }; function Stop-ScheduledTask { }; function Unregister-ScheduledTask { }; "
         "function Register-ScheduledTask { param($TaskPath, $TaskName, $Action, $Principal, $Settings) "
         "Add-Content (Join-Path $here 'tasks.log') \"registered $TaskPath$TaskName\" }; "
         "function Start-ScheduledTask { param($TaskPath, $TaskName) "
         "Add-Content (Join-Path $here 'tasks.log') \"started $TaskPath$TaskName\" }; ")


@unittest.skipIf(os.name != "nt", "start-run.ps1 runs on Windows")
class Start(unittest.TestCase):
    # A test of the start registers no scheduled task of its own; a task that one registered is
    # removed here.
    def tearDown(self):
        self.assertFalse(removed_task(), f"a test of the start registered the task \\junto-ci\\{RUN}")

    # tools/ci/ci.sh then reports the run as failed, and "ci.sh forget" removes it.
    def test_an_error_before_the_run_has_started_records_its_end(self):
        with tempfile.TemporaryDirectory(prefix="junto-start-") as temporary:
            here = run_folder(temporary)
            (here / "commit.bundle").write_bytes(b"bundle")
            result = functions(temporary, TASKS + "function New-ScheduledTaskAction { throw 'no action' }; Invoke-Start")
            self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
            self.assertEqual((here / "finished").read_text(), "1\n")
            self.assertIn(f"the run {RUN} did not start: no action", (here / "runner.log").read_text())
            self.assertFalse((here / "commit.bundle").exists())

    # The start records its own process before it registers the task, so that a run whose
    # start runs is not taken as ended.
    def test_an_error_after_the_run_has_started_leaves_the_run(self):
        with tempfile.TemporaryDirectory(prefix="junto-start-") as temporary:
            here = run_folder(temporary)
            (here / "started").write_text("")
            result = functions(temporary, TASKS + "function Get-CimInstance { throw 'no processes' }; "
                               "\"start $PID\"; Invoke-Start")
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            number = int(result.stdout.split("\n")[0].split()[1])
            recorded, created = (here / "start").read_text().split()
            self.assertEqual(int(recorded), number)
            self.assertGreater(int(created), 0)
            self.assertFalse((here / "start.new").exists())
            self.assertIn(f"the run {RUN} started, but its start did not complete: no processes", result.stdout)
            self.assertIn("no processes", (here / "task.log").read_text())
            self.assertEqual((here / "tasks.log").read_text().split("\n")[:-1],
                             [f"registered \\junto-ci\\{RUN}", f"started \\junto-ci\\{RUN}"])
            self.assertFalse((here / "finished").exists())

    # A start removes the tasks of runs that have ended, and leaves the task of a start that
    # runs at the same time.
    def test_a_start_removes_only_the_tasks_of_ended_runs(self):
        with tempfile.TemporaryDirectory(prefix="junto-start-") as temporary:
            runs = pathlib.Path(temporary) / "runs"
            for name in ("finished-run", "starting-run", "earlier-start-run", "ended-run", "ended-start-run",
                         "running-run"):
                (runs / name).mkdir(parents=True)
            (runs / "finished-run" / "finished").write_text("0\n")
            # This process of the tests stands for the start that runs.
            (runs / "starting-run" / "start").write_text(f"{os.getpid()} {creation_time(os.getpid())}\n")
            (runs / "ended-run" / "process").write_text("4 1\n")  # the System process has another creation time
            (runs / "ended-start-run" / "start").write_text("4 1\n")
            (runs / "running-run" / "started").write_text("")
            # A start of an earlier version of the script writes no file "start"; it is found by its
            # command line, here replaced.
            names = ["finished-run", "gone-run", "starting-run", "earlier-start-run", "ended-run", "ended-start-run",
                     "running-run", RUN]
            listed = ", ".join(f"[pscustomobject]@{{ TaskName = '{name}' }}" for name in names)
            result = functions(temporary, f"function Get-ScheduledTask {{ @({listed}) }}; "
                               "function Unregister-ScheduledTask { [CmdletBinding(SupportsShouldProcess)] param($TaskPath, $TaskName) "
                               "\"removed $TaskName\" }; "
                               "function Get-RunStart([string]$Name) { if ($Name -eq 'earlier-start-run') { 'start' } }; "
                               "function Get-RunTask([string]$Name) { "
                               "[pscustomobject]@{ State = $(if ($Name -eq 'running-run') { 'Running' } else { 'Ready' }) } }; "
                               "Remove-EndedTask")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.split("\n")[:-1], ["removed finished-run", "removed gone-run",
                                                                "removed ended-run", "removed ended-start-run"])


if __name__ == "__main__":
    unittest.main()
