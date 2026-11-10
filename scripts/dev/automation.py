#!/usr/bin/env python3
# Grafida Shell — client for the debug-only automation channel (src/automation.rs).
#
# Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
#
# The shell must run with --automation (a debug build with the `automation` feature):
#   cargo run --features automation -- --dev ../grafida --automation
#
# Usage (APP_ID defaults to app.grafida, or set SHELL_APP_ID):
#   automation.py eval 'return location.origin'
#   automation.py eval-file script.js            # an async function body read from a file
#   automation.py screenshot /abs/path/shot.png
#   automation.py pick /abs/a.png [/abs/b.png …] # the next native dialog answers this
#   automation.py pick --cancel                  # the next native dialog is cancelled
#   automation.py opened                         # URLs recorded instead of opened in the browser
#   automation.py drop X Y /abs/a.png […]         # as if those files were dropped at X,Y
#   automation.py wait 'return document.readyState === "complete"' [timeout_s]
#   automation.py menu                           # macOS: the live menu bar, key equivalents included
#   automation.py menu "About Grafida"           # macOS: click that menu item
#   automation.py key cmd+shift+v                # macOS: a key press, queued as a real one would be
#                                                # (activates the app: it takes the keyboard focus)
#
# Prints the JSON answer; exits 1 when the answer is not ok.

import json
import os
import socket
import sys
import tempfile
import time


def channel_file(app_id):
    # Where the shell writes it: $XDG_RUNTIME_DIR, else the temp dir ($TMPDIR on macOS, which is
    # per user and not /tmp).
    base = os.environ.get("XDG_RUNTIME_DIR") or tempfile.gettempdir()
    return os.path.join(base, f"{app_id}.automation.json")


def send(command, timeout=130):
    app_id = os.environ.get("SHELL_APP_ID", "app.grafida")
    with open(channel_file(app_id)) as f:
        info = json.load(f)
    command["token"] = info["token"]
    with socket.create_connection(("127.0.0.1", info["port"]), timeout=timeout) as s:
        s.sendall((json.dumps(command) + "\n").encode())
        buf = b""
        while not buf.endswith(b"\n"):
            chunk = s.recv(65536)
            if not chunk:
                break
            buf += chunk
    if not buf:
        return {"ok": False, "error": "no answer: the connection closed (did the shell exit?)"}
    return json.loads(buf)


def main(argv):
    if len(argv) < 2:
        sys.exit(__doc__ or "see the header of this file")
    cmd, args = argv[1], argv[2:]
    if cmd == "eval":
        answer = send({"cmd": "eval", "js": args[0]})
    elif cmd == "eval-file":
        with open(args[0]) as f:
            answer = send({"cmd": "eval", "js": f.read()})
    elif cmd == "screenshot":
        answer = send({"cmd": "screenshot", "path": os.path.abspath(args[0])})
    elif cmd == "pick":
        paths = None if args == ["--cancel"] else [os.path.abspath(a) for a in args]
        answer = send({"cmd": "pick", "paths": paths})
    elif cmd == "drop":
        answer = send({"cmd": "drop", "x": int(args[0]), "y": int(args[1]),
                       "paths": [os.path.abspath(a) for a in args[2:]]})
    elif cmd == "menu":
        answer = send({"cmd": "menu", "perform": args[0]} if args else {"cmd": "menu"})
    elif cmd == "key":
        *mods, key = args[0].split("+")
        answer = send({"cmd": "key", "key": key, "modifiers": mods})
    elif cmd == "opened":
        answer = send({"cmd": "opened"})
    elif cmd == "wait":
        deadline = time.time() + (float(args[1]) if len(args) > 1 else 30)
        while True:
            try:
                answer = send({"cmd": "eval", "js": args[0]})
                if answer.get("ok") and answer.get("result"):
                    break
            except OSError as e:
                answer = {"ok": False, "error": str(e)}
            if time.time() > deadline:
                answer = {"ok": False, "error": "timeout", "last": answer}
                break
            time.sleep(0.25)
    else:
        sys.exit(f"unknown command {cmd}")
    print(json.dumps(answer, indent=2, ensure_ascii=False))
    sys.exit(0 if answer.get("ok") else 1)


main(sys.argv)
