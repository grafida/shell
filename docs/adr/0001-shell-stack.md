# ADR 0001 — Shell stack, deployment floors, architectures, licence

- **Status:** Accepted. Verified at runtime on Linux, Windows and macOS (Phase 4: the macOS webview path, and arm64 only).
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
all. Verified at runtime on Windows 11 (see "Results (Windows)").**

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

### Results (Windows)

Run on 2026-11-09 in a Windows 11 Pro build 26300 VM (quickemu/KVM, WebView2 Evergreen
157.0.4341.42). The static-CRT build of `spikes/rust-wry` ran in the logged-on desktop session via
an interactive scheduled task (`spikes/windows/run.ps1`), with the same page and the same tests as
on Linux.

| | Windows 11 + WebView2 157 |
|---|---|
| `location.origin` / `self.origin` | **`boson://app`** / `boson://app` |
| Secure context | yes |
| All body sizes 0 B → 50 MiB intact | ✅ (50 MiB in 865 ms; the first request ~1 s, environment warm-up) |
| Token flow / forged token / injected path | ✅ 200 / 400 / 400, stripped |
| Repeated response header | `one, two` |
| UI during a 3 s request (max frame gap) | 17 ms |
| Idle CPU, whole tree (spike + 6 `msedgewebview2` + `conhost`), 60 s | **0.47 %** of one core |
| WebView2 children after the host is killed | all exited; no orphans |

The `conhost` in the tree is only there because the spike is a console-subsystem binary; the shell
proper is GUI-subsystem. The other `msedgewebview2` processes in the VM belong to `SearchHost.exe`
(Windows Search), which is worth knowing before blaming the shell for stray WebView2 processes.

## The macOS question (Phase 4, grafida/grafida#88)

ADR 0001 left macOS to Phase 4: wry's `WKURLSchemeHandler` path was acceptable if it passed the
Linux tests. **wry was not spiked.** Its 0.57 macOS handler (`src/wkwebview/class/url_scheme_handler.rs`)
was ruled out by reading it:

- it reads a body stream 128 bytes at a time **on the UI thread**, in a loop on `hasBytesAvailable`,
  so it stops at the first moment no bytes happen to be ready, not at the end of the stream (the
  same class of bug as its Linux reader), and a `-1` read is cast to `usize`;
- it builds response headers with a dictionary `insert`, so a repeated header keeps only its last
  value (Foundation's header fields are a dictionary on any path; joining with `", "` is the most
  any path can do, and what fetch shows anyway);
- its navigation handler is given only a URL, with no `navigationType` or `targetFrame`, and nothing
  exposes the `WKUIDelegate` or `willOpenMenu:withEvent:` (context-menu filtering, file drops);
- the shell depends on neither wry nor muda today: Linux already builds its own webview.

Instead, `spikes/macos/` has the shell own the webview through objc2, as Linux does through the
`webkit2gtk` crate: its own `WKWebViewConfiguration`, a `WKURLSchemeHandler` class, tasks parked in a
main-thread map, answers from other threads posted back through tao's event loop and completed on
the main thread, `stopURLSchemeTask:` honoured. It serves `spikes/common/index.html` with the same
tests as the Phase 0 spikes, then `spikes/macos/extra.html` for what is specific to macOS: an aborted
request (WebKit stops the task; the late answer must be dropped, never sent to it) and the body
*forms* WebKit hands over. `spikes/macos/measure-macos.sh` measures idle CPU. WebKit's helpers there
are XPC services started by launchd, not children of the app, so "the tree" is the spike plus every
`com.apple.WebKit.*` process started after it.

### Results (macOS)

Run on 2026-11-09, on an Apple Silicon host and in tart VMs restored from Apple's IPSWs
(`scripts/dev/macvm.sh`, `docs/dev/macos-test-vms.md`). One binary throughout, built on the host with
`MACOSX_DEPLOYMENT_TARGET=12.0` (`minos 12.0`, SDK 27.2). A Safari update on macOS replaces the system
WebKit every WKWebView uses, so the Safari version is part of each target.

| | 27.2 host | 27.2.1 VM | 12.7.6 + Safari 17.6 | 12.7.6 + Safari 15.6.1 | 12.6 as restored |
|---|---|---|---|---|---|
| `location.origin` / `self.origin` | `boson://app` ×2 | `boson://app` ×2 | `boson://app` ×2 | `boson://app` ×2 | `boson://app` ×2 |
| Secure context | yes | yes | yes | yes | yes |
| Bodies 0 B → 50 MiB intact (length + SHA-256) | ✅ 127 ms | ✅ 153 ms | ✅ 256 ms | ✅ 281 ms | ✅ 196 ms |
| Token flow / forged / injected path | 200 / 400 / 400 | the same | the same | the same | the same |
| Repeated header: response / request | `one, two` / `a, b` | the same | the same | the same | the same |
| `Cache-Control` pass-through | `no-store` | the same | the same | the same | the same |
| UI during a 3 s request (max frame gap) | 21 ms | 26 ms | 21 ms | 26 ms | 21 ms |
| Answers completed on the main thread | 32 / 32 | 32 / 32 | 32 / 32 | 32 / 32 | 32 / 32 |
| Aborted request | `stopURLSchemeTask:` called, the late answer dropped, no exception, the next request fine | the same | the same | the same | the same |
| Idle CPU, spike + WebKit helpers, 60 s after 60 s | **0.01 %** | 0 % | 0 % | 0 % | 0 % |

