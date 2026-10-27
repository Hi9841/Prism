# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy", "pillow", "psutil"]
# ///
"""Functional test of the real Prism build on this machine (host integration).

It drives the owner's Prism (the bench's owner launcher, see apps.py)
with messages to its own window only (keys, characters, clicks; no global
input), captures the window, and checks real effects: saved settings, window
geometry, the registry, processes, files.

Before the tests it saves everything a test can change: prism.json, the
taskbar registry values, the taskbar auto-hide state, Prism's Start-icon
files, the pinned taskbar folder and the clipboard text. After the tests (also
after an error) it stops the test build, restores what changed, and starts the
build again hidden (`--autostart`), as the owner runs it.

    uv run gpui/oploop/bench/functest.py run [test ...]
    uv run gpui/oploop/bench/functest.py list
"""

from __future__ import annotations

import ctypes as C
import ctypes.wintypes as W
import json
import shutil
import subprocess
import sys
import time
import winreg
from dataclasses import dataclass, field
from pathlib import Path

import psutil
from PIL import Image

import apps
import winapi

# The owner's Prism (the installed Prism, or a build the owner runs from E2E_DIR\gpui).
EXE = apps.owner_launcher()
DATA = apps.ROAMING / "app.prism.launcher"
OUT_ROOT = apps.E2E_DIR / "functest"
PINNED = apps.ROAMING / "Microsoft" / "Internet Explorer" / "Quick Launch" / "User Pinned" / "TaskBar"
ADVANCED = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced"
TASKBAR_VALUES = ("TaskbarSi", "IconSizePreference", "TaskbarGlomLevel", "MMTaskbarGlomLevel", "TaskbarAl")
ICON_FILES = ("taskbar-start-icon.json", "taskbar-start-icon.png", "taskbar-alignment")
ICON_DIR = "taskbar-start-icons"

user32 = C.WinDLL("user32", use_last_error=True)
shell32 = C.WinDLL("shell32", use_last_error=True)
WM_MOUSEMOVE, WM_LBUTTONDOWN, WM_LBUTTONUP = 0x0200, 0x0201, 0x0202
VK_RETURN = 0x0D
WM_RBUTTONDOWN, WM_RBUTTONUP = 0x0204, 0x0205
MK_LBUTTON, MK_RBUTTON = 0x0001, 0x0002
user32.PostMessageW.argtypes = [W.HWND, W.UINT, W.WPARAM, W.LPARAM]


# ---------------------------------------------------------------- snapshot

class APPBARDATA(C.Structure):
    _fields_ = [("cbSize", W.DWORD), ("hWnd", W.HWND), ("uCallbackMessage", W.UINT),
                ("uEdge", W.UINT), ("rc", W.RECT), ("lParam", W.LPARAM)]


ABM_GETSTATE, ABM_SETSTATE, ABS_AUTOHIDE = 0x4, 0xA, 0x1
shell32.SHAppBarMessage.argtypes = [W.DWORD, C.POINTER(APPBARDATA)]
shell32.SHAppBarMessage.restype = C.c_size_t


def appbar_state() -> int:
    data = APPBARDATA(C.sizeof(APPBARDATA))
    return int(shell32.SHAppBarMessage(ABM_GETSTATE, C.byref(data)))


def set_appbar_state(state: int) -> None:
    data = APPBARDATA(C.sizeof(APPBARDATA))
    data.lParam = state
    shell32.SHAppBarMessage(ABM_SETSTATE, C.byref(data))


def taskbar_values() -> dict[str, int | None]:
    out = {}
    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, ADVANCED) as key:
        for name in TASKBAR_VALUES:
            try:
                out[name] = winreg.QueryValueEx(key, name)[0]
            except FileNotFoundError:
                out[name] = None
    return out


def clipboard_text() -> str | None:
    done = subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command",
                           "Add-Type -AssemblyName System.Windows.Forms; "
                           "if ([System.Windows.Forms.Clipboard]::ContainsText()) "
                           "{ [Console]::OutputEncoding = [Text.Encoding]::UTF8; "
                           "[Console]::Write([System.Windows.Forms.Clipboard]::GetText()) } else { exit 3 }"],
                          capture_output=True)
    return done.stdout.decode("utf8") if done.returncode == 0 else None


def set_clipboard_text(text: str) -> None:
    subprocess.run(["powershell.exe", "-NoProfile", "-NonInteractive", "-Command",
                    "Add-Type -AssemblyName System.Windows.Forms; "
                    "[System.Windows.Forms.Clipboard]::SetText([Console]::In.ReadToEnd())"],
                   input=text.encode("utf8"), check=True)


