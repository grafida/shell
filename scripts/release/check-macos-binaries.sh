#!/usr/bin/env bash
#
# Checks a macOS release shell before it is published (grafida/grafida#88, #92, ADR 0001):
#
#   a thin Mach-O of EXPECT_ARCH (arm64, the default, or x86_64 for Intel Macs);
#   LC_BUILD_VERSION minos at most MACOS_MIN (12.0, the floor), read from the binary rather than
#     assumed, because the SDK decides it when nobody sets MACOSX_DEPLOYMENT_TARGET (Grafida gh-58);
#   only the system's own libraries and frameworks linked (/usr/lib, /System/Library): a clean Mac
#     has nothing else;
#   no automation channel in it.
#
# Usage: scripts/release/check-macos-binaries.sh SHELL

set -euo pipefail

SHELL_BIN="$1"
EXPECT_ARCH="${EXPECT_ARCH:-arm64}"
MACOS_MIN="${MACOS_MIN:-12.0}"

fail() { echo "FAIL: $*" >&2; exit 1; }
version_le() { [ "$(printf '%s\n%s\n' "$1" "$2" | sort -t. -k1,1n -k2,2n -k3,3n | head -1)" = "$1" ]; }

echo "== $SHELL_BIN"
file "$SHELL_BIN" || true

archs="$(lipo -archs "$SHELL_BIN")"
[ "$archs" = "$EXPECT_ARCH" ] || fail "architectures '$archs', expected exactly $EXPECT_ARCH"
echo "architecture $archs"

minos="$(otool -l "$SHELL_BIN" | awk '$1 == "cmd" { cmd = $2 } cmd == "LC_BUILD_VERSION" && $1 == "minos" { print $2; exit }')"
sdk="$(otool -l "$SHELL_BIN" | awk '$1 == "cmd" { cmd = $2 } cmd == "LC_BUILD_VERSION" && $1 == "sdk" { print $2; exit }')"
[ -n "$minos" ] || fail "no LC_BUILD_VERSION minos"
echo "minos $minos (max $MACOS_MIN), sdk $sdk"
version_le "$minos" "$MACOS_MIN" || fail "minimum macOS $minos is above the $MACOS_MIN floor"

libs="$(otool -L "$SHELL_BIN" | tail -n +2 | awk '{ print $1 }')"
while read -r lib; do
    [ -n "$lib" ] || continue
    case "$lib" in
        /usr/lib/* | /System/Library/*) echo "  $lib" ;;
        *) fail "links $lib, which is not part of macOS" ;;
    esac
done <<< "$libs"

# The automation channel's runtime file name exists only in src/automation.rs.
if grep -qa 'automation.json' "$SHELL_BIN"; then
    fail "the shell carries the automation channel"
fi
echo "no automation channel"

echo "== all checks passed"
