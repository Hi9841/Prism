"""Win32 calls for the Prism bench: windows, posted keys, PrintWindow capture.

Every input goes to one window with PostMessage. Nothing here sends global
input (no SendInput, no keybd_event), so the desktop and other apps do not
see the keys.
"""

from __future__ import annotations

import ctypes as C
import ctypes.wintypes as W
from dataclasses import dataclass

import numpy as np

user32 = C.WinDLL("user32", use_last_error=True)
gdi32 = C.WinDLL("gdi32", use_last_error=True)
kernel32 = C.WinDLL("kernel32", use_last_error=True)
dwmapi = C.WinDLL("dwmapi")

HWND = W.HWND
LPARAM = W.LPARAM
WPARAM = W.WPARAM

user32.SetProcessDpiAwarenessContext.argtypes = [C.c_void_p]
user32.SetProcessDpiAwarenessContext.restype = W.BOOL
# Per-monitor v2: window rectangles and captures are in physical pixels.
user32.SetProcessDpiAwarenessContext(C.c_void_p(-4))

WNDENUMPROC = C.WINFUNCTYPE(W.BOOL, HWND, LPARAM)
user32.EnumWindows.argtypes = [WNDENUMPROC, LPARAM]
user32.EnumChildWindows.argtypes = [HWND, WNDENUMPROC, LPARAM]
user32.GetWindowThreadProcessId.argtypes = [HWND, C.POINTER(W.DWORD)]
user32.GetWindowThreadProcessId.restype = W.DWORD
user32.GetClassNameW.argtypes = [HWND, W.LPWSTR, C.c_int]
user32.GetWindowTextW.argtypes = [HWND, W.LPWSTR, C.c_int]
user32.IsWindowVisible.argtypes = [HWND]
user32.IsWindow.argtypes = [HWND]
user32.GetWindowRect.argtypes = [HWND, C.POINTER(W.RECT)]
user32.PostMessageW.argtypes = [HWND, W.UINT, WPARAM, LPARAM]
user32.PostMessageW.restype = W.BOOL
user32.SendMessageTimeoutW.argtypes = [HWND, W.UINT, WPARAM, LPARAM, W.UINT, W.UINT, C.POINTER(C.c_size_t)]
user32.SendMessageTimeoutW.restype = C.c_size_t
user32.MapVirtualKeyW.argtypes = [W.UINT, W.UINT]
user32.MapVirtualKeyW.restype = W.UINT
user32.GetDC.argtypes = [HWND]
user32.GetDC.restype = W.HDC
user32.ReleaseDC.argtypes = [HWND, W.HDC]
user32.PrintWindow.argtypes = [HWND, W.HDC, W.UINT]
user32.PrintWindow.restype = W.BOOL
user32.GetForegroundWindow.restype = HWND
gdi32.CreateCompatibleDC.argtypes = [W.HDC]
gdi32.CreateCompatibleDC.restype = W.HDC
gdi32.CreateDIBSection.argtypes = [W.HDC, C.c_void_p, W.UINT, C.POINTER(C.c_void_p), W.HANDLE, W.DWORD]
gdi32.CreateDIBSection.restype = W.HBITMAP
gdi32.SelectObject.argtypes = [W.HDC, W.HGDIOBJ]
gdi32.SelectObject.restype = W.HGDIOBJ
gdi32.DeleteObject.argtypes = [W.HGDIOBJ]
gdi32.DeleteDC.argtypes = [W.HDC]
kernel32.OpenEventW.argtypes = [W.DWORD, W.BOOL, W.LPCWSTR]
kernel32.OpenEventW.restype = W.HANDLE
kernel32.SetEvent.argtypes = [W.HANDLE]
kernel32.CloseHandle.argtypes = [W.HANDLE]
dwmapi.DwmGetWindowAttribute.argtypes = [HWND, W.DWORD, C.c_void_p, W.DWORD]