@dataclass
class Snapshot:
    prism_json: bytes
    taskbar: dict
    appbar: int
    icon_files: dict[str, bytes | None]
    icon_dir: dict[str, bytes]
    pinned: set[str]
    clipboard: str | None

    @staticmethod
    def take() -> "Snapshot":
        icon_dir = DATA / ICON_DIR
        return Snapshot(
            prism_json=(DATA / "prism.json").read_bytes(),
            taskbar=taskbar_values(),
            appbar=appbar_state(),
            icon_files={n: (DATA / n).read_bytes() if (DATA / n).exists() else None for n in ICON_FILES},
            icon_dir={p.name: p.read_bytes() for p in icon_dir.iterdir()} if icon_dir.exists() else {},
            pinned={p.name for p in PINNED.iterdir()},
            clipboard=clipboard_text(),
        )

    def save(self, folder: Path) -> None:
        folder.mkdir(parents=True, exist_ok=True)
        (folder / "prism.json").write_bytes(self.prism_json)
        (folder / "state.json").write_text(json.dumps({
            "taskbar": self.taskbar, "appbar": self.appbar, "pinned": sorted(self.pinned),
            "icon_files": {k: v is not None for k, v in self.icon_files.items()},
            "icon_dir": sorted(self.icon_dir),
        }, indent=1), encoding="utf8")

    def differences(self) -> list[str]:
        now = Snapshot.take()
        diff = []
        if now.prism_json != self.prism_json:
            diff.append("prism.json")
        if now.taskbar != self.taskbar:
            diff.append(f"taskbar {self.taskbar} -> {now.taskbar}")
        if (now.appbar & ABS_AUTOHIDE) != (self.appbar & ABS_AUTOHIDE):
            diff.append(f"auto-hide {self.appbar} -> {now.appbar}")
        if now.icon_files != self.icon_files or now.icon_dir != self.icon_dir:
            diff.append("start icon files")
        if now.pinned != self.pinned:
            diff.append(f"pinned {sorted(now.pinned ^ self.pinned)}")
        if self.clipboard is not None and now.clipboard != self.clipboard:
            diff.append("clipboard")
        return diff


def broadcast_taskbar_change() -> None:
    HWND_BROADCAST, WM_SETTINGCHANGE, SMTO_ABORTIFHUNG = 0xFFFF, 0x001A, 0x0002
    result = C.c_size_t()
    user32.SendMessageTimeoutW(W.HWND(HWND_BROADCAST), WM_SETTINGCHANGE, 0, C.c_wchar_p("TraySettings"),
                               SMTO_ABORTIFHUNG, 2000, C.byref(result))


def restore(snap: Snapshot) -> list[str]:
    """Puts back what a test changed. Runs with the test build stopped."""
    restored = []
    if (DATA / "prism.json").read_bytes() != snap.prism_json:
        (DATA / "prism.json").write_bytes(snap.prism_json)
        restored.append("prism.json")
    if taskbar_values() != snap.taskbar:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, ADVANCED, 0, winreg.KEY_SET_VALUE) as key:
            for name, value in snap.taskbar.items():
                if value is None:
                    try:
                        winreg.DeleteValue(key, name)
                    except FileNotFoundError:
                        pass
                else:
                    winreg.SetValueEx(key, name, 0, winreg.REG_DWORD, value)
        broadcast_taskbar_change()
        restored.append("taskbar registry")
    if (appbar_state() & ABS_AUTOHIDE) != (snap.appbar & ABS_AUTOHIDE):
        set_appbar_state(snap.appbar)
        restored.append("auto-hide")
    for name, data in snap.icon_files.items():
        path = DATA / name
        current = path.read_bytes() if path.exists() else None
        if current != data:
            if data is None:
                path.unlink()
            else:
                path.write_bytes(data)
            restored.append(name)
    icon_dir = DATA / ICON_DIR
    current_dir = {p.name: p.read_bytes() for p in icon_dir.iterdir()} if icon_dir.exists() else {}
    if current_dir != snap.icon_dir:
        if icon_dir.exists():
            shutil.rmtree(icon_dir)
        if snap.icon_dir:
            icon_dir.mkdir()
            for name, data in snap.icon_dir.items():
                (icon_dir / name).write_bytes(data)
        restored.append(ICON_DIR)
    if snap.clipboard is not None and clipboard_text() != snap.clipboard:
        set_clipboard_text(snap.clipboard)
        restored.append("clipboard")
    extra = {p.name for p in PINNED.iterdir()} - snap.pinned
    if extra:
        restored.append(f"NOT restored, pinned by a test: {sorted(extra)} (unpin by hand)")
    return restored


# ---------------------------------------------------------------- app control

def running() -> list[psutil.Process]:
    out = []
    for proc in apps.prism_processes():
        try:
            if Path(proc.exe()) == EXE:
                out.append(proc)
        except (psutil.AccessDenied, psutil.NoSuchProcess):
            pass
    return out


def stop_app() -> None:
    for proc in running():
        apps.log(f"stop test build pid={proc.pid}")
        apps.kill_tree(proc.pid)
    time.sleep(1.0)


