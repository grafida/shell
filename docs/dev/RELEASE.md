# Releasing the shell

A release is the tag `X.Y.Z` plus seven binaries, each with a `.sha256`:

| Asset | Built |
|---|---|
| `grafida-shell-linux-x86_64`, `grafida-launcher-linux-x86_64` | Ubuntu 22.04 container, emulated (amd64) |
| `grafida-shell-linux-aarch64`, `grafida-launcher-linux-aarch64` | Ubuntu 22.04 container, native (arm64) |
| `grafida-shell-macos-arm64` | natively, macOS 12.0 floor, SDK 15.5 |
| `grafida-shell-macos-x86_64` | cross-compiled from arm64, macOS 12.0 floor, SDK 15.5; smoke-tested under Rosetta 2 |
| `grafida-shell-windows-x86_64.exe` | cross-compiled with `cargo xwin` (lld-link, static CRT) |

They are built, checked, smoke-tested and published from the maintainer's Mac by
`scripts/release/local-release.sh`. GitHub Actions (`.github/workflows/release.yml`) still builds,
checks and smoke-tests every push and tag as an independent second build, but **publishes nothing**.
Consumers pin the tag and the hashes, so a release is written once and never changed.

## 1. Initial Mac setup (once)

An Apple silicon Mac with:

