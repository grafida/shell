#!/usr/bin/env bash
#
# The code checks the removed ci.yml ran on each platform, for one target: clippy with -D warnings,
# with and without the automation feature; the guard that a release build refuses the automation
# feature; and the test suites. Run by `local-release.sh check`, on the Mac and in the Linux
# container (where the checkout is /src).
#
# Usage: scripts/release/checks.sh [--xwin] [--target TRIPLE] [--tests run|compile|none]
#   --xwin     use `cargo xwin` (the Windows target from a Mac)
#   --tests    run the tests (the default), only compile them (a target this host cannot run), or skip

set -euo pipefail
# Arrays that may be empty are expanded as ${a[@]+"${a[@]}"}: macOS's bash 3.2 calls them unbound.

cd "$(dirname "${BASH_SOURCE[0]}")/../.."

cargo=(cargo)
target=()
tests=run
while [ $# -gt 0 ]; do
    case "$1" in
        --xwin) cargo=(cargo xwin) ;;
        --target) target=(--target "${2:?}"); shift ;;
        --tests) tests="${2:?}"; shift ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
    shift
done
step() { printf '== %s\n' "$*"; }

step "clippy -D warnings ${target[*]+${target[*]}}"
"${cargo[@]}" clippy --locked --workspace --all-targets ${target[@]+"${target[@]}"} -- -D warnings
step "clippy -D warnings, automation"
"${cargo[@]}" clippy --locked --workspace --all-targets ${target[@]+"${target[@]}"} --features automation -- -D warnings

# A release build must refuse the automation feature outright (a compile_error! in src/automation.rs).
step "a release build refuses the automation feature"
if out="$("${cargo[@]}" check --locked --release ${target[@]+"${target[@]}"} --features automation 2>&1)"; then
    echo "$out"; echo "FAIL: a release build accepted the automation feature" >&2; exit 1
fi
grep -q 'must never be compiled into a release build' <<<"$out" \
    || { echo "$out"; echo "FAIL: cargo failed, but not on the automation guard" >&2; exit 1; }
echo ok

case "$tests" in
    run)
        step "tests"
        "${cargo[@]}" test --locked --workspace ${target[@]+"${target[@]}"}
        # The automation module's own tests (debug build only); the workspace run does not enable it.
        "${cargo[@]}" test --locked -p grafida-shell ${target[@]+"${target[@]}"} --features automation --lib automation::tests
        ;;
    compile)
        step "tests (compiled only: this host cannot run them)"
        "${cargo[@]}" test --locked --workspace ${target[@]+"${target[@]}"} --no-run
        "${cargo[@]}" test --locked -p grafida-shell ${target[@]+"${target[@]}"} --features automation --lib --no-run
        ;;
    none) ;;
    *) echo "--tests run|compile|none" >&2; exit 2 ;;
esac
