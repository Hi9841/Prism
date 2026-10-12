"""Builds the parity fixture from this machine's Prism data (apps, history).

Usage: python -I make-fixture.py <out.json>
The output holds local app names and paths, so it goes under target/ and is
never committed. Both capture runners read the same file.
"""

import json
import os
import sys
from pathlib import Path

appdata = Path(os.environ["APPDATA"]) / "app.prism.launcher"
home = Path(os.environ["USERPROFILE"])

apps = json.loads((appdata / "apps.json").read_text(encoding="utf8"))["apps"]
persisted = json.loads((appdata / "prism.json").read_text(encoding="utf8"))
history = persisted.get("history", [])

# History paths that exist decide which file rows Recent shows.
paths = []
for entry in history:
    for prefix in ("file::d::", "file::f::"):
        if entry["id"].startswith(prefix):
            paths.append(entry["id"][len(prefix):])
existing = [p for p in paths if os.path.exists(p)]

by_name = {a["name"].lower(): a for a in apps}


def app_id(name):
    entry = by_name.get(name.lower())
    return entry["appId"] if entry else None


group_apps = [i for i in (app_id("Notepad"), app_id("Paint"), app_id("Task Manager"), app_id("Snipping Tool")) if i]

settings = {
    "accent": "iris",
    "width": 640,
    "viewZoom": 100,
    "shortcut": "Win",
    "alwaysOnTop": True,
    "taskbarAlignment": "center",
    "taskbarScrollVolume": True,
    "theme": "dark",
    "quickAccess": ["home", "desktop", "downloads", "documents", "pictures", "music"],
    "quickAccessCollapsed": False,
    "pinnedApps": persisted["settings"].get("pinnedApps", [])[:4],
    "appGroups": [{"id": "group-tools", "name": "Tools", "appIds": group_apps, "collapsed": False}] if group_apps else [],
    "sectionOrder": ["pinned", "recent", "quick", "apps"],
}

quick_access = [
    {"name": "Home", "path": str(home), "kind": "home"},
    {"name": "Desktop", "path": str(home / "Desktop"), "kind": "desktop"},
    {"name": "Downloads", "path": str(home / "Downloads"), "kind": "downloads"},
    {"name": "Documents", "path": str(home / "Documents"), "kind": "documents"},
    {"name": "Pictures", "path": str(home / "Pictures"), "kind": "pictures"},
    {"name": "Music", "path": str(home / "Music"), "kind": "music"},
    {"name": "Videos", "path": str(home / "Videos"), "kind": "videos"},
]


def file(name, path, directory=False):
    parent = path.rsplit("\\", 1)[0]
    return {"name": name, "path": path, "parent": parent, "isDirectory": directory}


docs = str(home / "Documents")
files = {
    "report": {
        "items": [
            file("report.pdf", docs + "\\report.pdf"),
            file("report-final.docx", docs + "\\report-final.docx"),
            file("Reports", docs + "\\Reports", True),
            file("report-data.xlsx", docs + "\\report-data.xlsx"),
            file("report.zip", docs + "\\report.zip"),
            file("report-video.mp4", docs + "\\report-video.mp4"),
            file("report-song.mp3", docs + "\\report-song.mp3"),
            file("report-notes.txt", docs + "\\report-notes.txt"),
            file("setup-report.exe", docs + "\\setup-report.exe"),
        ],
        "pathBrowse": False,
    },
}

phase1 = {
    "display": {
        "recents": [],
        "windows": [],
        "apps": [],
        "actions": [
            {"id": "action::settings.display", "kind": "action", "title": "Display", "subtitle": "Windows Settings",
             "score": 900, "actionId": "settings.display", "uri": "ms-settings:display", "iconKey": "display"},
            {"id": "action::settings.nightlight", "kind": "action", "title": "Night light", "subtitle": "Windows Settings",
             "score": 800, "actionId": "settings.nightlight", "uri": "ms-settings:nightlight", "iconKey": "nightlight"},
            {"id": "action::windows.displayswitch", "kind": "action", "title": "Project display", "subtitle": "Windows tool",
             "score": 700, "actionId": "windows.displayswitch", "iconKey": "tool", "source": "windows-tool"},
        ],
    },
    "lock": {
        "recents": [],
        "windows": [],
        "apps": [],
        "actions": [
            {"id": "action::power.lock", "kind": "action", "title": "Lock", "subtitle": "Power", "score": 900,
             "actionId": "power.lock", "iconKey": "lock"},
        ],
    },
}

power_plans = {
    "power": [
        {"guid": "381b4222-f694-41f0-9685-ff5bb260df2e", "name": "Balanced", "active": True, "score": 900, "pickerIntent": True},
        {"guid": "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c", "name": "High performance", "active": False, "score": 800, "pickerIntent": True},
        {"guid": "a1841308-3541-4fab-bc81-f71556f20b4a", "name": "Power saver", "active": False, "score": 700, "pickerIntent": True},
    ],
}

fixture = {
    "version": "0.11.9",
    "systemTheme": "dark",
    "state": {"version": 3, "settings": settings, "history": history},
    "apps": apps,
    "quickAccess": quick_access,
    "existingPaths": existing,
    "thumbnails": {},
    "files": files,
    "fileStatus": {"ready": True, "indexing": False, "volumes": [{"drive": "C:\\", "state": "ready", "indexedCount": 1200000}], "totalIndexed": 1200000},
    "phase1": phase1,
    "powerPlans": power_plans,
    "taskbar": {"thickness": "default", "autoHide": False, "combineButtons": "always", "startIcon": "system",
                "selectedCustomIcon": None, "customStartIcons": []},
    "pinnedToTaskbar": [],
}

out = Path(sys.argv[1])
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text(json.dumps(fixture), encoding="utf8")
print(f"fixture: {len(apps)} apps, {len(history)} history, {len(existing)} existing paths -> {out}")
