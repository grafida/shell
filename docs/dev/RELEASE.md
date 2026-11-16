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
`scripts/release/local-release.sh`. There is no CI: nothing else builds, tests or publishes the
shell, so whatever this script and the steps below do not run does not run at all. Consumers pin the tag and the hashes, so a release is written once and never changed.

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
15.5 (Xcode 16.4, what GitHub's `macos-15` runner used while there was a CI), and AppKit and WebKit change some behaviour
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

   Run the code checks, then commit on `main` as `Version X.Y.Z: <what the release is about>`:

   ```sh
   scripts/release/local-release.sh check
   ```

   About three minutes. These are the checks the removed `ci.yml` ran on each platform (section 4).
3. **The tag.** An annotated, signed tag:

   ```sh
   git tag -s X.Y.Z -m "Grafida Shell X.Y.Z"
   git push origin main X.Y.Z
   ```

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

## 3. What the local build does not cover

Each platform runs its check and smoke scripts (`scripts/release/check-*.sh`, `smoke-*.sh`), plus
the debug automation build and its marker canary. `check` covers the code (section 4). Three
things are never tested:

- **The Windows `.exe` is not smoke-tested.** `smoke-windows.sh` needs Windows, so the published
  file has not run before release. To run it, copy it into a Windows guest
  (`docs/dev/windows-test-vm.md`).
- **The x86_64 Linux shell never runs with WebKit's sandbox on.** Under emulation bubblewrap cannot
  load its seccomp filter, because the kernel underneath is arm64, so its smoke test runs with the
  sandbox off. The arm64 smoke test runs with the sandbox on. The x86_64 test VMs
  (`docs/dev/linux-test-vms.md`) can run it with the sandbox on.
- **x86_64 macOS is never run on Intel hardware.** It is cross-compiled from arm64 and
  smoke-tested under Rosetta 2.

Build paths are embedded in the binaries. On the Mac, `$CARGO_HOME` and the checkout are remapped
to `/cargo` and `/src`; in the container, the paths are already `/opt/cargo` and `/src`.

## 4. The code checks: `check`

`scripts/release/local-release.sh check [GROUP…]` runs `scripts/release/checks.sh` for each group.
That script runs clippy with `-D warnings`, with and without the `automation` feature. It checks
that a release build refuses the automation feature. It runs the tests, including the automation
module's own tests. `build` does not run `check`; run it yourself before a release.

| Group | Where | Also |
|---|---|---|
| `linux-aarch64` (default) | the Ubuntu 22.04 container, native | the checkout is mounted read-write: the tests write git-ignored logs |
| `macos` (default) | natively, arm64; x86_64 clippy and the guard only | `cargo fmt --all --check`; the protocol fixtures are complete and untouched (`make-fixtures.py` adds nothing) |
| `windows` (default) | `cargo xwin`, cross | the tests are compiled, not run: that needs Windows |
| `linux-x86_64` | the container, emulated (slow) | |

`tests/real_workers.rs` runs shell-php's and Grafida's workers only where the sibling clones exist.
That means on the Mac, never in the container.

## 5. Other uses of `build`

```sh
scripts/release/local-release.sh build linux-aarch64 windows-x86_64   # some platforms only
scripts/release/local-release.sh build --refresh-image                # rebuild the Linux images
```

- **Platforms:** `linux-x86_64`, `linux-aarch64`, `macos-arm64`, `macos-x86_64` and
  `windows-x86_64`.
- **`--refresh-image`:** rebuilds the Linux images from a fresh `ubuntu:22.04` with the current
  package versions. Do this before a release once the images are
  more than a few weeks old.
- **A test build against another macOS SDK:** `MACOS_SDK=/path/to/MacOSX26.5.sdk
  MACOS_SDK_VERSION=26.5` builds against it. `publish` reads the SDK from the binaries and refuses
  anything but 15.5.
- **The debug automation builds** go to `dist/LABEL/debug-automation/PLATFORM/`. They are for the
  `parity-*.sh` runs on the test VMs and are never uploaded. The Linux ones are built on 22.04, so
  they load on the oldest supported distribution.

## 6. When something goes wrong

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

## 7. Changing the release toolchain

- **The Rust compiler** is `rust-toolchain.toml`, shared by the Mac and the Linux images. After
  bumping it, the next `build` rebuilds the images.
- **The macOS SDK** is `RELEASE_MACOS_SDK` in `local-release.sh`, plus the Command Line Tools
  version named in section 1. A new SDK changes how AppKit and WebKit behave for the shell, so
  test the macOS guests (`docs/dev/macos-test-vms.md`) before releasing with it.
- **The glibc floor** (2.35) and the Linux runtime packages are in `check-linux-binaries.sh` and
  `linux-build.Dockerfile`, which must stay on Ubuntu 22.04 (ADR 0001).
