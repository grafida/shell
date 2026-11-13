#!/usr/bin/env python3
# Grafida Shell — conformance fixture writer for the shell <-> worker wire protocol.
#
# Copyright (c) 2026 Nicholas K. Dionysopoulos
# MIT; see LICENSE
#
# This is the THIRD, independent writer of protocol frames: the fixtures must not be the output of
# either codec under test (src/protocol.rs, grafida/shell-php), or a bug shared by a codec and its
# fixtures would pass unnoticed. Frames are assembled byte by byte here, and malformed headers are
# written as literal JSON text, not through a serialiser.
#
# It ONLY ADDS fixtures. An existing NAME.bin / NAME.json is never overwritten, and an existing
# SHA256SUMS line that no longer matches its file is an error, not something to update: a failing
# fixture is a question about which side is wrong, never a file to regenerate (protocol/v1.md §8).
#
# Usage: scripts/protocol/make-fixtures.py            (from the repository root)

import base64
import hashlib
import json
import os
import struct
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
OUT = os.path.join(ROOT, "protocol", "fixtures")
SUMS = os.path.join(OUT, "SHA256SUMS")

MAGIC = b"GSF1"
MAX_HEADER = 1 << 20
MAX_BODY = 128 << 20
MAX_ID = (1 << 53) - 1
INLINE_BODY = 4096  # bodies up to this size are spelled out in the .json as text or base64

FIXTURES = []  # (name, description, bytes, expected_frames, end)


def u32(n):
    return struct.pack("<I", n)


def u64(n):
    return struct.pack("<Q", n)


def frame(header, body=b"", body_len=None):
    """One frame. `header` is raw bytes or a str of literal JSON; `body_len` overrides B."""
    raw = header.encode("utf-8") if isinstance(header, str) else header
    return MAGIC + u32(len(raw)) + raw + u64(len(body) if body_len is None else body_len) + body


def j(obj):
    """Compact JSON with raw UTF-8 (only for well-formed headers)."""
    return json.dumps(obj, ensure_ascii=False, separators=(",", ":"))


def body_meta(body):
    meta = {"length": len(body), "sha256": hashlib.sha256(body).hexdigest()}
    if len(body) <= INLINE_BODY:
        try:
            text = body.decode("utf-8")
            if all(c in "\t\n\r" or ord(c) >= 0x20 for c in text):
                meta["text"] = text
                return meta
        except UnicodeDecodeError:
            pass
        meta["base64"] = base64.b64encode(body).decode("ascii")
    return meta


def expect(header, body=b""):
    return {"header": header, "body": body_meta(body)}


def add(name, description, data, frames, end="eof"):
    FIXTURES.append((name, description, data, frames, end))


def ok(name, description, header, body=b""):
    """A single well-formed frame whose normal form is `header` itself."""
    add(name, description, frame(j(header), body), [expect(header, body)])


def bad(name, description, data, code, frames=()):
    add(name, description, data, list(frames), code)


# --- Valid single frames ---------------------------------------------------------------------

READY = {"type": "ready", "protocol": 1, "pid": 4242, "app": "grafida", "version": "1.4.0"}
ok("ready", "A ready frame with every member.", READY)
ok("ready-minimal", "A ready frame with only the required members; optional ones stay absent.",
   {"type": "ready", "protocol": 1, "pid": 0})

ok("request-get-empty-body", "A GET with an empty body and one header.",
   {"type": "request", "id": 1, "method": "GET", "url": "shell://app/api/bootstrap",
    "headers": {"accept": ["application/json"]}})

ok("request-post-json", "A POST with a JSON body.",
   {"type": "request", "id": 17, "method": "POST", "url": "shell://app/api/drafts/3",
    "headers": {"content-type": ["application/json"]}},
   '{"title":"Καλημέρα","state":1}'.encode("utf-8"))

ok("request-no-headers", "An empty header map is valid.",
   {"type": "request", "id": 2, "method": "DELETE", "url": "shell://app/api/drafts/9", "headers": {}})

