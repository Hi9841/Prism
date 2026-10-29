"""Interleaved A/B of launch_e2e.py across two Prism builds.

usage: python launch_ab.py <A exe> <B exe> <blocks> <reps> <out.jsonl> -- <launch_e2e args...>

It stops the running Prism, runs blocks in the order A, B, A, B, ... (each
build started with --autostart, then a 20 s settle), and always restarts the
owner's installed Prism at the end.
"""

import json
import os
import subprocess
import sys
import time

OWNER = os.path.expandvars(r"%LOCALAPPDATA%\Prism\prism.exe")
HERE = os.path.dirname(os.path.abspath(__file__))


def stop():
    subprocess.run(["taskkill", "/f", "/im", "prism.exe"], capture_output=True)
    time.sleep(2)


def start(exe):
    subprocess.Popen([exe, "--autostart"], creationflags=subprocess.DETACHED_PROCESS, close_fds=True)
    time.sleep(20)


def main():
    split = sys.argv.index("--")
    a, b, blocks, reps, out = sys.argv[1:split]
    a, b = os.path.abspath(a), os.path.abspath(b)
    e2e = sys.argv[split + 1 :]
    try:
        for block in range(int(blocks)):
            for name, exe in (("A", a), ("B", b)):
                stop()
                start(exe)
                result = subprocess.run(
                    [sys.executable, os.path.join(HERE, "launch_e2e.py"), e2e[0], e2e[1], reps, *e2e[2:]],
                    capture_output=True,
                    text=True,
                )
                with open(out, "a", encoding="utf-8") as handle:
                    for line in result.stdout.splitlines():
                        record = json.loads(line)
                        record.update(build=name, block=block)
                        handle.write(json.dumps(record) + "\n")
                        print(json.dumps(record), flush=True)
    finally:
        stop()
        start(OWNER)


if __name__ == "__main__":
    main()
