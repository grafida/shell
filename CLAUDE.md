# grafida/shell — AI assistant orientation

A small, app-agnostic native desktop shell in **Rust (wry/tao/rfd/muda)**: window + system webview,
a custom scheme serving the app at `<scheme>://app`, a supervised PHP **worker** process spoken to
over length-prefixed frames on stdin/stdout, native endpoints under `/_native/*`, menus and failure
alerts. Grafida is consumer #1 (umbrella issue grafida/grafida#82); nothing Grafida-specific may be
hard-coded — everything comes from the manifest (`shell.json`).

Read `docs/adr/0001-shell-stack.md` before changing the webview glue.

## Rules that are not guessable from the code

- **The origin is sacred.** The page origin must be byte-identical — `boson://app` for Grafida — on
  every platform. Stored documents embed it. Never accept wry's `http://<scheme>.localhost` mapping on
  Windows: the shell creates the WebView2 environment itself with a
  `CoreWebView2CustomSchemeRegistration` and handles `WebResourceRequested` for `<scheme>://*` itself.
- **We own the scheme bridge where wry's falls short.** wry's Linux body reader stops at the first
  short read and collapses repeated headers; register the scheme on the WebKit `WebContext`
  ourselves, read to EOF in bounded chunks, keep repeated headers, mark the scheme secure **and**
  CORS-enabled.
- **Responses are completed on the UI thread.** Worker I/O runs off it; answers are posted back
  through the event loop. Never block the UI thread on the worker.
- **The protocol is specified here** (`protocol/`), with fixtures both sides run. A failing fixture
  is a question about which side is wrong — never a file to regenerate. `PROTOCOL_VERSION` must
  equal `Grafida\Shell\Protocol::VERSION` in grafida/shell-php.
- **Menu accelerators fire before the webview sees the key.** Never bind a chord the manifest
  reserves (Grafida: `CmdOrCtrl+Shift+V`).
- Windows builds use a static CRT (`-C target-feature=+crt-static`).

## Conventions

- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` must pass.
- `spikes/` is frozen Phase 0 evidence; it is excluded from the workspace and not maintained.
- Keep a terse `CHANGELOG.md` entry per change (Grafida's `!` `+` `-` `~` `# [HIGH|MEDIUM|LOW]` style).
- Commit directly on `main`.
