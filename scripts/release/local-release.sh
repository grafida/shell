#!/usr/bin/env bash
#
# Builds, checks, smoke-tests and publishes the release assets on the maintainer's Mac. There is no
# CI: this script is the only build of a release (docs/dev/RELEASE.md).
#
#   check [GROUP…]                        the code checks the removed ci.yml ran (scripts/release/
#                                         checks.sh): clippy -D warnings, the release-build
#                                         automation guard, the tests; GROUP is linux-aarch64 (the
#                                         default three: linux-aarch64 macos windows), linux-x86_64
#                                         (emulated, slow), macos (plus fmt and the protocol
#                                         fixtures) or windows (tests compiled, not run)
#   build [--refresh-image] [PLATFORM…]   build the assets into dist/LABEL/ (all platforms by default)
#   publish TAG                           upload dist/TAG/ as the release TAG
#   macos-sdk DMG                         extract the macOS SDK the assets are linked against from
#                                         Apple's "Command Line Tools for Xcode 16.4" disk image
#
# PLATFORM is linux-x86_64, linux-aarch64, macos-arm64, macos-x86_64 or windows-x86_64. LABEL is the
# tag when HEAD is exactly a tag (X.Y.Z) and the tree is clean, otherwise the short commit (+ -dirty).
# Each platform runs its check and smoke scripts (scripts/release/check-*.sh, smoke-*.sh):
#
#   linux-*   in an Ubuntu 22.04 container (linux-build.Dockerfile, Docker Desktop): aarch64 natively,
#             x86_64 emulated. linux-container-build.sh: build, check, the glibc check's negative
#             test, the headless smoke test, the debug automation build and its marker canary.
#             The emulated x86_64 smoke test runs with WebKit's sandbox off (it cannot start under
#             emulation), so this x86_64 shell has never run with it on.
#   macos-*   natively, linked for macOS 12.0 against MACOS_SDK (SDK 15.5, the one macos-15's Xcode
#             16.4 linked every earlier release with; this Mac's Xcode is newer). x86_64 is
#             cross-compiled from arm64 and smoke-tested under Rosetta 2.
#             The smoke test shows a window for a moment.
#   windows-* cross-compiled with cargo-xwin (lld-link, Microsoft's CRT and SDK downloaded by xwin,
#             whose licence this script accepts: XWIN_ACCEPT_LICENSE=1) and checked. NOT smoke-tested:
#             smoke-windows.sh needs Windows, so the published .exe has not run before release; a
#             Windows guest (docs/dev/windows-test-vm.md) can run it.
#
# Output: dist/LABEL/ holds the named assets and their .sha256 (what a release publishes),
# BUILD-INFO (the commit), and debug-automation/PLATFORM/ (verification builds, never published).
#
# Needs: Docker Desktop, Rosetta 2, rustup (rust-toolchain.toml), cargo-xwin, gh (publish only).

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$ROOT"

ALL_PLATFORMS=(linux-x86_64 linux-aarch64 macos-arm64 macos-x86_64 windows-x86_64)
ASSETS=(grafida-shell-linux-x86_64 grafida-launcher-linux-x86_64
        grafida-shell-linux-aarch64 grafida-launcher-linux-aarch64
        grafida-shell-macos-arm64 grafida-shell-macos-x86_64
        grafida-shell-windows-x86_64.exe)
# The SDK every macOS release is linked against. MACOS_SDK/MACOS_SDK_VERSION may point a test build
# at another one; publish accepts only this one, read from the binaries.
RELEASE_MACOS_SDK=15.5
MACOS_SDK_VERSION="${MACOS_SDK_VERSION:-$RELEASE_MACOS_SDK}"
MACOS_SDK="${MACOS_SDK:-$ROOT/.sdk/MacOSX$MACOS_SDK_VERSION.sdk}"
IMAGE=grafida-shell-build

die() { echo "FAIL: $*" >&2; exit 1; }
step() { printf '\n=== %s\n' "$*"; }
crate_version() { sed -n '/^version = /{s/^version = "\(.*\)"$/\1/p;q;}' "$1"; }
clean_tree() { [ -z "$(git status --porcelain)" ]; }