Every body arrived as `HTTPBody` (17 of 17 on every target; never a stream). macOS 12 runs WebKit
with no separate GPU process (WebContent + Networking).

**A hidden page measures the lock, not the bridge.** Four VM runs showed a ~1 s worst frame gap
(9 timer ticks in 3 s instead of ~160): one on 12.7.6 straight after its update's restart, and three
on 27.2.1, whose screen had **locked** after its update's restart. WebKit clamps
the timers of a page it considers hidden to 1 s. Every bridge counter was normal in those runs, and
each target gave the figures above once its screen was unlocked (`macvm.sh prepare` now switches the
lock off). A timing check in the parity script must record `document.visibilityState` beside its
result.

### Finding: WKWebView drops file bodies

**A `Blob` or `File` body, and a `FormData` that contains one, reaches a `WKURLSchemeHandler` as no
body at all**: `HTTPBody` and `HTTPBodyStream` are both nil, through `fetch()` and
`XMLHttpRequest` alike, on 12.6 and on 27.2. There is no `Content-Length` and nothing else to detect it
by; only the `Content-Type` survives (and a `FormData` with a file part loses its text parts too).
Strings, `ArrayBuffer`s, typed arrays, `URLSearchParams` and file-less `FormData` arrive intact.
This is WebKit's behaviour, not the bridge's: wry's handler reads the same two properties, and it
is the same on every target above. Grafida's internal API posts only strings (`apiFetch()` sends
`JSON.stringify(body)`; images travel as base64 inside it, or as a dialog token PHP reads), so nothing
breaks today.

**Decided: fail loudly in the page.** On macOS the shell injects a document-start script that wraps
`fetch()` and `XMLHttpRequest.send()` for same-origin requests and throws a `TypeError` naming the
limitation for a `Blob`/`File` body or a `FormData` with a file part, rather than letting it go out
empty. No conversion.

## The Windows port (Phase 5, grafida/grafida#91)

**The shell owns WebView2 through webview2-com 0.39 / windows-rs 0.62, and wry is not used on
Windows either**, as on macOS. The spike above proved the scheme registration through wry's
`with_environment`; reading wry 0.57.1's WebView2 backend for the port showed how much of it the
shell would have had to work around:

- custom schemes only through the `http(s)://<scheme>.localhost` rewrite, and `with_environment`
  skips wry's whole environment setup (browser arguments, language, data folder);
- the request body read **synchronously on the UI thread**, 1 KiB at a time, inside the
  `WebResourceRequested` callback, so a 50 MiB upload freezes the window;
- always-on handlers whose tokens are discarded: every popup silently blocked, Edge's download UI
  hidden, `WindowCloseRequested` destroying only its container;
- a file-drop handler replacing Chromium's `IDropTarget` on every child window, which costs the
  page HTML5 drag and drop;
- nested `wait_with_pump` loops for the environment, the controller and every init script, which
  swallow `WM_QUIT`; no handlers for accelerators, the context menu, focus, process failure or DPI.

What the port took from wry instead: the browser arguments
`--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection`; `SetBounds` on resize,
`MoveFocus` on focus, `NotifyParentWindowPositionChanged` on move; the request filter with every
source kind (`ICoreWebView2_22`); deferrals completed on the UI thread; repeated headers one line
per value; the UI language passed to the environment.

### Results (Windows, Phase 5)

Dev-mode parity, `scripts/dev/parity-windows.sh` driven from the Linux host: **31/31 on Windows 11
Pro 26H2 (build 26300.9711) and on Windows 10 Pro 22H2 (build 19045.6456, Greek)**, WebView2
157.0.4341.42, recorded on grafida/grafida#91. Nothing in it is by hand: real keys, clicks and drags
go through `SendInput`, native dialogs and Explorer through UI Automation, and AI streaming through
a server on the host.

| | Windows 11 | Windows 10 (Greek) |
|---|---|---|
| `location.origin`, bodies 0 B → 50 MiB, 413 past the limit | `boson://app`, 50 MiB in 624 ms | `boson://app`, 50 MiB in 789 ms |
| Worker under the shell's Job Object; the shell killed outright | the worker gone within 1.7 s | gone within 1.5–1.6 s |
| Close through the window's X → worker stopped | 0.06 s | 0.19 s |
| Console windows over the whole run (WinEvent hook) | none | none |
| Idle CPU, release shell, Grafida on Articles with a site, whole tree | 0.26 % of one core | 0.13 % |

Findings that shaped the code:

- **Every key reaches the page.** With `AreBrowserAcceleratorKeysEnabled = false` the browser acts
  on none of its own shortcuts, and with no menu bar nothing takes Ctrl+Shift+V.
