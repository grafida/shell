# Grafida Shell

A native PHP desktop application shell.

## What it does

Grafida Shell allows you to write desktop applications using PHP and web technologies.

The application runs inside a window with a system webview, serves the app over a custom scheme (`<scheme>://app`), and supervises a **PHP worker process** that answers those requests over a framed stdio protocol. It is driven entirely by a manifest (`shell.json`). It runs on Windows, Linux, and macOS. It comes with a PHP library to facilitate application development, and a packager to put it all together in a distributable, end-user-friendly form. Distributable installers can be code-signed on Windows and macOS (requires third-party paid certificates).

This shell was originally written for [Akeeba Grafida](https://github.com/grafida/grafida), but is made deliberately generic so it can be used by anyone. It draws heavy inspiration from [Boson](https://bosonphp.com) and aims to replace it – at least the parts we are using in our own software.

## Getting started

Start with the [user documentation](docs/user/README.md): how the pieces fit together, how to start a project, the
`shell.json` manifest, the PHP and JavaScript sides, and packaging. The [architecture diagrams](docs/user/architecture.md) show how each piece fits together in a shipped application.

## System requirements

This what Grafida Shell itself needs on every supported platform. The reasoning behind each minimum is in [ADR 0001](docs/adr/0001-shell-stack.md).

On Linux, the shell's launcher alert asks users to read that documentation when the shell cannot start. Downstream projects cannot change the launcher's alert message. 

Downstream projects should copy this information in their own documentation, and ammend it with any project-specific requirements. 

### Windows

- Windows 10 22H2 or later, Windows 11
- x86_64 only; there is no Arm build
- Microsoft Edge **WebView2 Runtime** (Evergreen)

The WebView2 Runtime ships with Windows 11. It is installed on Windows 10 through Windows Update, so it's almost always already present. If it's missing, the shell shows an alert when it starts. It can be installed directly [from Microsoft](https://developer.microsoft.com/microsoft-edge/webview2/).

Tested on Windows 11 Pro 26H2 and Windows 10 Pro 22H2.

### macOS

- macOS 12 Monterey or later
- Apple Silicon (arm64) or Intel (x86_64)

On an older macOS, the system refuses to open the app. Separate build per architecture; there's no universal binary. Support for macOS on x86 is deprecated, supported only up to macOS 26 (last version with x86 support, last version with Rosetta 2 on Apple Silicon) and will be removed when macOS 28 is released.

WKWebView is part of macOS. There's nothing to install.

### Linux

- Linux on x86_64 or arm64 (aarch64)
- GNU C Library (glibc) **2.35** or later.
- **GTK 3** and **WebKitGTK 4.1**, version **2.40** or later.

Ubuntu 22.04 and later, Debian 12 and later (both with their updates), and current releases of Fedora and Arch Linux meet the requirements and are recommended.

Tested on Ubuntu 22.04 (GNOME), Fedora 43 (GNOME) and Arch Linux (KDE Plasma), all on Wayland.

Install the libraries with:

| Distribution | Command |
|---|---|
| Debian, Ubuntu and their derivatives | `sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0` |
| Fedora | `sudo dnf install webkit2gtk4.1 gtk3` |
| Arch Linux | `sudo pacman -S webkit2gtk-4.1 gtk3` |

An Ubuntu 22.04 installation does not have WebKitGTK 4.1 by default.

Also used, and normally installed with the desktop:

- **File dialogs** go through the XDG Desktop Portal, so they need a portal backend for the desktop:
  `xdg-desktop-portal-gnome`, `xdg-desktop-portal-kde` or `xdg-desktop-portal-gtk`. Without one,
  the dialogs fall back to `zenity`. The wlroots backend (`xdg-desktop-portal-wlr`) does not offer
  file dialogs.
- **Spell checking** needs a Hunspell dictionary for each language the user writes in. For English
  (United States): `hunspell-en-us` on Debian and Ubuntu, `hunspell-en-US` on Fedora,
  `hunspell-en_us` on Arch Linux. Without one, spell check does nothing (no red squiggly lines).
- **The launcher's alert** is shown with the first of `zenity`, `kdialog`, a desktop notification
  (`notify-send`) or `xmessage` that works. Its text is always written to stderr as well.

## Development

### Stack

The shell is written in Rust with the following crates:
* [tao](https://github.com/tauri-apps/tao)  - windows and event loop
* [rfd](https://github.com/PolyMeilex/rfd) - file dialogs
* [gtk-rs](https://gtk-rs.org) - shell window on Linux
* [webkit2gtk](https://crates.io/crates/webkit2gtk) - webview on Linux
* [objc2](https://github.com/madsmtm/objc2) - using AppKit (shell window) and WebKit (WKWebView, for the webview) on macOS
* [windows](https://github.com/microsoft/windows-rs) - shell window on Windows
* [webview2-com](https://github.com/wravery/webview2-rs) - webview on Windows
 
We are not using wry or muda: the shell drives each webview itself, and builds the macOS menus with AppKit.

### Companion repositories

This repository is the Rust side of the shell. It works in step with the following repositories:
* [shell-php](https://github.com/grafida/shell-php) - Worker runtime, the PHP side of the shell
* [phpmicro](https://github.com/grafida/phpmicro) - The worker's PHP runtime
* [shell-packager](https://github.com/grafida/shell-packager) - Build, sign, and package applications for distribution

### Wire protocol

The shell communicates with the PHP working using a custom protocol.

You can find the wire protocol specification and its conformance fixtures under the `protocol/` directory. This is what both the Rust and PHP sides test against.

### Development environment

We develop Grafida Shell on Linux and macOS. Windows is not supported per se, but you do have WSL2 which is a Linux VM.

#### Building and checking the shell

The compiler is pinned in `rust-toolchain.toml`; install [rustup](https://rustup.rs) and it picks the right one when you
run `cargo` in the checkout. Python 3 is needed for the tests (the fake worker) and the protocol fixtures.

```sh
cargo build                          # the shell and the launcher (a Cargo workspace)
cargo test --workspace
cargo fmt --all --check              # run before every commit
cargo clippy --workspace --all-targets
```

What each platform needs on top of that:

* **Linux**: a C toolchain, `pkg-config` and the WebKitGTK 4.1 and GTK 3 development files. Ubuntu and Debian:
  `build-essential pkg-config libwebkit2gtk-4.1-dev libgtk-3-dev`. Fedora: `webkit2gtk4.1-devel gtk3-devel`. Arch
  Linux: `webkit2gtk-4.1 gtk3`. Ubuntu 22.04 is the oldest Linux we support, so build there when you need to check the
  binary's glibc and WebKitGTK floor.
* **macOS**: the Xcode Command Line Tools (`xcode-select --install`). Nothing else; the webview is WKWebView.
* **Windows**: the MSVC build tools (Visual Studio Build Tools, "Desktop development with C++"). The WebView2 runtime
  ships with Windows 11 and current Windows 10. You can also cross-compile from another system with
  [cargo-xwin](https://github.com/rust-cross/cargo-xwin) (strongly recommended; only method we have tested).

The debug build can be driven from scripts: build with `--features automation` and start it with `--automation` (see
`src/automation.rs`). The release builds, their checks and publishing are in [docs/dev/RELEASE.md](docs/dev/RELEASE.md).

#### Test machines

Packaged apps are tested on clean virtual machines which never see a build tool, so they behave like a user's computer.
Each platform has a script in `scripts/dev/` and a setup guide. Which host you need depends on the guest:

| Guests | Host | Script | Guide |
|---|---|---|---|
| Windows 11 (and Windows 10) | Linux x86_64 with KVM, QEMU and [quickemu](https://github.com/quickemu-project/quickemu) | `winvm.sh` | [Windows test VM](docs/dev/windows-test-vm.md) |
| Ubuntu 22.04, Fedora 43, Arch Linux (KDE Plasma), Ubuntu 22.04 arm64 (emulated, headless) | Linux x86_64 with KVM, QEMU and quickemu | `linuxvm.sh` | [Linux test VMs](docs/dev/linux-test-vms.md) |
| macOS 12 and macOS 27, each also under Rosetta 2 | Apple silicon Mac with [tart](https://tart.run) | `macvm.sh` | [macOS test VMs](docs/dev/macos-test-vms.md) |

Notes:

* macOS guests only run on Apple silicon, and Apple's licence allows two at a time. An Intel build is tested under
  Rosetta 2 in a guest.
* The Linux and Windows guests are set up unattended from their vendors' images and snapshotted as `clean`; the macOS
  guests need Setup Assistant done by hand once. Every test run starts from the clean state.
* A GUI program on a Windows guest must be started in the desktop session (`winvm.sh run`), not directly over SSH.
* `scripts/dev/parity-linux.sh`, `parity-macos.sh` and `parity-windows.sh` run the shell and the automation checks in
  `tests/apps/` on a desktop, or in a guest, and print a PASS/FAIL table.
* `scripts/dev/macvm.conf` (copy `macvm.conf.sample`) sets where tart keeps the VMs and the macOS restore images; it
  is git-ignored.

#### Testing: what to run, and in which order

Go from the cheapest and fastest check to the most expensive. Stop at the first failure: a later stage tests nothing
useful if an earlier one fails.

1. **While you work**, on your own machine (seconds):
   ```sh
   cargo test --workspace
   cargo clippy --workspace --all-targets
   ```
   `tests/` holds the supervisor, protocol-fixture and packaged-layout tests; they use the fake worker
   (`tests/workers/fake_worker.py`). `tests/real_workers.rs` also runs the real shell-php and Grafida workers, but only
   when their repositories are cloned next to this one.
2. **Before every commit**: `cargo fmt --all --check`, then the two commands above.
3. **Before a release**, on the maintainer's Mac (about three minutes):
   ```sh
   scripts/release/local-release.sh check
   ```
   This is what CI used to run: clippy with `-D warnings` (with and without `automation`), the guard that a release
   build refuses the `automation` feature, and the tests, for Linux (in an Ubuntu 22.04 container), macOS and Windows
   (compiled only; it needs Windows to run). `build` does not run `check` for you.
4. **Build the release candidate**: `scripts/release/local-release.sh build` (about four minutes). It builds every
   platform, runs the binary checks (glibc and library floors, macOS minimum version, Windows imports, no automation
   code in a release) and a headless smoke test of each shell it can run. It also leaves debug builds with the
   automation channel in `dist/LABEL/debug-automation/PLATFORM/` for the next stages.
5. **Parity checks on a desktop**, with those debug builds: `scripts/dev/parity-linux.sh` or `scripts/dev/parity-macos.sh`
   run the shell against the apps in `tests/apps/` and Grafida, with throwaway user directories, and print a PASS/FAIL
   table (`QUICK=1` shortens the idle-CPU sample). Use `SHELL_BIN=` to point them at a prebuilt debug build.
6. **Clean machines**, last, with the packaged app: this is the only stage that tests what a user installs. Start each
   run from the `clean` state (`linuxvm.sh snapshot DISTRO apply clean`, `winvm.sh snapshot VM apply clean`,
   `macvm.sh fresh NAME`). Within a guest the order matters, because some steps change it:
   1. Install and start the app as a user would, before any developer tool goes on the machine.
   2. Check the floors (macOS: a copy with a too-high minimum system version must be refused).
   3. Only then add what the scripts need (Python; on macOS the Command Line Tools) and run the parity script with
      `GRAFIDA_PACKAGED=<the installed app>` and `SHELL_BIN=<the debug build for that platform>`
      (`parity-windows.sh` runs on the host and drives the Windows guest).
   4. The checks no script can make, by hand (context menus, native dialogs, drag and drop, and so on), then the
      idle-CPU check on the release app with your hands off the machine.

   Test the floor of each platform (Ubuntu 22.04, macOS 12, Windows 10) and the newest (Fedora, Arch, macOS 27,
   Windows 11). x86_64 macOS is only ever tested under Rosetta 2, and the Windows `.exe` is not smoke-tested by
   `build`: run it in a Windows guest before you publish. Only then tag and publish
   ([docs/dev/RELEASE.md](docs/dev/RELEASE.md)).


## Licence

MIT — see [LICENSE](LICENSE).