# Host builds (macOS, Windows) would embed this Mac's paths, the user's home among them, in panic
# locations; these map them to neutral ones, as the container's /opt/cargo and /src already are.
# Through --config, not RUSTFLAGS: that would replace .cargo/config.toml's target rustflags (the
# Windows static CRT), where --config's array is appended to them.
CARGO_HOME_DIR="${CARGO_HOME:-$HOME/.cargo}"
remap_config() { # TARGET
    printf 'target.%s.rustflags=["--remap-path-prefix=%s=/cargo", "--remap-path-prefix=%s=/src"]' \
        "$1" "$CARGO_HOME_DIR" "$ROOT"
}
no_home_path() { # FILE: fail if this Mac's home directory is anywhere in it
    if grep -qaF "$HOME/" "$1"; then die "$1 embeds paths under $HOME"; fi
}

# The tag must equal both crates' versions.
check_versions() {
    local tag=$1 toml version
    for toml in Cargo.toml launcher/Cargo.toml; do
        version="$(crate_version "$toml")"
        [ "$version" = "$tag" ] || die "$toml says $version, the tag is $tag"
    done
}

label() {
    local tag
    if clean_tree && tag="$(git describe --exact-match --tags HEAD 2>/dev/null)" \
        && [[ $tag =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
        echo "$tag"
    else
        echo "$(git rev-parse --short HEAD)$(clean_tree || echo -dirty)"
    fi
}

# --- Linux, in the Ubuntu 22.04 container --------------------------------------------------------
docker_platform() { case "$1" in x86_64) echo linux/amd64 ;; aarch64) echo linux/arm64 ;; esac; }

linux_image() { # ARCH: build (or reuse, from Docker's cache) the image
    local arch=$1 platform ctx
    platform="$(docker_platform "$arch")"
    step "linux-$arch: the build image ($platform)"
    ctx="$(mktemp -d)"
    cp scripts/release/linux-build.Dockerfile "$ctx/Dockerfile"
    cp rust-toolchain.toml "$ctx/"
    docker build ${REFRESH_IMAGE:+--pull --no-cache} --platform "$platform" -t "$IMAGE:$arch" "$ctx"
    rm -rf "$ctx"
}

build_linux() {
    local arch=$1 platform
    platform="$(docker_platform "$arch")"
    linux_image "$arch"

    step "linux-$arch: build, check, smoke-test"
    # /dev/shm: WebKit's processes share memory through it, and Docker's default is 64 MB.
    # The smoke test runs with WebKit's bubblewrap sandbox on, as on a user's machine, which takes two
    # relaxations of the container (it runs only this repository's code): seccomp=unconfined, as
    # Docker's default profile refuses user namespaces, and systempaths=unconfined, as the kernel
    # refuses bwrap's new /proc while Docker masks parts of the container's.
    # Emulated (x86_64 on Apple silicon), the sandbox cannot start at all: bwrap's seccomp filter is
    # an x86_64 program the arm64 kernel underneath refuses (prctl(PR_SET_SECCOMP): Invalid argument).
    # That smoke test runs without it.
    # (Expanded as ${a[@]+"${a[@]}"}: macOS's bash 3.2 calls an empty array unbound under set -u.)
    local nosandbox=()
    [ "$arch" = "$(uname -m | sed 's/arm64/aarch64/')" ] || nosandbox=(-e WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1)
    docker run --rm --platform "$platform" --shm-size=1g \
        --security-opt seccomp=unconfined --security-opt systempaths=unconfined ${nosandbox[@]+"${nosandbox[@]}"} \
        -v "$ROOT:/src:ro" -v "$OUT:/out" \
        -v "grafida-shell-cargo-$arch:/opt/cargo/registry" \
        -v "grafida-shell-target-$arch:/target" -e CARGO_TARGET_DIR=/target \
        "$IMAGE:$arch" /src/scripts/release/linux-container-build.sh /out
}

# --- macOS, natively ---------------------------------------------------------------------------------
macho_field() { # FIELD BINARY: minos or sdk from LC_BUILD_VERSION
    otool -l "$2" | awk -v f="$1" '$1 == "cmd" { cmd = $2 } cmd == "LC_BUILD_VERSION" && $1 == f { print $2; exit }'
}