BINARY = bytes(range(256)) + MAGIC + b"\xff\xfe\x00\xc3\x28" + frame(j({"type": "shutdown"}))
ok("request-binary-body",
   "A body with every byte value, invalid UTF-8, and bytes that look like a frame is opaque data.",
   {"type": "request", "id": 3, "method": "POST", "url": "shell://app/api/sites/1/media",
    "headers": {"content-type": ["application/octet-stream"]}}, BINARY)

LARGE = bytes(i % 251 for i in range((1 << 20) + 1))
ok("request-large-body",
   "A 1 MiB + 1 body, crossing every pipe and stream buffer size, arrives intact.",
   {"type": "request", "id": 4, "method": "POST", "url": "shell://app/api/sites/1/media/files",
    "headers": {"content-type": ["application/json"]}}, LARGE)

ok("request-repeated-headers",
   "Repeated headers keep every value, in order (the reason values are arrays).",
   {"type": "request", "id": 5, "method": "GET", "url": "shell://app/",
    "headers": {"accept": ["text/html", "application/json"], "x-multi": ["one", "two", "three"],
                "cookie": ["a=1", "b=2"]}})

ok("request-header-values-edge",
   "An empty string is a valid header value, and non-ASCII values pass through unchanged.",
   {"type": "request", "id": 6, "method": "GET", "url": "shell://app/",
    "headers": {"x-empty": [""], "x-greek": ["Ελληνικά"], "x-token-chars": ["!#$%&'*+-.^_`|~"]}})

ok("request-header-name-token-chars", "Every tchar is allowed in a lower-case header name.",
   {"type": "request", "id": 7, "method": "GET", "url": "shell://app/",
    "headers": {"x-a!#$%&'*+-.^_`|~0123456789": ["1"]}})

ok("request-method-case-preserved", "The method is a token whose case is preserved, not normalised.",
   {"type": "request", "id": 8, "method": "patch", "url": "shell://app/api/drafts/1", "headers": {}})

NON_ASCII_URL = "shell://app/api/help/page/Ελληνικά?q=καλημέρα&emoji=🎉"
ok("request-non-ascii-url-raw", "A URL with raw non-ASCII, written as raw UTF-8, is kept verbatim.",
   {"type": "request", "id": 9, "method": "GET", "url": NON_ASCII_URL, "headers": {}})

add("request-non-ascii-url-escaped",
    "The same URL written with \\u escapes (a surrogate pair included) decodes to the same string.",
    frame(json.dumps({"type": "request", "id": 9, "method": "GET", "url": NON_ASCII_URL,
                      "headers": {}}, ensure_ascii=True, separators=(",", ":"))),
    [expect({"type": "request", "id": 9, "method": "GET", "url": NON_ASCII_URL, "headers": {}})])

ok("request-percent-encoded-url",
   "Percent escapes are not decoded by the codec; the URL is passed through verbatim.",
   {"type": "request", "id": 10, "method": "GET",
    "url": "shell://app/api/sites/1/media?path=a%20b%2Fc&x=%E2%82%AC+%25", "headers": {}})

ok("request-id-zero", "Id 0 is valid.",
   {"type": "request", "id": 0, "method": "GET", "url": "shell://app/", "headers": {}})
ok("request-id-max", "Id 2^53 - 1 is the largest valid id.",
   {"type": "request", "id": MAX_ID, "method": "GET", "url": "shell://app/", "headers": {}})

add("request-unknown-members",
    "Unknown members, nested ones included, are ignored and left out of the normal form.",
    frame('{"x-future":{"nested":[1,2,{"a":null}]},"type":"request","priority":"high",'
          '"id":11,"method":"GET","url":"shell://app/","headers":{},"trace":true}'),
    [expect({"type": "request", "id": 11, "method": "GET", "url": "shell://app/", "headers": {}})])

add("header-whitespace",
    "Insignificant JSON whitespace, leading and trailing included, is allowed.",
    frame(' \n{ "type" : "shutdown" }\r\n\t '),
    [expect({"type": "shutdown"})])

