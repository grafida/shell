# ADR 0001 — Shell stack, deployment floors, architectures, licence

- **Status:** Accepted.
- **Context issue:** [grafida/grafida#82](https://github.com/grafida/grafida/issues/82)

## Context

Grafida replaces Boson with a self-owned native shell (window, webview, custom scheme, PHP worker
supervisor) talking to a PHP worker over framed stdio. This record fixes the implementation stack,
the deployment floors, the architectures and the licence of the reusable repositories.

The requirement behind the stack is that the page origin is **byte-identical, `boson://app`, on every
platform**. Stored draft HTML and `.grafida` exports embed `boson://app/api/media/…` URLs,
`localStorage` is per-origin, and users have configured local AI endpoints' CORS for that origin.

## Decisions

### 1. Stack: Rust, with the shell owning each platform's webview

Rust, on tao (windowing) and rfd (dialogs). The shell builds the system webview itself on every
platform:

- **Linux:** WebKitGTK 4.1 (GTK 3) through the `webkit2gtk` crate.
- **macOS:** WKWebView through objc2.
- **Windows:** WebView2 through webview2-com / windows-rs.

wry and muda are not used. Each of wry's custom-scheme paths falls short of what the bridge needs:

- **Linux:** POST bodies are empty unless the `linux-body` feature is on, and then the reader takes
  1 KiB chunks and stops at the first short read, not at EOF. It collapses repeated request headers
  and registers the scheme as secure but not CORS-enabled.
- **macOS:** the handler reads a body stream 128 bytes at a time on the UI thread and stops when no
  bytes happen to be ready; a repeated response header keeps only its last value; the navigation
  handler is given only a URL, and nothing exposes the `WKUIDelegate` or `willOpenMenu:withEvent:`
  (context-menu filtering, file drops).
- **Windows:** custom schemes only through the `http(s)://<scheme>.localhost` rewrite, which breaks
  the origin; the request body is read synchronously on the UI thread, so a 50 MiB upload freezes the
  window; always-on handlers silently block popups and hide Edge's download UI; its file-drop handler
  replaces Chromium's `IDropTarget`, which costs the page HTML5 drag and drop; nested message loops
  swallow `WM_QUIT`.

The rules that follow, on every platform:

- **The shell owns the scheme bridge.** It registers the scheme itself, reads each body to EOF in
  bounded chunks, keeps repeated headers (one line per value on the wire to the webview, an array in
  the protocol), and completes every response on the UI thread. Worker I/O runs elsewhere; answers
  are posted back through the event loop.
- **Linux:** the scheme is registered on the `WebContext` as secure **and** CORS-enabled. The
  WebKitGTK sandbox is on (`WebContext::set_sandbox_enabled`): the editor renders article HTML
  fetched from sites.
- **macOS:** the shell has its own `WKWebViewConfiguration` and `WKURLSchemeHandler` class. Tasks
  are parked in a main-thread map; answers from other threads are posted back and completed on the
  main thread; `stopURLSchemeTask:` is honoured and a late answer to an aborted task is dropped.
- **Windows:** the shell creates the WebView2 environment with a
  `CoreWebView2CustomSchemeRegistration("boson")` (`TreatAsSecure`, `HasAuthorityComponent`,
  `AllowedOrigins = ["boson://app"]`) and handles `WebResourceRequested` for `boson://*` itself,
  with the event args and deferral parked in a UI-thread map. The CRT is linked statically. A
  **release** build drops the `WEBVIEW2_*` variables that change the browser, so no debugging port can
  be injected.

wry's good ideas are taken over where they apply: the browser arguments
`--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection`; `SetBounds` on resize, `MoveFocus`
on focus, `NotifyParentWindowPositionChanged` on move; the request filter with every source kind
(`ICoreWebView2_22`); the UI language passed to the environment.

**Rejected: saucer (C++23), what Boson wraps.** It needs GTK ≥ 4.12, webkitgtk-6.0 and libadwaita,
which is out of reach of Ubuntu 22.04 (GTK 4.6); its `std::map<string,string>` headers cannot
express a repeated header in either direction; its GTK executor requires marshalling every answer
onto the main thread; and its build fetches about ten repositories at configure time.

**Known risk accepted:** tao sits on GTK 3 and the WebKitGTK **4.1** API, while Boson used GTK 4 /
webkitgtk-6.0. Upstream WebKitGTK still ships the 4.1 API. If a future WebKitGTK drops it, the scheme
bridge is already the shell's own code, but the windowing would need a GTK 4 port.

### 2. Deployment floors

- **macOS 12** (Monterey): `MACOSX_DEPLOYMENT_TARGET=12.0` and `LSMinimumSystemVersion`.
- **Linux:** the WebKitGTK **4.1** API, **≥ 2.40** (for `webkit_uri_scheme_request_get_http_body`;
  `webkit2gtk` crate feature `v2_40`). Built on **Ubuntu 22.04** for a glibc 2.35 floor. Runtime
  dependencies: `libwebkit2gtk-4.1-0`, `libgtk-3-0`, not the GTK 4 stack Boson pulled in.
- **Windows 10 22H2 or later**, x86_64 only, with the WebView2 **Evergreen** runtime (the
  custom-scheme registration needs `ICoreWebView2EnvironmentOptions4`, which every current Evergreen
  runtime has). There is no bootstrapper: the runtime ships with Windows 11 and comes through Windows
  Update on Windows 10. Without it the installer warns and offers the download page, and the shell
  shows an alert at launch.

### 3. Architectures: per-arch

`linux-{x86_64,aarch64}`, `macos-{arm64,x86_64}`, `windows-x86_64`. The worker SFX is per-arch, so a
universal2 macOS shell buys nothing; an app ships one bundle per architecture.

The x86_64 macOS shell, `grafida-shell-macos-x86_64`, is built like the arm64 one on an Intel runner,
which checks it and smoke-tests it natively. Beyond CI it is tested **only under Rosetta 2**, in
Rosetta clones of the Apple Silicon guests (`scripts/dev/macvm.sh rosetta`): there is no Intel
hardware or Intel guest. Lifecycle: macOS 27 is the last macOS for Intel Macs; when macOS 28 drops
Intel and Rosetta 2, the x86_64 shell is deprecated, and when macOS 29 lands it is discontinued.

### 4. Licence of `grafida/shell`, `grafida/shell-php`, `grafida/shell-packager`: MIT

This lets non-GPL consumers use the shell. All dependencies (tao, rfd, webkit2gtk, objc2,
webview2-com, windows-rs) are MIT/Apache-2.0. Grafida itself stays GPL-3.0-or-later.

## Verified behaviour

What the stack was measured to do, and what the code relies on.

| | Linux (WebKitGTK 2.54) | macOS 12.6 → 27.2 (Safari 15.6 → 17.6) | Windows 10 22H2 / 11 (WebView2 157) |
|---|---|---|---|
| `location.origin` | `boson://app` | `boson://app` | `boson://app` |
| Secure context | yes | yes | yes |
| `POST` bodies, 0 B → 50 MiB, length + SHA-256 | intact, 142 ms for 50 MiB | intact, 127–281 ms | intact, 624–789 ms |
| Token flow / forged token / injected `X-Shell-File-Path` | 200 / 400 / 400, stripped | the same | the same |
| Repeated response header | `one, two` | `one, two` | `one, two` |
| UI during a 3 s request (max frame gap) | 21 ms | 21–26 ms | 17 ms |
| Idle CPU, whole tree | 0.02 % of one core | 0–0.01 % | 0.13–0.26 % |

Boson idled at about 49 % of a core (see Grafida's `EventLoopThrottle`).

### Windows

- **Every key reaches the page.** With `AreBrowserAcceleratorKeysEnabled = false` the browser acts on
  none of its own shortcuts, and with no menu bar nothing takes Ctrl+Shift+V.
- **A spelling suggestion's context-menu item is named `spellcheck`** (lower case, with an empty
  label), not `spellCheck`; a case-sensitive allowlist removes every suggestion.
- **WebView2 checks the user's primary language only**, and after a cold boot the first text typed
  into a subframe can stay unmarked until the next edit.
- **The worker is phpmicro's GUI SFX**, started suspended through `CreateProcessW`, put in the Job
  Object, then resumed. Stable Rust has no `raw_attribute`, `main_thread_handle` or
  `startupinfo_force_feedback`. The call passes `STARTF_FORCEOFFFEEDBACK`: without it Windows 10 22H2
  shows the busy pointer for 4.5–5.3 s on every launch, because a GUI program that never shows a
  window keeps it until it times out. The SFX needed two fork fixes, `CREATE_NO_WINDOW` for its own
  children and the sibling `.phar` through the file's final path, and shell-php 0.2.1's blocking
  stdin (PHP polls a pipe it did not open as blocking).
- **Static CRT, and the application manifest linked from a `.res` that `build.rs` writes**, because
  `embed-manifest` needs `mt.exe`, which `lld-link` under cargo-xwin lacks.
- With the worker under the shell's Job Object, killing the shell outright leaves no worker beyond
  1.7 s; closing the window stops it within 0.2 s; no console window appears.
- Stray `msedgewebview2` processes in a VM may belong to `SearchHost.exe` (Windows Search), not the
  shell.

### macOS

- **A hidden page measures the lock, not the bridge.** WebKit clamps the timers of a page it
  considers hidden to 1 s, so a locked screen shows a ~1 s worst frame gap. `macvm.sh prepare`
  switches the lock off. A timing check in the parity script must record `document.visibilityState`
  beside its result.
- Every body arrives as `HTTPBody`, never a stream. macOS 12 runs WebKit with no separate GPU process.
- WebKit's helpers are XPC services started by launchd, not children of the app, so "the tree" for
  an idle-CPU measurement is the shell plus every `com.apple.WebKit.*` process started after it.

### WKWebView drops file bodies

**A `Blob` or `File` body, and a `FormData` that contains one, reaches a `WKURLSchemeHandler` as no
body at all**: `HTTPBody` and `HTTPBodyStream` are both nil, through `fetch()` and `XMLHttpRequest`
alike, on 12.6 and on 27.2. There is no `Content-Length` and nothing else to detect it by; only the
`Content-Type` survives (and a `FormData` with a file part loses its text parts too). Strings,
`ArrayBuffer`s, typed arrays, `URLSearchParams` and file-less `FormData` arrive intact. This is
WebKit's behaviour, not the bridge's. Grafida's internal API posts only strings (`apiFetch()` sends
`JSON.stringify(body)`; images travel as base64 inside it, or as a dialog token PHP reads), so nothing
breaks.

**The page fails loudly.** On macOS the shell injects a document-start script that wraps `fetch()`
and `XMLHttpRequest.send()` for same-origin requests and throws a `TypeError` naming the limitation
for a `Blob`/`File` body or a `FormData` with a file part, rather than letting it go out empty. No
conversion.

## Consequences

- `grafida/shell` is a Rust crate (edition 2024).
- Windows binaries are cross-compiled from Linux/macOS (`cargo-xwin`) and run in a VM, set up as
  described in [`docs/dev/windows-test-vm.md`](../dev/windows-test-vm.md).
