"""Runs a command while this process holds an exclusive lock of a file, and returns the exit
code of the command (127 when it cannot start). tools/serial.sh uses it on systems that have
neither `lockf` nor `flock`, such as the Bash of Git for Windows.

    python3 tools/windows/lock.py <lock file> <command> [arguments]

A second caller waits until the first has ended. The lock covers every process that the command
starts, also when this process is ended from outside:

- On Windows the command and every process that it starts are in a job object whose only handle
  this process holds, with the limit "end the processes when the job is closed". When this
  process is ended, Windows closes the handle and ends every process of the job. When the
  command ends by itself, this process ends the processes that it left and waits until none
  remains, and only then releases the lock.
- On other systems the command inherits the locked file, as with `flock`: the lock lasts until
  the last process that holds the file has ended. TERM, INT and HUP to this process are sent on
  to the command.

The system releases the lock when the process that holds it ends, also when it is killed, so a
run that ended without its cleanup leaves no lock behind.
"""

import os
import signal
import subprocess
import sys


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
    """A Windows job object that ends its processes when its last handle is closed, with a
    completion port that reports when the job has no process left."""

    def __init__(self):
        import ctypes
        from ctypes import wintypes

        self.ctypes = ctypes
        kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
        self.kernel32 = kernel32
        kernel32.CreateJobObjectW.argtypes = [ctypes.c_void_p, wintypes.LPCWSTR]
        kernel32.CreateJobObjectW.restype = wintypes.HANDLE
        kernel32.CreateIoCompletionPort.argtypes = [wintypes.HANDLE, wintypes.HANDLE, ctypes.c_size_t,
                                                    wintypes.DWORD]
        kernel32.CreateIoCompletionPort.restype = wintypes.HANDLE
        kernel32.SetInformationJobObject.argtypes = [wintypes.HANDLE, ctypes.c_int, ctypes.c_void_p,
                                                     wintypes.DWORD]
        kernel32.AssignProcessToJobObject.argtypes = [wintypes.HANDLE, wintypes.HANDLE]
        kernel32.TerminateJobObject.argtypes = [wintypes.HANDLE, wintypes.UINT]
        kernel32.GetQueuedCompletionStatus.argtypes = [
            wintypes.HANDLE, ctypes.POINTER(wintypes.DWORD), ctypes.POINTER(ctypes.c_size_t),
            ctypes.POINTER(ctypes.c_void_p), wintypes.DWORD]
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

        class Port(ctypes.Structure):
            _fields_ = [("key", ctypes.c_void_p), ("port", wintypes.HANDLE)]

        self.handle = self.checked(kernel32.CreateJobObjectW(None, None))
        self.port = self.checked(kernel32.CreateIoCompletionPort(wintypes.HANDLE(-1), None, 0, 1))
        limits = ExtendedLimits()
        limits.basic.flags = 0x2000  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        self.checked(kernel32.SetInformationJobObject(self.handle, 9, ctypes.byref(limits),
                                                      ctypes.sizeof(limits)))
        port = Port(None, self.port)
        self.checked(kernel32.SetInformationJobObject(self.handle, 7, ctypes.byref(port), ctypes.sizeof(port)))

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

    def end(self):
        """Ends every process of the job and waits until the job has none."""
        from ctypes import wintypes

        self.kernel32.TerminateJobObject(self.handle, 1)
        message, key, overlapped = wintypes.DWORD(), self.ctypes.c_size_t(), self.ctypes.c_void_p()
        while True:
            self.checked(self.kernel32.GetQueuedCompletionStatus(
                self.port, self.ctypes.byref(message), self.ctypes.byref(key), self.ctypes.byref(overlapped),
                0xFFFFFFFF))
            if message.value == 4:  # JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO
                return


def run(file, command):
    if os.name == "nt":
        job = Job()
        process = job.start(command)
        code = process.wait()
        job.end()
        return code
    process = subprocess.Popen(command, pass_fds=[file.fileno()])
    for number in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(number, lambda received, _: process.send_signal(received))
    code = process.wait()
    return 128 - code if code < 0 else code


def main(arguments):
    if len(arguments) < 2:
        print("usage: lock.py <lock file> <command> [arguments]", file=sys.stderr)
        return 2
    with open(arguments[0], "a+b") as file:
        hold(file)
        try:
            return run(file, arguments[1:])
        except OSError as error:
            print(f"lock.py: {arguments[1]} did not start: {error}", file=sys.stderr)
            return 127


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
