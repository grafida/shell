# grafida/shell

A small, **app-agnostic native desktop shell**. It owns the window and the system webview, serves
the app over a custom scheme (`<scheme>://app`), and supervises a **PHP worker process** that answers
those requests over a framed stdio protocol. It is driven entirely by a manifest (`shell.json`).

> **Status: Phase 0.** The stack has been chosen ([ADR 0001](docs/adr/0001-shell-stack.md)); the
> shell itself is a skeleton. Design and roadmap: [grafida/grafida#82](https://github.com/grafida/grafida/issues/82).

- Stack: Rust + [wry](https://github.com/tauri-apps/wry) / [tao](https://github.com/tauri-apps/tao) /
  [rfd](https://github.com/PolyMeilex/rfd) / [muda](https://github.com/tauri-apps/muda)
- Platforms: Linux (WebKitGTK 4.1 ≥ 2.40), macOS 12+ (WKWebView), Windows 10 1809+ (WebView2)
- Companion repositories: [shell-php](https://github.com/grafida/shell-php) (worker runtime),
  [shell-packager](https://github.com/grafida/shell-packager) (build/sign/package),
  [phpmicro](https://github.com/grafida/phpmicro) (the worker's PHP runtime)

Developer setup: [Windows 11 test VM on a Linux host](docs/dev/windows-test-vm.md).

This repository is also the home of the **wire protocol specification and its conformance
fixtures** (`protocol/`), which both the Rust and PHP sides test against.

## Licence

MIT — see [LICENSE](LICENSE).