WM_KEYDOWN, WM_KEYUP, WM_CHAR, WM_COPYDATA = 0x0100, 0x0101, 0x0102, 0x004A
VK_ESCAPE, VK_BACK = 0x1B, 0x08
PW_RENDERFULLCONTENT = 2
EVENT_MODIFY_STATE = 0x0002
DWMWA_CLOAKED = 14


@dataclass
class Win:
    hwnd: int
    pid: int
    cls: str
    title: str
    visible: bool
    rect: tuple[int, int, int, int]

    @property
    def size(self) -> tuple[int, int]:
        return self.rect[2] - self.rect[0], self.rect[3] - self.rect[1]


def _describe(handle) -> Win:
    if isinstance(handle, C.c_void_p):
        handle = handle.value
    handle = int(handle or 0)
    hwnd = HWND(handle)
    pid = W.DWORD()
    user32.GetWindowThreadProcessId(hwnd, C.byref(pid))
    cls = C.create_unicode_buffer(256)
    user32.GetClassNameW(hwnd, cls, 256)
    title = C.create_unicode_buffer(256)
    user32.GetWindowTextW(hwnd, title, 256)
    rect = W.RECT()
    user32.GetWindowRect(hwnd, C.byref(rect))
    return Win(
        handle, pid.value, cls.value, title.value, bool(user32.IsWindowVisible(hwnd)),
        (rect.left, rect.top, rect.right, rect.bottom),
    )


def top_windows(pids: set[int]) -> list[Win]:
    found: list[Win] = []

    def visit(hwnd, _):
        win = _describe(hwnd)
        if win.pid in pids:
            found.append(win)
        return True

    user32.EnumWindows(WNDENUMPROC(visit), 0)
    return found


def child_windows(hwnd: int) -> list[Win]:
    found: list[Win] = []

    def visit(child, _):
        found.append(_describe(child))
        return True

    user32.EnumChildWindows(HWND(hwnd), WNDENUMPROC(visit), 0)
    return found


def is_visible(hwnd: int) -> bool:
    """Visible to the user: WS_VISIBLE set and not cloaked by DWM."""
    if not user32.IsWindowVisible(HWND(hwnd)):
        return False
    cloaked = W.DWORD()
    dwmapi.DwmGetWindowAttribute(HWND(hwnd), DWMWA_CLOAKED, C.byref(cloaked), 4)
    return cloaked.value == 0


def foreground() -> int:
    return int(user32.GetForegroundWindow() or 0)


def window_rect(hwnd: int) -> tuple[int, int, int, int]:
    return _describe(hwnd).rect


def _key_lparam(vk: int, up: bool) -> int:
    scan = user32.MapVirtualKeyW(vk, 0)
    value = 1 | (scan << 16)
    if up:
        value |= 0xC0000000
    return value


def post_key(hwnd: int, vk: int) -> None:
    user32.PostMessageW(HWND(hwnd), WM_KEYDOWN, vk, _key_lparam(vk, False))
    user32.PostMessageW(HWND(hwnd), WM_KEYUP, vk, _key_lparam(vk, True))


def post_text(hwnd: int, text: str) -> None:
    """One WM_CHAR per character, the message that TranslateMessage makes."""
    for ch in text:
        vk_scan = user32.MapVirtualKeyW(ord(ch.upper()), 0) if ch.isalnum() else 0
        user32.PostMessageW(HWND(hwnd), WM_CHAR, ord(ch), 1 | (vk_scan << 16))


def set_named_event(name: str) -> bool:
    handle = kernel32.OpenEventW(EVENT_MODIFY_STATE, False, name)
    if not handle:
        return False
    try:
        return bool(kernel32.SetEvent(handle))
    finally:
        kernel32.CloseHandle(handle)


class COPYDATASTRUCT(C.Structure):
    _fields_ = [("dwData", C.c_size_t), ("cbData", W.DWORD), ("lpData", C.c_void_p)]