PAD_PREFIX = '{"type":"request","id":12,"method":"GET","url":"shell://app/","headers":{},"pad":"'
PAD_SUFFIX = '"}'
PAD = "x" * (MAX_HEADER - len(PAD_PREFIX) - len(PAD_SUFFIX))
add("request-header-at-size-limit", "A header of exactly 1 MiB is within the limit.",
    frame(PAD_PREFIX + PAD + PAD_SUFFIX),
    [expect({"type": "request", "id": 12, "method": "GET", "url": "shell://app/", "headers": {}})])

RESPONSE = {"type": "response", "id": 17, "status": 200,
            "headers": {"content-type": ["application/json; charset=utf-8"],
                        "cache-control": ["no-store, no-cache, must-revalidate"]}}
ok("response-json", "A 200 response with a JSON body.", RESPONSE,
   b'{"ok":true,"data":{"title":"\xce\x9a\xce\xb1\xce\xbb\xce\xb7\xce\xbc\xce\xad\xcf\x81\xce\xb1"}}')

ok("response-repeated-headers", "A response keeps repeated headers, e.g. two Set-Cookie lines.",
   {"type": "response", "id": 18, "status": 200,
    "headers": {"set-cookie": ["a=1; Path=/", "b=2; Path=/"], "x-multi": ["one", "two"]}})

ok("response-empty-body", "A 204 with no body.",
   {"type": "response", "id": 19, "status": 204, "headers": {}})

PNG = bytes.fromhex("89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c489"
                    "0000000d4944415478da63f8cfc0f01f0005000201ffa9f2b8460000000049454e44ae426082")
ok("response-binary-body", "A binary response body (a 1x1 PNG) is opaque data.",
   {"type": "response", "id": 20, "status": 200, "headers": {"content-type": ["image/png"]}}, PNG)

add("response-status-bounds", "Statuses 100 and 599 are the inclusive bounds.",
    frame(j({"type": "response", "id": 21, "status": 100, "headers": {}}))
    + frame(j({"type": "response", "id": 22, "status": 599, "headers": {}})),
    [expect({"type": "response", "id": 21, "status": 100, "headers": {}}),
     expect({"type": "response", "id": 22, "status": 599, "headers": {}})])

ok("shutdown", "A shutdown frame.", {"type": "shutdown"})

add("empty-stream", "Zero bytes is a clean end of stream with no frames.", b"", [])

# --- Valid streams ---------------------------------------------------------------------------

S1 = {"type": "request", "id": 1, "method": "GET", "url": "shell://app/api/bootstrap", "headers": {}}
S2 = {"type": "request", "id": 2, "method": "POST", "url": "shell://app/api/settings/language",
      "headers": {"content-type": ["application/json"]}}
S2_BODY = b'{"language":"el-GR"}'
add("stream-shell-to-worker", "What a shell writes: two requests, then shutdown.",
    frame(j(S1)) + frame(j(S2), S2_BODY) + frame(j({"type": "shutdown"})),
    [expect(S1), expect(S2, S2_BODY), expect({"type": "shutdown"})])

R1 = {"type": "response", "id": 1, "status": 200, "headers": {"content-type": ["application/json"]}}
R1_BODY = b'{"ok":true}'
R2 = {"type": "response", "id": 2, "status": 404, "headers": {"x-multi": ["one", "two"]}}
add("stream-worker-to-shell", "What a worker writes: ready, then two responses.",
    frame(j(READY)) + frame(j(R1), R1_BODY) + frame(j(R2)),
    [expect(READY), expect(R1, R1_BODY), expect(R2)])

# --- Truncation: EOF inside a frame is an error, never a wait -------------------------------

SHUT = frame(j({"type": "shutdown"}))
REQ = frame(j({"type": "request", "id": 1, "method": "POST", "url": "shell://app/x", "headers": {}}),
            b"0123456789")