| What | Check | Install |
|---|---|---|
| Docker Desktop | `docker info` | docker.com; the Linux builds run in its VM |
| Rosetta 2 | `arch -x86_64 /usr/bin/true` | `softwareupdate --install-rosetta` (accepts Apple's licence) |
| rustup | `rustup show` in the checkout | rustup.rs; `rust-toolchain.toml` picks the compiler, the script adds the targets |
| cargo-xwin | `cargo xwin --version` | `cargo install --locked cargo-xwin` |
| GitHub CLI, signed in | `gh auth status` | `brew install gh`, `gh auth login` |
| Xcode or the Command Line Tools | `xcrun --show-sdk-path` | `xcode-select --install` (for `otool`, `lipo` and the linker) |

Then the macOS SDK the macOS assets are linked against. Every release so far was linked against SDK
15.5 (Xcode 16.4, what CI's `macos-15` runner used), and AppKit and WebKit change some behaviour
depending on the SDK a binary was linked with, so a newer Xcode's SDK is not used:

1. Download **Command Line Tools for Xcode 16.4** from
   <https://developer.apple.com/download/all/?q=Command%20Line%20Tools> (an Apple ID sign-in).
2. Extract the SDK from it:

   ```sh
   scripts/release/local-release.sh macos-sdk ~/Downloads/Command_Line_Tools_for_Xcode_16.4.dmg
   ```

   This mounts the disk image, expands the installer package in a temporary directory (a few GB,
   removed afterwards), and copies `MacOSX15.5.sdk` to `.sdk/MacOSX15.5.sdk` in the checkout
   (git-ignored). Nothing is installed: the Mac's own Command Line Tools stay as they are. The
   disk image can be deleted afterwards.

The first `build` also creates the two Linux build images (`grafida-shell-build:aarch64` and
`:x86_64`, a few minutes each) and the Docker volumes that keep cargo's registry and target
directory between builds (`grafida-shell-{cargo,target}-{aarch64,x86_64}`).

## 2. Every release

1. **The changelog.** The top section of `CHANGELOG.md` is `shell X.Y.Z` and holds every change
   that ships (see the conventions in `AGENTS.md`).
2. **The version.** Set `version = "X.Y.Z"` in both `Cargo.toml` and `launcher/Cargo.toml`, then
   update the lock file for the workspace's own crates only:

   ```sh
   cargo update --workspace
   git diff Cargo.lock          # only the two crates' version lines change
   ```

   Run `cargo fmt --all --check` and `cargo clippy --workspace --all-targets`, then commit on `main`
   as `Version X.Y.Z: <what the release is about>`.
3. **The tag.** An annotated, signed tag:

   ```sh
   git tag -s X.Y.Z -m "Grafida Shell X.Y.Z"
   git push origin main X.Y.Z
   ```

   CI starts its own build of the tag; it checks that the tag equals both crates' versions.
4. **Build.** With `HEAD` at the tag and a clean working tree:

   ```sh
   scripts/release/local-release.sh build
   ```

   About four minutes with warm caches. A smoke-test window flashes up twice on the Mac (macOS
   arm64, then x86_64 under Rosetta 2). The assets land in `dist/X.Y.Z/`. The directory is named
   after the tag only when `HEAD` is exactly that tag and the tree is clean, otherwise after the
   short commit, with `-dirty` when it is.
5. **Publish.**

   ```sh
   scripts/release/local-release.sh publish X.Y.Z
   ```

   It refuses unless:
   - the tree is clean and `HEAD` is the tag;
   - the tag equals both crates' versions;
   - origin has the same tag on the same commit;
   - `dist/X.Y.Z` was built from that clean commit, every asset matches its `.sha256`, and no asset
     contains the Mac's home directory;
   - the macOS assets pass `check-macos-binaries.sh` and are linked against SDK 15.5;
   - the Windows asset passes `check-windows-binaries.sh`;
   - no release `X.Y.Z` exists yet.

   It creates the release as a draft, uploads the fourteen files, publishes it, and prints its URL.
6. **Check it.** Download the published files and compare them with their hashes:

   ```sh
   d=$(mktemp -d) && gh release download X.Y.Z -D "$d" && (cd "$d" && shasum -a 256 -c ./*.sha256)
   ```

   Then look at CI's run for the tag (`gh run list --workflow Release --branch X.Y.Z`). It covers
   what the local build cannot: see the next section.

## 3. What the local build does not cover

Each platform runs the same check and smoke scripts as its CI job (`scripts/release/check-*.sh`,
`smoke-*.sh`), plus the debug automation build and its marker canary. There are three exceptions:

- **The Windows `.exe` is not smoke-tested.** `smoke-windows.sh` needs Windows, so the published,
  lld-linked file has not run before release. CI's Windows smoke test runs CI's own MSVC-linked
  build of the same commit, not these bytes. To run this one, copy it into a Windows guest
  (`docs/dev/windows-test-vm.md`).
- **The x86_64 Linux smoke test runs with WebKit's sandbox off.** Under emulation bubblewrap cannot
  load its seccomp filter, because the kernel underneath is arm64. The arm64 smoke test runs with the
  sandbox on, and so does CI's x86_64 run of its own build.
- **x86_64 macOS is built by cross-compiling from arm64** and smoke-tested under Rosetta 2. CI
  builds it natively on an Intel runner.

The local binaries are not byte-identical to CI's, so their hashes differ. Build paths are embedded:
on the Mac, `$CARGO_HOME` and the checkout are remapped to `/cargo` and `/src`; in the container,
the paths are already `/opt/cargo` and `/src`.

## 4. Other uses of `build`

```sh
scripts/release/local-release.sh build linux-aarch64 windows-x86_64   # some platforms only
scripts/release/local-release.sh build --refresh-image                # rebuild the Linux images
```

- **Platforms:** `linux-x86_64`, `linux-aarch64`, `macos-arm64`, `macos-x86_64` and
  `windows-x86_64`.
- **`--refresh-image`:** rebuilds the Linux images from a fresh `ubuntu:22.04` with the current
  package versions, which is what a CI run installs. Do this before a release once the images are
  more than a few weeks old.
- **A test build against another macOS SDK:** `MACOS_SDK=/path/to/MacOSX26.5.sdk
  MACOS_SDK_VERSION=26.5` builds against it. `publish` reads the SDK from the binaries and refuses
  anything but 15.5.
- **The debug automation builds** go to `dist/LABEL/debug-automation/PLATFORM/`. They are for the
  `parity-*.sh` runs on the test VMs and are never uploaded. The Linux ones are built on 22.04, so
  they load on the oldest supported distribution.

## 5. When something goes wrong

- **The Linux smoke test fails with exit status 133** (SIGTRAP). WebKit could not start its
  sandbox. The script runs the container as the unprivileged user `builder`, with
  `seccomp=unconfined` and `systempaths=unconfined`. Without those, bubblewrap stops with `Creating
  new namespace failed` or `Can't mount proc`. An image built before that change is replaced on the
  next `build`.
- **Odd cargo errors in the container.** Remove the cached volumes and build again:
  `docker volume rm grafida-shell-cargo-aarch64 grafida-shell-target-aarch64` (or `-x86_64`).
- **`publish` stopped halfway.** The release is still a draft, so nobody sees it, but the next
  `publish` refuses because a release exists. Check with `gh release view X.Y.Z` that it really is a
  draft, delete it with `gh release delete X.Y.Z --yes` (this keeps the tag), and run `publish`
  again.
- **A published release is wrong.** Never replace its assets: consumers have pinned their hashes.
  Release `X.Y.Z+1`.

## 6. Changing the release toolchain

- **The Rust compiler** is `rust-toolchain.toml`, shared by CI, the Mac and the Linux images. After
  bumping it, the next `build` rebuilds the images.
- **The macOS SDK** is `RELEASE_MACOS_SDK` in `local-release.sh`, plus the Command Line Tools
  version named in section 1. A new SDK changes how AppKit and WebKit behave for the shell, so
  test the macOS guests (`docs/dev/macos-test-vms.md`) before releasing with it.
- **The glibc floor** (2.35) and the Linux runtime packages are in `check-linux-binaries.sh` and
  `linux-build.Dockerfile`, which must stay on Ubuntu 22.04 (ADR 0001).