def start_app(autostart: bool) -> int:
    env = apps.default_environment(apps.E2E_DIR / "default-env.txt")
    args = f'"{EXE}"' + (" --autostart" if autostart else "")
    pid = apps.wmi_create(args, EXE.parent, env)
    apps.log(f"started test build pid={pid} autostart={autostart}")
    return pid


@dataclass
class App:
    pid: int
    out: Path
    palette: int = 0
    shots: int = 0
    notes: list[str] = field(default_factory=list)

    def find_palette(self, timeout: float = 20.0) -> int:
        deadline = time.time() + timeout
        while time.time() < deadline:
            wins = [w for w in winapi.top_windows({self.pid}) if w.cls == "Zed::Window"]
            if wins:
                self.palette = wins[0].hwnd
                return self.palette
            time.sleep(0.2)
        raise RuntimeError("no palette window")

    def visible(self) -> bool:
        return winapi.is_visible(self.palette)

    def rect(self) -> tuple[int, int, int, int]:
        return winapi.window_rect(self.palette)

    def activate(self) -> None:
        winapi.set_named_event(apps.GPUI_ACTIVATE_EVENT)

    def open(self) -> None:
        if not self.visible():
            self.activate()
            self.wait(lambda: self.visible(), 3.0, "palette did not open")
        time.sleep(0.5)

    def key(self, vk: int, settle: float = 0.25) -> None:
        winapi.post_key(self.palette, vk)
        time.sleep(settle)

    def type(self, text: str, settle: float = 0.6) -> None:
        winapi.post_text(self.palette, text)
        time.sleep(settle)

    def move(self, x: int, y: int) -> None:
        user32.PostMessageW(self.palette, WM_MOUSEMOVE, 0, (y << 16) | (x & 0xFFFF))
        time.sleep(0.08)

    def click(self, x: int, y: int, settle: float = 0.4, right: bool = False) -> None:
        self.move(x, y)
        lparam = (y << 16) | (x & 0xFFFF)
        down, up, flag = (WM_RBUTTONDOWN, WM_RBUTTONUP, MK_RBUTTON) if right else (WM_LBUTTONDOWN, WM_LBUTTONUP, MK_LBUTTON)
        user32.PostMessageW(self.palette, down, flag, lparam)
        time.sleep(0.05)
        user32.PostMessageW(self.palette, up, 0, lparam)
        time.sleep(settle)

    def shot(self, name: str) -> Path:
        cap = winapi.Capturer(self.palette)
        try:
            frame = cap.grab()
        finally:
            cap.close()
        self.shots += 1
        path = self.out / f"{self.shots:02d}-{name}.png"
        Image.fromarray(frame[:, :, [2, 1, 0]]).save(path)
        return path

    @staticmethod
    def wait(check, timeout: float, message: str) -> None:
        deadline = time.time() + timeout
        while time.time() < deadline:
            if check():
                return
            time.sleep(0.1)
        raise AssertionError(message)


def settings() -> dict:
    return json.loads((DATA / "prism.json").read_text(encoding="utf8"))["settings"]


# ---------------------------------------------------------------- tests

TESTS: dict[str, callable] = {}


def test(fn):
    TESTS[fn.__name__] = fn
    return fn


@test
def open_and_close(app: App) -> str:
    app.open()
    app.shot("open")
    app.key(winapi.VK_ESCAPE)
    App.wait(lambda: not app.visible(), 3.0, "Escape did not hide the palette")
    app.activate()
    App.wait(app.visible, 3.0, "activation did not show the palette")
    app.activate()
    time.sleep(0.5)
    if not app.visible():
        raise AssertionError("a second activation hid the palette (it must only show it)")
    app.key(winapi.VK_ESCAPE)
    App.wait(lambda: not app.visible(), 3.0, "Escape did not hide the palette")
    return "activation shows the palette (twice: stays open); Escape hides it"


WM_MOUSEWHEEL = 0x020A


