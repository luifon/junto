#!/usr/bin/env python3
"""Check desktop process ownership and window restoration on an isolated X11 display."""
import pathlib
import subprocess
import sys
import tempfile
import time

program = sys.argv[1]
children = []


def windows(pid):
    result = subprocess.run(['xdotool', 'search', '--onlyvisible', '--pid', str(pid)],
                            text=True, capture_output=True, timeout=5)
    return result.stdout.split()


def wait_window(child):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        found = windows(child.pid)
        if found:
            return found[0]
        if child.poll() is not None:
            raise RuntimeError(f'primary exited with {child.returncode}')
        time.sleep(0.02)
    raise RuntimeError('primary window was not mapped in 30 seconds')


root = tempfile.TemporaryDirectory(prefix='junto-instance-')
try:
    work = root.name
    profile = str(pathlib.Path(work) / 'profile')
    with open(pathlib.Path(work) / 'primary.log', 'w+') as first_log, \
         open(pathlib.Path(work) / 'secondary.log', 'w+') as second_log:
        first = subprocess.Popen([program, '--data-dir', profile], stdout=first_log, stderr=first_log)
        children.append(first)
        first_window = wait_window(first)
        subprocess.run(['xdotool', 'windowminimize', first_window], check=True, timeout=5)
        second = subprocess.Popen([program, '--data-dir', profile], stdout=second_log, stderr=second_log)
        children.append(second)
        try:
            exit_code = second.wait(timeout=5)
        except subprocess.TimeoutExpired:
            raise RuntimeError('the second start stayed running instead of forwarding activation')
        if exit_code != 0:
            second_log.seek(0)
            raise RuntimeError(f'the second start failed: {exit_code}\n{second_log.read()}')
        deadline = time.monotonic() + 5
        while first_window not in windows(first.pid):
            if time.monotonic() >= deadline:
                raise RuntimeError('the second start did not restore the primary window')
            time.sleep(0.02)
        if first.poll() is not None:
            raise RuntimeError('the first process ended after activation')
        deadline = time.monotonic() + 5
        while subprocess.run(['xdotool', 'getactivewindow'], capture_output=True,
                             text=True, timeout=5).stdout.strip() != first_window:
            if time.monotonic() >= deadline:
                raise RuntimeError('the restored window did not receive focus')
            time.sleep(0.02)
        subprocess.run(['xdotool', 'key', '--clearmodifiers', 'ctrl+w'], check=True, timeout=5)
        deadline = time.monotonic() + 5
        while windows(first.pid):
            if time.monotonic() >= deadline:
                raise RuntimeError('the close shortcut did not close the main window')
            time.sleep(0.02)
        if first.poll() is not None:
            raise RuntimeError('closing the window ended the primary process')
        third = subprocess.Popen([program, '--data-dir', profile], stdout=second_log, stderr=second_log)
        children.append(third)
        if third.wait(timeout=5) != 0:
            raise RuntimeError('the third start failed')
        wait_window(first)
        other = subprocess.Popen([program, '--data-dir', str(pathlib.Path(work) / 'other-profile')],
                                 stdout=second_log, stderr=second_log)
        children.append(other)
        wait_window(other)
        print('single_instance: forwarded activation, restored closed and minimized windows, and kept profiles independent')
finally:
    for child in children:
        if child.poll() is None:
            child.terminate()
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=5)

    root.cleanup()