bad("error-truncated-magic", "EOF after 2 of the 4 magic bytes.", b"GS", "truncated")
bad("error-truncated-header-length", "EOF inside the header length.", MAGIC + b"\x10\x00", "truncated")
bad("error-truncated-header", "EOF inside the header.", MAGIC + u32(100) + b'{"type":"shu', "truncated")
bad("error-truncated-body-length", "EOF inside the body length.", SHUT[:-5], "truncated")
bad("error-truncated-body", "EOF after 3 of 10 body bytes.", REQ[:-7], "truncated")
bad("error-truncated-second-frame", "A complete frame, then EOF inside the next one.",
    REQ + REQ[:10], "truncated",
    [expect({"type": "request", "id": 1, "method": "POST", "url": "shell://app/x", "headers": {}},
            b"0123456789")])

# --- Lost sync: stop at once, never scan for the next magic ----------------------------------

bad("error-bad-magic-text", "A stream that starts with text is not a frame stream.",
    b"Hello from a stray echo\n" + SHUT, "bad_magic")
bad("error-bad-magic-version", "GSF2 is not GSF1.", b"GSF2" + SHUT[4:], "bad_magic")
bad("error-bad-magic-case", "The magic is case-sensitive.", b"gsf1" + SHUT[4:], "bad_magic")
bad("error-stray-output-after-frame",
    "A PHP warning on stdout after a good frame breaks the stream; the good frame still counts.",
    SHUT + b"PHP Warning:  Undefined variable $x in /app/src/X.php on line 3\n" + SHUT,
    "bad_magic", [expect({"type": "shutdown"})])

# --- Lengths: refused before anything is read or allocated -----------------------------------

bad("error-header-too-large", "H = 1 MiB + 1, with nothing after it.",
    MAGIC + u32(MAX_HEADER + 1), "header_too_large")
bad("error-header-length-u32-max", "H = 2^32 - 1.", MAGIC + u32(0xFFFFFFFF), "header_too_large")
RQH = j({"type": "request", "id": 1, "method": "POST", "url": "shell://app/x", "headers": {}})
bad("error-body-too-large", "B = 128 MiB + 1, with no body bytes after it.",
    frame(RQH, body_len=MAX_BODY + 1), "body_too_large")
bad("error-body-length-u64-max", "B = 2^64 - 1 (negative if read as a signed 64-bit integer).",
    frame(RQH, body_len=(1 << 64) - 1), "body_too_large")
bad("error-body-length-sign-bit", "B = 2^63, the smallest length that is negative as an i64.",
    frame(RQH, body_len=1 << 63), "body_too_large")

# --- JSON ------------------------------------------------------------------------------------

bad("error-header-empty", "H = 0: an empty header is not JSON.", frame(""), "bad_json")
bad("error-header-not-json", "A header that is not JSON.", frame("{type: shutdown}"), "bad_json")
bad("error-header-trailing-garbage", "Trailing non-whitespace after the JSON value.",
    frame('{"type":"shutdown"} x'), "bad_json")
bad("error-header-invalid-utf8", "Invalid UTF-8 inside a JSON string.",
    frame(b'{"type":"request","id":1,"method":"GET","url":"shell://app/\xc3\x28","headers":{}}'),
    "bad_json")
bad("error-header-lone-surrogate", "An unpaired UTF-16 surrogate escape is not a Unicode string.",
    frame('{"type":"request","id":1,"method":"GET","url":"shell://app/\\ud800","headers":{}}'),
    "bad_json")
bad("error-bad-json-before-body",
    "A malformed header is reported as soon as it has arrived, before any body length is read.",
    MAGIC + u32(5) + b"{oops", "bad_json")

# --- Header shape ----------------------------------------------------------------------------

bad("error-header-not-object", "A JSON array is not a header.", frame("[1,2]"), "bad_header")
bad("error-header-string", "A JSON string is not a header.", frame('"request"'), "bad_header")
bad("error-type-missing", "A header without a type.", frame('{"id":1}'), "bad_header")
bad("error-type-not-string", "A type that is not a string.", frame('{"type":1}'), "bad_header")
bad("error-unknown-type", "A type v1 does not define.", frame('{"type":"ping"}'), "unknown_type")
bad("error-unknown-type-before-members",
    "An unknown type is reported before any member is looked at.",
    frame('{"type":"Request","id":"x"}'), "unknown_type")

