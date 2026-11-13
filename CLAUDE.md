# grafida/shell — AI assistant orientation

A small, app-agnostic native desktop shell in **Rust (tao/rfd, with the system webview built by the
shell itself: WebKitGTK on Linux, WKWebView through objc2 on macOS, WebView2 through webview2-com on
Windows)**: window + system webview,
a custom scheme serving the app at `<scheme>://app`, a supervised PHP **worker** process spoken to
over length-prefixed frames on stdin/stdout, native endpoints under `/_native/*`, menus and failure
alerts. Grafida is consumer #1 (umbrella issue grafida/grafida#82); nothing Grafida-specific may be
hard-coded — everything comes from the manifest (`shell.json`).

Read `docs/adr/0001-shell-stack.md` before changing the webview glue. To test a packaged app on clean
Linux machines, use `scripts/dev/linuxvm.sh` (`docs/dev/linux-test-vms.md`), which also drives the
guests' real pointer (QMP, absolute: `move`/`click`/`drag`) for what the automation channel only
fakes. Its `ubuntu-22.04-arm64` guest is emulated and headless: there is no arm64 hardware, so the arm64
tarball's worker, headless launch and `install.sh` are checked there. On clean macOS 12 and 27 guests, `scripts/dev/macvm.sh` (`docs/dev/macos-test-vms.md`). To
run anything on Windows, use the test VMs in `docs/dev/windows-test-vm.md` through
`scripts/dev/winvm.sh` (`ssh winvm`, `winvm10`). A GUI program must be started in the desktop
session (`winvm.sh run`, an interactive scheduled task), never directly over SSH; the automation
channel reaches the host through `winvm.sh tunnel`.

## Platform and tooling notes (read before touching the area)

These are in `.claude/docs/` and are **not** loaded automatically: read the file before changing the
code it covers. Their rules are as binding as the ones below.

- `.claude/docs/linux.md` — the Linux webview (`src/linux/`), the `shell://` request map, clipboard and file drops, the navigation policy, KDE decoration, and process-wide settings.
- `.claude/docs/macos.md` — WKWebView (`src/macos/`), the Blob guard, macOS menus and key routing, spell checking, and run-time selectors.
- `.claude/docs/windows.md` — WebView2 (`src/windows/`), its scheme bridge, navigation, keys and context menu, the worker Job Object and spawn (`spawn.rs`), the application manifest, and packaged paths.
- `.claude/docs/dev-tooling.md` — the automation channel (`src/automation.rs`) and the `scripts/dev/parity-*.sh` scripts.

## Rules that are not guessable from the code

- **The trust model is `docs/user/security-model.md`.** The manifest, the worker and the page are
  the app developer's code and are trusted (a user who installs the app trusts its developer); the
  line the shell defends runs between them and foreign content embedded in the page. A finding that
  needs a *malicious* worker is out of scope (security.md I2). A *buggy* worker is not: the
  protocol checks, limits, crash handling and restart budget stay exactly as specified. Keep the
  page in step when a change moves the line.
- **The origin is sacred.** The page origin must be byte-identical — `shell://app` for Grafida — on
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
- **`src/bridge.rs` is the platform-neutral half and is where request policy lives**: a foreign
  `Origin` → `403 cross_origin` (an article's `<iframe>` must not drive the app's API); any host of
  the scheme but the origin's (the engines route by scheme alone) → `404 not_found`; an escaped `/`
  or NUL in the path → `404 bad_path` (Apache's default); the path checks (`/_native/`,
  `noStorePaths`) compare the path percent-decoded once, as a decoding worker routes it, never the
  raw form (security.md L6), while the worker still gets the URL verbatim; `/_native/*`
  never reaches the worker; `x-shell-file-path` from the page is always dropped and an `x-shell-file`
  token is resolved into it (the token itself is not forwarded); `no-store` is added under the
  manifest's `noStorePaths` when the worker sent no `cache-control`/`expires`;
  `content-security-policy: frame-ancestors 'self'` is added to every worker response unless the
  worker sent `x-frame-options` or a `frame-ancestors` directive of its own (security.md L1).
- **Only the app frames its pages while `webview.frameGuard` is on (the default), and the header is
  not what holds it**: WKWebView enforces neither `frame-ancestors` nor `x-frame-options` on a scheme
  response (measured; the rest of the CSP it does), so every platform also injects
  `navigation::frame_buster` into every frame at document start, which empties an app page with a
  foreign ancestor. Off, the bridge adds no header and lets a cross-site *frame navigation* through,
  the buster is not injected, and macOS drops M1's source-frame checks; subresources stay refused.
  `tests/apps/echo/framing.js` checks both states (`tests/apps/unguarded` is the guard-off app).