def wheel(app: "App", x: int, y: int, delta: int) -> None:
    """One wheel message per 120 units; the position is in screen coordinates."""
    left, top, _, _ = app.rect()
    sx, sy = left + x, top + y
    app.move(x, y)
    for _ in range(abs(delta) // 120):
        step = -120 if delta > 0 else 120
        user32.PostMessageW(app.palette, WM_MOUSEWHEEL, (step & 0xFFFF) << 16, ((sy & 0xFFFF) << 16) | (sx & 0xFFFF))
        time.sleep(0.05)
    time.sleep(0.6)


def settings_button(app: "App") -> tuple[int, int]:
    left, _, right, _ = app.rect()
    return right - left - 53, 48


@test
def probe_settings_layout(app: App) -> str:
    app.open()
    app.click(*settings_button(app), settle=0.8)
    app.shot("settings-top")
    for i in range(6):
        wheel(app, 400, 400, 360)
        app.shot(f"settings-scroll-{i + 1}")
    app.key(winapi.VK_ESCAPE, 0.6)
    app.key(winapi.VK_ESCAPE, 0.6)
    return "captured the settings sheet"


@test
def probe_palette_layout(app: App) -> str:
    app.open()
    app.type("character map", 1.2)
    app.shot("search-charmap")
    app.click(150, 147, settle=0.8, right=True)
    app.shot("context-menu")
    app.key(winapi.VK_ESCAPE, 0.5)
    app.key(winapi.VK_ESCAPE, 0.5)
    app.open()
    for x in (408, 447, 484):
        app.move(x, 596)
        time.sleep(1.4)
        app.shot(f"footer-{x}")
    app.click(522, 596, settle=0.8)
    app.shot("power-menu")
    app.key(winapi.VK_ESCAPE, 0.5)
    app.key(winapi.VK_ESCAPE, 0.5)
    return "captured the palette"


WM_CLOSE = 0x0010
GWL_EXSTYLE, WS_EX_TOPMOST = -20, 0x00000008
user32.GetWindowLongPtrW.restype = C.c_ssize_t
user32.GetWindowLongPtrW.argtypes = [W.HWND, C.c_int]
user32.RegisterHotKey.argtypes = [W.HWND, C.c_int, W.UINT, W.UINT]
user32.UnregisterHotKey.argtypes = [W.HWND, C.c_int]
# Context menu rows (x, y) in the window, with the first result row right-clicked at (150, 147).
MENU = {"open": (260, 169), "admin": (260, 201), "location": (260, 242), "copy_path": (260, 274),
        "pin": (260, 315), "properties": (260, 347)}
FOOTER = {"updates": (408, 596), "rebuild_index": (447, 596), "refresh_apps": (484, 596), "power": (522, 596)}


def pids(name: str) -> set[int]:
    return {p.pid for p in psutil.process_iter(["name"]) if (p.info["name"] or "").lower() == name.lower()}


def wait_new(name: str, before: set[int], timeout: float = 8.0) -> set[int]:
    deadline = time.time() + timeout
    while time.time() < deadline:
        new = pids(name) - before
        if new:
            return new
        time.sleep(0.2)
    return set()


def close_processes(new: set[int]) -> None:
    """WM_CLOSE to each window of the processes a test started, then kill what is left."""
    for pid in new:
        for win in winapi.top_windows({pid}):
            user32.PostMessageW(win.hwnd, WM_CLOSE, 0, 0)
    deadline = time.time() + 5
    while time.time() < deadline and any(psutil.pid_exists(pid) for pid in new):
        time.sleep(0.2)
    for pid in new:
        if psutil.pid_exists(pid):
            apps.kill_tree(pid)


def explorer_windows() -> set[int]:
    return {w.hwnd for w in winapi.top_windows(pids("explorer.exe")) if w.cls == "CabinetWClass"}


def close_palette(app: App) -> None:
    """Escape clears a query first, then hides the palette. Escape works only in the search
    field (as in the original), so a click puts the focus there first."""
    if app.visible():
        app.click(200, 48, settle=0.3)
    for _ in range(5):
        if not app.visible():
            return
        app.key(winapi.VK_ESCAPE, 0.4)
    if app.visible():
        raise AssertionError("Escape did not hide the palette")


def search(app: App, text: str, settle: float = 1.2) -> None:
    close_palette(app)
    app.open()
    app.type(text, settle)


def context(app: App, item: str) -> None:
    app.click(150, 147, settle=0.7, right=True)
    app.click(*MENU[item], settle=0.8)


def panel_left(app: App) -> int:
    left, _, right, _ = app.rect()
    return right - left - 340


def open_settings(app: App, steps: int = 0) -> None:
    close_palette(app)
    app.open()
    app.click(*settings_button(app), settle=0.8)
    for _ in range(steps):
        wheel(app, panel_left(app) + 180, 400, 360)


def close_settings(app: App) -> None:
    app.key(winapi.VK_ESCAPE, 0.6)
    app.key(winapi.VK_ESCAPE, 0.6)


def setting_click(app: App, x: int, y: int, steps: int = 0, name: str = "") -> None:
    open_settings(app, steps)
    app.click(panel_left(app) + x, y, settle=1.0)
    if name:
        app.shot(name)
    close_settings(app)


def wait_setting(key: str, expected, timeout: float = 4.0):
    deadline = time.time() + timeout
    while time.time() < deadline:
        value = settings().get(key)
        if value == expected or (callable(expected) and expected(value)):
            return value
        time.sleep(0.2)
    raise AssertionError(f"settings.{key} = {settings().get(key)!r}, expected {expected!r}")


def wait_true(check, timeout: float, message: str):
    deadline = time.time() + timeout
    while time.time() < deadline:
        value = check()
        if value:
            return value
        time.sleep(0.2)
    raise AssertionError(message)


def history() -> list:
    return json.loads((DATA / "prism.json").read_text(encoding="utf8"))["history"]


@test
def launch_app(app: App) -> str:
    before = pids("charmap.exe")
    search(app, "character map")
    app.shot("charmap-results")
    app.key(VK_RETURN, 0.5)
    new = wait_new("charmap.exe", before)
    try:
        if not new:
            raise AssertionError("Enter did not start Character Map")
        wait_true(lambda: not app.visible(), 3, "the palette stayed open after the launch")
        wait_true(lambda: "charmap" in json.dumps(history()[:1]).lower(), 4,
                  f"history does not start with Character Map: {json.dumps(history()[:1])[:200]}")
    finally:
        close_processes(new)
    return "Enter launched Character Map, the palette hid, history saved it first"


@test
def context_open_location(app: App) -> str:
    before = explorer_windows()
    search(app, "character map")
    context(app, "location")
    new = wait_true(lambda: explorer_windows() - before, 8, "Open location opened no Explorer window")
    for hwnd in new:
        user32.PostMessageW(hwnd, WM_CLOSE, 0, 0)
    return f"Open location opened {len(new)} Explorer window(s); closed"


@test
def context_copy_path(app: App) -> str:
    search(app, "character map")
    context(app, "copy_path")
    wait_true(lambda: "charmap" in (clipboard_text() or "").lower() or "character map" in (clipboard_text() or "").lower(),
              4, f"clipboard has no Character Map path: {clipboard_text()!r}")
    return f"Copy as path put {clipboard_text()!r} on the clipboard"


@test
def pin_taskbar(app: App) -> str:
    """Pin is the only safe direction: unpinning one of the owner's pins could not be undone on a
    Windows build that blocks programmatic pinning."""
    before = {p.name for p in PINNED.iterdir()}
    search(app, "character map")
    context(app, "pin")
    toast = False
    deadline = time.time() + 8
    while time.time() < deadline:
        added = {p.name for p in PINNED.iterdir()} - before
        if added:
            app.notes.append(f"pinned {sorted(added)}: unpin it by hand (a test never unpins)")
            return f"Pin to taskbar pinned {sorted(added)}"
        frame = Image.open(app.shot("pin-wait")).convert("L")
        # The toast sits above the footer (y 550-580) and has light text.
        if max(frame.crop((60, 550, 500, 580)).getdata()) > 200:
            toast = True
            break
        time.sleep(0.5)
    if not toast:
        raise AssertionError("Pin to taskbar gave no pin and no message")
    return "Windows blocks automatic pinning on this build; the app shows the manual-pin message (no pin changed)"


@test
def calculator_copy(app: App) -> str:
    search(app, "2+2*3")
    app.shot("calc")
    app.key(VK_RETURN, 0.8)
    wait_true(lambda: (clipboard_text() or "").strip() == "8", 4, f"clipboard is {clipboard_text()!r}, expected 8")
    return "2+2*3 shows a result; Enter copied '8'"


@test
def settings_page(app: App) -> str:
    before = pids("SystemSettings.exe")
    if before:
        return "SKIP: Windows Settings already runs (a test does not close your Settings window)"
    search(app, "display settings", 1.5)
    app.shot("display-settings")
    app.key(VK_RETURN, 0.5)
    new = wait_new("SystemSettings.exe", before, 10)
    close_processes(new)
    if not new:
        raise AssertionError("Enter on a Settings result did not open Windows Settings")
    return "a Settings result opened Windows Settings; closed"


@test
def file_search_open(app: App) -> str:
    if pids("notepad.exe"):
        return "SKIP: Notepad already runs (a test does not touch your Notepad)"
    name = f"prism-functest-{int(time.time())}"
    path = Path.home() / "Documents" / f"{name}.txt"
    path.write_text("Prism functional test file. It is deleted after the test.\n", encoding="utf8")
    before = pids("notepad.exe")
    new: set[int] = set()
    found_at = None
    try:
        t0 = time.time()
        while time.time() - t0 < 30:
            search(app, name, 1.0)
            frame = Image.open(app.shot("file-search")).convert("L")
            # The first result row (y 135-160) has light text when a result exists.
            if max(frame.crop((70, 135, 300, 160)).getdata()) > 150:
                found_at = time.time() - t0
                break
            app.key(winapi.VK_ESCAPE, 0.3)
            app.key(winapi.VK_ESCAPE, 0.3)
            time.sleep(2)
        if found_at is None:
            raise AssertionError("the new file did not appear in search within 30 s")
        app.key(VK_RETURN, 0.5)
        new = wait_new("notepad.exe", before, 10)
        if not new:
            raise AssertionError("Enter on the file did not open it")
        cmd = " ".join(psutil.Process(next(iter(new))).cmdline())
        if name not in cmd:
            app.notes.append(f"editor command line does not name the file: {cmd[-160:]}")
    finally:
        close_processes(new)
        time.sleep(0.5)
        path.unlink(missing_ok=True)
    return f"a new file appeared in search after {found_at:.1f} s and opened in the editor"


@test
def power_menu_opens(app: App) -> str:
    app.open()
    app.click(*FOOTER["power"], settle=0.8)
    app.shot("power-menu")
    app.key(winapi.VK_ESCAPE, 0.5)
    app.key(winapi.VK_ESCAPE, 0.5)
    return "the power menu opens and closes (no power action run)"


@test
def refresh_apps(app: App) -> str:
    apps_json = DATA / "apps.json"
    before = apps_json.stat().st_mtime
    app.open()
    app.click(*FOOTER["refresh_apps"], settle=1.0)
    app.shot("refresh-apps")
    wait_true(lambda: apps_json.stat().st_mtime > before, 30, "Refresh applications did not rewrite apps.json")
    app.key(winapi.VK_ESCAPE, 0.5)
    return "Refresh applications rescanned (apps.json rewritten)"


@test
def single_instance(app: App) -> str:
    env = apps.default_environment(apps.E2E_DIR / "default-env.txt")
    app.open()
    app.key(winapi.VK_ESCAPE, 0.5)
    second = apps.wmi_create(f'"{EXE}"', EXE.parent, env)
    wait_true(lambda: not psutil.pid_exists(second), 8, "a second launch kept running")
    wait_true(app.visible, 4, "a second launch did not show the running palette")
    if len(running()) != 1:
        raise AssertionError(f"{len(running())} Prism processes run")
    app.key(winapi.VK_ESCAPE, 0.5)
    return "a second launch exits and shows the running palette"


@test
def theme_light_dark(app: App) -> str:
    setting_click(app, 180, 242, name="theme-light")
    wait_setting("theme", "light")
    app.open()
    frame = Image.open(app.shot("light")).convert("L")
    light = sum(frame.getdata()) / (frame.width * frame.height)
    app.key(winapi.VK_ESCAPE, 0.5)
    setting_click(app, 118, 242)
    wait_setting("theme", "dark")
    if light < 150:
        raise AssertionError(f"the light theme looks dark (mean {light:.0f})")
    return f"Light (mean brightness {light:.0f}) and Dark apply and save"


@test
def accent_colors(app: App) -> str:
    seen = []
    for x in (65, 96, 129, 160, 32):
        setting_click(app, x, 339)
        time.sleep(0.3)
        seen.append(settings().get("accent"))
    if len(set(seen)) != 5 or seen[-1] != "iris":
        raise AssertionError(f"accents saved: {seen}")
    return f"accents saved in order: {seen}"


@test
def window_width(app: App) -> str:
    sizes = {}
    for label, x, expected in (("L", 112, 720), ("M", 76, 640), ("S", 41, 560)):
        setting_click(app, x, 418)
        wait_setting("width", expected)
        app.open()
        left, _, right, _ = app.rect()
        sizes[label] = right - left
        app.key(winapi.VK_ESCAPE, 0.4)
        if sizes[label] != expected:
            raise AssertionError(f"width {label}: window is {sizes[label]} px, expected {expected}")
    return f"window widths {sizes}"


@test
def view_zoom(app: App) -> str:
    open_settings(app)
    app.click(panel_left(app) + 118, 523, settle=0.8)
    zoomed = wait_setting("viewZoom", lambda v: bool(v) and v > 100)
    app.shot("zoom-in")
    close_settings(app)
    app.open()
    app.shot("zoomed-palette")
    app.key(winapi.VK_ESCAPE, 0.4)
    # Ctrl+0 cannot be posted (modifier state): use the reset button. Its place scales with the zoom.
    open_settings(app)
    scale = zoomed / 100
    left, _, right, _ = app.rect()
    width = right - left
    app.click(width - int(340 * scale) + int(149 * scale), int(523 * scale), settle=0.8)
    app.shot("zoom-reset")
    close_settings(app)
    wait_setting("viewZoom", 100)
    return f"zoom + saved {zoomed}%, reset saved 100%"


@test
def keep_on_top(app: App) -> str:
    def toggle() -> bool:
        open_settings(app, steps=4)
        app.click(panel_left(app) + 289, 362, settle=1.0)
        app.key(winapi.VK_ESCAPE, 0.6)  # closes the sheet; the palette stays open
        return bool(user32.GetWindowLongPtrW(app.palette, GWL_EXSTYLE) & WS_EX_TOPMOST)

    on = settings().get("alwaysOnTop")
    after = toggle()
    wait_setting("alwaysOnTop", not on)
    close_palette(app)
    back = toggle()
    wait_setting("alwaysOnTop", on)
    close_palette(app)
    if (after, back) != (not on, on):
        raise AssertionError(f"topmost {on} -> {after} -> {back}")
    return f"keep on top saves and sets the window's topmost style ({on} -> {after} -> {back})"


@test
def global_shortcut(app: App) -> str:
    MOD_ALT, MOD_CONTROL, VK_SPACE = 0x1, 0x2, 0x20
    setting_click(app, 168, 459, steps=3, name="shortcut-ctrl-alt-space")
    wait_setting("shortcut", lambda v: bool(v) and "space" in v.lower())
    taken = not user32.RegisterHotKey(None, 0x5A5A, MOD_ALT | MOD_CONTROL, VK_SPACE)
    if not taken:
        user32.UnregisterHotKey(None, 0x5A5A)
    setting_click(app, 82, 459, steps=3, name="shortcut-win")
    wait_setting("shortcut", "Win")
    time.sleep(0.5)
    free = bool(user32.RegisterHotKey(None, 0x5A5B, MOD_ALT | MOD_CONTROL, VK_SPACE))
    if free:
        user32.UnregisterHotKey(None, 0x5A5B)
    if not taken or not free:
        raise AssertionError(f"Ctrl+Alt+Space held by Prism: {taken}; released after Win: {free}")
    return "Ctrl+Alt+Space registers the global hotkey; back to Win releases it"


@test
def taskbar_alignment(app: App) -> str:
    for x, expected in ((54, 0), (114, 1)):
        setting_click(app, x, 344, steps=1, name=f"align-{expected}")
        wait_true(lambda: taskbar_values()["TaskbarAl"] == expected, 6,
                  f"TaskbarAl is {taskbar_values()['TaskbarAl']}, expected {expected}")
    return "alignment Left (TaskbarAl 0) and Center (1) apply"


@test
def taskbar_density(app: App) -> str:
    keys = ("TaskbarSi", "IconSizePreference")
    start = {k: taskbar_values()[k] for k in keys}
    seen = {}
    for label, x in (("compact", 62), ("when_full", 215), ("default", 138)):
        setting_click(app, x, 584, steps=1)
        time.sleep(0.8)
        seen[label] = {k: taskbar_values()[k] for k in keys}
    if seen["default"] != start:
        raise AssertionError(f"density did not return to the start values {start}: {seen}")
    if len({json.dumps(v, sort_keys=True) for v in seen.values()}) < 2:
        raise AssertionError(f"density options did not change the registry: {seen}")
    return f"density values {seen}"


@test
def taskbar_auto_hide(app: App) -> str:
    start = appbar_state() & ABS_AUTOHIDE
    setting_click(app, 289, 347, steps=2, name="auto-hide")
    wait_true(lambda: (appbar_state() & ABS_AUTOHIDE) != start, 6, "auto-hide did not change")
    setting_click(app, 289, 347, steps=2)
    wait_true(lambda: (appbar_state() & ABS_AUTOHIDE) == start, 6, "auto-hide did not change back")
    return "auto-hide turns on and off"


@test
def taskbar_combine(app: App) -> str:
    seen = {}
    for label, x in (("when_full", 139), ("never", 216), ("always", 61)):
        setting_click(app, x, 464, steps=2)
        time.sleep(0.8)
        seen[label] = taskbar_values()["TaskbarGlomLevel"]
    if seen != {"when_full": 1, "never": 2, "always": 0}:
        raise AssertionError(f"TaskbarGlomLevel {seen}")
    return f"combine buttons {seen}"


@test
def start_icon(app: App) -> str:
    for label, x in (("gem", 99), ("diamond", 158), ("system", 45)):
        setting_click(app, x, 591, steps=2, name=f"start-icon-{label}")
        time.sleep(0.8)
        text = (DATA / "taskbar-start-icon.json").read_text(encoding="utf8")
        if label not in text.lower():
            raise AssertionError(f"start icon {label}: {text}")
    return "Start icon Gem, Diamond, System apply"


@test
def quick_access_folder(app: App) -> str:
    before = list(settings().get("quickAccess") or [])
    setting_click(app, 66, 470, steps=4, name="quick-home")
    added = wait_setting("quickAccess", lambda v: v is not None and len(v) == len(before) + 1)
    setting_click(app, 66, 470, steps=4)
    wait_setting("quickAccess", lambda v: (v or []) == before)
    return f"Home added to Quick Access ({added}) and removed"


@test
def new_collection(app: App) -> str:
    open_settings(app, steps=6)
    left = panel_left(app)
    app.click(left + 71, 400, settle=0.4)
    app.type("Functest", 0.4)
    app.click(left + 262, 400, settle=0.8)
    app.shot("collection")
    close_settings(app)
    groups = wait_setting("appGroups", lambda v: any("Functest" in json.dumps(g) for g in (v or [])))
    return f"a collection was created and saved ({len(groups)} groups; restored after the run)"


@test
def recent_clear(app: App) -> str:
    count = len(history())
    if not count:
        return "SKIP: no recent items"
    open_settings(app, steps=6)
    app.click(panel_left(app) + 274, 459, settle=0.8)
    app.shot("recent-clear")
    close_settings(app)
    wait_true(lambda: not history(), 4, "Clear did not empty the recent items")
    return f"Clear removed {count} recent items (restored after the run)"


@test
def prism_pin_unpin(app: App) -> str:
    before = list(settings().get("pinnedApps") or [])
    search(app, "character map")
    app.click(522, 147, settle=1.0)
    app.shot("prism-pin")
    added = wait_setting("pinnedApps", lambda v: v is not None and len(v) == len(before) + 1)
    search(app, "character map")
    app.click(522, 147, settle=1.0)
    app.shot("prism-unpin")
    wait_setting("pinnedApps", lambda v: (v or []) == before)
    close_palette(app)
    new = [p for p in added if p not in before]
    return f"the row pin button pinned {new} in Prism and unpinned it"


@test
def properties_dialog(app: App) -> str:
    def dialogs() -> set[int]:
        # ShellExecuteEx("properties") hosts the sheet in the calling process (Prism).
        owners = pids("explorer.exe") | {app.pid}
        return {w.hwnd for w in winapi.top_windows(owners) if w.cls == "#32770" and winapi.is_visible(w.hwnd)}

    before = dialogs()
    search(app, "character map")
    context(app, "properties")
    new = wait_true(lambda: dialogs() - before, 8, "Properties opened no dialog")
    for hwnd in new:
        user32.PostMessageW(hwnd, WM_CLOSE, 0, 0)
    close_palette(app)
    return f"Properties opened {len(new)} dialog(s); closed"


@test
def check_for_updates(app: App) -> str:
    """With no newer release the original and the port show nothing (a toast appears only when an
    install fails). The check passes when the control comes back to its normal icon."""
    close_palette(app)
    app.open()
    app.move(300, 300)
    time.sleep(0.6)
    box = (390, 580, 426, 612)
    before = Image.open(app.shot("updates-before")).convert("L").crop(box)
    app.click(*FOOTER["updates"], settle=0.3)
    app.move(300, 300)
    time.sleep(8)
    after = Image.open(app.shot("updates-after")).convert("L").crop(box)
    close_palette(app)
    changed = sum(abs(a - b) > 40 for a, b in zip(before.getdata(), after.getdata()))
    if changed > 20:
        raise AssertionError(f"the update control changed after the check ({changed} pixels): see the capture")
    return "Check for updates ran; no newer release, so the control is unchanged (as in the original)"


@test
def autostart_entry(app: App) -> str:
    run_key = r"Software\Microsoft\Windows\CurrentVersion\Run"
    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, run_key) as key:
        values = {}
        index = 0
        while True:
            try:
                name, value, _ = winreg.EnumValue(key, index)
            except OSError:
                break
            values[name] = value
            index += 1
    prism = {k: v for k, v in values.items() if "prism" in (k + str(v)).lower()}
    if not prism:
        raise AssertionError("no Run entry for Prism")
    return f"Run entry: {prism}"