bad("error-ready-protocol-missing", "ready without protocol.",
    frame('{"type":"ready","pid":1}'), "bad_header")
bad("error-ready-protocol-zero", "protocol must be at least 1.",
    frame('{"type":"ready","protocol":0,"pid":1}'), "bad_header")
bad("error-ready-pid-missing", "ready without pid.", frame('{"type":"ready","protocol":1}'), "bad_header")
bad("error-ready-pid-negative", "pid must not be negative.",
    frame('{"type":"ready","protocol":1,"pid":-1}'), "bad_header")
bad("error-ready-app-not-string", "app, when present, must be a string.",
    frame('{"type":"ready","protocol":1,"pid":1,"app":null}'), "bad_header")
bad("error-ready-version-not-string", "version, when present, must be a string.",
    frame('{"type":"ready","protocol":1,"pid":1,"version":1}'), "bad_header")
bad("error-ready-with-body", "ready must not carry a body.",
    frame('{"type":"ready","protocol":1,"pid":1}', b"abc"), "bad_header")
bad("error-shutdown-huge-body",
    "A body where none is allowed is bad_header, checked before the body limit.",
    frame('{"type":"shutdown"}', body_len=(1 << 64) - 1), "bad_header")


def req(**overrides):
    base = {"type": "request", "id": 1, "method": "GET", "url": "shell://app/", "headers": {}}
    base.update(overrides)
    return base


def without(d, key):
    return {k: v for k, v in d.items() if k != key}


bad("error-request-id-missing", "request without id.", frame(j(without(req(), "id"))), "bad_header")
bad("error-request-id-negative", "A negative id.", frame(j(req(id=-1))), "bad_header")
bad("error-request-id-too-large", "Id 2^53 is beyond the JSON-safe range.",
    frame(j(req(id=MAX_ID + 1))), "bad_header")
bad("error-request-id-u64-max", "Id 2^64 - 1 is a u64 but beyond the JSON-safe range.",
    frame(j(req(id=(1 << 64) - 1))), "bad_header")
bad("error-request-id-beyond-u64", "Id 2^64 is not even a u64.", frame(j(req(id=1 << 64))), "bad_header")
bad("error-request-id-float", "17.0 is not an integer on the wire.",
    frame('{"type":"request","id":17.0,"method":"GET","url":"shell://app/","headers":{}}'), "bad_header")
bad("error-request-id-exponent", "1e2 is not an integer on the wire.",
    frame('{"type":"request","id":1e2,"method":"GET","url":"shell://app/","headers":{}}'), "bad_header")
bad("error-request-id-string", "An id written as a string.", frame(j(req(id="17"))), "bad_header")
bad("error-request-method-missing", "request without method.",
    frame(j(without(req(), "method"))), "bad_header")
bad("error-request-method-empty", "An empty method.", frame(j(req(method=""))), "bad_header")
bad("error-request-method-not-token", "A method containing a space.",
    frame(j(req(method="GET /"))), "bad_header")
bad("error-request-url-missing", "request without url.", frame(j(without(req(), "url"))), "bad_header")
bad("error-request-url-empty", "An empty url.", frame(j(req(url=""))), "bad_header")
bad("error-request-url-not-string", "A url that is not a string.", frame(j(req(url=None))), "bad_header")
bad("error-request-headers-missing", "request without headers.",
    frame(j(without(req(), "headers"))), "bad_header")
bad("error-request-headers-array", "headers must be an object; an empty array is not one.",
    frame(j(req(headers=[]))), "bad_header")
bad("error-request-header-value-not-array",
    "A header value must be an array, even for a single value (arrays end to end).",
    frame(j(req(headers={"content-type": "application/json"}))), "bad_header")