- **`launcher/` is a separate crate and must stay free of GTK, WebKit and C dependencies** (std +
  serde_json): it is built static (musl) precisely so it runs where the shell cannot load. It asks
  the system loader whether the shell beside it would load (`LD_TRACE_LOADED_OBJECTS` +
  `LD_BIND_NOW` + `LD_WARN`, i.e. `ldd -r`, ~30 ms; only when the shell's `PT_INTERP` is glibc's
  `ld-linux-*`, since under any other loader, or none, the probe would run its `main()`) and
  either `exec`s it with the same arguments or alerts (zenity → kdialog → notify-send → xmessage →
  stderr) with the packages to install per distro family, always ending by asking the user to read
  the app's documentation for its minimum requirements (the shell's floors are not the app's;
  README "System requirements" is the text apps copy). Only undefined symbols *the shell itself* needs count (it is `-z now`; a library's
  own unresolved references bind lazily), and output it does not recognise **fails open**.
  `launcher/tests/loader.rs` breaks real C programs' libraries to pin the loader's formats.
- **Releases** (`.github/workflows/release.yml`): every push builds, checks
  (`scripts/release/check-linux-binaries.sh`: glibc ≤ 2.35, every NEEDED library from
  `libwebkit2gtk-4.1-0` + `libgtk-3-0`'s dependency closure, no automation marker, a static
  launcher) and smoke-tests (`scripts/release/smoke-linux.sh`: the launcher → the shell → the fake
  worker's `page` mode, which POSTs, reports to the log and quits; no automation channel) on
  `ubuntu-22.04` and `ubuntu-22.04-arm`; and on `macos-15` (arm64, grafida/grafida#88) and
  `macos-15-intel` (x86_64, grafida/grafida#92) checks (`check-macos-binaries.sh` with `EXPECT_ARCH`:
  one thin architecture, `LC_BUILD_VERSION` minos ≤ 12.0, only `/usr/lib` and `/System/Library`
  linked, no automation marker) and smoke-tests natively (`smoke-macos.sh`: the shell in a bundle
  layout, manifest in `Contents/Resources`, worker beside the shell, the fake worker's `page` mode)
  each macOS shell, built with an explicit `--target`; and on `windows-2022` (grafida/grafida#91) checks
  (`check-windows-binaries.sh`: PE32+ AMD64, the GUI subsystem, only DLLs that ship with Windows, so
  no VC++ runtime and no `WebView2Loader.dll`, the manifest, no automation marker) and smoke-tests
  (`smoke-windows.sh`: the packaged layout, a `.cmd` running the fake worker's `page` mode) the
  Windows shell. A tag `X.Y.Z` equal to **both** crates' versions publishes
  `grafida-{shell,launcher}-linux-{x86_64,aarch64}`, `grafida-shell-macos-{arm64,x86_64}` and
  `grafida-shell-windows-x86_64.exe`, each with a
  `.sha256`, once; consumers pin tag + hashes. The macOS asset is only linker-signed (ad hoc): the
  app's packager signs it inside the bundle. Both platforms also upload a debug automation build as
  a workflow artifact (`debug-automation-{linux-*,macos-arm64,macos-x86_64,windows-x86_64}`), never
  a release asset. ⚠️ **x86_64 macOS is tested only under Rosetta 2** (ADR 0001, amended in #92): CI
  lints it by cross-compiling from arm64 (`--target x86_64-apple-darwin`, with and without
  `automation`, plus the release guard) and the release workflow runs it on an Intel runner, but the
  packaged app is verified in the guests' Rosetta clones (`macvm.sh rosetta`, which needs
  `--agree-to-license`: installing Rosetta 2 accepts Apple's licence, so ask the operator first).
- **Menu accelerators fire before the webview sees the key.** Never bind a chord the manifest
  reserves (Grafida: `CmdOrCtrl+Shift+V`).
- **In packaged mode the worker is resolved beside the shell's real path** (`cli.rs`), not beside the
  manifest: the same directory on Linux, `Contents/MacOS` in a bundle (codesign wants only code
  there); the manifest is beside the binary or in `../Resources`, and the worker runs in its directory.
- **`SHELL_ALERTS=stderr` keeps the failure alerts to stderr** on both platforms (tests, headless
  smoke runs); on macOS so does a process with no window-server session.
## Conventions

- Keep a terse `CHANGELOG.md` entry per change that ships: the shell, the launcher, the protocol,
  the schema, release assets, the README (Grafida's `!` `+` `-` `~` `# [HIGH|MEDIUM|LOW]` style).
  **Development tooling gets none**: `scripts/dev/` (the VM, parity and automation scripts), tests
  and CI (decided, grafida/grafida#92). Released sections keep the tooling entries they already
  have, as history.
- Commit directly on `main`.
- Before every commit run `cargo fmt --all --check` (CI fails all three platforms on it before any
  test runs) and `cargo clippy --workspace --all-targets`.
