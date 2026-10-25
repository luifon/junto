"""The frame of the main window of a running client on Windows, read and used through the messages
that Windows sends to the window (ADR 0703), and the key of the identity of its notifications
(ADR 0742). Windows only."""
import contextlib
import ctypes
import time
import winreg
from ctypes import wintypes

user32 = ctypes.WinDLL("user32", use_last_error=True)
user32.SendMessageW.restype = ctypes.c_ssize_t
user32.SendMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
user32.GetDpiForWindow.argtypes = [wintypes.HWND]
user32.GetDpiForWindow.restype = wintypes.UINT
ENUM = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

WM_CLOSE = 0x0010
WM_NCHITTEST = 0x0084
WM_NCLBUTTONDOWN = 0x00A1
WM_NCLBUTTONUP = 0x00A2
WM_NCLBUTTONDBLCLK = 0x00A3
WM_MOUSEMOVE = 0x0200
HTCLIENT, HTCAPTION, HTMINBUTTON, HTMAXBUTTON, HTCLOSE = 1, 2, 8, 9, 20
SW_RESTORE = 9

# The strip and its buttons in pixels at 96 dots per inch (crates/client/src/shell/title_bar.rs).
STRIP_HEIGHT = 32
BUTTON_WIDTH = 46

IDENTITY_KEY = r"Software\Classes\AppUserModelId\chat.junto.Junto"


def windows_of(process_id):
    """The visible top-level windows of a process."""
    found = []

    def visit(window, _):
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(window, ctypes.byref(owner))
        if owner.value == process_id and user32.IsWindowVisible(window):
            found.append(window)
        return True

    user32.EnumWindows(ENUM(visit), 0)
    return found


def until(what, condition, seconds=60):
    """Reads `condition` until it holds; fails after `seconds`."""
    deadline = time.monotonic() + seconds
    while True:
        value = condition()
        if value:
            return value
        if time.monotonic() > deadline:
            raise AssertionError(f"{what}: not within {seconds} seconds")
        time.sleep(0.1)


def lparam(x, y):
    return ((y & 0xFFFF) << 16) | (x & 0xFFFF)


