#!/usr/bin/env python3
# Grafida Shell — real pointer input and screendumps for the Linux test VMs, over QEMU's QMP socket.
#
# Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
#
# Called by scripts/dev/linuxvm.sh (move, click, drag, screen, screendump); see its help text and
# docs/dev/linux-test-vms.md. Host-side only: nothing here is copied into a guest.
#
#   qmp.py SOCKET screen                                   # the guest's screen size, WxH
#   qmp.py SOCKET move X Y
#   qmp.py SOCKET click X Y [left|right|middle] [MODS]     # MODS: ctrl, shift, alt, altgr, super, joined by -
#   qmp.py SOCKET drag X1 Y1 X2 Y2 [left|right|middle] [MODS]
#   qmp.py SOCKET screendump /abs/OUT.png|OUT.ppm
#
# Pointer events are ABSOLUTE (`input-send-event` abs axes, which reach the guest as quickemu's USB
# tablet), never HMP `mouse_move`: that sends relative motion, which arrives as the PS/2 mouse and
# goes through the desktop's pointer acceleration, so it lands somewhere near the target, not on it.
# An absolute axis spans 0…32767 over the whole screen whatever its resolution, so every command
# first reads the screen's current size from a screendump (the size of QEMU's display surface, the
# one the tablet's range is spread over) and aims at the centre of the requested pixel.

import json
import os
import shutil
import socket
import sys
import tempfile
import time

ABS_MAX = 0x7FFF
BUTTONS = ("left", "right", "middle")
MODIFIERS = {"ctrl": "ctrl", "shift": "shift", "alt": "alt", "altgr": "alt_r", "super": "meta_l",
             "meta": "meta_l"}


class Qmp:
    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX)
        try:
            self.sock.connect(path)
        except OSError as e:
            sys.exit(f"qmp: cannot connect to {path} ({e.strerror}): is the VM running, and was it "
                     "started by a linuxvm.sh that opens the QMP socket? If not, stop it and start it again.")
        self.sock.settimeout(30)
        self.io = self.sock.makefile("rwb")
        self.read()  # the greeting
        self.call("qmp_capabilities")

    def read(self):
        line = self.io.readline()
        if not line:
            sys.exit("qmp: QEMU closed the connection")
        return json.loads(line)

    def call(self, command, **arguments):
        message = {"execute": command}
        if arguments:
            message["arguments"] = arguments
        self.io.write(json.dumps(message).encode() + b"\n")
        self.io.flush()
        while True:
            answer = self.read()  # asynchronous events ({"event": …}) are skipped
            if "error" in answer:
                sys.exit(f"qmp: {command}: {answer['error'].get('desc', answer['error'])}")
            if "return" in answer:
                return answer["return"]

    def events(self, *events):
        self.call("input-send-event", events=list(events))


def screendump(qmp, path):
    fmt = "png" if path.lower().endswith(".png") else "ppm"
    qmp.call("screendump", filename=path, format=fmt)


def screen_size(qmp):
    # A PPM header is "P6 <width> <height> <maxval>", whitespace-separated, comments allowed.
    tmp = tempfile.mkdtemp(prefix="linuxvm-qmp-")
    try:
        path = os.path.join(tmp, "screen.ppm")
        screendump(qmp, path)
        with open(path, "rb") as f:
            head = f.read(256)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    fields = []
    for line in head.split(b"\n"):
        fields += line.split(b"#", 1)[0].split()
        if len(fields) >= 3:
            break
    if len(fields) < 3 or fields[0] != b"P6":
        sys.exit("qmp: the screendump is not a PPM; has the guest's display started?")
    return int(fields[1]), int(fields[2])


