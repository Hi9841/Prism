"""Records the file name events below a folder (a read-only watch), and tells how long new files live.

    python -I fsevents.py record <folder> <seconds> <out.jsonl>
    python -I fsevents.py lifetimes <out.jsonl> [--exclude TEXT] [window seconds ...]

`record` watches the folder tree with `ReadDirectoryChangesW` for names only (added, removed,
renamed), as Prism's watcher does, and writes one line for each event: milliseconds since the
start, the action, and the path below the folder. It changes nothing.

`lifetimes` reads a recording. An entry that is added and removed again inside a flush window
costs the catalog no write when the watcher folds the window's events. For each window it prints
how many of the added entries are gone again before the window ends, and the catalog writes
(additions and removals that stay) for that window against a 0.75 s window. `--exclude` drops the
events whose path contains the text (case does not count), for example a bench's own test tree.
"""

from __future__ import annotations

import ctypes as C
import ctypes.wintypes as W
import json
import sys
import time

FILE_LIST_DIRECTORY, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS = 0x1, 3, 0x02000000
SHARE_ALL = 0x7
NOTIFY_NAMES = 0x1 | 0x2  # FILE_NOTIFY_CHANGE_FILE_NAME | FILE_NOTIFY_CHANGE_DIR_NAME
ACTIONS = {1: "added", 2: "removed", 4: "renamed_old", 5: "renamed_new"}
BUFFER = 512 * 1024


def record(folder: str, seconds: float, out_path: str) -> None:
    kernel32 = C.WinDLL("kernel32", use_last_error=True)
    kernel32.CreateFileW.restype = W.HANDLE
    kernel32.CreateFileW.argtypes = [W.LPCWSTR, W.DWORD, W.DWORD, W.LPVOID, W.DWORD, W.DWORD, W.HANDLE]
    kernel32.ReadDirectoryChangesW.argtypes = [W.HANDLE, W.LPVOID, W.DWORD, W.BOOL, W.DWORD, C.POINTER(W.DWORD),
                                               W.LPVOID, W.LPVOID]
    handle = kernel32.CreateFileW(folder, FILE_LIST_DIRECTORY, SHARE_ALL, None, OPEN_EXISTING,
                                  FILE_FLAG_BACKUP_SEMANTICS, None)
    if handle in (None, W.HANDLE(-1).value):
        raise SystemExit(f"cannot open {folder}: error {C.get_last_error()}")
    buffer = C.create_string_buffer(BUFFER)
    started = time.perf_counter()
    events = overflows = 0
    # The read blocks until an event comes: the recording ends with the first event after the time.
    with open(out_path, "w", encoding="utf-8") as out:
        while time.perf_counter() - started < seconds:
            returned = W.DWORD()
            ok = kernel32.ReadDirectoryChangesW(handle, buffer, BUFFER, True, NOTIFY_NAMES, C.byref(returned), None, None)
            now = round((time.perf_counter() - started) * 1000, 1)
            if not ok or returned.value == 0:
                overflows += 1
                out.write(json.dumps({"t": now, "a": "overflow"}) + "\n")
                continue
            offset = 0
            data = buffer.raw[:returned.value]
            while True:
                next_offset, action, length = (int.from_bytes(data[offset + i:offset + i + 4], "little") for i in (0, 4, 8))
                name = data[offset + 12:offset + 12 + length].decode("utf-16-le", "replace")
                out.write(json.dumps({"t": now, "a": ACTIONS.get(action, str(action)), "p": name}) + "\n")
                events += 1
                if next_offset == 0:
                    break
                offset += next_offset
    kernel32.CloseHandle(handle)
    print(f"{events} events, {overflows} overflows in {time.perf_counter() - started:.0f} s -> {out_path}")


def lifetimes(path: str, windows: list[float], exclude: str | None = None) -> None:
    rows = [json.loads(line) for line in open(path, encoding="utf-8")]
    if exclude:
        rows = [row for row in rows if exclude.lower() not in row.get("p", "").lower()]
    events = [row for row in rows if row["a"] != "overflow"]
    duration = max((row["t"] for row in rows), default=0.0) / 1000
    added = sum(1 for row in events if row["a"] == "added")
    removed = sum(1 for row in events if row["a"] == "removed")
    renamed = sum(1 for row in events if row["a"] == "renamed_new")
    print(f"{len(events)} events in {duration:.0f} s: {added} added, {removed} removed, {renamed} renamed, "
          f"{sum(1 for row in rows if row['a'] == 'overflow')} overflows")
    born: dict[str, float] = {}
    lives: list[float] = []
    for row in events:
        if row["a"] == "added":
            born[row["p"]] = row["t"]
        elif row["a"] == "removed" and row["p"] in born:
            lives.append((row["t"] - born.pop(row["p"])) / 1000)
    lives.sort()
    print(f"added entries that are removed again in the recording: {len(lives)} of {added}")
    for limit in (0.05, 0.25, 0.75, 2, 5, 10, 30, 60):
        gone = sum(1 for life in lives if life <= limit)
        print(f"  gone within {limit:>5} s: {gone:7} ({gone / max(added, 1) * 100:5.1f}% of the added entries)")

    def writes(window_s: float) -> int:
        """Additions and removals that a watcher with this flush window applies (renames are not folded)."""
        total = 0
        pending: dict[str, str] = {}
        edge = window_s * 1000
        for row in events:
            while row["t"] >= edge:
                total += len(pending)
                pending.clear()
                edge += window_s * 1000
            path, action = row.get("p", ""), row["a"]
            if action == "added":
                pending[path] = "added"
            elif action == "removed":
                # Added and removed in one window: no row was written, so nothing is removed.
                if pending.pop(path, None) != "added":
                    pending[path] = "removed"
        return total + len(pending)

    base = writes(0.75)
    print(f"catalog row writes with a 0.75 s window: {base}")
    for window in windows:
        count = writes(window)
        print(f"  with a {window:g} s window: {count} ({(count - base) / max(base, 1) * 100:+.1f}%)")


def main() -> None:
    if len(sys.argv) >= 5 and sys.argv[1] == "record":
        record(sys.argv[2], float(sys.argv[3]), sys.argv[4])
    elif len(sys.argv) >= 3 and sys.argv[1] == "lifetimes":
        rest = sys.argv[3:]
        exclude = None
        if "--exclude" in rest:
            index = rest.index("--exclude")
            exclude = rest[index + 1]
            del rest[index:index + 2]
        lifetimes(sys.argv[2], [float(value) for value in rest] or [2, 5, 10, 30, 60], exclude)
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main()
