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
- **The manifest's contract is `schema/shell.schema.json`**, compiled into the binary. `Manifest::parse()`
  validates against it *before* serde sees the JSON, so errors name the member in the schema's terms.
  The schema's `default` annotations must equal the serde defaults in `src/manifest.rs`;
  `schema_defaults_match_the_shell_defaults` walks the schema and fails on any drift. Every member is
  `additionalProperties: false`: a typo is an error, never a silently ignored setting.
- **The worker is spawned from the supervisor thread, and must stay so.** `PR_SET_PDEATHSIG` fires
  when the *thread* that forked the child exits, not the process. Spawn from a short-lived thread and
  the worker is SIGTERMed the moment that thread ends. The supervisor thread is one event loop over
  one channel (submissions, frames from a stdout-reader thread, end-of-stdout); the UI thread only
  submits and never waits. A crash answers the in-flight request `502 {"error":"worker_restarted"}`
  and respawns; one crash more than `restart.max` inside `restart.windowSeconds` is fatal, and so is
  any failure **before the first `ready`** (a worker that cannot start once will not start on retry).
  The shell gives the worker its body limit as `SHELL_MAX_BODY_BYTES`.
- **`tests/workers/fake_worker.py` is a third, independent implementation of the worker side**, for
  the misbehaviour the supervisor must survive (never ready, wrong protocol, garbage, crashes, ignoring
  EOF). Like the fixture writer, it must never share code with either codec. `tests/real_workers.rs`
  runs shell-php's echo worker and Grafida's `bin/worker.php` (sibling clones, or `SHELL_PHP_DIR` /
  `GRAFIDA_DIR`) and skips with a note when they are absent, as on CI. Grafida's worker runs with every
  per-user directory pointed at a temp dir, never the real data.
- **On Linux the shell builds the WebKitGTK webview itself; wry is not used there.** wry's WebKit
  context is crate-private, so the scheme, the data directories and the sandbox could only be set
  after wry had created the webview, and `set_sandbox_enabled()` is a *fatal* error once a web process
  exists. `src/linux/` does it in WebKit's order: context (data manager, sandbox, spelling, scheme,
  security manager: secure **and** CORS-enabled) → webview → load the start URL after `ready`.
  wry stays the plan for macOS/Windows (ADR 0001). The sandbox is verifiably on (`WebKitWebProcess`
  under `bwrap`); dialogs run in the UI process, outside it.
- **Every `boson://` request is parked in a UI-thread map and completed from it on the UI thread**
  (`linux/scheme.rs`). The body is read asynchronously on the main loop in 1 MiB chunks **to EOF**,
  and reading stops past `maxBodyBytes` → `413`. The responder only posts `(id, response)` through
  the tao event loop. `content-length`/`transfer-encoding` from the worker are dropped (B is the
  length).
- **`src/bridge.rs` is the platform-neutral half and is where request policy lives**: a foreign
  `Origin` → `403 cross_origin` (an article's `<iframe>` must not drive the app's API); `/_native/*`
  never reaches the worker; `x-shell-file-path` from the page is always dropped and an `x-shell-file`
  token is resolved into it (the token itself is not forwarded); `no-store` is added under the
  manifest's `noStorePaths` when the worker sent no `cache-control`/`expires`.
- **The navigation policy is layered, because WebKitGTK cannot tell a main-frame navigation from a
  subframe one** (`src/navigation.rs`). A navigation *action* off-origin is cancelled only when it
  carries a **user gesture** (then http(s) opens in the OS browser); a gesture-less one may be an
  embed loading in its `<iframe>` and is let through, and the main-frame **response** check
  (`is_main_frame_main_resource`) refuses anything off-origin before it renders. "User" is the
  gesture alone: `el.click()` from a script is a `LinkClicked` navigation too. `create` (window.open)
  never makes a window. Downloads and the context-menu items that navigate, reload, open windows or
  download are removed.
- **The automation channel (`src/automation.rs`) exists only with the `automation` feature in a
  debug build** (a release build with the feature is a `compile_error!`, and CI checks it), and only
  opens with `--automation`. Two traps when writing checks with it: a script it injects **counts as
  a user gesture** in WebKit, and a gesture survives about a second through timers, so a no-gesture
  case must run from a `setTimeout` well past that; and while it is open the shell **records**
  external URLs instead of launching the browser (`opened`). `tests/apps/echo/` (the fake worker in
  echo mode, behind a 60 MiB limit) holds the in-webview checks: `bridge.js`, `navigation.js`,
  `dialogs.js`.
- **Menu accelerators fire before the webview sees the key.** Never bind a chord the manifest
  reserves (Grafida: `CmdOrCtrl+Shift+V`).
- Windows builds use a static CRT (`-C target-feature=+crt-static`).

## Conventions

- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` must pass.
- `spikes/` is frozen Phase 0 evidence; it is excluded from the workspace and not maintained.
- Keep a terse `CHANGELOG.md` entry per change (Grafida's `!` `+` `-` `~` `# [HIGH|MEDIUM|LOW]` style).
- Commit directly on `main`.
