# Dev tooling notes

Moved from the root `CLAUDE.md`; same rules, loaded on demand.

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
  automation file lives there. `GRAFIDA_PACKAGED=<…>.app` copies the bundle and swaps in `SHELL_BIN`,
  re-signing **only the shell and then the bundle**, ad hoc, with the hardened runtime and the
  original shell's entitlements (checked afterwards); the worker stub keeps shell-packager's
  signature. Never `codesign --deep`: it re-signs the stub without either, and the run would test a
  bundle under weaker restrictions than the one that ships. The script runs under bash 3.2 (a clean
  guest's), so no associative arrays and `${a[@]+"${a[@]}"}` for a possibly empty array.
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
- **`scripts/dev/parity-windows.sh` runs on the host and drives a guest** (`winvm.sh`), and **nothing in
  it is left to a human**: real keys, clicks and drags through SendInput (the channel's `type`, `key`,
  `mouse`, `drag`), native dialogs, Explorer, the clipboard, the cursor and the colour mode through
  desktop-session helpers (`scripts/dev/winvm/desk/`, UI Automation), AI streaming through a host test
  server at `10.0.2.2`. It runs the shell in the
  guest's desktop session with `APPDATA`/`LOCALAPPDATA`/`TEMP` in `C:\winvm\run`, the channel tunnelled
  here (`SHELL_AUTOMATION_REMOTE=1`, so paths are the guest's). Its last row is the **console-window
  watcher** (`winvm.sh watch-start`/`watch-stop`, a WinEvent hook in the desktop session): any
  conhost or Windows Terminal window shown during the run fails it. ⚠️ Every helper the guest runs is
  launched through `conhost.exe --headless` for that reason: a hidden PowerShell started by a
  scheduled task still flashes a window under Windows Terminal. Worker timings come from the shell's
  log ("stopping", then "worker stopped"), never from SSH round trips, and nothing that needs a
  window handle runs in the SSH session (its window station sees none of the desktop's).