bad("error-request-header-empty-array", "An empty value array is not a header.",
    frame(j(req(headers={"x-a": []}))), "bad_header")
bad("error-request-header-value-not-string", "Header values must be strings.",
    frame(j(req(headers={"x-a": [1]}))), "bad_header")
bad("error-request-header-name-uppercase", "Header names are lower-case; receivers reject, not fold.",
    frame(j(req(headers={"Content-Type": ["text/html"]}))), "bad_header")
bad("error-request-header-name-empty", "An empty header name.", frame(j(req(headers={"": ["x"]}))), "bad_header")
bad("error-request-header-name-space", "A header name with a space.",
    frame(j(req(headers={"x y": ["1"]}))), "bad_header")
bad("error-request-header-name-non-ascii", "A header name with a non-ASCII letter.",
    frame(j(req(headers={"x-é": ["1"]}))), "bad_header")
bad("error-request-header-value-crlf", "CR LF in a value would smuggle a second header.",
    frame(j(req(headers={"x-a": ["1\r\nx-injected: 1"]}))), "bad_header")
bad("error-request-header-value-lf", "A bare LF in a value.", frame(j(req(headers={"x-a": ["1\n2"]}))),
    "bad_header")
bad("error-request-header-value-nul", "A NUL in a value.", frame(j(req(headers={"x-a": ["1\u00002"]}))),
    "bad_header")


def resp(**overrides):
    base = {"type": "response", "id": 1, "status": 200, "headers": {}}
    base.update(overrides)
    return base


bad("error-response-id-missing", "response without id.", frame(j(without(resp(), "id"))), "bad_header")
bad("error-response-status-missing", "response without status.",
    frame(j(without(resp(), "status"))), "bad_header")
bad("error-response-status-99", "Status 99 is below the range.", frame(j(resp(status=99))), "bad_header")
bad("error-response-status-600", "Status 600 is above the range.", frame(j(resp(status=600))), "bad_header")
bad("error-response-status-string", "A status written as a string.",
    frame(j(resp(status="200"))), "bad_header")
bad("error-response-status-float", "200.0 is not an integer on the wire.",
    frame('{"type":"response","id":1,"status":200.0,"headers":{}}'), "bad_header")
bad("error-response-headers-missing", "response without headers.",
    frame(j(without(resp(), "headers"))), "bad_header")
bad("error-response-header-value-not-array", "Response header values are arrays too.",
    frame(j(resp(headers={"cache-control": "no-store"}))), "bad_header")


# --- Writing -------------------------------------------------------------------------------------

def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main():
    os.makedirs(OUT, exist_ok=True)

    sums = {}
    if os.path.exists(SUMS):
        with open(SUMS, encoding="ascii") as f:
            for line in f:
                digest, name = line.rstrip("\n").split("  ", 1)
                sums[name] = digest

    for name, digest in sums.items():
        path = os.path.join(OUT, name)
        if not os.path.exists(path) or sha256_file(path) != digest:
            sys.exit(f"{name}: changed or missing since it was listed in SHA256SUMS. "
                     "Fixtures are never regenerated; see protocol/v1.md §8.")

    names = [f[0] for f in FIXTURES]
    if len(names) != len(set(names)):
        sys.exit("duplicate fixture name")

    added = 0
    for name, description, data, frames, end in FIXTURES:
        meaning = {"description": description, "frames": frames, "end": end}
        for ext, content in ((".bin", data),
                             (".json", (json.dumps(meaning, ensure_ascii=False, indent=2) + "\n")
                              .encode("utf-8"))):
            path = os.path.join(OUT, name + ext)
            if os.path.exists(path):
                continue
            with open(path, "wb") as f:
                f.write(content)
            sums[name + ext] = sha256_file(path)
            added += 1

    with open(SUMS, "w", encoding="ascii", newline="\n") as f:
        for name in sorted(sums):
            f.write(f"{sums[name]}  {name}\n")

    print(f"{len(FIXTURES)} fixtures defined, {added} files added")


if __name__ == "__main__":
    main()