build_macos() {
    local arch=$1 target bin
    case "$arch" in arm64) target=aarch64-apple-darwin ;; x86_64) target=x86_64-apple-darwin ;; esac
    [ -d "$MACOS_SDK" ] || die "no macOS SDK at $MACOS_SDK; download \"Command Line Tools for Xcode 16.4\" from developer.apple.com and run: $0 macos-sdk FILE.dmg"
    # The SDK the binaries are linked against (LC_BUILD_VERSION sdk); the floor is .cargo/config.toml's.
    export SDKROOT="$MACOS_SDK" MACOSX_DEPLOYMENT_TARGET=12.0
    rustup target add "$target" >/dev/null 2>&1 || true

    step "macos-$arch: build (release profile, no automation)"
    cargo build --release --locked -p grafida-shell --target "$target" --config "$(remap_config "$target")"
    bin="target/$target/release/grafida-shell"

    step "macos-$arch: check"
    EXPECT_ARCH=$arch scripts/release/check-macos-binaries.sh "$bin"
    [ "$(macho_field sdk "$bin")" = "$MACOS_SDK_VERSION" ] \
        || die "linked against SDK $(macho_field sdk "$bin"), expected $MACOS_SDK_VERSION"
    echo "sdk $MACOS_SDK_VERSION"
    no_home_path "$bin"

    step "macos-$arch: smoke test (a window appears for a moment)"
    scripts/release/smoke-macos.sh "$bin"

    cp "$bin" "$OUT/grafida-shell-macos-$arch"
    (cd "$OUT" && shasum -a 256 "grafida-shell-macos-$arch" > "grafida-shell-macos-$arch.sha256" && cat "grafida-shell-macos-$arch.sha256")

    step "macos-$arch: debug automation shell (verification only)"
    cargo build --locked -p grafida-shell --features automation --target "$target" --config "$(remap_config "$target")"
    bin="target/$target/debug/grafida-shell"
    [ "$(lipo -archs "$bin")" = "$arch" ] || die "debug build is $(lipo -archs "$bin"), expected $arch"
    [ "$(macho_field minos "$bin")" = 12.0 ] || die "debug build has minos $(macho_field minos "$bin")"
    grep -qa 'automation.json' "$bin" || die "the debug automation shell lacks the marker the release checks look for"
    mkdir -p "$OUT/debug-automation/macos-$arch"
    cp "$bin" "$OUT/debug-automation/macos-$arch/grafida-shell"
    unset SDKROOT MACOSX_DEPLOYMENT_TARGET
}

# --- Windows, cross-compiled -------------------------------------------------------------------------
build_windows() {
    local target=x86_64-pc-windows-msvc bin
    command -v cargo-xwin >/dev/null || die "cargo-xwin is not installed (cargo install --locked cargo-xwin)"
    export XWIN_ACCEPT_LICENSE=1

    step "windows-x86_64: build (release profile, no automation; cargo-xwin)"
    cargo xwin build --release --locked -p grafida-shell --target "$target" --config "$(remap_config "$target")"
    bin="target/$target/release/grafida-shell.exe"

    step "windows-x86_64: check"
    scripts/release/check-windows-binaries.sh "$bin"
    no_home_path "$bin"
    echo "(no smoke test: smoke-windows.sh needs Windows; this .exe has not run anywhere yet)"

    cp "$bin" "$OUT/grafida-shell-windows-x86_64.exe"
    (cd "$OUT" && shasum -a 256 grafida-shell-windows-x86_64.exe > grafida-shell-windows-x86_64.exe.sha256 && cat grafida-shell-windows-x86_64.exe.sha256)

    step "windows-x86_64: debug automation shell (verification only)"
    cargo xwin build --locked -p grafida-shell --features automation --target "$target" --config "$(remap_config "$target")"
    bin="target/$target/debug/grafida-shell.exe"
    grep -qa 'automation.json' "$bin" || die "the debug automation shell lacks the marker the release checks look for"
    mkdir -p "$OUT/debug-automation/windows-x86_64"
    cp "$bin" "$OUT/debug-automation/windows-x86_64/grafida-shell.exe"
}

cmd_build() {
    local platforms=() p commit info
    while [ $# -gt 0 ]; do
        case "$1" in
            --refresh-image) REFRESH_IMAGE=1 ;;
            linux-x86_64|linux-aarch64|macos-arm64|macos-x86_64|windows-x86_64) platforms+=("$1") ;;
            *) die "unknown platform or option '$1' (${ALL_PLATFORMS[*]})" ;;
        esac
        shift
    done
    [ ${#platforms[@]} -gt 0 ] || platforms=("${ALL_PLATFORMS[@]}")

    LABEL="$(label)"
    [[ $LABEL =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] && check_versions "$LABEL"
    OUT="$ROOT/dist/$LABEL"
    commit="$(git rev-parse HEAD)"
    info="commit=$commit"$'\n'"dirty=$(clean_tree && echo no || echo yes)"
    # One directory holds one tree's build: anything else in it is from another tree and goes.
    if [ -f "$OUT/BUILD-INFO" ] && [ "$(cat "$OUT/BUILD-INFO")" != "$info" ]; then rm -rf "$OUT"; fi
    mkdir -p "$OUT"
    printf '%s\n' "$info" > "$OUT/BUILD-INFO"
    echo "building ${platforms[*]} of $LABEL ($commit) into dist/$LABEL"

    local start=$SECONDS
    for p in "${platforms[@]}"; do
        local t=$SECONDS
        case "$p" in
            linux-*) build_linux "${p#linux-}" ;;
            macos-*) build_macos "${p#macos-}" ;;
            windows-x86_64) build_windows ;;
        esac
        echo "--- $p: done in $((SECONDS - t))s"
    done
    step "all done in $((SECONDS - start))s: dist/$LABEL"
    ls -l "$OUT"
}

