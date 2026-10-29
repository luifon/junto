#!/usr/bin/env python3
"""Exercise saved main-window geometry with real processes on the isolated X11 desktop."""
import ctypes
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import time
import uuid

call_window = sys.argv[1] == '--call-target'
if call_window:
    messages = (json.loads(line) for line in pathlib.Path(sys.argv[2]).read_text().splitlines())
    program = next((message['executable'] for message in messages
                    if message.get('target', {}).get('name') == 'native_placement' and message.get('executable')), None)
    if program is None:
        raise RuntimeError('the native_placement build produced no executable')
else:
    program = sys.argv[1]
children = []


def run(*arguments):
    return subprocess.run(arguments, check=True, capture_output=True, text=True, timeout=10).stdout


def until(description, condition):
    deadline = time.monotonic() + 10
    while True:
        value = condition()
        if value:
            return value
        if time.monotonic() >= deadline:
            raise RuntimeError(description)
        time.sleep(0.02)


def window_of(child):
    def found():
        if child.poll() is not None:
            raise RuntimeError(f'the desktop process exited: {child.returncode}')
        reply = subprocess.run(['xdotool', 'search', '--onlyvisible', '--pid', str(child.pid)],
                               capture_output=True, text=True, timeout=5)
        return next(iter(reply.stdout.split()), None)
    return until('the main window did not appear', found)


def geometry(window):
    info = run('xwininfo', '-id', window)
    fields = ('Absolute upper-left X', 'Absolute upper-left Y', 'Width', 'Height')
    return tuple(int(re.search(rf'{field}:\s+(-?\d+)', info)[1]) for field in fields)


def maximized(window):
    state = run('xprop', '-id', window, '_NET_WM_STATE')
    return all(name in state for name in ('_NET_WM_STATE_MAXIMIZED_VERT', '_NET_WM_STATE_MAXIMIZED_HORZ'))


def state_request(window, enabled, first, second=None):
    class ClientMessage(ctypes.Structure):
        _fields_ = [('type', ctypes.c_int), ('serial', ctypes.c_ulong), ('send_event', ctypes.c_int),
                    ('display', ctypes.c_void_p), ('window', ctypes.c_ulong), ('message_type', ctypes.c_ulong),
                    ('format', ctypes.c_int), ('data', ctypes.c_long * 5)]
    class Event(ctypes.Union):
        _fields_ = [('client', ClientMessage), ('padding', ctypes.c_long * 24)]
    x11 = ctypes.CDLL('libX11.so.6')
    x11.XOpenDisplay.argtypes, x11.XOpenDisplay.restype = [ctypes.c_char_p], ctypes.c_void_p
    x11.XDefaultRootWindow.argtypes, x11.XDefaultRootWindow.restype = [ctypes.c_void_p], ctypes.c_ulong
    x11.XInternAtom.argtypes, x11.XInternAtom.restype = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int], ctypes.c_ulong
    x11.XSendEvent.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.c_int, ctypes.c_long, ctypes.POINTER(Event)]
    x11.XFlush.argtypes = x11.XCloseDisplay.argtypes = [ctypes.c_void_p]
    display = x11.XOpenDisplay(None)
    if not display:
        raise RuntimeError('no X11 display')
    try:
        atom = lambda name: x11.XInternAtom(display, name.encode(), False)
        event = Event()
        event.client = ClientMessage(33, 0, True, display, int(window), atom('_NET_WM_STATE'), 32,
                                     (ctypes.c_long * 5)(int(enabled), atom(first), atom(second) if second else 0, 2, 0))
        if not x11.XSendEvent(display, x11.XDefaultRootWindow(display), False, (1 << 19) | (1 << 20), ctypes.byref(event)):
            raise RuntimeError('the window manager did not accept the state request')
        x11.XFlush(display)
    finally:
        x11.XCloseDisplay(display)
    until('the state request did not take effect',
          lambda: (first in run('xprop', '-id', window, '_NET_WM_STATE')) == enabled)


def maximize(window, enabled):
    state_request(window, enabled, '_NET_WM_STATE_MAXIMIZED_VERT', '_NET_WM_STATE_MAXIMIZED_HORZ')
    until('the maximize request did not take effect', lambda: maximized(window) == enabled)


