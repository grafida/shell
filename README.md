# Grafida Shell

A native PHP desktop application shell.

## What it does

Grafida Shell allows you to write desktop applications using PHP and web technologies.

The application runs inside a window with a system webview, serves the app over a custom scheme (`<scheme>://app`), and supervises a **PHP worker process** that answers those requests over a framed stdio protocol. It is driven entirely by a manifest (`shell.json`). It runs on Windows, Linux, and macOS.

This shell was originally written for [Akeeba Grafida](https://github.com/grafida/grafida), but is made deliberately generic so it can be used by anyone. It draws heavy inspiration from [Boson](https://bosonphp.com) and aims to replace it – at least the parts we are using in our own software.

### Getting started

Start with the [user documentation](docs/user/README.md): how the pieces fit together, how to start a project, the
`shell.json` manifest, the PHP and JavaScript sides, and packaging. The [architecture diagrams](docs/user/architecture.md)
show how each piece fits together in a shipped application.

You can read the [architecture decisions](docs/adr/0001-shell-stack.md) and the [wire protocol v1](protocol/v1.md) for more information.

- Stack: Rust + [wry](https://github.com/tauri-apps/wry) / [tao](https://github.com/tauri-apps/tao) /
  [rfd](https://github.com/PolyMeilex/rfd) / [muda](https://github.com/tauri-apps/muda)
- Platforms: Windows 10 22H2+ (WebView2), macOS 12+ (WKWebView), Linux (WebKitGTK 4.1 ≥ 2.40); see
  [System requirements](#system-requirements)
- Companion repositories: [shell-php](https://github.com/grafida/shell-php) (worker runtime),
  [shell-packager](https://github.com/grafida/shell-packager) (build/sign/package),
  [phpmicro](https://github.com/grafida/phpmicro) (the worker's PHP runtime)

Developer setup: [Windows 11 test VM on a Linux host](docs/dev/windows-test-vm.md).

This repository is also the home of the **wire protocol specification and its conformance
fixtures** (`protocol/`), which both the Rust and PHP sides test against.

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

## Licence

MIT — see [LICENSE](LICENSE).