class Frame:
    """The main window of a client process."""

    def __init__(self, process_id):
        self.window = until("the window of the client is shown", lambda: (windows_of(process_id) or [None])[0])

    def scale(self):
        return user32.GetDpiForWindow(self.window) / 96

    def client_width(self):
        rect = wintypes.RECT()
        user32.GetClientRect(self.window, ctypes.byref(rect))
        return rect.right - rect.left

    def points(self):
        """Points of the client area, in physical pixels: the middle of each button, of the empty
        part of the strip and of the content under it."""
        scale, width = self.scale(), self.client_width()
        middle = round(STRIP_HEIGHT / 2 * scale)
        button = lambda index: round(width - (index + 0.5) * BUTTON_WIDTH * scale)
        return {
            HTCLOSE: (button(0), middle),
            HTMAXBUTTON: (button(1), middle),
            HTMINBUTTON: (button(2), middle),
            HTCAPTION: (width // 3, middle),
            HTCLIENT: (width // 3, round(200 * scale)),
        }

    def screen(self, point):
        position = wintypes.POINT(*point)
        user32.ClientToScreen(self.window, ctypes.byref(position))
        return position.x, position.y

    def point_at(self, point):
        """Puts the pointer at this point of the client area. gpui answers the hit test from the
        position of its last pointer event, and Windows sends the window a pointer event with the
        position of the cursor of the desktop when the window appears or changes its size under
        it, so the cursor of the desktop moves there too."""
        user32.SetCursorPos(*self.screen(point))
        user32.SendMessageW(self.window, WM_MOUSEMOVE, 0, lparam(*point))

    def hit_test(self, point):
        """What the window answers Windows for a pointer at this point of its client area."""
        self.point_at(point)
        return user32.SendMessageW(self.window, WM_NCHITTEST, 0, lparam(*self.screen(point)))

    def press(self, area, point, double=False):
        """A press of the left button on a part of the frame, as Windows reports it."""
        position = lparam(*self.screen(point))
        self.point_at(point)
        if double:
            user32.SendMessageW(self.window, WM_NCLBUTTONDBLCLK, area, position)
        else:
            user32.SendMessageW(self.window, WM_NCLBUTTONDOWN, area, position)
            user32.SendMessageW(self.window, WM_NCLBUTTONUP, area, position)

    def maximized(self):
        return bool(user32.IsZoomed(self.window))

    def minimized(self):
        return bool(user32.IsIconic(self.window))

    def restore(self):
        user32.ShowWindow(self.window, SW_RESTORE)


def check_frame(process_id, expect):
    """The strip answers the hit test of Windows with its buttons and its title area, and a press
    on each button and a double press on the strip act as on every window."""
    frame = Frame(process_id)
    cursor = wintypes.POINT()
    user32.GetCursorPos(ctypes.byref(cursor))
    try:
        check_controls(frame, expect)
    finally:
        user32.SetCursorPos(cursor.x, cursor.y)


def check_controls(frame, expect):
    def answering(area, what):
        """The points of the window once it answers `area` at the point of that area, read again
        at each test: Windows can report a new state of the window before its new size."""
        until(what, lambda: frame.hit_test(frame.points()[area]) == area)
        return frame.points()

    # The window draws the strip after its first frame; until then gpui answers no area.
    points = answering(HTCLOSE, "the window answers the hit test of its close button")
    for area, point in points.items():
        answer = frame.hit_test(point)
        expect(answer == area, f"the hit test at {point} answers {answer}, not {area}")
    frame.press(HTCAPTION, points[HTCAPTION], double=True)
    until("a double press on the strip maximizes the window", frame.maximized, 10)
    points = answering(HTCAPTION, "the maximized window answers the hit test of its strip")
    frame.press(HTCAPTION, points[HTCAPTION], double=True)
    until("a second double press on the strip restores the window", lambda: not frame.maximized(), 10)
    points = answering(HTMAXBUTTON, "the restored window answers the hit test of its maximize button")
    frame.press(HTMAXBUTTON, points[HTMAXBUTTON])
    until("the maximize button maximizes the window", frame.maximized, 10)
    points = answering(HTMAXBUTTON, "the restore button is where the maximize button was")
    frame.press(HTMAXBUTTON, points[HTMAXBUTTON])
    until("the restore button restores the window", lambda: not frame.maximized(), 10)
    points = answering(HTMINBUTTON, "the restored window answers the hit test of its minimize button")
    frame.press(HTMINBUTTON, points[HTMINBUTTON])
    until("the minimize button minimizes the window", frame.minimized, 10)
    frame.restore()
    until("the window is restored", lambda: not frame.minimized(), 10)
    points = answering(HTCLOSE, "the window answers the hit test of its close button after the restore")
    frame.press(HTCLOSE, points[HTCLOSE])


def identity():
    """The values of the key of the identity of the notifications, or None without the key."""
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, IDENTITY_KEY) as key:
            values, index = {}, 0
            while True:
                try:
                    name, value, kind = winreg.EnumValue(key, index)
                except OSError:
                    return values
                values[name] = (value, kind)
                index += 1
    except FileNotFoundError:
        return None


@contextlib.contextmanager
def kept_identity():
    """Restores the key of the identity of the notifications as it was, also after a failure: the
    client writes it at each start, and a check leaves no key of its own behind (ADR 0915)."""
    saved = identity()
    try:
        yield saved
    finally:
        with contextlib.suppress(FileNotFoundError):
            winreg.DeleteKey(winreg.HKEY_CURRENT_USER, IDENTITY_KEY)
        if saved is not None:
            with winreg.CreateKey(winreg.HKEY_CURRENT_USER, IDENTITY_KEY) as key:
                for name, (value, kind) in saved.items():
                    winreg.SetValueEx(key, name, 0, kind, value)
