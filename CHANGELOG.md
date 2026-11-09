shell 0.1.1 (unreleased)
================================================================================
+ `scripts/dev/parity-linux.sh` runs against a packaged app (`GRAFIDA_PACKAGED`) with a prebuilt debug shell (`SHELL_BIN`)
+ CI uploads a debug automation build per arch for verifying packaged apps; never a release asset

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
