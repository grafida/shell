#!/usr/bin/env bash
#
# Runs INSIDE the linux-build.Dockerfile container, started by local-release.sh: the Linux release
# build, step for step, for the container's own architecture. The repository is /src
# (read-only), cargo builds in $CARGO_TARGET_DIR (a volume), and the named assets, their .sha256
# and the debug automation build go to OUT.
#
# Usage (in the container): scripts/release/linux-container-build.sh OUT

set -euo pipefail

OUT="$1"
cd /src

case "$(uname -m)" in
    x86_64) ARCH=x86_64 ;;
    aarch64) ARCH=aarch64 ;;
    *) echo "unsupported architecture $(uname -m)" >&2; exit 1 ;;
esac
MUSL="$ARCH-unknown-linux-musl"
T="${CARGO_TARGET_DIR:?}"

echo "webkit2gtk-4.1 $(pkg-config --modversion webkit2gtk-4.1)"
ldd --version | sed -n 1p   # not head -1: under pipefail, ldd's SIGPIPE would fail the build
rustc --version

echo "== build (release profile, no automation)"
cargo build --release --locked -p grafida-shell
cargo build --release --locked -p grafida-launcher --target "$MUSL"
SHELL_BIN="$T/release/grafida-shell"
LAUNCHER="$T/$MUSL/release/grafida-launcher"

echo "== check the binaries"
scripts/release/check-linux-binaries.sh "$SHELL_BIN" "$LAUNCHER"

# The glibc floor check must fail, and say why, when objdump lists no GLIBC versions at all
# (security.md I8): a negative test of the check itself.
echo "== the glibc check refuses a shell with no symbol versions"
shim="$(mktemp -d)"
printf '#!/bin/sh\nexit 0\n' > "$shim/objdump"
chmod +x "$shim/objdump"
if out="$(PATH="$shim:$PATH" scripts/release/check-linux-binaries.sh "$SHELL_BIN" "$LAUNCHER" 2>&1)"; then
    echo "$out"; echo "FAIL: the check passed with no GLIBC symbol versions" >&2; exit 1
fi
grep -q 'no GLIBC symbol versions' <<<"$out" || { echo "$out"; echo "FAIL: the check failed for another reason" >&2; exit 1; }
echo "ok"

echo "== smoke test (headless, through the launcher)"
scripts/release/smoke-linux.sh "$SHELL_BIN" "$LAUNCHER"

echo "== name the assets"
cp "$SHELL_BIN" "$OUT/grafida-shell-linux-$ARCH"
cp "$LAUNCHER" "$OUT/grafida-launcher-linux-$ARCH"
(cd "$OUT" && for f in "grafida-shell-linux-$ARCH" "grafida-launcher-linux-$ARCH"; do sha256sum "$f" > "$f.sha256"; done && cat ./*-linux-"$ARCH".sha256)

# The verification build for packaged apps on clean machines (parity-linux.sh), never a release
# asset; and the canary that the release check's automation marker is still the right string.
echo "== debug automation shell (verification only)"
cargo build --locked -p grafida-shell --features automation
grep -qa 'automation.json' "$T/debug/grafida-shell" \
    || { echo "FAIL: the debug automation shell lacks the marker the release checks look for" >&2; exit 1; }
mkdir -p "$OUT/debug-automation/linux-$ARCH"
cp "$T/debug/grafida-shell" "$OUT/debug-automation/linux-$ARCH/grafida-shell"