- **A spelling suggestion's context-menu item is named `spellcheck`** (lower case, with an empty
  label), not `spellCheck`; a case-sensitive allowlist had removed every suggestion.
- **WebView2 checks the user's primary language only**, and after a cold boot the first text typed
  into a subframe can stay unmarked until the next edit.
- **The worker is phpmicro's GUI SFX**, started suspended, put in the Job Object, then resumed.
  Stable Rust has no `raw_attribute`, `main_thread_handle` or `startupinfo_force_feedback`; the one
  thread was first found with a Toolhelp32 snapshot, and the shell now calls `CreateProcessW` itself
  (amended in grafida/grafida#91, the clean-machine run): without `STARTF_FORCEOFFFEEDBACK`, Windows
  10 22H2 showed the busy pointer for 4.5–5.3 s on every launch, because a GUI program that never
  shows a window keeps it until it times out (0.1 s with the flag, and with the worker flipped to the
  console subsystem; Windows 11 26H2: 0.4 s either way). It needed two fork fixes, `CREATE_NO_WINDOW` for
  its own children and the sibling `.phar` through the file's final path, and shell-php 0.2.1's
  blocking stdin (PHP polls a pipe it did not open as blocking).
- **Static CRT, and the application manifest linked from a `.res` that `build.rs` writes**, because
  `embed-manifest` needs `mt.exe`, which `lld-link` under cargo-xwin lacks.
- A **release** build drops the `WEBVIEW2_*` variables that change the browser, so no debugging
  port can be injected.

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
  Phase 4: wry's `WKURLSchemeHandler` path is acceptable if it passes the same tests. **Decided
  (grafida/grafida#88): the shell owns the macOS webview through objc2**, and wry is not used on
  macOS either; see "The macOS question". **Decided (grafida/grafida#91): the shell owns WebView2
  through webview2-com on Windows too**; see "The Windows port".
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
  **Amended in Phase 5 (grafida/grafida#91): Windows 10 22H2 or later**, the oldest release still
  tested (Windows 10's last), x86_64 only. **No bootstrapper**: the runtime ships with Windows 11 and
  comes through Windows Update on Windows 10; without it the installer warns and offers the
  download page, and the shell shows an alert at launch.

### 3. Architectures: **per-arch**, as today

`linux-{x86_64,aarch64}`, `macos-{arm64,x86_64}`, `windows-x86_64`. The worker SFX is per-arch
anyway, so a universal2 macOS shell buys nothing.

~~**Amended in Phase 4 (grafida/grafida#88): macOS is arm64 only.** No x86_64 shell build, release
asset or test: Intel Mac support ends next year, and anyone who needs it can build their own.~~

**Amended in Phase 6 (grafida/grafida#92): macOS arm64 and x86_64.** Intel Macs keep a shell,
`grafida-shell-macos-x86_64`, built like the arm64 one (`MACOSX_DEPLOYMENT_TARGET=12.0`) on an Intel
runner, which checks it and smoke-tests it natively. Beyond CI it is tested **only under Rosetta 2**,
in Rosetta clones of the Apple Silicon guests (`scripts/dev/macvm.sh rosetta`): there is no Intel
hardware or Intel guest. Lifecycle: macOS 27 is the last macOS for Intel Macs; when macOS 28 drops
Intel and Rosetta 2, the x86_64 shell is deprecated, and when macOS 29 lands it is discontinued.
Still no universal2 binary: the worker SFX is per-arch, so an app ships one bundle per architecture.

### 4. Licence of `grafida/shell`, `grafida/shell-php`, `grafida/shell-packager`: **MIT**

This lets non-GPL consumers use the shell. All dependencies (wry, tao, rfd, muda, webview2-com,
windows-rs) are MIT/Apache-2.0. Grafida itself stays GPL-3.0-or-later.

## Open items

1. ~~**Windows runtime proof.**~~ **Done 2026-11-09**: the origin is `boson://app` and every test
   passes (see "Results (Windows)").
2. ~~**Interactive dialogs.**~~ **Done 2026-11-09**: the wry spike, run without `SPIKE_AUTOPICK`,
   showed the native dialog on KDE Plasma 6 (Wayland, rfd's default xdg-portal backend) and on
   Windows 11. On both, a picked file came back through the token with a matching SHA-256 (a 36 MB
   file on Linux), and cancelling returned `{cancelled:true}`. The forged token and the injected
   path were refused again in the same runs.

## Consequences

- `grafida/shell` is a Rust crate (edition 2024). Phase 2 builds it on Linux in dev mode.
- Cross-compiling Windows binaries from Linux/macOS (`cargo-xwin`) and running them in a VM is the
  expected Windows workflow. The VM is set up as described in
  [`docs/dev/windows-test-vm.md`](../dev/windows-test-vm.md).
- The Linux packaging (Phase 3) documents `libwebkit2gtk-4.1-0` + `libgtk-3-0`, not the GTK 4
  stack Boson pulls in today.
