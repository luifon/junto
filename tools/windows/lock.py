"""Runs a command while this process holds an exclusive lock of a file, and returns the exit
code of the command (127 when it cannot start). tools/serial.sh uses it on systems that have
neither `lockf` nor `flock`, such as the Bash of Git for Windows.

    python3 tools/windows/lock.py <lock file> <command> [arguments]

A second caller waits until the first has ended. The lock covers every process that the command
starts, also when this process is ended from outside:

- On Windows this process starts a second copy of itself, the keeper, and returns its exit code.
  The keeper takes the lock and runs the command and every process that it starts in a job
  object whose only handle the keeper holds. When the command ends, or when this process ends
  first (it was ended from outside), the keeper ends every process of the job, waits until each
  has ended, and only then exits, which releases the lock. When the keeper itself is ended, the
  limit "end the processes when the job is closed" ends them; the lock is then free while
  Windows ends them.
- On other systems the command inherits the locked file: the lock lasts until the last process
  that holds the file has ended. BSD `lockf`, which tools/serial.sh prefers on macOS, does not
  pass its file on, so its lock ends with its direct child. TERM, INT and HUP to this process
  end it while it waits for the lock, and are sent on to the command after the command has
  started.

The system releases the lock when the process that holds it ends, also when it is killed, so a
run that ended without its cleanup leaves no lock behind.
"""

import os
import signal
import subprocess
import sys
import time

# The time after which the keeper reports processes of the job that have not ended.
REPORT_SECONDS = 60


def hold(file):
    """Waits until this process holds the exclusive lock of the first byte of `file`."""
    if os.name == "nt":
        import ctypes
        from ctypes import wintypes
        import msvcrt

        class Overlapped(ctypes.Structure):
            _fields_ = [("internal", ctypes.c_size_t), ("internal_high", ctypes.c_size_t),
                        ("offset", wintypes.DWORD), ("offset_high", wintypes.DWORD),
                        ("event", wintypes.HANDLE)]

        lock_file = ctypes.WinDLL("kernel32", use_last_error=True).LockFileEx
        lock_file.argtypes = [wintypes.HANDLE, wintypes.DWORD, wintypes.DWORD, wintypes.DWORD,
                              wintypes.DWORD, ctypes.POINTER(Overlapped)]
        lock_file.restype = wintypes.BOOL
        exclusive = 0x2  # LOCKFILE_EXCLUSIVE_LOCK; without LOCKFILE_FAIL_IMMEDIATELY it waits.
        if not lock_file(msvcrt.get_osfhandle(file.fileno()), exclusive, 0, 1, 0, ctypes.byref(Overlapped())):
            raise ctypes.WinError(ctypes.get_last_error())
    else:
        import fcntl

        fcntl.flock(file.fileno(), fcntl.LOCK_EX)