# --- check -------------------------------------------------------------------------------------------
check_linux() {
    local arch=$1 platform
    platform="$(docker_platform "$arch")"
    linux_image "$arch"
    step "linux-$arch: clippy, the automation guard, the tests"
    # The checkout is mounted read-write here: the tests write logs under tests/apps/*/logs
    # (git-ignored). No smoke test, so no sandbox options.
    docker run --rm --platform "$platform" --shm-size=1g \
        -v "$ROOT:/src" \
        -v "grafida-shell-cargo-$arch:/opt/cargo/registry" \
        -v "grafida-shell-target-$arch:/target" -e CARGO_TARGET_DIR=/target \
        "$IMAGE:$arch" /src/scripts/release/checks.sh
}

check_macos() {
    step "macos: fmt"
    cargo fmt --all --check
    step "macos-arm64: clippy, the automation guard, the tests"
    scripts/release/checks.sh
    rustup target add x86_64-apple-darwin >/dev/null 2>&1 || true
    step "macos-x86_64: clippy, the automation guard (tests run on arm64 only)"
    scripts/release/checks.sh --target x86_64-apple-darwin --tests none
    # The fixture writer only adds cases: anything it would add must already be committed.
    step "protocol fixtures complete and untouched"
    python3 -I scripts/protocol/make-fixtures.py
    [ -z "$(git status --porcelain protocol/fixtures)" ] \
        || { git status --porcelain protocol/fixtures; die "make-fixtures.py changed protocol/fixtures"; }
    echo ok
}

check_windows() {
    command -v cargo-xwin >/dev/null || die "cargo-xwin is not installed (cargo install --locked cargo-xwin)"
    export XWIN_ACCEPT_LICENSE=1
    step "windows-x86_64: clippy, the automation guard, the tests compiled (not run: needs Windows)"
    scripts/release/checks.sh --xwin --target x86_64-pc-windows-msvc --tests compile
}

cmd_check() {
    local groups=() g start=$SECONDS t
    for g in "$@"; do
        case "$g" in
            linux-aarch64|linux-x86_64|macos|windows) groups+=("$g") ;;
            *) die "unknown group '$g' (linux-aarch64, linux-x86_64, macos, windows)" ;;
        esac
    done
    [ ${#groups[@]} -gt 0 ] || groups=(linux-aarch64 macos windows)
    for g in "${groups[@]}"; do
        t=$SECONDS
        case "$g" in
            linux-*) check_linux "${g#linux-}" ;;
            macos) check_macos ;;
            windows) check_windows ;;
        esac
        echo "--- $g: passed in $((SECONDS - t))s"
    done
    step "all checks passed in $((SECONDS - start))s: ${groups[*]}"
}

