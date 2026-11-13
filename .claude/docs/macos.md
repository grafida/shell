# macOS notes

Moved from the root `CLAUDE.md`; same rules, loaded on demand.

- **On macOS the shell owns WKWebView through objc2; neither wry nor muda is used** (ADR 0001,
  grafida/grafida#88). `src/macos/` builds its own `WKWebViewConfiguration` (the `WKURLSchemeHandler`,
  devtools, the Blob guard), a `WKWebView` subclass (`ShellWebView`: file drops, the context menu),
  one delegate object for `WKNavigationDelegate` + `WKUIDelegate`, and the AppKit menu bar. The scheme
  handler parks each task in a main-thread map; `stopURLSchemeTask:` removes it, so a late answer is
  dropped (touching a stopped task raises). An `HTTPBody` is checked against `maxBodyBytes` *before*
  it is copied; an `HTTPBodyStream` is read to EOF in 1 MiB chunks on a reader thread, which posts the
  request back to the main thread for `Bridge::handle`. Navigation: macOS knows the target frame, so a
  nil target is a new window, the main frame gets `navigation::for_main_frame_navigation` (nothing
  off-origin, ever) and a subframe `navigation::for_subframe_navigation`: the layered Linux rule, plus a
  subframe may only be sent to `<scheme>://app` by a frame whose `sourceFrame.securityOrigin` is the app's
  (`protocol`/`host`; a nil source is refused too: measured, WebKit names the parent even for an iframe's
  initial load). WKWebView sends **no `Sec-Fetch-Site` (nor `Origin` on a GET)** to the scheme handler
  (macOS 27.2, `tests/apps/echo/bridge.js`), so `Bridge`'s `cross-site` refusal is a no-op here, and the
  navigation delegate never sees a subresource load: a foreign frame's `<img src="shell://app/…">` or
  `no-cors` fetch reached the worker (measured with the echo app's `/stderr?lines=N`). So the **scheme
  handler** refuses a task whose frame is neither the main frame nor of the app's origin
  (`Scheme::frame_may_request`, `navigation::frame_may_request`), read from WebKit's private `_frame` on
  `WKURLSchemeTask` (macOS 10.15+) behind `respondsToSelector:`; the selector absent, every request is let
  through and the log says so once; the selector present but answering nil, the request is refused
  (security.md M1). Measured values: the main frame's first load has origin `://` and
  `isMainFrame`; the app's own iframe's first request already reports `shell://app`; a foreign frame's
  subresources report its `http://host`. The gesture comes from WebKit's private
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
