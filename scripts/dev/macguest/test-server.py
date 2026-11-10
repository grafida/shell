#!/usr/bin/env python3
# Grafida Shell — a test server for the clean macOS guests (grafida/grafida#88).
#
# Copyright (c) 2026 Nicholas K. Dionysopoulos. MIT; see LICENSE.
#
#   test-server.py PORT [DMG]
#
# Run on the host (the guests reach it at their gateway, 192.168.64.1) or in a guest:
#   GET  /<basename of DMG> or /Grafida.dmg  the disk image, as a download (a browser in the guest
#                                            fetches it, so it carries the quarantine attribute)
#   GET  …/models                            an OpenAI-style model list ("fake-model")
#   GET  anything else                       {"ok": true, "host": …, "path": …}
#   POST …/chat/completions                  an OpenAI-style reply, streamed word by word with
#                                            "stream": true (the AI panel's path)
#   OPTIONS                                  a CORS preflight; every answer carries CORS headers,
#                                            as a local AI server configured for the app would
# It listens on IPv6 and IPv4 alike: a .local name may resolve to IPv6 only.
import http.server, json, os, sys

PORT = int(sys.argv[1]); DMG = sys.argv[2] if len(sys.argv) > 2 else None

class H(http.server.BaseHTTPRequestHandler):
    def cors(self):
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Headers", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
    def do_OPTIONS(self):
        self.send_response(204); self.cors(); self.end_headers()
    def do_GET(self):
        if DMG and self.path in ("/Grafida.dmg", "/" + os.path.basename(DMG)):
            size = os.path.getsize(DMG)
            self.send_response(200)
            self.send_header("Content-Type", "application/x-apple-diskimage")
            self.send_header("Content-Length", str(size))
            self.send_header("Content-Disposition", 'attachment; filename="' + os.path.basename(DMG) + '"')
            self.end_headers()
            with open(DMG, "rb") as f:
                while chunk := f.read(1 << 20):
                    self.wfile.write(chunk)
            return
        if self.path.rstrip("/").endswith("/models"):
            body = json.dumps({"object": "list", "data": [{"id": "fake-model", "object": "model", "owned_by": "test"}]}).encode()
        else:
            body = json.dumps({"ok": True, "host": self.headers.get("Host"), "path": self.path}).encode()
        self.send_response(200); self.cors()
        self.send_header("Content-Type", "application/json"); self.send_header("Content-Length", str(len(body)))
        self.end_headers(); self.wfile.write(body)
    def do_POST(self):
        # An OpenAI-style chat completion, streamed word by word when asked (the AI panel's path).
        length = int(self.headers.get("Content-Length") or 0)
        req = json.loads(self.rfile.read(length) or b"{}")
        words = ("This reply is streamed from " + (self.headers.get("Host") or "?") + " over plain HTTP, one word at a time, "
                 "through ATS, to show the native shell lets a local model stream.").split(" ")
        if not req.get("stream"):
            body = json.dumps({"id": "x", "object": "chat.completion", "model": req.get("model", "fake-model"),
                               "choices": [{"index": 0, "message": {"role": "assistant", "content": " ".join(words)}, "finish_reason": "stop"}]}).encode()
            self.send_response(200); self.cors(); self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body); return
        self.send_response(200); self.cors(); self.send_header("Content-Type", "text/event-stream"); self.send_header("Cache-Control", "no-store")
        self.end_headers()
        import time
        for i, w in enumerate(words):
            chunk = {"id": "x", "object": "chat.completion.chunk", "model": req.get("model", "fake-model"),
                     "choices": [{"index": 0, "delta": {"content": (w if i == 0 else " " + w)}, "finish_reason": None}]}
            self.wfile.write(b"data: " + json.dumps(chunk).encode() + b"\n\n"); self.wfile.flush(); time.sleep(0.25)
        self.wfile.write(b"data: [DONE]\n\n"); self.wfile.flush()
    def log_message(self, fmt, *args):
        sys.stderr.write("%s %s\n" % (self.address_string(), fmt % args))

import socket

class DualStack(http.server.ThreadingHTTPServer):
    # IPv6 and IPv4 on one socket, as a local AI server listening on every address would: a .local
    # name may resolve to IPv6 only (macOS 27 does), and an IPv4-only server then never answers it.
    address_family = socket.AF_INET6
    def server_bind(self):
        self.socket.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 0)
        super().server_bind()

DualStack(("::", PORT), H).serve_forever()