def send_copydata(hwnd: int, tag: int, payload: bytes, timeout_ms: int = 2000) -> bool:
    buf = C.create_string_buffer(payload)
    cds = COPYDATASTRUCT(tag, len(payload), C.cast(buf, C.c_void_p))
    result = C.c_size_t()
    ok = user32.SendMessageTimeoutW(
        HWND(hwnd), WM_COPYDATA, 0, C.cast(C.pointer(cds), C.c_void_p).value, 0x0002, timeout_ms, C.byref(result)
    )
    return bool(ok)


class BITMAPINFOHEADER(C.Structure):
    _fields_ = [
        ("biSize", W.DWORD), ("biWidth", W.LONG), ("biHeight", W.LONG), ("biPlanes", W.WORD),
        ("biBitCount", W.WORD), ("biCompression", W.DWORD), ("biSizeImage", W.DWORD),
        ("biXPelsPerMeter", W.LONG), ("biYPelsPerMeter", W.LONG), ("biClrUsed", W.DWORD),
        ("biClrImportant", W.DWORD),
    ]


class Capturer:
    """PrintWindow(PW_RENDERFULLCONTENT) into one reused 32-bit DIB.

    The capture is the window's own content as DWM composes it, so windows
    behind the palette and the mouse pointer do not change it.
    """

    def __init__(self, hwnd: int):
        self.hwnd = hwnd
        left, top, right, bottom = window_rect(hwnd)
        self.w, self.h = right - left, bottom - top
        screen = user32.GetDC(None)
        self.dc = gdi32.CreateCompatibleDC(screen)
        user32.ReleaseDC(None, screen)
        header = BITMAPINFOHEADER(C.sizeof(BITMAPINFOHEADER), self.w, -self.h, 1, 32, 0, 0, 0, 0, 0, 0)
        bits = C.c_void_p()
        self.bitmap = gdi32.CreateDIBSection(self.dc, C.byref(header), 0, C.byref(bits), None, 0)
        if not self.bitmap:
            raise OSError("CreateDIBSection failed")
        gdi32.SelectObject(self.dc, self.bitmap)
        self.nbytes = self.w * self.h * 4
        self.bits = bits.value
        self.view = np.ctypeslib.as_array((C.c_ubyte * self.nbytes).from_address(self.bits)).reshape(self.h, self.w, 4)

    def grab(self) -> np.ndarray:
        """Full frame, BGRA, a copy."""
        C.memset(self.bits, 0, self.nbytes)
        user32.PrintWindow(HWND(self.hwnd), self.dc, PW_RENDERFULLCONTENT)
        return self.view.copy()

    def grab_small(self) -> np.ndarray:
        """Every second pixel, BGR only: enough to see a glyph, 8x less to compare."""
        C.memset(self.bits, 0, self.nbytes)
        user32.PrintWindow(HWND(self.hwnd), self.dc, PW_RENDERFULLCONTENT)
        return self.view[::2, ::2, :3].copy()

    def close(self) -> None:
        gdi32.DeleteObject(self.bitmap)
        gdi32.DeleteDC(self.dc)


def change_box(a: np.ndarray, b: np.ndarray, threshold: int = 24) -> tuple[int, int, int, int]:
    """Changed pixel count, the width of the changed columns, and the first
    and last changed row."""
    mask = (np.abs(a.astype(np.int16) - b.astype(np.int16)).max(axis=2)) > threshold
    count = int(mask.sum())
    if count == 0:
        return 0, 0, 0, 0
    cols = np.flatnonzero(mask.any(axis=0))
    rows = np.flatnonzero(mask.any(axis=1))
    return count, int(cols[-1] - cols[0] + 1), int(rows[0]), int(rows[-1])


def change(a: np.ndarray, b: np.ndarray, threshold: int = 24) -> tuple[int, int]:
    """Changed pixel count and the width of their bounding columns."""
    return change_box(a, b, threshold)[:2]


def is_blank(frame: np.ndarray) -> bool:
    """True when no pixel has content (a transparent, unpainted window)."""
    return int(frame.max()) < 8
