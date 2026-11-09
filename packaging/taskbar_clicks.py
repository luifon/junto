"""Three presses on the taskbar button of the main window of a running client on Windows, with what
the system states after each press: whether the main window is minimized, which window is the
foreground window, and how many taskbar buttons name Junto. It reproduces the report that the first
press after a minimize does not restore the window, and names which explanation the record fits.

Run it in the desktop session of a person (a scheduled task, as the unelevated CI steps do), with the
program of an installed client:

    python packaging/taskbar_clicks.py "<folder>\\junto.exe"

It starts the program with an empty data directory of its own and ends it afterwards. The decision
(`verdict`) is a plain function with tests on every system (`test_taskbar_clicks.py`)."""
import dataclasses
import json
import subprocess
import sys
import tempfile
import time
from pathlib import Path

# The time after a press in which the window has to reach its new state.
SETTLE_SECONDS = 1.5
SAMPLE_SECONDS = 0.05


@dataclasses.dataclass(frozen=True)
class Observation:
    """The state after one press, at the end of its settle time."""
    minimized: bool
    # The foreground window belongs to the client process.
    client_in_front: bool
    # The foreground window is the main window itself.
    main_in_front: bool
    # The count of taskbar buttons whose name starts with "Junto".
    buttons: int
    # The main window was minimized at some sample during the settle time.
    minimized_meanwhile: bool = False


def verdict(observations):
    """What three presses on the taskbar button of an active main window show.

    The expected order: the first press minimizes the window, the second restores it, the third
    minimizes it again. The report: the second press leaves the window minimized and the third
    restores it."""
    if len(observations) != 3:
        return "incomplete: three presses are needed"
    first, second, third = observations
    if not first.minimized:
        return "not reproduced: the first press did not minimize the window"
    if not second.minimized:
        return "not reproduced: the second press restored the window"
    findings = ["reproduced: the second press left the window minimized"]
    if third.minimized:
        findings.append("the third press did not restore the window either")
    two_buttons = any(observation.buttons > 1 for observation in observations)
    if two_buttons:
        findings.append("Junto has more than one taskbar button: the presses and the active state "
                        "can belong to two buttons (the window properties of ADR 0755 are set after "
                        "the window is shown)")
    if first.client_in_front:
        findings.append("after the minimize a window of the client stayed the foreground window, so "
                        "the taskbar took the second press as a press on the active button and "
                        "minimized the window again" if first.main_in_front else
                        "after the minimize another window of the client became the foreground "
                        "window")
    elif not two_buttons:
        findings.append("after the minimize the foreground window belonged to another program: the "
                        "record does not explain the second press; the messages of the window are "
                        "needed")
    return "; ".join(findings)


def _user32():
    import ctypes
    from ctypes import wintypes
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    user32.GetForegroundWindow.restype = wintypes.HWND
    user32.IsIconic.argtypes = [wintypes.HWND]
    user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    return user32


def _process_of(user32, window):
    import ctypes
    from ctypes import wintypes
    owner = wintypes.DWORD()
    user32.GetWindowThreadProcessId(window, ctypes.byref(owner))
    return owner.value


def _buttons():
    """The taskbar buttons whose name starts with "Junto", with their rectangles in pixels of the
    screen, through UI Automation of the shell."""
    script = r"""
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes
$root = [System.Windows.Automation.AutomationElement]::RootElement
$condition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ClassNameProperty, 'Shell_TrayWnd')
$bar = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $condition)
$found = @()
foreach ($element in $bar.FindAll([System.Windows.Automation.TreeScope]::Descendants,
        [System.Windows.Automation.Condition]::TrueCondition)) {
    $name = $element.Current.Name
    $class = $element.Current.ClassName
    if ($name -like 'Junto*' -and $class -like '*TaskListButton*') {
        $box = $element.Current.BoundingRectangle
        $found += @{ name = $name; x = $box.X + $box.Width / 2; y = $box.Y + $box.Height / 2 }
    }
}
ConvertTo-Json -Compress -InputObject @($found)
"""
    result = subprocess.run(["powershell", "-NoProfile", "-NonInteractive", "-Command", script],
                            capture_output=True, text=True, timeout=60)
    if result.returncode != 0:
        raise RuntimeError(f"the taskbar buttons could not be read: {result.stderr}")
    text = result.stdout.strip()
    return json.loads(text) if text else []


def _press(point):
    """One press of the left button at a point of the screen, as a person makes it."""
    import ctypes
    from ctypes import wintypes

    class MOUSEINPUT(ctypes.Structure):
        _fields_ = [("dx", wintypes.LONG), ("dy", wintypes.LONG), ("mouseData", wintypes.DWORD),
                    ("dwFlags", wintypes.DWORD), ("time", wintypes.DWORD),
                    ("dwExtraInfo", ctypes.c_size_t)]

    # The mouse form is the largest member of the union of INPUT, so INPUT has its size.
    class INPUT(ctypes.Structure):
        _fields_ = [("type", wintypes.DWORD), ("mi", MOUSEINPUT)]

    user32 = _user32()
    user32.SetCursorPos(int(point[0]), int(point[1]))
    time.sleep(0.2)
    left_down, left_up = 0x0002, 0x0004
    inputs = (INPUT * 2)(INPUT(0, MOUSEINPUT(0, 0, 0, left_down, 0, 0)),
                         INPUT(0, MOUSEINPUT(0, 0, 0, left_up, 0, 0)))
    if user32.SendInput(2, inputs, ctypes.sizeof(INPUT)) != 2:
        raise RuntimeError(f"the press was not sent: {ctypes.get_last_error()}")


def _observe(user32, main, process_id):
    minimized_meanwhile = False
    deadline = time.monotonic() + SETTLE_SECONDS
    while time.monotonic() < deadline:
        minimized_meanwhile |= bool(user32.IsIconic(main))
        time.sleep(SAMPLE_SECONDS)
    front = user32.GetForegroundWindow()
    return Observation(minimized=bool(user32.IsIconic(main)),
                       client_in_front=bool(front) and _process_of(user32, front) == process_id,
                       main_in_front=bool(front) and front == main,
                       buttons=len(_buttons()),
                       minimized_meanwhile=minimized_meanwhile)


def run(program):
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import ctypes
    import window_frame
    user32 = _user32()
    # Pixels of the screen, as UI Automation states the rectangles of the buttons
    # (DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).
    user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
    with tempfile.TemporaryDirectory() as data:
        process = subprocess.Popen([str(program), "--data-dir", data], stdout=subprocess.DEVNULL,
                                   stderr=subprocess.DEVNULL)
        try:
            main = window_frame.until("the main window opens",
                                      lambda: next(iter(window_frame.windows_of(process.pid)), None))
            window_frame.until("the main window is the foreground window",
                               lambda: user32.GetForegroundWindow() == main, seconds=30)
            buttons = window_frame.until("the taskbar shows a button of Junto", _buttons)
            point = (buttons[0]["x"], buttons[0]["y"])
            observations = []
            for _ in range(3):
                _press(point)
                observations.append(_observe(user32, main, process.pid))
                print(f"    {observations[-1]}", flush=True)
            return observations
        finally:
            process.kill()
            process.communicate(timeout=60)


def main():
    if len(sys.argv) != 2 or sys.platform != "win32":
        print(__doc__)
        return 2
    observations = run(Path(sys.argv[1]))
    print(verdict(observations))
    return 0


if __name__ == "__main__":
    sys.exit(main())
