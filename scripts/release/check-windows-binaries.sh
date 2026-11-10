#!/usr/bin/env bash
#
# Checks the Windows release shell before it is published (grafida/grafida#91, ADR 0001):
#
#   PE32+, AMD64 (x86_64 only), the GUI subsystem (no console window of its own);
#   imports only from DLLs that ship with Windows: no Visual C++ runtime (VCRUNTIME*, MSVCP*,
#     api-ms-win-crt-*, ucrtbase: the CRT is static) and no WebView2Loader.dll (linked statically);
#   the application manifest (RT_MANIFEST 1) embedded, with Common Controls 6 and PerMonitorV2;
#   no automation channel in it.
#
# Reads the PE with Python's standard library only, so it runs on any host with python3.
#
# Usage: scripts/release/check-windows-binaries.sh SHELL.exe

set -euo pipefail

SHELL_BIN="$1"
PYTHON=$(command -v python3 || command -v python)

fail() { echo "FAIL: $*" >&2; exit 1; }

echo "== $SHELL_BIN"

"$PYTHON" -I - "$SHELL_BIN" <<'PY' || fail "PE checks"
import struct, sys

data = open(sys.argv[1], "rb").read()
u16 = lambda o: struct.unpack_from("<H", data, o)[0]
u32 = lambda o: struct.unpack_from("<I", data, o)[0]
failures = []

pe = u32(0x3C)
assert data[pe:pe + 4] == b"PE\0\0", "no PE signature"
coff, opt = pe + 4, pe + 24
machine, nsections, optsize = u16(coff), u16(coff + 2), u16(coff + 16)
magic, subsystem = u16(opt), u16(opt + 68)
print(f"machine 0x{machine:04x}, optional header 0x{magic:03x}, subsystem {subsystem}")
if machine != 0x8664: failures.append("not AMD64")
if magic != 0x20B: failures.append("not PE32+")
if subsystem != 2: failures.append(f"subsystem {subsystem}, expected 2 (WINDOWS_GUI)")

sections = []
for i in range(nsections):
    s = opt + optsize + 40 * i
    sections.append((u32(s + 12), max(u32(s + 8), u32(s + 16)), u32(s + 20)))  # va, size, raw ptr
def off(rva):
    for va, size, ptr in sections:
        if va <= rva < va + size:
            return rva - va + ptr
    return None
directory = lambda i: (u32(opt + 112 + 8 * i), u32(opt + 112 + 8 * i + 4))
def cstr(rva):
    o = off(rva)
    return data[o:data.index(b"\0", o)].decode("ascii", "replace")

imports = []
for index, size, name_at in ((1, 20, 12), (13, 32, 4)):  # imports, delay-load imports
    rva, _ = directory(index)
    o = off(rva) if rva else None
    while o is not None and data[o:o + size].strip(b"\0"):
        imports.append(cstr(u32(o + name_at)))
        o += size
print("imports:", ", ".join(sorted(imports, key=str.lower)))
system = {"advapi32", "bcrypt", "bcryptprimitives", "combase", "comctl32", "comdlg32", "crypt32",
          "dbghelp", "dwmapi", "gdi32", "imm32", "kernel32", "ntdll", "ole32", "oleaut32", "propsys",
          "secur32", "setupapi", "shcore", "shell32", "shlwapi", "user32", "userenv", "uxtheme",
          "version", "winmm", "ws2_32"}
for dll in imports:
    name = dll.lower().removesuffix(".dll")
    if name.startswith(("vcruntime", "msvcp", "api-ms-win-crt-", "ucrtbase")):
        failures.append(f"imports {dll}: the Visual C++ runtime (the CRT must be static)")
    elif name == "webview2loader":
        failures.append("imports WebView2Loader.dll: the loader must be linked statically")
    elif not (name in system or name.startswith("api-ms-win-core-")):
        failures.append(f"imports {dll}, which is not on the list of DLLs that ship with Windows")

# The resource tree: type 24 (RT_MANIFEST) → name 1 → its first language → the data.
rva, _ = directory(2)
base = off(rva) if rva else None
def entries(o):
    named, ids = u16(o + 12), u16(o + 14)
    for i in range(named + ids):
        e = o + 16 + 8 * i
        yield u32(e), u32(e + 4)
manifest = None
if base is not None:
    for kind, sub in entries(base):
        if kind == 24 and sub & 0x80000000:
            for name, lang_dir in entries(base + (sub & 0x7FFFFFFF)):
                if name == 1 and lang_dir & 0x80000000:
                    for _, leaf in entries(base + (lang_dir & 0x7FFFFFFF)):
                        data_rva, data_size = u32(base + leaf), u32(base + leaf + 4)
                        manifest = data[off(data_rva):off(data_rva) + data_size].decode("utf-8", "replace")
                        break
if manifest is None:
    failures.append("no application manifest (RT_MANIFEST 1)")
else:
    for needle in ("Microsoft.Windows.Common-Controls", "PerMonitorV2", "8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a"):
        if needle not in manifest:
            failures.append(f"the manifest lacks {needle}")
    print("manifest: Common Controls 6, PerMonitorV2, supportedOS Windows 10/11")

for f in failures:
    print("FAIL:", f, file=sys.stderr)
sys.exit(1 if failures else 0)
PY

# The automation channel's runtime file name exists only in src/automation.rs.
if grep -qa 'automation.json' "$SHELL_BIN"; then
    fail "the shell carries the automation channel"
fi
echo "no automation channel"
echo "ok: $SHELL_BIN"
