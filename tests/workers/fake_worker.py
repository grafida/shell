#!/usr/bin/env python3
# Grafida Shell — a deliberately misbehaving worker for the supervisor tests.
#
# Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
#
# An independent implementation of protocol v1's worker side (it shares no code with either codec
# under test). Usage: fake_worker.py MODE
#
#   echo     a correct worker. Paths: /crash (exit 1 without answering), /sleep?ms=N,
#            /stderr?lines=N (write N numbered lines to stderr, then answer), /pid (answer its pid);
#            anything else echoes the request as JSON, with a repeated `x-multi` response header.
#   page     echo, plus a page at / that POSTs through the scheme, reports what it saw to stderr
#            (`smoke: origin=… post=…`, so it lands in the shell's log) and asks the shell to quit:
#            the release smoke test, which has no automation channel (scripts/release/smoke-linux.sh)
#   silent   never sends `ready`
#   version  announces protocol 2
#   garbage  writes junk instead of `ready`
#   die      writes 50 lines to stderr and exits 3 before `ready`
#   linger   a correct worker that ignores EOF and SIGTERM, so it must be killed
#   deaf     a correct worker that ignores EOF (but not signals): only PR_SET_PDEATHSIG ends it

import hashlib
import json
import os
import signal
import struct
import sys
import time
from urllib.parse import urlsplit, parse_qs

MAGIC = b"GSF1"
out = sys.stdout.buffer
inp = sys.stdin.buffer


def write_frame(header, body=b""):
    h = json.dumps(header, separators=(",", ":")).encode()
    out.write(MAGIC + struct.pack("<I", len(h)) + h + struct.pack("<Q", len(body)) + body)
    out.flush()


def read_exact(n):
    buf = b""
    while len(buf) < n:
        chunk = inp.read(n - len(buf))
        if not chunk:
            return None
        buf += chunk
    return buf


def read_frame():
    magic = read_exact(4)
    if magic is None:
        return None
    assert magic == MAGIC, magic
    (hlen,) = struct.unpack("<I", read_exact(4))
    header = json.loads(read_exact(hlen))
    (blen,) = struct.unpack("<Q", read_exact(8))
    body = read_exact(blen) if blen else b""
    return header, body


def respond(rid, status, payload, extra=None):
    headers = {"content-type": ["application/json"]}
    headers.update(extra or {})
    write_frame({"type": "response", "id": rid, "status": status, "headers": headers},
                json.dumps(payload).encode())


SMOKE_PAGE = b"""<!doctype html><meta charset="utf-8"><title>smoke</title><body>smoke
<script>
addEventListener('load', async () => {
  let post = 'failed';
  try {
    const r = await fetch('/api/echo', {method: 'POST', body: 'hello'});
    const j = await r.json();
    post = r.status + ':' + j.bodyLength;
  } catch (e) { post = 'error:' + e; }
  await fetch('/smoke-report?origin=' + encodeURIComponent(location.origin) + '&post=' + encodeURIComponent(post));
  fetch('/_native/quit', {method: 'POST', body: '{}'});
});
</script>"""


def serve(linger=False, deaf=False, page=False):
    write_frame({"type": "ready", "protocol": 1, "pid": os.getpid(), "app": "fake", "version": "0"})
    while True:
        frame = read_frame()
        if frame is None or frame[0]["type"] == "shutdown":
            if linger:
                signal.signal(signal.SIGTERM, signal.SIG_IGN)
            if linger or deaf:
                time.sleep(30)
            sys.exit(0)
        header, body = frame
        rid = header["id"]
        url = urlsplit(header["url"])
        query = parse_qs(url.query)
        if url.path == "/crash":
            sys.exit(1)
        if url.path == "/sleep":
            time.sleep(int(query["ms"][0]) / 1000)
        if url.path == "/stderr":
            for i in range(int(query["lines"][0])):
                sys.stderr.write(f"line {i + 1}\n")
            sys.stderr.flush()
        if page and url.path == "/":
            write_frame({"type": "response", "id": rid, "status": 200,
                         "headers": {"content-type": ["text/html; charset=utf-8"]}}, SMOKE_PAGE)
            continue
        if page and url.path == "/smoke-report":
            sys.stderr.write("smoke: origin=%s post=%s\n" % (query["origin"][0], query["post"][0]))
            sys.stderr.flush()
        if url.path == "/framed.html":
            # Tells the top window it rendered (tests/apps/echo/framing.js); `csp` is a policy of
            # the worker's own, sent as `content-security-policy`, `xfo` as `x-frame-options`.
            extra = {name: query[key] for key, name in
                     (("csp", "content-security-policy"), ("xfo", "x-frame-options")) if key in query}
            write_frame({"type": "response", "id": rid, "status": 200,
                         "headers": {"content-type": ["text/html; charset=utf-8"], **extra}},
                        b"<!doctype html><meta charset=\"utf-8\"><script>"
                        b"top.postMessage({framed: location.search}, '*')</script>framed")
            continue
        if url.path == "/pid":
            respond(rid, 200, {"pid": os.getpid()})
            continue
        respond(rid, 200, {
            "method": header["method"],
            "url": header["url"],
            "headers": header["headers"],
            "bodyLength": len(body),
            "bodySha256": hashlib.sha256(body).hexdigest(),
            "maxBody": os.environ.get("SHELL_MAX_BODY_BYTES"),
        }, {"x-multi": ["one", "two"]})


def main():
    mode = sys.argv[1]
    if mode == "echo":
        serve()
    elif mode == "page":
        serve(page=True)
    elif mode == "linger":
        serve(linger=True)
    elif mode == "deaf":
        serve(deaf=True)
    elif mode == "silent":
        time.sleep(60)
    elif mode == "version":
        write_frame({"type": "ready", "protocol": 2, "pid": os.getpid()})
        time.sleep(60)
    elif mode == "garbage":
        out.write(b"<?php Warning: something printed\n")
        out.flush()
        time.sleep(60)
    elif mode == "die":
        for i in range(50):
            sys.stderr.write(f"fatal line {i + 1}\n")
        sys.exit(3)
    else:
        sys.exit(f"unknown mode {mode}")


main()
