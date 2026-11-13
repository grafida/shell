# Windows notes

Moved from the root `CLAUDE.md`; same rules, loaded on demand.

- **The Windows application manifest is linked from a `.res` that `build.rs` writes** (one
  `RT_MANIFEST` entry, the documented RESOURCEHEADER format): `embed-manifest` and friends need
  `mt.exe` to merge manifests, which `lld-link` under `cargo xwin` does not have. An app's packager
  adds its icon and version resources and must keep this one.
- **On Windows the shell owns WebView2 through webview2-com; wry is not used anywhere**
  (ADR 0001, grafida/grafida#91). `src/windows/` creates the **environment** itself — the app's
  scheme as a `CoreWebView2CustomSchemeRegistration` (secure, with an authority, the origin allowed),
  the user data folder `%LOCALAPPDATA%\<dataDir>\WebView2` (the default beside the executable is
  not writable under Program Files) — and the **controller** directly on tao's window. Both are
  created asynchronously; their completion handlers run on the UI thread from the message loop and
  stash the result in a thread-local (COM objects are not `Send`) before posting
  `UserEvent::Created`. Never `wait_with_pump`: it is a nested, unfiltered pump that swallows
  `WM_QUIT`. Bounds, focus (`MoveFocus` on activation), `NotifyParentWindowPositionChanged` and
  hiding while minimised come from tao's window events; there is no subclassing.
- **The Windows scheme bridge reads bodies on a reader thread** through an `AgileReference` to the
  request's `IStream`, 1 MiB at a time to EOF, past `maxBodyBytes` → `413`; the deferral is taken in
  the handler and completed from a UI-thread map. Response headers go one line per value. The
  filter uses `ICoreWebView2_22`'s source kinds (shared workers, iframes) when present.
- **WebView2 says which frame navigates**: `NavigationStarting` is the main frame
  (`for_main_frame_navigation`), `FrameNavigationStarting` a subframe (`for_navigation`).
  `NewWindowRequested` is handled with no window. With `webview.frameGuard` on,
  `navigation::frame_buster` is added with `AddScriptToExecuteOnDocumentCreated` (it runs in child
  frames too) beside the bridge's `frame-ancestors` (security.md L1). ⚠️ **A script's `el.click()` on a `target=_blank`
  link is not user-initiated for WebView2** (a same-window one is), so it is refused; a real click is
  (the automation channel's `click`, DevTools' `Input.dispatchMouseEvent`, proves it).
- **WebView2 needs no key routing of ours** (probed in grafida/grafida#91): every key reaches the
  page's `keydown`; editing keys are not even offered as accelerators, and with
  `AreBrowserAcceleratorKeysEnabled = false` the browser acts on none of the others (with it on,
  Ctrl+R and F5 reload the app and Ctrl+F opens a find bar). No menu bar on Windows, so nothing can
  take Ctrl+Shift+V. Zoom, pinch zoom, swipe navigation, the status bar, autofill and password saving
  are off. Debug builds: `SHELL_WINDOWS_BROWSER_KEYS=on`, `SHELL_WINDOWS_KEY_LOG=1` (every
  accelerator offered to the host) and `SHELL_WINDOWS_MENU_LOG=1` (the context menu's item names).
- **The Windows context menu is an allowlist** of WebView2 item names (editing, spelling, emoji,
  copying a link or image; Inspect only with devtools), so an item a future runtime adds never
  appears. ⚠️ A spelling suggestion is named **`spellcheck`, all lower case** (runtime 157), with an
  empty label; the match ignores case, and `parity-windows.sh` checks the suggestions really appear
  (a case-sensitive `spellCheck` had removed every one). WebView2 checks the user's **primary
  language only** (`spellcheck.dictionaries` holds one entry, whatever else Windows has installed),
  and there is no WebView2 API to add another. After a cold boot the first text typed into a
  subframe (TinyMCE's iframe) can stay unchecked until the next edit, which then checks it all. Debug builds: `SHELL_WINDOWS_LANGUAGE=<tag>|none`
  overrides the language the shell passes to WebView2. A **browser-process** exit is fatal (a native
  alert, then quit: nothing else would ever repaint the window); a **renderer** exit or hang reloads
  the page.
- **The worker's Job Object and its spawn** (`spawn.rs`, Windows): the shell calls `CreateProcessW`
  itself, not std's `Command`, with `CREATE_SUSPENDED | CREATE_NO_WINDOW`, assigns the worker to one
  `KILL_ON_JOB_CLOSE` job the shell never closes, and resumes the thread `CreateProcessW` hands back.
  ⚠️ **`STARTF_FORCEOFFFEEDBACK` is the reason it is not std's**: Windows 10 shows the "working in
  background" pointer for a new GUI program until it shows a window, and the packaged worker is a
  GUI-subsystem SFX that never does, so every launch had a 5 s busy pointer (0.1 s with the flag;
  Windows 11 showed 0.4 s either way). std has the flag, the job list and the thread handle only
  behind unstable features. Only the three pipe ends are inherited
  (`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`): a leaked write end of the worker's stdout would hide its
  death from the supervisor. A bare program name (the dev worker's `php`) is looked up on the
  worker's PATH, never in the current directory. Unix keeps std's `Command`. The shell is
  deliberately not in the job: a browser `ShellExecute` starts would die with the app. `CREATE_NO_WINDOW` matters for the dev worker (`php.exe`, a console program, whose
  children inherit its hidden console); the GUI-subsystem SFX's PHP adds the flag to its own children.
- **A release Windows shell removes `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` and the other
  browser-changing `WEBVIEW2_*` variables** before it creates the environment
  (`windows::environment::BROWSER_VARIABLES`), so no debugging port can be injected into a release
  build; a debug build keeps them. The automation channel's `eval` uses `CallDevToolsProtocolMethod`
  in process: no port is opened in any build. This closes the environment only: WebView2 also reads
  `Software\Policies\Microsoft\Edge\WebView2` (`AdditionalBrowserArguments`, `BrowserExecutableFolder`,
  `UserDataFolder`, ...) from `HKLM`, then `HKCU`, when no variable is set, and Microsoft treats the
  `HKCU` ones as standard-user-modifiable. The shell neither reads nor clears them (a same-user process
  could attach a debugger anyway; security L16).
- **Packaged Windows paths**: `canonicalize()` answers verbatim `\\?\` paths; `cli::real_path`
  writes them plainly (the worker is started by that path, and phpmicro derives its sibling `.phar`
  from it). A `worker.binary` without an extension is `<name>.exe` on Windows.
- Windows builds use a static CRT (`-C target-feature=+crt-static`, `.cargo/config.toml`) and link
  the WebView2 loader statically: the shell imports system DLLs only. `#![windows_subsystem =
  "windows"]`: no console; started from a terminal it attaches to that console for `--dev` logging.
  Cross-check with `cargo xwin clippy --target x86_64-pc-windows-msvc --workspace --all-targets`.
