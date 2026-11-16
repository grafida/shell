# grafida/shell — AI assistant orientation

A small, app-agnostic native desktop shell in **Rust (tao/rfd, with the system webview built by the
shell itself: WebKitGTK on Linux, WKWebView through objc2 on macOS, WebView2 through webview2-com on
Windows)**: window + system webview,
a custom scheme serving the app at `<scheme>://app`, a supervised PHP **worker** process spoken to
over length-prefixed frames on stdin/stdout, native endpoints under `/_native/*`, menus and failure
alerts. Grafida is consumer #1 (umbrella issue grafida/grafida#82); nothing Grafida-specific may be
hard-coded — everything comes from the manifest (`shell.json`).

Read `docs/adr/0001-shell-stack.md` before changing the webview glue.

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
- **Responses are completed on the UI thread.** Worker I/O runs off it; answers are posted back
  through the event loop. Never block the UI thread on the worker.
- **Menu accelerators fire before the webview sees the key.** Never bind a chord the manifest
  reserves (Grafida: `CmdOrCtrl+Shift+V`).
- **`SHELL_ALERTS=stderr` keeps the failure alerts to stderr** on both platforms (tests, headless
  smoke runs); on macOS so does a process with no window-server session.

## Conventions

- Keep a terse `CHANGELOG.md` entry per change that ships: the shell, the launcher, the protocol,
  the schema, release assets, the README (Grafida's `!` `+` `-` `~` `# [HIGH|MEDIUM|LOW]` style).
  **Development tooling gets none**: `scripts/dev/` (the VM, parity and automation scripts), tests
  and the build and release scripts in `scripts/release/` (decided, grafida/grafida#92). Released sections keep the tooling entries they already
  have, as history.
- Commit directly on `main`.
- Before every commit run `cargo fmt --all --check` (there is no CI: nothing else
  runs them); before a release, `scripts/release/local-release.sh check` (what `ci.yml` ran on each
  platform: clippy `-D warnings`, the release-build automation guard, the tests, the fixtures) and `cargo clippy --workspace --all-targets`.

## Project memory

Project memory lives in `.claude/memory/`. Read the matching file before starting that kind of work. Platform notes in `.claude/docs/` are listed under "Platform and tooling notes" above.

| File | Read it when… |
|------|---------------|
| [`test-vms-and-guests.md`](.claude/memory/test-vms-and-guests.md) | Before testing a packaged app on a clean Linux, macOS or Windows machine, or driving a guest's pointer |
| [`scheme-bridge-and-request-policy.md`](.claude/memory/scheme-bridge-and-request-policy.md) | Before you touch `src/bridge.rs`, a platform scheme handler, the CSP / `frameGuard` behaviour or `navigation::frame_buster` |
| [`wire-protocol-and-manifest.md`](.claude/memory/wire-protocol-and-manifest.md) | Before you touch `protocol/`, `src/protocol.rs`, the fixtures, `schema/shell.schema.json` or `src/manifest.rs` |
| [`worker-supervision.md`](.claude/memory/worker-supervision.md) | Before you touch the supervisor thread, worker spawn / restart, `tests/workers/fake_worker.py` or worker resolution in `cli.rs` |
| [`linux-launcher.md`](.claude/memory/linux-launcher.md) | Before you touch `launcher/` |
| [`release-process.md`](.claude/memory/release-process.md) | Before building, checking, smoke-testing or publishing a release |