class Job:
    """A Windows job object that ends its processes when its last handle is closed. With a name,
    another process can open the job by that name (`Job.opened`) and end its processes."""

    def __init__(self, name=None, opened=False):
        import ctypes
        from ctypes import wintypes

        self.ctypes = ctypes
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        self.kernel32 = kernel32
        kernel32.CreateJobObjectW.argtypes = [ctypes.c_void_p, wintypes.LPCWSTR]
        kernel32.CreateJobObjectW.restype = wintypes.HANDLE
        kernel32.SetInformationJobObject.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p,
                                                     wintypes.DWORD]
        kernel32.QueryInformationJobObject.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p,
                                                       wintypes.DWORD, ctypes.c_void_p]
        kernel32.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
        kernel32.TerminateProcess.argtypes = [wintypes.HANDLE, wintypes.UINT]
        kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        kernel32.OpenProcess.restype = wintypes.HANDLE
        kernel32.IsProcessInJob.argtypes = [wintypes.HANDLE, wintypes.HANDLE, ctypes.POINTER(wintypes.BOOL)]
        kernel32.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
        kernel32.WaitForSingleObject.restype = wintypes.DWORD
        kernel32.WaitForMultipleObjects.argtypes = [wintypes.DWORD, ctypes.POINTER(wintypes.HANDLE),
                                                    wintypes.BOOL, wintypes.DWORD]
        kernel32.WaitForMultipleObjects.restype = wintypes.DWORD
        kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
        kernel32.OpenJobObjectW.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.LPCWSTR]
        kernel32.OpenJobObjectW.restype = wintypes.HANDLE
        self.resume_process = ctypes.WinDLL("ntdll").NtResumeProcess
        self.resume_process.argtypes = [wintypes.HANDLE]
        self.resume_process.restype = ctypes.c_long

        class BasicLimits(ctypes.Structure):
            _fields_ = [("per_process_user_time", ctypes.c_int64), ("per_job_user_time", ctypes.c_int64),
                        ("flags", wintypes.DWORD), ("minimum_working_set", ctypes.c_size_t),
                        ("maximum_working_set", ctypes.c_size_t), ("active_processes", wintypes.DWORD),
                        ("affinity", ctypes.c_size_t), ("priority_class", wintypes.DWORD),
                        ("scheduling_class", wintypes.DWORD)]

        class Counters(ctypes.Structure):
            _fields_ = [(name, ctypes.c_uint64) for name in
                        ("reads", "writes", "others", "read_bytes", "written_bytes", "other_bytes")]

        class ExtendedLimits(ctypes.Structure):
            _fields_ = [("basic", BasicLimits), ("io", Counters), ("process_memory", ctypes.c_size_t),
                        ("job_memory", ctypes.c_size_t), ("peak_process_memory", ctypes.c_size_t),
                        ("peak_job_memory", ctypes.c_size_t)]

        if opened:
            self.handle = self.checked(kernel32.OpenJobObjectW(0x1F001F, False, name))  # JOB_OBJECT_ALL_ACCESS
            return
        self.handle = self.checked(kernel32.CreateJobObjectW(None, name))
        limits = ExtendedLimits()
        limits.basic.flags = 0x2000  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        self.checked(kernel32.SetInformationJobObject(self.handle, 9, ctypes.byref(limits),
                                                      ctypes.sizeof(limits)))

    @classmethod
    def opened(cls, name):
        """The job of that name, which another process created."""
        return cls(name, opened=True)

    def checked(self, value):
        if not value:
            raise self.ctypes.WinError(self.ctypes.get_last_error())
        return value

    def start(self, command):
        """Starts `command` suspended, puts it into the job, and lets it run. Its children are in
        the job from their start."""
        process = subprocess.Popen(command, creationflags=0x4)  # CREATE_SUSPENDED
        try:
            self.checked(self.kernel32.AssignProcessToJobObject(self.handle, int(process._handle)))
            if self.resume_process(int(process._handle)) != 0:
                raise OSError("the command could not be resumed")
        except OSError:
            process.kill()
            process.wait()
            raise
        return process

    def running(self):
        """The numbers and handles of the processes of the job that run. Windows removes a
        process from the list of its job, and from the count of its active processes, as soon
        as the process is being ended, before it has ended; only its handle tells when it has
        ended and its memory is released."""
        from ctypes import wintypes

        size = 256
        while True:
            class Ids(self.ctypes.Structure):
                _fields_ = [("assigned", wintypes.DWORD), ("listed", wintypes.DWORD),
                            ("ids", self.ctypes.c_size_t * size)]

            ids = Ids()
            if self.kernel32.QueryInformationJobObject(self.handle, 3, self.ctypes.byref(ids),
                                                       self.ctypes.sizeof(ids), None):
                break
            if self.ctypes.get_last_error() != 234:  # ERROR_MORE_DATA
                raise self.ctypes.WinError(self.ctypes.get_last_error())
            size *= 4
        processes = []
        for number in ids.ids[:ids.listed]:
            # SYNCHRONIZE, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE
            handle = self.kernel32.OpenProcess(0x00101001, False, number)
            if not handle:
                continue  # it has ended and its number is free
            member = wintypes.BOOL()
            # The number can belong to a later process outside the job.
            if self.kernel32.IsProcessInJob(handle, self.handle, self.ctypes.byref(member)) and member:
                processes.append((number, handle))
            else:
                self.kernel32.CloseHandle(handle)
        return processes

    def end(self, report_after=REPORT_SECONDS):
        """Ends every process of the job and waits until each has ended. Each process is ended
        through a handle that this method holds, so that its end can be awaited; the list of the
        job is read again until it is empty and every ended process has ended, so that a
        process that an ending process started is ended as well. When processes remain after
        `report_after` seconds, writes their numbers to the error output once and goes on
        waiting: a process that has not ended can still hold files of the run."""
        from ctypes import wintypes

        report_at = time.monotonic() + report_after
        ending = []
        while True:
            # A process stays in the list until Windows begins its end; its number is not reused
            # while a handle of it is open.
            known = {number for number, _ in ending}
            started = []
            for number, handle in self.running():
                self.kernel32.TerminateProcess(handle, 1)
                if number in known:
                    self.kernel32.CloseHandle(handle)
                else:
                    started.append((number, handle))
            left = []
            for number, handle in ending + started:
                if self.kernel32.WaitForSingleObject(handle, 0) == 0x102:  # WAIT_TIMEOUT
                    left.append((number, handle))
                else:
                    self.kernel32.CloseHandle(handle)
            ending = left
            if not ending:
                if not started:
                    return
                continue
            wait = report_at - time.monotonic()
            if report_at and wait <= 0:
                numbers = " ".join(str(number) for number, _ in ending)
                print(f"lock.py: {len(ending)} processes of the job have not ended {report_after} s after "
                      f"they were ended ({numbers}); the lock stays held until they end", file=sys.stderr,
                      flush=True)
                report_at = 0
            handles = (wintypes.HANDLE * min(len(ending), 64))(*[handle for _, handle in ending[:64]])
            timeout = int(wait * 1000) if report_at else 0xFFFFFFFF  # INFINITE
            self.kernel32.WaitForMultipleObjects(len(handles), handles, False, timeout)


