# grafida/shell

A small, **app-agnostic native desktop shell**. It owns the window and the system webview, serves
the app over a custom scheme (`<scheme>://app`), and supervises a **PHP worker process** that answers
those requests over a framed stdio protocol. It is driven entirely by a manifest (`shell.json`).

> **Status: Phase 1.** The stack has been chosen ([ADR 0001](docs/adr/0001-shell-stack.md)) and the
> [wire protocol v1](protocol/v1.md) is specified, with conformance fixtures and a Rust codec. The
> shell itself is a skeleton until Phase 2. Design and roadmap: [grafida/grafida#82](https://github.com/grafida/grafida/issues/82).

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

What the shell needs, as of this version, on every platform it supports. These are the **shell's**
minimums; an app built on it may need more (Grafida, for example, needs `secret-tool` on Linux for its
secret store). Downstream projects copy this section into their own documentation and add their
app's requirements to it. On Linux, the shell's launcher alert asks users to read that
documentation when the shell cannot start. The reasoning behind each minimum is in
[ADR 0001](docs/adr/0001-shell-stack.md).

### Windows

- Windows 10 22H2 or later, or Windows 11, on an x64 (x86_64) processor. There is no Arm build.
- The Microsoft Edge **WebView2 Runtime** (Evergreen). It ships with Windows 11 and arrives through
  Windows Update on Windows 10, so it is almost always there already. If it is missing, the shell
  shows an alert when it starts; it can be installed
  [from Microsoft](https://developer.microsoft.com/microsoft-edge/webview2/).

Tested on Windows 11 Pro 26H2 and Windows 10 Pro 22H2.

### macOS

- macOS 12 Monterey or later. On an older macOS, the system refuses to open the app.
- Apple Silicon (arm64) or Intel (x86_64), one build per architecture: there is no universal
  binary, so pick the build that matches the Mac.
- Nothing to install: WKWebView is part of macOS.

> [!NOTE]
> **Intel Macs have a limited lifetime.** macOS 27 is the last macOS for Intel Macs. When macOS 28
> drops Intel and Rosetta 2, the Intel build is deprecated; when macOS 29 is released, it is
> discontinued. The Intel build is tested under Rosetta 2 on Apple Silicon only.

### Linux

- An x86_64 or arm64 (aarch64) computer with the GNU C Library (glibc) **2.35** or later.
- **GTK 3** and **WebKitGTK 4.1**, version **2.40** or later.

Ubuntu 22.04 and later, Debian 12 and later (both with their updates), and current releases of
Fedora and Arch Linux are new enough. Tested on Ubuntu 22.04 (GNOME), Fedora 43 (GNOME) and Arch
Linux (KDE Plasma), all on Wayland.

Install the libraries with:

| Distribution | Command |
|---|---|
| Debian, Ubuntu and their derivatives | `sudo apt install libwebkit2gtk-4.1-0 libgtk-3-0` |
| Fedora | `sudo dnf install webkit2gtk4.1 gtk3` |
| Arch Linux | `sudo pacman -S webkit2gtk-4.1 gtk3` |

A new Ubuntu 22.04 installation does not have WebKitGTK 4.1. If a library is missing or too old,
the shell does not start: its launcher shows an alert naming what is missing and the command that
installs it, and asks the user to read the app's documentation for its minimum requirements.

Also used, and normally installed with the desktop:

- **File dialogs** go through the XDG Desktop Portal, so they need a portal backend for the desktop:
  `xdg-desktop-portal-gnome`, `xdg-desktop-portal-kde` or `xdg-desktop-portal-gtk`. Without one,
  the dialogs fall back to `zenity`. The wlroots backend (`xdg-desktop-portal-wlr`) does not offer
  file dialogs.
- **Spell checking** needs a Hunspell dictionary for each language the user writes in. For English
  (United States): `hunspell-en-us` on Debian and Ubuntu, `hunspell-en-US` on Fedora,
  `hunspell-en_us` on Arch Linux. Without one, nothing is underlined.
- **The launcher's alert** is shown with the first of `zenity`, `kdialog`, a desktop notification
  (`notify-send`) or `xmessage` that works. Its text is always written to stderr as well.

## Licence

MIT — see [LICENSE](LICENSE).
