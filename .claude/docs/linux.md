# Linux notes

Moved from the root `CLAUDE.md`; same rules, loaded on demand.

- **On Linux the shell builds the WebKitGTK webview itself; wry is not used there.** wry's WebKit
  context is crate-private, so the scheme, the data directories and the sandbox could only be set
  after wry had created the webview, and `set_sandbox_enabled()` is a *fatal* error once a web process
  exists. `src/linux/` does it in WebKit's order: context (data manager, sandbox, spelling, scheme,
  security manager: secure **and** CORS-enabled) → webview → load the start URL after `ready`.
  The sandbox is verifiably on (`WebKitWebProcess`
  under `bwrap`); dialogs run in the UI process, outside it.
- **Every `shell://` request is parked in a UI-thread map and completed from it on the UI thread**
  (`linux/scheme.rs`). The body is read asynchronously on the main loop in 1 MiB chunks **to EOF**,
  and reading stops past `maxBodyBytes` → `413`. The responder only posts `(id, response)` through
  the tao event loop. `content-length`/`transfer-encoding` from the worker are dropped (B is the
  length).
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
- **On KDE Plasma the title bar is KWin's, so the window theme is told to KWin too**
  (`src/linux/decoration.rs`). tao's `set_theme` only sets GTK's `gtk-application-prefer-dark-theme`,
  which reaches a title bar GTK draws (GNOME's) and not one KWin draws: on Plasma it stayed light in a
  dark app (grafida/grafida#92). Each theme change also sends Breeze's light or dark colour scheme
  (an absolute path found in the XDG data dirs; `system` = the empty name, the user's own) through
  `org_kde_kwin_server_decoration_palette` over GDK's own `wl_display`/`wl_surface` (a foreign-display
  wayland-client backend, so no new library), or as `_KDE_NET_WM_COLOR_SCHEME` (`STRING`) on X11,
  only when `XDG_CURRENT_DESKTOP` has `KDE`. Without the global (GNOME, wlroots) nothing is sent.
- **Process-wide settings go in `linux::environment::prepare()`, called from `main()` before any
  thread exists** (it sets environment variables): the program name = the manifest `id` (GTK's
  Wayland `app_id` and X11 `WM_CLASS`, so a packaged app's desktop entry must be named
  `<id>.desktop`), and `WEBKIT_DISABLE_DMABUF_RENDERER` per `webview.dmabufRenderer`. The user's
  own value of that variable always wins.