def opened_process(number):
    """A handle of a running process and its creation time as Windows stores it, or None."""
    import ctypes
    from ctypes import wintypes

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.OpenProcess.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
    kernel32.OpenProcess.restype = wintypes.HANDLE
    kernel32.GetProcessTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(ctypes.c_uint64)] * 4
    handle = kernel32.OpenProcess(0x00101000, False, number)  # SYNCHRONIZE, QUERY_LIMITED
    if not handle:
        return None
    times = [ctypes.c_uint64() for _ in range(4)]
    if not kernel32.GetProcessTimes(handle, *[ctypes.byref(value) for value in times]):
        kernel32.CloseHandle(handle)
        return None
    return handle, times[0].value


def keep(front, created, lock, command):
    """The keeper: holds the lock for the process `front` (number and creation time) and runs
    `command` in a job until the command or `front` has ended, then ends the job."""
    import ctypes
    from ctypes import wintypes

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    kernel32.WaitForSingleObject.argtypes = [wintypes.HANDLE, wintypes.DWORD]
    kernel32.WaitForSingleObject.restype = wintypes.DWORD
    kernel32.WaitForMultipleObjects.argtypes = [wintypes.DWORD, ctypes.POINTER(wintypes.HANDLE), wintypes.BOOL,
                                                wintypes.DWORD]
    kernel32.WaitForMultipleObjects.restype = wintypes.DWORD
    opened = opened_process(front)
    if opened is None or opened[1] != created:
        return 1  # the caller has already ended
    front_handle = opened[0]
    with open(lock, "a+b") as file:
        hold(file)
        if kernel32.WaitForSingleObject(front_handle, 0) == 0:
            return 1  # the caller ended while this process waited for the lock
        job = Job()
        try:
            process = job.start(command)
        except OSError as error:
            print(f"lock.py: {command[0]} did not start: {error}", file=sys.stderr)
            return 127
        handles = (wintypes.HANDLE * 2)(int(process._handle), front_handle)
        kernel32.WaitForMultipleObjects(2, handles, False, 0xFFFFFFFF)
        code = process.poll()
        job.end()
        return 1 if code is None else code


class Forward:
    """The handling of TERM, INT and HUP: before the command starts they end this process; a
    signal that arrives while the command is being started is sent to it once it has started;
    after that each is sent on to the command."""

    def __init__(self):
        self.process = None
        self.starting = False
        self.pending = []
        for number in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
            signal.signal(number, self.received)

    def received(self, number, _):
        if self.process is not None:
            self.process.send_signal(number)
        elif self.starting:
            self.pending.append(number)
        else:
            raise SystemExit(128 + number)

    def start(self, command, file):
        self.starting = True
        self.process = subprocess.Popen(command, pass_fds=[file.fileno()])
        for number in self.pending:
            self.process.send_signal(number)
        return self.process


def run(file, command, forward):
    code = forward.start(command, file).wait()
    return 128 - code if code < 0 else code


def main(arguments):
    if arguments[:1] == ["--keeper"] and len(arguments) >= 5:
        return keep(int(arguments[1]), int(arguments[2]), arguments[3], arguments[4:])
    if len(arguments) < 2:
        print("usage: lock.py <lock file> <command> [arguments]", file=sys.stderr)
        return 2
    if os.name == "nt":
        # The keeper waits for this process by its number and creation time.
        _, created = opened_process(os.getpid())
        keeper = subprocess.Popen([sys.executable, os.path.abspath(__file__), "--keeper", str(os.getpid()),
                                   str(created), *arguments])
        return keeper.wait()
    forward = Forward()
    with open(arguments[0], "a+b") as file:
        hold(file)
        try:
            return run(file, arguments[1:], forward)
        except OSError as error:
            print(f"lock.py: {arguments[1]} did not start: {error}", file=sys.stderr)
            return 127


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
