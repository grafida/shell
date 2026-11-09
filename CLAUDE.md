# grafida/shell — AI assistant orientation

A small, app-agnostic native desktop shell in **Rust (wry/tao/rfd/muda)**: window + system webview,
a custom scheme serving the app at `<scheme>://app`, a supervised PHP **worker** process spoken to
over length-prefixed frames on stdin/stdout, native endpoints under `/_native/*`, menus and failure
alerts. Grafida is consumer #1 (umbrella issue grafida/grafida#82); nothing Grafida-specific may be
hard-coded — everything comes from the manifest (`shell.json`).

Read `docs/adr/0001-shell-stack.md` before changing the webview glue. To run anything on Windows,
use the test VM in `docs/dev/windows-test-vm.md` (`ssh winvm`). A GUI program must be started in
the desktop session through an interactive scheduled task, never directly over SSH.

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
- **The protocol is specified here** (`protocol/v1.md`), with fixtures both sides run
  (`tests/protocol_fixtures.rs` here; a vendored, checksummed copy in grafida/shell-php). A failing
  fixture is a question about which side is wrong — never a file to regenerate.
  `scripts/protocol/make-fixtures.py` is the fixtures' **independent** writer (it must never call
  into either codec) and only ever *adds* cases; changing an existing `.bin`/`.json` is a protocol
  change that needs its reason in the commit message. `PROTOCOL_VERSION` (`src/protocol.rs`) must
  equal `Grafida\Shell\Protocol::VERSION` in grafida/shell-php.
- **The order of checks in v1.md §4 is normative**, because it decides which error code a byte
  sequence produces on both sides. The header is validated *before* `B` is read; a body on
  `ready`/`shutdown` is `bad_header` *before* the body limit applies; both length limits are
  enforced before reading what they announce. Never scan for the next magic after a bad frame.
- **Every integer on the wire is within 0 … 2^53 − 1**, not merely ids: PHP has no `u64`, so an
  integer Rust would accept and PHP would turn into a float is a split-brain fixture waiting to
  happen.
- **Menu accelerators fire before the webview sees the key.** Never bind a chord the manifest
  reserves (Grafida: `CmdOrCtrl+Shift+V`).
- Windows builds use a static CRT (`-C target-feature=+crt-static`).

## Conventions

- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` must pass.
- `spikes/` is frozen Phase 0 evidence; it is excluded from the workspace and not maintained.
- Keep a terse `CHANGELOG.md` entry per change (Grafida's `!` `+` `-` `~` `# [HIGH|MEDIUM|LOW]` style).
- Commit directly on `main`.
