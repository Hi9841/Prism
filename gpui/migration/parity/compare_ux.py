"""Scores UX parity: per scenario, the recorded backend/window calls must match
and every capture must score >= 9.4 (compare.py's UI score).

Usage: python -I compare_ux.py <orig-dir> <port-dir> <ui-scores.jsonl> <scenarios.json>
Prints one JSON line per scenario and a summary (UX score = 10 x passed / total).
"""

import json
import re
import sys
from pathlib import Path

# Calls with user-visible effects. Reads (get_*, search_files, query_*) are
# timing-dependent polling and are not compared.
ACTIONS = {
    "launch_app", "launch_app_as_admin", "open_path", "open_path_location", "run_path_as_admin",
    "show_path_properties", "set_taskbar_pinned", "execute_action", "set_power_plan", "focus_window",
    "accept_intent", "perform_power_action", "hide_palette", "save_state", "set_shortcut",
    "set_taskbar_alignment", "set_taskbar_thickness", "set_taskbar_auto_hide", "set_taskbar_combine_buttons",
    "set_taskbar_start_icon", "select_custom_start_icon", "remove_custom_start_icon", "set_window_width",
    "plugin:clipboard-manager|write_text", "plugin:window|set_always_on_top", "rebuild_file_index",
    "refresh_apps", "quit_app", "start_file_drag", "set_osd_accent", "set_window_style",
    "plugin:webview|set_webview_zoom",
}


def normalize(calls):
    out = []
    for call in calls:
        cmd = call["cmd"]
        if cmd not in ACTIONS:
            continue  # reads and `#eval` probes
        # JavaScript has one number type: 4242.0 and 4242 are the same value.
        args = json.loads(json.dumps(call.get("args") or {}), parse_float=lambda text: int(float(text)) if float(text).is_integer() else float(text))
        if cmd == "save_state":
            state = json.loads(json.dumps(args.get("state", {})))
            for entry in state.get("history", []):
                entry.pop("ts", None)
            # A new collection's id holds the time and a random part.
            for group in state.get("settings", {}).get("appGroups", []):
                if re.fullmatch(r"group-[0-9a-z]+-[0-9a-z]{5}", group.get("id", "")):
                    group["id"] = "group-new"
            args = {"state": state}
        if cmd == "plugin:clipboard-manager|write_text":
            args = {"text": args.get("text")}
        if cmd == "plugin:window|set_always_on_top":
            args = {"value": args.get("value")}
        if cmd == "plugin:webview|set_webview_zoom":
            # The port applies zoom through the rem size; compare the zoom level via save_state.
            continue
        out.append(json.dumps({"cmd": cmd, "args": args}, sort_keys=True))
    # Animated width steps depend on frame timing: keep the last width only.
    widths = [c for c in out if '"set_window_width"' in c]
    out = [c for c in out if '"set_window_width"' not in c] + widths[-1:]
    # Debounced saves can coalesce differently: compare the last saved state.
    saves = [c for c in out if '"save_state"' in c]
    out = [c for c in out if '"save_state"' not in c] + saves[-1:]
    return sorted(out)


def main():
    orig_dir, port_dir, scores_path, scenarios_path = map(Path, sys.argv[1:5])
    ui = {}
    for line in scores_path.read_text(encoding="utf8").splitlines():
        row = json.loads(line)
        if "capture" in row:
            ui[row["capture"]] = row.get("score", 0)
    scenarios = json.loads(scenarios_path.read_text(encoding="utf8"))["scenarios"]
    passed = 0
    for scenario in scenarios:
        name = scenario["name"]
        captures = [s["capture"] for s in scenario["steps"] if "capture" in s]
        try:
            a = normalize(json.loads((orig_dir / f"{name}.calls.json").read_text(encoding="utf8")))
            b = normalize(json.loads((port_dir / f"{name}.calls.json").read_text(encoding="utf8")))
        except FileNotFoundError as error:
            print(json.dumps({"scenario": name, "pass": False, "error": str(error)}))
            continue
        calls_ok = a == b
        visual = {c: ui.get(c, 0) for c in captures}
        visual_ok = all(score >= 9.4 for score in visual.values())
        ok = calls_ok and visual_ok
        passed += ok
        row = {"scenario": name, "pass": ok, "calls": calls_ok, "visual": visual}
        if not calls_ok:
            row["only_orig"] = [c for c in a if c not in b][:6]
            row["only_port"] = [c for c in b if c not in a][:6]
        print(json.dumps(row))
    print(json.dumps({"summary": True, "scenarios": len(scenarios), "passed": passed, "ux_score": round(10 * passed / len(scenarios), 2)}))


if __name__ == "__main__":
    main()
