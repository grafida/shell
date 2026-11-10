# grafida/shell — AI assistant orientation

A small, app-agnostic native desktop shell in **Rust (tao/rfd, with the system webview built by the
shell itself: WebKitGTK on Linux, WKWebView through objc2 on macOS)**: window + system webview,
a custom scheme serving the app at `<scheme>://app`, a supervised PHP **worker** process spoken to
over length-prefixed frames on stdin/stdout, native endpoints under `/_native/*`, menus and failure
alerts. Grafida is consumer #1 (umbrella issue grafida/grafida#82); nothing Grafida-specific may be
hard-coded — everything comes from the manifest (`shell.json`).

Read `docs/adr/0001-shell-stack.md` before changing the webview glue. To test a packaged app on clean
Linux machines, use `scripts/dev/linuxvm.sh` (`docs/dev/linux-test-vms.md`); on clean macOS 12 and 27
guests, `scripts/dev/macvm.sh` (`docs/dev/macos-test-vms.md`). To run anything on Windows,
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
  wry stays the plan for Windows only (ADR 0001). The sandbox is verifiably on (`WebKitWebProcess`
  under `bwrap`); dialogs run in the UI process, outside it.
- **On macOS the shell owns WKWebView through objc2; neither wry nor muda is used** (ADR 0001,
  grafida/grafida#88). `src/macos/` builds its own `WKWebViewConfiguration` (the `WKURLSchemeHandler`,
  devtools, the Blob guard), a `WKWebView` subclass (`ShellWebView`: file drops, the context menu),
  one delegate object for `WKNavigationDelegate` + `WKUIDelegate`, and the AppKit menu bar. The scheme
  handler parks each task in a main-thread map; `stopURLSchemeTask:` removes it, so a late answer is
  dropped (touching a stopped task raises). An `HTTPBody` is checked against `maxBodyBytes` *before*
  it is copied; an `HTTPBodyStream` is read to EOF in 1 MiB chunks on a reader thread, which posts the
  request back to the main thread for `Bridge::handle`. Navigation: macOS knows the target frame, so a
  nil target is a new window, the main frame gets `navigation::for_main_frame_navigation` (nothing
  off-origin, ever) and a subframe the layered Linux rule. The gesture comes from WebKit's private
  `_isUserInitiated` behind `respondsToSelector:` (absent = no gesture, the side that never opens a
  browser), because `navigationType` is `LinkActivated` for a script's `el.click()` too.
- **WKWebView hands a scheme handler no body for a `Blob`/`File` or a `FormData` with a file part**
  (ADR 0001). A document-start script (`webview::blob_guard`, macOS only, all frames, page world)
  makes `fetch()` reject and `XMLHttpRequest.send()`/`sendBeacon()` throw a `TypeError` for such a
  body to the app's own origin; `tests/apps/echo/blob.js` pins it on both platforms.
- **macOS has no parent-death signal: the worker's lifetime is stdin EOF only.** An idle worker exits
  the moment the shell dies; a busy one when its request ends. `tests/supervisor.rs` checks both
  (Linux keeps the PDEATHSIG test), and so does `parity-macos.sh` against the real worker.
- **macOS menus come from `src/menu.rs`, a platform-neutral table** tested on every platform: the
  app menu (About posts a `shell:about` DOM event, no native panel; Quit goes through the shell's own
  stop path, not `terminate:`), Edit with the Spelling and Grammar selectors (no `pasteAsPlainText:`)
  and Window. A reserved chord loses its key equivalent, and the live menu bar is walked once more
  after it is installed, since AppKit adds items. **An uppercase AppKit key equivalent implies Shift.**
  Dock ▸ Quit and logout arrive as tao's `LoopDestroyed`, which runs the same stop path.
- **On macOS the shell routes menu shortcuts itself** (`src/macos/keys.rs`). WKWebView claims every
  Cmd/Ctrl key for the page and, as a subview of tao's view, never hands an unhandled one back, so
  without this no menu shortcut fires at all. **Edit shortcuts go to the page first** (undo, redo,
  cut, copy, paste, select all, Spelling): the key is parked, a document-start script in every frame
  reports `defaultPrevented` once the page's handlers have run, and only an uncancelled key goes to
  the menu bar (after 1 s without a report it goes anyway). That is what lets TinyMCE's own undo
  history undo a paste, while a plain field still gets native undo; the webview's native Undo knows
  nothing of an edit made by script. **App and Window shortcuts go to the menu first** (a page must
  not block Cmd+Q). Anything the menu does not bind (Cmd+Shift+V, Cmd+B/I/K) goes to the page.
  The automation channel's `menu` (the live menu bar; `perform` clicks an item) and `key` (a queued
  key press; it activates the app) commands check all of this in `parity-macos.sh`.
- **`WebContinuousSpellCheckingEnabled` is set in the shell's own defaults domain on every macOS
  launch** (`macos::prepare`), as Grafida's `MacSpellCheck` does under Boson: WebKit reads it once,
  and the page's `spellcheck` attribute is then the switch. Defaults go through cfprefsd, which
  ignores `HOME` and `CFFIXED_USER_HOME`: a dev run writes `~/Library/Preferences/grafida-shell.plist`.
