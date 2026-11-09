# ADR 0001 — Shell stack, deployment floors, architectures, licence

- **Status:** Accepted for Linux. **Windows runtime verification pending** (see "Open items").
- **Date:** 2026-11-09
- **Context issue:** [grafida/grafida#82](https://github.com/grafida/grafida/issues/82), Phase 0

## Context

Grafida replaces Boson with a self-owned native shell (window, webview, custom scheme, PHP worker
supervisor) talking to a PHP worker over framed stdio. Phase 0 had to settle four things:

1. the implementation stack: **Rust + wry/tao/rfd/muda** or **C++23 + saucer** (what Boson wraps);
2. the deployment floors;
3. the architectures;
4. the licence of the reusable repositories.

The hard requirement driving Decision 1 is that the page origin is **byte-identical, `boson://app`,
on every platform**. Stored draft HTML and `.grafida` exports embed `boson://app/api/media/…` URLs,
`localStorage` is per-origin, and users have configured local AI endpoints' CORS for that origin.

## Method

Both stacks were spiked on Linux (Arch, KDE Plasma/Wayland, WebKitGTK 2.54.2, Rust 1.99.1,
GCC 16.2) against the same page (`spikes/common/index.html`) and the same harness
(`spikes/measure.sh`). The page reports to the shell through a `POST /report`. The tests:

- **(a)** `location.origin`, `isSecureContext`, and `POST` bodies of 0, 1, 1023, 1024, 1025,
  65 536, 1 048 577, 10 MiB and 50 MiB bytes, each checked by byte count **and** SHA-256 on both
  sides. The odd sizes probe chunked-read edge cases.
- **(b)** the `_native/dialog/open-file` token round trip. The dialog returns an opaque token, the
  page sends it as `X-Shell-File`, and the shell resolves it to a path. The test also covers a
  forged token, and an `X-Shell-File-Path` header injected by the page, which the shell must strip.
  It ran unattended with `SPIKE_AUTOPICK` replacing the dialog.
- **(c)** idle CPU of the whole process tree (shell + WebKit network/web processes, plus the
  bubblewrap sandbox where present), sampled from `/proc/*/stat` over 60 s after a 60 s settle.
- **(d)** a 3 s request answered off the UI thread, with a 16 ms JS interval measuring main-thread
  jank meanwhile.
- **(e)** a response carrying a repeated header (`x-multi: one` + `x-multi: two`), and
  `Cache-Control` pass-through.

Spike sources are in `spikes/`. The saucer spike pins saucer `c7dca16` (v8.0.0 + 32 commits).

## Results (Linux)

| | Rust: wry 0.57.1 / tao 0.37.2 / rfd 0.18.0 | C++23: saucer v8 (+ saucer-desktop 4.1) |
|---|---|---|
| WebKitGTK API used | webkit2gtk-**4.1** (GTK 3) | webkitgtk-**6.0** (GTK 4 ≥ 4.12 + libadwaita) |
| `location.origin` / `self.origin` | `boson://app` / `boson://app` | `boson://app` / `boson://app` |
| Secure context | yes | yes |
| All body sizes 0 B → 50 MiB intact | ✅ | ✅ |
| 50 MiB `POST` round trip | **142 ms** | 376 ms |
| Token flow / forged token / injected path | ✅ 200 / 400 / 400, stripped | ✅ 200 / 400 / 400, stripped |
| UI during a 3 s request (max frame gap) | 21 ms | 21 ms |
| Idle CPU, whole tree, 60 s | **0.02 %** of one core | 0.05 % of one core |
| Repeated response header | `one, two` | **`one` only**: `std::map<string,string>` in both directions |
| WebKit web-process sandbox | off by default (can be enabled) | on (bubblewrap) |
| Build | `cargo build` 38 s cold; no network at configure time | CMake + CPM fetching ~10 repos at **configure** time (hit a GitHub 504 once) |

For comparison, Boson idles at **~49 %** of a core today (see Grafida's `EventLoopThrottle`).

### Findings that shape the implementation, whichever stack

- **wry's Linux body reader is opt-in and fragile.** POST bodies are empty unless the `linux-body`
  feature is enabled. With it on, the reader (`webkitgtk/web_context.rs`) reads 1 KiB chunks and
  **stops at the first short read** (`count != 1024 → break`), not at EOF. All sizes passed here,
  because WebKit hands over an in-memory stream that only short-reads at the end. But nothing in
  the `GInputStream` contract guarantees that. It also collapses repeated request headers
  (`HeaderMap::insert`), and registers the scheme as secure but **not** CORS-enabled.
- **saucer's GTK executor is not thread-safe.** `resolve()` finishes the WebKit request directly,
  so an answer from a worker-reader thread must be posted back with `app->post()`.
- Both stacks gave an idle CPU of ~0, an unblocked UI, and the exact origin on Linux. The stack
  decision therefore came down to Windows, the Linux floor and header fidelity.

## The Windows question

**Can wry serve a real `boson://` scheme on WebView2 without a deep fork? Yes, with no fork at
all, in principle. Runtime proof is pending (see Open items).**

wry maps a custom protocol `boson` to `http://boson.localhost` (`custom_protocol_workaround.rs`).
But wry 0.57 exposes everything needed to avoid that path:

1. `WebViewBuilderExtWindows::with_environment(ICoreWebView2Environment)`. The shell creates the
   environment itself, with `CoreWebView2EnvironmentOptions` carrying a
   `CoreWebView2CustomSchemeRegistration("boson")` (`TreatAsSecure`, `HasAuthorityComponent`,
   `AllowedOrigins = ["boson://app"]`), which webview2-com 0.39 implements
   (`set_scheme_registrations`).
2. The shell registers **no** wry custom protocol on Windows. Instead it adds its own
   `AddWebResourceRequestedFilter("boson://*")` and `add_WebResourceRequested` handler on the raw
   `ICoreWebView2` that `WebViewExtWindows::webview()` returns.
3. Answers are asynchronous. The event args and deferral are parked in a UI-thread map, the
   responder posts `UserEvent::Respond(id, response)` through the tao event loop, and the UI
   thread builds the `ICoreWebView2WebResourceResponse` and completes the deferral. This is the
   same shape the worker pipe needs on every platform.

`spikes/rust-wry/src/win.rs` implements all of this. It cross-compiles from Linux with
`cargo xwin build --target x86_64-pc-windows-msvc` with no warnings. With
`-C target-feature=+crt-static` it is a 1.2 MB executable importing only OS DLLs (no
`VCRUNTIME140`), and the WebView2 loader is linked statically.

## Decision

### 1. Stack: **Rust + wry / tao / rfd / muda**

Rust is chosen. The issue's own rule decides it: the Windows custom scheme needs no patch at all,
only the extension points wry already exposes. Beyond that:

- **Linux floor.** saucer needs GTK ≥ 4.12, webkitgtk-6.0 and libadwaita, which puts it out of
  reach of Ubuntu 22.04 (GTK 4.6). That fails Decision 2's proposed floor; Ubuntu 24.04 would
  become the minimum. wry's webkit2gtk-4.1 is available there (CI builds the spike on
  `ubuntu-22.04`, x86_64 and arm64).
- **Header fidelity.** The protocol carries repeated headers as arrays. saucer's
  `std::map<string,string>` cannot express that in either direction without patching saucer.
- Faster large bodies (142 ms vs 376 ms for 50 MiB), a self-contained build, and trivial
  cross-compilation to Windows from Linux and macOS.

**Conditions attached to the choice** (Phase 2 must honour them):

- **The shell owns the scheme bridge wherever wry's falls short.** On Windows that is the approach
  above. On Linux, register the `boson` scheme on the `WebContext` ourselves through the
  `webkit2gtk` crate rather than through `with_asynchronous_custom_protocol`. Read the body until
  EOF in bounded chunks, keep repeated headers, and register the scheme as secure **and**
  CORS-enabled. Offer the reader fix upstream to wry; do not depend on it. macOS is decided in
  Phase 4: wry's `WKURLSchemeHandler` path is acceptable if it passes the same tests.
- **Turn the WebKitGTK sandbox on** (`WebContext::set_sandbox_enabled`) unless it breaks the file
  dialogs. The editor renders article HTML fetched from sites, and saucer/WebKitGTK 6 already
  sandboxes by default, so we should not regress below that.
- Build Windows with a static CRT.

**Known risk accepted:** wry/tao sit on GTK 3 and the WebKitGTK **4.1** API, while Boson today uses
GTK 4 / webkitgtk-6.0. Upstream WebKitGTK still ships the 4.1 API (2.54.2 here). If a future
WebKitGTK drops it, our scheme bridge is already our own code, but tao's windowing would need
tao/wry's GTK 4 migration, or a fork.

**Rejected: saucer.** It is the lower-behavioural-risk option on Windows, since Boson already uses
it there, and its Linux body reader is correct. But Ubuntu 22.04 is out of reach, repeated headers
cannot be expressed, its executor needs main-thread marshalling, and its build fetches sources at
configure time. Together these outweigh the one advantage the Windows result above removes.

### 2. Deployment floors: **adopted as proposed**

- **macOS 12** (Monterey) → `MACOSX_DEPLOYMENT_TARGET=12.0` and `LSMinimumSystemVersion`. This
  replaces the vendored-dylib coupling in Grafida's `App::MIN_MACOS` (today macOS 15, gh-58).
- **Linux:** the WebKitGTK **4.1** API, **≥ 2.40** (for `webkit_uri_scheme_request_get_http_body`;
  `webkit2gtk` crate feature `v2_40`). Built on **Ubuntu 22.04** for a glibc 2.35 floor. Runtime
  dependencies: `libwebkit2gtk-4.1-0`, `libgtk-3-0`.
- **Windows:** 10 1809+ with the WebView2 **Evergreen** runtime. The custom-scheme registration
  needs `ICoreWebView2EnvironmentOptions4`, which every current Evergreen runtime has. The NSIS
  installer runs the Evergreen bootstrapper when the runtime is missing.

### 3. Architectures: **per-arch**, as today

`linux-{x86_64,aarch64}`, `macos-{arm64,x86_64}`, `windows-x86_64`. The worker SFX is per-arch
anyway, so a universal2 macOS shell buys nothing.

### 4. Licence of `grafida/shell`, `grafida/shell-php`, `grafida/shell-packager`: **MIT**

This lets non-GPL consumers use the shell. All dependencies (wry, tao, rfd, muda, webview2-com,
windows-rs) are MIT/Apache-2.0. Grafida itself stays GPL-3.0-or-later.

## Open items (do not close Phase 0 without them)

1. **Windows runtime proof.** Run `spikes/rust-wry` (static-CRT build) in the Windows 11 VM.
   `location.origin` must equal `boson://app`, and the 50 MiB body, the token flow, the slow
   request and repeated headers must pass, with `measure`-equivalent idle CPU via Task Manager or
   `Get-Process`. If the origin is not `boson://app`, Decision 1 is reopened: try owning the
   controller via `webview2-com`, else fall back to saucer.
2. **Interactive dialogs on Linux.** Run both spikes once without `SPIKE_AUTOPICK` on KDE Plasma
   (Wayland). Confirm that rfd's xdg-portal backend shows the native KDE dialog and that cancel
   returns `{cancelled:true}`.

## Consequences

- `grafida/shell` is a Rust crate (edition 2024). Phase 2 builds it on Linux in dev mode.
- Cross-compiling Windows binaries from Linux/macOS (`cargo-xwin`) and running them in a VM
  (`build/readme`-style notes to follow in Phase 5) is the expected Windows workflow.
- The Linux packaging (Phase 3) documents `libwebkit2gtk-4.1-0` + `libgtk-3-0`, not the GTK 4
  stack Boson pulls in today.