# --- publish -----------------------------------------------------------------------------------------
cmd_publish() {
    local tag=${1:?usage: $0 publish TAG} dir commit remote a sha_list=() files=()
    [[ $tag =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "'$tag' is not X.Y.Z"
    check_versions "$tag"
    clean_tree || die "the working tree is not clean"
    commit="$(git rev-parse "$tag^{commit}" 2>/dev/null)" || die "no tag $tag here"
    [ "$(git rev-parse HEAD)" = "$commit" ] || die "HEAD is not $tag"
    # The tag must be on GitHub, pointing at the same commit (gh release create --verify-tag needs it).
    remote="$(git ls-remote origin "refs/tags/$tag" "refs/tags/$tag^{}" | tail -1 | cut -f1)"
    [ "$remote" = "$commit" ] || die "origin's tag $tag is ${remote:-missing}, not $commit: push it first"

    dir="$ROOT/dist/$tag"
    [ -f "$dir/BUILD-INFO" ] || die "nothing built in dist/$tag: run $0 build"
    [ "$(cat "$dir/BUILD-INFO")" = "commit=$commit"$'\n'"dirty=no" ] || die "dist/$tag was not built from a clean $commit"
    for a in "${ASSETS[@]}"; do
        [ -f "$dir/$a" ] && [ -f "$dir/$a.sha256" ] || die "dist/$tag lacks $a (or its .sha256): run $0 build"
        (cd "$dir" && shasum -a 256 -c "$a.sha256" >/dev/null) || die "$a does not match its .sha256"
        no_home_path "$dir/$a"
        files+=("$dir/$a" "$dir/$a.sha256")
        sha_list+=("$(cat "$dir/$a.sha256")")
    done
    # What a test build could have changed (another SDK) is checked again on the files themselves.
    for a in arm64 x86_64; do
        EXPECT_ARCH=$a scripts/release/check-macos-binaries.sh "$dir/grafida-shell-macos-$a" >/dev/null \
            || die "grafida-shell-macos-$a fails check-macos-binaries.sh"
        [ "$(macho_field sdk "$dir/grafida-shell-macos-$a")" = "$RELEASE_MACOS_SDK" ] \
            || die "grafida-shell-macos-$a is linked against SDK $(macho_field sdk "$dir/grafida-shell-macos-$a"), not $RELEASE_MACOS_SDK"
    done
    scripts/release/check-windows-binaries.sh "$dir/grafida-shell-windows-x86_64.exe" >/dev/null \
        || die "grafida-shell-windows-x86_64.exe fails check-windows-binaries.sh"

    if gh release view "$tag" >/dev/null 2>&1; then die "a release $tag already exists: a published release is never rewritten"; fi
    printf '%s\n' "${sha_list[@]}"
    # A draft until every asset is up, so a half-uploaded release is never visible.
    gh release create "$tag" "${files[@]}" --draft --verify-tag \
        --title "Grafida Shell $tag" \
        --notes "Binaries of the Grafida Shell $tag (commit $commit), built by the maintainer with scripts/release/local-release.sh. Linux x86_64 and aarch64: built on Ubuntu 22.04 (glibc 2.35, WebKitGTK 4.1), with the static launcher. macOS arm64 and x86_64: each linked for macOS 12.0, ad-hoc signed by the linker only (an app signs it inside its bundle). Windows x86_64: Windows 10 22H2 or later with the WebView2 Evergreen runtime, a static CRT, unsigned (an app signs it with its installer). See CHANGELOG.md. Pin the tag and the .sha256 of each asset."
    gh release edit "$tag" --draft=false
    gh release view "$tag" --json url --jq .url
}

# --- the macOS SDK -----------------------------------------------------------------------------------
cmd_macos_sdk() {
    local dmg=${1:?usage: $0 macos-sdk Command_Line_Tools_for_Xcode_16.4.dmg} mnt work pkg sdk version
    [ -f "$dmg" ] || die "no file $dmg"
    mnt="$(mktemp -d)"; work="$(mktemp -d)"
    # EXIT, not RETURN: die() exits, and the image must be detached then too.
    trap 'hdiutil detach -quiet "$mnt" 2>/dev/null || true; rm -rf "$work"' EXIT
    hdiutil attach -nobrowse -readonly -quiet -mountpoint "$mnt" "$dmg"
    pkg="$(find "$mnt" -maxdepth 1 -name '*.pkg' -print -quit)"
    [ -n "$pkg" ] || die "no installer package in $dmg"
    # Expanded only, never installed: this Mac's Command Line Tools stay as they are.
    echo "expanding $(basename "$pkg") (a few GB, temporarily)"
    pkgutil --expand-full "$pkg" "$work/pkg"
    sdk="$(find "$work/pkg" -type d -name "MacOSX$MACOS_SDK_VERSION.sdk" -print -quit)"
    [ -n "$sdk" ] || die "no MacOSX$MACOS_SDK_VERSION.sdk in $(basename "$pkg")"
    version="$(plutil -extract Version raw -o - "$sdk/SDKSettings.plist")"
    [ "$version" = "$MACOS_SDK_VERSION" ] || die "the SDK says it is $version"
    rm -rf "$MACOS_SDK"
    mkdir -p "$(dirname "$MACOS_SDK")"
    ditto "$sdk" "$MACOS_SDK"
    echo "macOS SDK $version: $MACOS_SDK"
}

case "${1:-}" in
    check) shift; cmd_check "$@" ;;
    build) shift; cmd_build "$@" ;;
    publish) shift; cmd_publish "$@" ;;
    macos-sdk) shift; cmd_macos_sdk "$@" ;;
    *) sed -n '3,/^$/p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