- **Selectors and symbols newer than macOS 12 are looked up at run time** (`isInspectable` behind
  `respondsToSelector:`); UTIs and WebKit menu identifiers are string literals. Builds link for 12.0
  (`.cargo/config.toml`, checked from `LC_BUILD_VERSION` in CI).
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
- **The shell reads the clipboard for the app, and takes file drops for it**, because it is the
  process that owns the window and the page is sandboxed. A request carrying `x-shell-clipboard:
  read` gets the clipboard's text (GTK's own clipboard: no `wl-paste`/`xclip`, no prompt) as its
  **body**, with `x-shell-clipboard: text`; any other value of that header from the page is dropped,
  so a page cannot pass its own text off as the clipboard. A **file drag** (`text/uri-list` without
  `text/html`) is taken in GTK's drag signals *before* WebKit's class handlers (they are RUN_LAST),
  because the sandboxed web process cannot read the user's files — WebKit would hand the page only
  the `file://` URI. The shell issues tokens for the files and dispatches `shell:drop`
  (`{files: [{token, name, size}], x, y}`) into the page; a link or image dragged out of a browser
  (it offers `text/html`) and any drag within the page stay WebKit's. The automation channel's
  `drop` command delivers a drop the same way, since a native drag cannot be scripted on Wayland.
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
- **`scripts/dev/parity-macos.sh` is its macOS counterpart** (grafida/grafida#88), with the same
  checks plus the Blob guard and the shell-SIGKILL lifetime checks. It isolates with `HOME` +
  `CFFIXED_USER_HOME` (Grafida's data, its log and WebKit's website data follow both) and sets
  `TMPDIR` explicitly for shell and client, because `launchctl asuser`/`sudo` strip it and the
  automation file lives there. `GRAFIDA_PACKAGED=<…>.app` copies the bundle, swaps in `SHELL_BIN` and
  re-signs the copy ad hoc.
- **`scripts/dev/parity-linux.sh` is the automatable half of #82's Linux parity column**: it runs
  the debug shell on the current desktop against `tests/apps/{echo,silent}` and against Grafida
  (`GRAFIDA_DIR`, default `../grafida`) with every per-user directory in a temp dir, and prints a
  PASS/FAIL Markdown table with the commits it ran on (`QUICK=1` shortens the idle-CPU sample).
  `GRAFIDA_PACKAGED=<extracted tarball>/grafida` runs the Grafida checks against the **packaged**
  app instead (a copy of it, with only `grafida-shell` swapped for the debug build), and
  `SHELL_BIN` takes a prebuilt debug build, e.g. the `debug-automation-linux-<arch>` artifact the
  release workflow uploads (built on 22.04, so it runs on every supported distribution; a release
  asset never). On a clean VM it needs python3 only. The
  automation channel's runtime file is removed in the shell's stop path, not only in `Drop`: tao
  ends the process without dropping anything, and a stale file hands a client a dead pid.
- **`launcher/` is a separate crate and must stay free of GTK, WebKit and C dependencies** (std +
  serde_json): it is built static (musl) precisely so it runs where the shell cannot load. It asks
  the system loader whether the shell beside it would load (`LD_TRACE_LOADED_OBJECTS` +
  `LD_BIND_NOW` + `LD_WARN`, i.e. `ldd -r`, ~30 ms) and either `exec`s it with the same arguments
  or alerts (zenity → kdialog → notify-send → xmessage → stderr) with the packages to install per
  distro family. Only undefined symbols *the shell itself* needs count (it is `-z now`; a library's
  own unresolved references bind lazily), and output it does not recognise **fails open**.
  `launcher/tests/loader.rs` breaks real C programs' libraries to pin the loader's formats.
- **Process-wide settings go in `linux::environment::prepare()`, called from `main()` before any
  thread exists** (it sets environment variables): the program name = the manifest `id` (GTK's
  Wayland `app_id` and X11 `WM_CLASS`, so a packaged app's desktop entry must be named
  `<id>.desktop`), and `WEBKIT_DISABLE_DMABUF_RENDERER` per `webview.dmabufRenderer`. The user's
  own value of that variable always wins.
- **Releases** (`.github/workflows/release.yml`): every push builds, checks
  (`scripts/release/check-linux-binaries.sh`: glibc ≤ 2.35, every NEEDED library from
  `libwebkit2gtk-4.1-0` + `libgtk-3-0`'s dependency closure, no automation marker, a static
  launcher) and smoke-tests (`scripts/release/smoke-linux.sh`: the launcher → the shell → the fake
  worker's `page` mode, which POSTs, reports to the log and quits; no automation channel) on
  `ubuntu-22.04` and `ubuntu-22.04-arm`. A tag `X.Y.Z` equal to **both** crates' versions publishes
  `grafida-{shell,launcher}-linux-{x86_64,aarch64}` + `.sha256`, once; consumers pin tag + hashes.
- **Menu accelerators fire before the webview sees the key.** Never bind a chord the manifest
  reserves (Grafida: `CmdOrCtrl+Shift+V`).
- **In packaged mode the worker is resolved beside the shell's real path** (`cli.rs`), not beside the
  manifest: the same directory on Linux, `Contents/MacOS` in a bundle (codesign wants only code
  there); the manifest is beside the binary or in `../Resources`, and the worker runs in its directory.
- **`SHELL_ALERTS=stderr` keeps the failure alerts to stderr** on both platforms (tests, headless
  smoke runs); on macOS so does a process with no window-server session.
- Windows builds use a static CRT (`-C target-feature=+crt-static`).

## Conventions

- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test` must pass.
- `spikes/` is frozen Phase 0 evidence; it is excluded from the workspace and not maintained.
- Keep a terse `CHANGELOG.md` entry per change (Grafida's `!` `+` `-` `~` `# [HIGH|MEDIUM|LOW]` style).
- Commit directly on `main`.
