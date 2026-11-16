# Releases

Read before building, checking, smoke-testing or publishing a release.

- **Releases** (`docs/dev/RELEASE.md`). There is no CI (removed 2026-11-16: it cost money for
  software given away). The maintainer builds, checks, smoke-tests and publishes on the Mac with
  `scripts/release/local-release.sh` (`build`, then `publish TAG` once the tag `X.Y.Z`, equal to
  **both** crates' versions, is pushed). Linux x86_64 and aarch64 build in an Ubuntu 22.04 container
  (`linux-build.Dockerfile` + `linux-container-build.sh`; aarch64 native, x86_64 emulated; an
  unprivileged user with `seccomp` and `systempaths` unconfined, or WebKit's bubblewrap sandbox
  cannot start; emulated it cannot start at all, so the x86_64 smoke test runs with it off), checked
  by `check-linux-binaries.sh` (glibc ≤ 2.35, every NEEDED library from `libwebkit2gtk-4.1-0` +
  `libgtk-3-0`'s dependency closure, no automation marker, a static launcher) and smoke-tested by
  `smoke-linux.sh` (the launcher → the shell → the fake worker's `page` mode, which POSTs, reports
  to the log and quits; no automation channel). macOS arm64 and x86_64 (cross-compiled; smoke-tested
  under Rosetta 2) build natively against the **SDK 15.5** in `.sdk/` (`macos-sdk DMG` extracts it
  from Command Line Tools for Xcode 16.4; every earlier release was linked against it), checked by
  `check-macos-binaries.sh` with `EXPECT_ARCH` (one thin architecture, `LC_BUILD_VERSION` minos ≤
  12.0, only `/usr/lib` and `/System/Library` linked, no automation marker) and smoke-tested by
  `smoke-macos.sh` (the shell in a bundle layout, manifest in `Contents/Resources`, worker beside
  the shell). Windows x86_64 builds with `cargo xwin`, checked by `check-windows-binaries.sh` (PE32+
  AMD64, the GUI subsystem, only DLLs that ship with Windows, so no VC++ runtime and no
  `WebView2Loader.dll`, the manifest, no automation marker), and is **not smoke-tested**:
  `smoke-windows.sh` (the packaged layout, a `.cmd` running the fake worker's `page` mode) needs
  Windows, so the published `.exe` has not run before release unless someone runs it in a guest.
  Host builds remap `$CARGO_HOME` and the checkout to `/cargo` and `/src` (through `--config`, never
  `RUSTFLAGS`, which would drop the Windows static CRT); `publish` refuses an asset holding `$HOME`
  or a macOS asset not linked against SDK 15.5. It uploads
  `grafida-{shell,launcher}-linux-{x86_64,aarch64}`, `grafida-shell-macos-{arm64,x86_64}` and
  `grafida-shell-windows-x86_64.exe`, each with a `.sha256`, once; consumers pin tag + hashes. The
  macOS asset is only linker-signed (ad hoc): the app's packager signs it inside the bundle. `build`
  also leaves a debug automation build per platform in `dist/LABEL/debug-automation/PLATFORM/` (for
  the parity runs; never a release asset), and checks it carries the marker the release checks look
  for. ⚠️ **x86_64 macOS is tested only under Rosetta 2** (ADR 0001, amended in #92): it is
  cross-compiled from arm64, and the packaged app is verified in the guests' Rosetta clones
  (`macvm.sh rosetta`, which needs `--agree-to-license`: installing Rosetta 2 accepts Apple's
  licence, so ask the operator first).