@test
def quit_app(app: App) -> str:
    open_settings(app, steps=6)
    app.click(panel_left(app) + 275, 561, settle=0.5)
    wait_true(lambda: not psutil.pid_exists(app.pid), 6, "Quit did not end the process")
    return "Quit ends the process"


# ---------------------------------------------------------------- runner

def run(names: list[str]) -> None:
    stamp = time.strftime("%Y%m%d-%H%M%S")
    out = OUT_ROOT / stamp
    out.mkdir(parents=True, exist_ok=True)
    snap = Snapshot.take()
    snap.save(out / "snapshot")
    apps.log(f"snapshot saved: {out / 'snapshot'}")
    results = []
    stop_app()
    try:
        pid = start_app(autostart=True)
        app = App(pid, out)
        app.find_palette()
        time.sleep(3.0)
        for name in names:
            started = time.time()
            try:
                detail = TESTS[name](app)
                results.append({"test": name, "ok": True, "detail": detail, "s": round(time.time() - started, 1)})
            except Exception as error:  # noqa: BLE001 - report every test
                results.append({"test": name, "ok": False, "detail": f"{type(error).__name__}: {error}",
                                "s": round(time.time() - started, 1)})
                try:
                    app.shot(f"fail-{name}")
                except Exception:  # noqa: BLE001
                    pass
            apps.log(f"{name}: {'PASS' if results[-1]['ok'] else 'FAIL'} {results[-1]['detail']}")
            if app.notes:
                results[-1]["notes"] = app.notes
                app.notes = []
    finally:
        stop_app()
        restored = restore(snap)
        apps.log(f"restored: {restored or 'nothing to restore'}")
        left = snap.differences()
        apps.log(f"differences after restore: {left or 'none'}")
        start_app(autostart=True)
        (out / "results.json").write_text(json.dumps({"results": results, "restored": restored, "left": left},
                                                      indent=1), encoding="utf8")
        apps.log(f"results: {out / 'results.json'}")


def main() -> None:
    if len(sys.argv) < 2 or sys.argv[1] not in ("run", "list"):
        print(__doc__)
        sys.exit(2)
    if sys.argv[1] == "list":
        print("\n".join(TESTS))
        return
    names = sys.argv[2:] or [n for n in TESTS if not n.startswith("probe_")]
    unknown = [n for n in names if n not in TESTS]
    if unknown:
        sys.exit(f"unknown tests: {unknown}")
    run(names)


if __name__ == "__main__":
    main()