class Pointer:
    def __init__(self, qmp):
        self.qmp = qmp
        self.width, self.height = screen_size(qmp)

    def axis(self, pixel, size):
        # The centre of the pixel: the guest maps value * size / 32768 back to a position (libinput),
        # so aiming at a pixel's edge would round into its neighbour.
        return max(0, min(ABS_MAX, int((pixel + 0.5) * (ABS_MAX + 1) / size)))

    def check(self, x, y):
        if not (0 <= x < self.width and 0 <= y < self.height):
            sys.exit(f"qmp: {x},{y} is off the guest's {self.width}x{self.height} screen")

    def move(self, x, y):
        self.check(x, y)
        self.qmp.events({"type": "abs", "data": {"axis": "x", "value": self.axis(x, self.width)}},
                        {"type": "abs", "data": {"axis": "y", "value": self.axis(y, self.height)}})

    def approach(self, x, y):
        # Arrive from a neighbouring pixel, so the compositor and the client see a motion onto the
        # target even when the pointer was already there (enter/hover before the press).
        self.move(x + 2 if x < 2 else x - 2, y + 2 if y < 2 else y - 2)
        time.sleep(0.1)
        self.move(x, y)
        time.sleep(0.2)

    def button(self, name, down):
        self.qmp.events({"type": "btn", "data": {"button": name, "down": down}})

    def modifiers(self, qcodes, down):
        for qcode in (qcodes if down else reversed(qcodes)):
            self.qmp.events({"type": "key", "data": {"down": down, "key": {"type": "qcode", "data": qcode}}})
            time.sleep(0.03)
        if qcodes and down:
            # The keyboard (PS/2) and the tablet (USB) are separate devices: a press sent 30–60 ms
            # after the modifiers sometimes reached the page without them on Ubuntu 22.04, the first after
            # a pause most often. 0.1 s per key never failed; settle before the button goes down.
            time.sleep(0.2)


def options(args):
    """[left|right|middle] [MODS], in either order."""
    button, mods = "left", []
    for arg in args:
        if arg in BUTTONS:
            button = arg
        else:
            for name in arg.lower().split("-"):
                if name not in MODIFIERS:
                    sys.exit(f"qmp: unknown button or modifier '{name}' ({', '.join(BUTTONS)}; "
                             f"{', '.join(MODIFIERS)})")
                mods.append(MODIFIERS[name])
    return button, mods


def numbers(args, count):
    try:
        return [int(a) for a in args[:count]]
    except ValueError:
        sys.exit(f"qmp: expected {count} whole pixel coordinates, got {' '.join(args[:count])}")


def main(argv):
    if len(argv) < 3:
        sys.exit("usage: qmp.py SOCKET screen|move|click|drag|screendump …; see its first lines")
    qmp, cmd, args = Qmp(argv[1]), argv[2], argv[3:]
    if cmd == "screen":
        print("%dx%d" % screen_size(qmp))
    elif cmd == "screendump":
        if len(args) != 1 or not os.path.isabs(args[0]):
            sys.exit("qmp: screendump needs one absolute path")
        screendump(qmp, args[0])
        print(args[0])
    elif cmd == "move" and len(args) == 2:
        Pointer(qmp).move(*numbers(args, 2))
    elif cmd == "click" and 2 <= len(args) <= 4:
        x, y = numbers(args, 2)
        button, mods = options(args[2:])
        p = Pointer(qmp)
        p.check(x, y)
        p.approach(x, y)
        p.modifiers(mods, True)
        p.button(button, True)
        time.sleep(0.08)
        p.button(button, False)
        p.modifiers(mods, False)
    elif cmd == "drag" and 4 <= len(args) <= 6:
        x1, y1, x2, y2 = numbers(args, 4)
        button, mods = options(args[4:])
        p = Pointer(qmp)
        p.check(x1, y1)
        p.check(x2, y2)
        p.approach(x1, y1)
        p.modifiers(mods, True)
        p.button(button, True)
        time.sleep(0.15)
        # Many small motions, as a hand makes: a toolkit starts a drag only past its threshold, and
        # a drop target only learns of the pointer from motion over it.
        steps = max(10, min(60, int(max(abs(x2 - x1), abs(y2 - y1)) / 10)))
        for i in range(1, steps + 1):
            p.move(round(x1 + (x2 - x1) * i / steps), round(y1 + (y2 - y1) * i / steps))
            time.sleep(0.015)
        time.sleep(0.2)
        p.button(button, False)
        p.modifiers(mods, False)
    else:
        sys.exit(f"qmp: bad command line: {' '.join(argv[2:])}")


main(sys.argv)