def close(child, window):
    run('xdotool', 'windowactivate', '--sync', window)
    run('xdotool', 'key', '--clearmodifiers', 'ctrl+q')
    if child.wait(timeout=10) != 0:
        raise RuntimeError('the desktop process failed while quitting')


def near(actual, expected):
    return all(abs(first - second) <= 8 for first, second in zip(actual, expected))


with tempfile.TemporaryDirectory(prefix='junto-placement-') as temporary:
    root = pathlib.Path(temporary)
    profile = root / 'profile'
    saved_file = profile / ('call-window.json' if call_window else 'main-window.json')
    with (root / 'desktop.log').open('w+') as output:
        def start():
            arguments = [program] if call_window else [program, '--data-dir', str(profile)]
            environment = dict(os.environ, JUNTO_TEST_PLACEMENT_DATA_DIR=str(profile)) if call_window else None
            child = subprocess.Popen(arguments, env=environment, stdout=output, stderr=output)
            children.append(child)
            return child, window_of(child)
        try:
            first, window = start()
            run('xdotool', 'windowmove', '--sync', window, '140', '110')
            run('xdotool', 'windowsize', '--sync', window, '900', '620')
            until('the resize did not take effect', lambda: geometry(window)[2:] == (900, 620))
            normal = geometry(window)
            until('the main window placement was not stored', lambda: saved_file.exists())
            close(first, window)
            saved = json.loads(saved_file.read_text())
            if not near((saved['width'], saved['height']), normal[2:]):
                raise RuntimeError(f'the stored size does not describe the normal window: {saved}, expected {normal}')
            second, window = start()
            until('restart did not restore the normal geometry', lambda: near(geometry(window), normal))
            maximize(window, True)
            close(second, window)
            saved = json.loads(saved_file.read_text())
            if not saved['maximized'] or not near((saved['width'], saved['height']), normal[2:]):
                raise RuntimeError('maximizing lost the normal restore size or the maximized state')
            third, window = start()
            until('restart did not restore the maximized state', lambda: maximized(window))
            maximize(window, False)
            until('restoring lost the normal geometry', lambda: near(geometry(window), normal))
            until('restoring did not persist the normal state', lambda: not json.loads(saved_file.read_text())['maximized'])
            state_request(window, True, '_NET_WM_STATE_FULLSCREEN')
            screen = tuple(map(int, run('xdotool', 'getdisplaygeometry').split()))
            until('fullscreen did not resize the window', lambda: geometry(window)[2:] == screen)
            close(third, window)
            restored = json.loads(saved_file.read_text())
            if restored['maximized'] or not near((restored['width'], restored['height']), normal[2:]):
                raise RuntimeError(f'fullscreen replaced the normal restore geometry: {restored}, expected {normal}')
            saved.update(display=str(uuid.uuid4()), x=80000, y=-80000, width=7600, height=4200, maximized=False)
            saved_file.write_text(json.dumps(saved))
            screen = tuple(map(int, run('xdotool', 'getdisplaygeometry').split()))
            work = (40, 60, screen[0] - 80, screen[1] - 120)
            run('xprop', '-root', '-f', '_NET_WORKAREA', '32c', '-set', '_NET_WORKAREA', ', '.join(map(str, work)))
            fourth, window = start()
            def visible():
                x, y, width, height = geometry(window)
                return (x >= work[0] - 8 and y >= work[1] - 8 and
                        x + width <= work[0] + work[2] + 8 and y + height <= work[1] + work[3] + 8)
            until('a removed monitor left the window outside the available display', visible)
            close(fourth, window)
            if list(profile.glob('panics/*')):
                raise RuntimeError('the desktop recorded a panic during placement restoration')
            print(f'window_placement ({"call" if call_window else "main"}): move/resize, restart, maximize/restore, fullscreen and a removed monitor/work area passed')
        except BaseException:
            output.flush()
            output.seek(0)
            print(output.read(), file=sys.stderr)
            raise
        finally:
            for child in children:
                if child.poll() is None:
                    child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=5)
