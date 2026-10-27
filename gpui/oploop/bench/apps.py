"""Start, find, drive, and stop one Prism build, and guard the owner's setup.

Only one Prism runs at a time: the installed Prism, the GPUI test build, or
the Tauri test build. `Session` stops the installed Prism, runs one test
build, and in every case (also after an error or Ctrl+C) stops the test
build, starts the installed Prism again, and checks that prism.json did not
change.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import time
from dataclasses import dataclass, field
from pathlib import Path

import psutil

import winapi

LOCAL = Path(os.environ["LOCALAPPDATA"])
ROAMING = Path(os.environ["APPDATA"])
INSTALLED_EXE = LOCAL / "Prism" / "prism.exe"
E2E_DIR = LOCAL / "PrismGpuiE2E"
PRISM_JSON = ROAMING / "app.prism.launcher" / "prism.json"
PRISM_JSON_BACKUP = E2E_DIR / "backup" / "prism.json"
# prism.exe modes that are not a launcher instance: the Start-button helpers,
# and the parity capture mode that other agents run from gpui/target.
HELPER_ARGS = ("--prism-start-restore-watchdog", "--prism-restore-start-menu", "--parity")

REPO = Path(__file__).resolve().parents[3]
BUILDS = {
    "gpui": REPO / "gpui" / "target" / "agent-e2e" / "release" / "prism.exe",
    "tauri": REPO / "gpui" / "target" / "agent-e2e-tauri" / "release" / "prism.exe",
}
# Tauri's single-instance plugin: second launches send WM_COPYDATA with this
# tag to the hidden "<identifier>-siw" window (tauri-plugin-single-instance 2.4).
TAURI_SI_TITLE = "app.prism.launcher-siw"
TAURI_SI_TAG = 1542
GPUI_ACTIVATE_EVENT = "Local\\PrismGpuiActivate"
PALETTE_CLASS = {"gpui": "Zed::Window", "tauri": "Tauri Window"}


def log(message: str) -> None:
    print(f"[{time.strftime('%H:%M:%S')}] {message}", flush=True)


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def powershell(script: str) -> str:
    done = subprocess.run(
        ["powershell.exe", "-NoProfile", "-NonInteractive", "-Command", script],
        capture_output=True, text=True, check=True,
    )
    return done.stdout.strip()


def ps_quote(text: str) -> str:
    return "'" + text.replace("'", "''") + "'"


def wmi_create(command_line: str, cwd: Path, env: dict[str, str] | None = None) -> int:
    """Starts a process through WMI (Win32_Process.Create).

    The process is a child of WmiPrvSE, not of this script, so it runs at
    the user's normal integrity level and outside the caller's job. With
    `env`, WMI gives the process exactly these variables, no others.
    """
    lines = [f"$a = @{{CommandLine={ps_quote(command_line)}; CurrentDirectory={ps_quote(str(cwd))}}}"]
    if env is not None:
        pairs = ",".join(ps_quote(f"{k}={v}") for k, v in env.items())
        lines.append(
            "$a.ProcessStartupInformation = New-CimInstance -ClassName Win32_ProcessStartup -ClientOnly "
            f"-Property @{{EnvironmentVariables=[string[]]@({pairs})}}"
        )
    lines.append("$r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments $a")
    lines.append("Write-Output \"$($r.ReturnValue) $($r.ProcessId)\"")
    status, pid = powershell("; ".join(lines)).split()
    if status != "0":
        raise RuntimeError(f"Win32_Process.Create returned {status} for {command_line}")
    return int(pid)


def default_environment(cache: Path) -> dict[str, str]:
    """The environment that WMI gives a process by default (the user's own)."""
    cache.parent.mkdir(parents=True, exist_ok=True)
    wmi_create(f'cmd.exe /d /c set > "{cache}"', cache.parent)
    for _ in range(50):
        if cache.exists() and cache.stat().st_size > 0:
            break
        time.sleep(0.1)
    time.sleep(0.2)
    text = cache.read_text(encoding="mbcs")
    # The file holds the owner's secrets; keep it on disk only for this read.
    cache.unlink()
    env = {}
    for line in text.splitlines():
        if "=" in line:
            key, value = line.split("=", 1)
            if not any(word in key.upper() for word in ("KEY", "TOKEN", "SECRET")):
                env[key] = value
    return env


def is_helper(cmdline: list[str] | None) -> bool:
    return any(arg in HELPER_ARGS for arg in (cmdline or [])[1:2])


def prism_processes() -> list[psutil.Process]:
    """Every launcher instance of Prism (no helper or parity processes)."""
    found = []
    for proc in psutil.process_iter(["name", "cmdline"]):
        if (proc.info["name"] or "").lower() != "prism.exe":
            continue
        if is_helper(proc.info["cmdline"]):
            continue
        found.append(proc)
    return found


def kill_tree(pid: int) -> None:
    try:
        root = psutil.Process(pid)
    except psutil.NoSuchProcess:
        return
    children = [c for c in root.children(recursive=True) if not is_helper(c.cmdline())]
    for proc in [root, *children]:
        try:
            proc.kill()
        except psutil.NoSuchProcess:
            pass
    psutil.wait_procs([root, *children], timeout=10)


def stop_installed() -> None:
    for proc in prism_processes():
        try:
            exe = Path(proc.exe())
        except (psutil.AccessDenied, psutil.NoSuchProcess):
            continue
        if exe == INSTALLED_EXE:
            log(f"stop installed Prism pid={proc.pid}")
            kill_tree(proc.pid)


def start_installed() -> int:
    for proc in prism_processes():
        try:
            if Path(proc.exe()) == INSTALLED_EXE:
                return proc.pid
        except (psutil.AccessDenied, psutil.NoSuchProcess):
            continue
    # The Run entry starts it with --autostart: hidden, no palette on start.
    pid = wmi_create(f'"{INSTALLED_EXE}" --autostart', INSTALLED_EXE.parent)
    log(f"started installed Prism pid={pid}")
    return pid


def check_prism_json() -> bool:
    """True if prism.json equals the backup. Restores the backup if not."""
    if sha256(PRISM_JSON) == sha256(PRISM_JSON_BACKUP):
        return True
    log("prism.json CHANGED: restoring the backup")
    shutil.copy2(PRISM_JSON_BACKUP, PRISM_JSON)
    return False


def deploy(kind: str) -> Path:
    """Copies the built exe out of the repo (the repo has a Low integrity label)."""
    source = BUILDS[kind]
    target = E2E_DIR / kind / "prism.exe"
    target.parent.mkdir(parents=True, exist_ok=True)
    if not target.exists() or sha256(source) != sha256(target):
        shutil.copy2(source, target)
    return target


_guardian_started = False


def start_guardian() -> None:
    """A detached watcher that restores the owner's setup if this process dies."""
    global _guardian_started
    if _guardian_started:
        return
    script = Path(__file__).with_name("guardian.ps1")
    wmi_create(
        f'powershell.exe -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass '
        f'-File "{script}" -BenchPid {os.getpid()}',
        E2E_DIR,
    )
    _guardian_started = True


@dataclass
class TestApp:
    kind: str
    exe: Path
    pid: int
    perf_log: Path
    palette: int = 0
    type_target: int = 0
    notes: list[str] = field(default_factory=list)

    def processes(self) -> list[psutil.Process]:
        root = psutil.Process(self.pid)
        return [root, *root.children(recursive=True)]

    def alive(self) -> bool:
        return psutil.pid_exists(self.pid)

    def find_palette(self, timeout: float = 20.0) -> int:
        deadline = time.time() + timeout
        while time.time() < deadline:
            wins = [w for w in winapi.top_windows({self.pid}) if w.cls == PALETTE_CLASS[self.kind]]
            if wins:
                self.palette = wins[0].hwnd
                return self.palette
            time.sleep(0.2)
        raise RuntimeError(f"{self.kind}: no palette window")

    def find_type_target(self) -> int:
        """GPUI reads keys on the palette window. WebView2 reads them on the
        Chromium render widget, a child window of another process."""
        if self.kind == "gpui":
            self.type_target = self.palette
            return self.type_target
        for _ in range(100):
            for child in winapi.child_windows(self.palette):
                if child.cls == "Chrome_RenderWidgetHostHWND":
                    self.type_target = child.hwnd
                    return self.type_target
            time.sleep(0.1)
        raise RuntimeError("tauri: no Chrome_RenderWidgetHostHWND")

    def activate(self) -> None:
        """The second-instance signal, sent the way a second launch sends it."""
        if self.kind == "gpui":
            if not winapi.set_named_event(GPUI_ACTIVATE_EVENT):
                raise RuntimeError("SetEvent(PrismGpuiActivate) failed")
            return
        si = [w for w in winapi.top_windows({self.pid}) if w.title == TAURI_SI_TITLE]
        if not si:
            raise RuntimeError("tauri: no single-instance window")
        exe = str(self.exe)
        payload = f"{self.exe.parent}|{exe}\0".encode()
        if not winapi.send_copydata(si[0].hwnd, TAURI_SI_TAG, payload):
            raise RuntimeError("tauri: WM_COPYDATA failed")

    def escape(self) -> None:
        winapi.post_key(self.type_target, winapi.VK_ESCAPE)

    def type_text(self, text: str) -> None:
        winapi.post_text(self.type_target, text)

    def perf_events(self) -> list[dict]:
        if not self.perf_log.exists():
            return []
        return [json.loads(line) for line in self.perf_log.read_text(encoding="utf8").splitlines() if line.strip()]


class Session:
    """with Session("gpui", out) as app: ... ; restores the owner's setup on exit."""

    def __init__(self, kind: str, out: Path, tag: str = ""):
        self.kind = kind
        self.out = out
        self.tag = tag or kind
        self.app: TestApp | None = None

    def __enter__(self) -> TestApp:
        if not PRISM_JSON_BACKUP.exists():
            PRISM_JSON_BACKUP.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(PRISM_JSON, PRISM_JSON_BACKUP)
        if not check_prism_json():
            raise RuntimeError("prism.json differed from the backup before the run")
        exe = deploy(self.kind)
        env = default_environment(E2E_DIR / "default-env.txt")
        perf_log = self.out / f"perf-{self.tag}.jsonl"
        perf_log.parent.mkdir(parents=True, exist_ok=True)
        perf_log.unlink(missing_ok=True)
        env["PRISM_PERF_LOG"] = str(perf_log)
        start_guardian()
        stop_installed()
        try:
            others = prism_processes()
            if others:
                raise RuntimeError(f"another Prism still runs: {[p.pid for p in others]}")
            pid = wmi_create(f'"{exe}" --autostart', exe.parent, env)
            log(f"started {self.kind} test build pid={pid}")
            self.app = TestApp(self.kind, exe, pid, perf_log)
            self.app.find_palette()
        except BaseException:
            self.__exit__(None, None, None)
            raise
        return self.app

    def __exit__(self, *exc) -> None:
        try:
            if self.app is not None:
                log(f"stop {self.kind} test build pid={self.app.pid}")
                kill_tree(self.app.pid)
                time.sleep(1.0)
        finally:
            start_installed()
            same = check_prism_json()
            log(f"prism.json unchanged: {same}")
