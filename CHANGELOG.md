shell 0.4.2
================================================================================
+ User documentation in `docs/user`: getting started, the PHP and JavaScript sides, the manifest, packaging
+ Manifest `webview.frameGuard` (default `true`): set it to `false` to let foreign pages frame the app's pages
~ Docs, protocol fixtures and examples use the `shell://` scheme; `boson://` is gone (the manifest names the scheme)
~ A request for the app's scheme with a host other than `app` is answered `404` and never reaches the worker
# [MEDIUM] Embedded foreign pages could send Origin-less GET requests to the worker
# [LOW] A foreign page embedded in the app could show the app's own pages inside its frame
# [LOW] Windows: a URL with a quote or a trailing backslash could break out of the browser's command line

shell 0.4.1
================================================================================
- Linux, KDE Plasma: the title bar stayed light in a dark app; KWin is now told the colour scheme (Wayland and X11)
- `linuxvm.sh start` failed while a Windows guest was running (both wanted SSH port 22221)
~ Linux: the launcher's alert asks the user to read the app's documentation for its minimum requirements
~ README: the shell's system requirements on Windows, macOS and Linux, for apps to copy into their docs

shell 0.4.0
================================================================================
+ macOS x86_64 (Intel, 12 or later): `grafida-shell-macos-x86_64` in releases, checked and smoke-tested on Intel
+ `macvm.sh rosetta`: a Rosetta 2 clone of each clean macOS guest, where x86_64 apps are tested
+ `parity-macos.sh`: runs an x86_64 shell under Rosetta 2, and proves which architecture the shell and the bundle run as
+ `ui.ps1 set`: puts a value in a field (UI Automation's value pattern), as a search box needs
- `winvm.sh put` failed on a freshly re-taken `clean` guest (no `C:\winvm`)
- `winvm.sh stop-run` left a program running when its pid file was written a fraction of a millisecond before Windows recorded the start
~ CI: the macOS gate also lints `x86_64-apple-darwin` (clippy with and without automation, the release guard)

shell 0.3.1
================================================================================
+ `parity-windows.sh`: the window's own icons (packaged), from the installed release shell
+ Desktop helpers for the clean-machine run: `ui.ps1` (read and press any window's controls, through UI Automation, in the guest's language), `keys.ps1`, `window-icon.ps1`
+ `parity-windows.sh`: Ctrl+A/C/X/V and Ctrl+Z after a paste, AI over HTTPS, the open-folder child, the menus' items, process names and icons (packaged)
+ `parity-windows.sh`: NOTE rows, reported in the record but not counted (Ctrl+Z after Ctrl+X, an editor bug on Chromium)
+ `winvm.sh put`, and a desktop helper that reads the clipboard back
- Windows 10: every launch showed the "working in background" pointer for about 5 s; the worker is started with `CreateProcessW` and `STARTF_FORCEOFFFEEDBACK`
- Windows: the window's title bar and Alt+Tab showed the generic icon instead of the app's (the executable's first icon)
- `winvm.sh`: `start` held a pipe open for as long as the VM ran; an update pass killed by Windows finishing an update with a second restart waited 2 hours; `watch-start` and `idle` failed on a guest without `C:\winvm`
~ Windows: the worker inherits only its three pipes, and its main thread comes back from `CreateProcessW` (no Toolhelp snapshot)
~ `scripts/dev/macguest/test-server.py --tls CERT KEY` serves HTTPS

shell 0.3.0
================================================================================
+ Windows (x86_64, 10 22H2 or later): apps run in a WebView2 the shell creates itself, on a real `boson://` scheme; dev mode for now
+ Windows: the worker joins a Job Object (kill on close) before it runs, and is started without a console window
+ Windows: browser shortcuts, zoom and swipe navigation are off; the context menu keeps editing and spelling items only
+ Windows: a WebView2 browser-process exit is a native alert; a crashed or hung renderer reloads the page
+ Windows: a release shell drops the `WEBVIEW2_*` variables that change the browser, so no debugging port can be injected
+ `scripts/dev/winvm.sh`: Windows 11 and 10 VMs: updates, snapshots, pinned dev tooling, programs in the desktop session, the automation channel over SSH
+ Windows: an application manifest (Common Controls 6, per-monitor DPI, Windows 10/11), linked from a `.res` written by `build.rs`
+ Releases include `grafida-shell-windows-x86_64.exe`, checked (GUI subsystem, system DLLs only, the manifest) and smoke-tested on `windows-2022`
+ CI: a Windows job (fmt, clippy with and without automation, the release guard, the tests)
+ `scripts/dev/parity-windows.sh`: the automatable half of the Windows parity column, driven from the host over SSH
+ `winvm.sh watch-start`/`watch-stop`: every console window shown on a guest's desktop, recorded by a WinEvent hook
+ The automation client takes guest paths as they are (`SHELL_AUTOMATION_REMOTE=1`) and gains `click`
- Windows: the context menu kept no spelling suggestions (WebView2 names them `spellcheck`, lower case)
~ The automation channel's `key` (Windows) brings the window forward first, and sends nothing if it cannot
+ Windows debug builds: `SHELL_WINDOWS_LANGUAGE` overrides the language passed to WebView2
~ `winvm.sh run`/`stop-run` no longer take a reused pid for the program they started; guest output is UTF-8
~ The Windows release smoke test's clean-up waits for WebView2 to let go of its profile instead of failing a passing run
~ The idle-CPU measurements count only real children of the shell (`ParentProcessId` is never updated when a parent exits)
+ Windows automation: real input (`type`, `mouse` left/right with modifiers, `drag`), `window` (title, frame, DPI, dark title bar) and `record off|on`
+ `winvm.sh desktop`: PowerShell helpers in the guest's desktop session (screenshots, native dialogs, clipboard, colour mode, Explorer, cursor, close button); `winvm.sh get`
~ `parity-windows.sh` covers what was a by-hand pass: spelling, real typing and undo, formatted paste, theme and title bar, native dialogs, an Explorer drag, links, reveal, AI streaming, the launch cursor, the close button, Inspect
~ Packaged mode on Windows: a worker named without an extension is `<name>.exe`, and no path is verbatim (`\\?\`)
+ `scripts/dev/macguest/`: a test server, the ATS matrix and the release app's idle CPU, for packaged runs in the guests

shell 0.2.0
================================================================================
+ `scripts/dev/parity-linux.sh` runs against a packaged app (`GRAFIDA_PACKAGED`) with a prebuilt debug shell (`SHELL_BIN`)
+ CI uploads a debug automation build per arch for verifying packaged apps; never a release asset
+ `scripts/dev/linuxvm.sh`: clean Ubuntu 22.04, Fedora 43 and Arch VMs, installed unattended by their own installers
+ `scripts/dev/macvm.sh`: clean macOS 12 and 27 VMs, restored by tart from Apple's IPSWs pinned by SHA-256
+ ADR 0001: the macOS webview path (the shell owns WKWebView through objc2), measured on macOS 12 and 27
+ macOS (arm64, 12 or later): apps run in a WKWebView the shell builds itself, in dev mode for now
+ macOS: app, Edit (with Spelling and Grammar) and Window menus; About sends `shell:about` to the page
+ macOS: a Blob, File or FormData-with-a-file body to the app fails with a TypeError instead of arriving empty
+ `scripts/dev/parity-macos.sh`: the automatable half of the macOS parity column, in dev mode or packaged
+ `SHELL_ALERTS=stderr` keeps failure alerts to stderr, for tests and headless runs
+ Releases include `grafida-shell-macos-arm64`, checked for the macOS 12 floor and smoke-tested in a bundle layout
~ The parity script waits for the app's own page, not the window's placeholder, and names a missing keyboard focus
~ A packaged worker is expected beside the shell (in a macOS bundle, `Contents/MacOS`), not beside the manifest

shell 0.1.0
================================================================================
+ Repository skeleton: crate, CI, licence, protocol and schema placeholders
+ ADR 0001: Rust + wry/tao/rfd/muda; deployment floors; per-arch builds; MIT
+ Phase 0 spikes (wry, saucer) with the measurement harness
+ ADR 0001: Windows runtime results; the real boson:// scheme works on WebView2 without a wry fork
+ ADR 0001: native dialogs verified on KDE Plasma (Wayland) and Windows; no open items
+ Developer docs: Windows 11 test VM on a Linux host, with `scripts/dev/winvm-unattend.py`
+ Wire protocol v1 specification (`protocol/v1.md`) with 97 conformance fixtures and their independent writer
+ Rust protocol codec (`src/protocol.rs`), tested against every fixture
+ `shell.json` manifest with a JSON Schema; an invalid manifest is refused with every offending member named
+ Worker supervisor: `ready` handshake, 502 `worker_restarted` and respawn on a crash, gives up after too many
+ Linux window and WebKitGTK webview serving the app at `<scheme>://app`, sandboxed, bodies up to the manifest limit
+ Navigation policy: off-origin links open in the OS browser; nothing else can navigate the app away
+ Native endpoints (`_native/*`): file dialogs that hand out per-session tokens, open-url, reveal, window title/theme
+ Failure alerts with the worker's last stderr lines; rotated log of the shell and the worker
+ The shell reads the clipboard for the app, so paste-as-plain-text needs no clipboard tool installed
+ Files dropped on the window reach the app as tokens, despite the sandboxed webview
+ `--dev [APP_DIR]` runs an app from its source tree with the manifest's `dev.worker` command
+ Debug-only automation channel (`--automation`) for driving the real window from scripts
+ Linux release workflow: shell + static launcher for x86_64 and aarch64, checked and smoke-tested headless
+ Static Linux launcher: names missing WebKitGTK/GTK libraries or a too-old glibc in an alert
+ `webview.dmabufRenderer` manifest member; "auto" turns the DMA-BUF renderer off with NVIDIA's driver
~ The window's Wayland app_id and X11 class are the manifest's `id`, so docks find `<id>.desktop`
~ A packaged app with a missing worker says where the worker was expected
~ The log names spell-check languages that have no dictionary installed
