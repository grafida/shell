# Grafida Shell Launcher

The Linux entry point of a packaged app. It checks that the system can load the shell, then either
starts the shell or tells the user which packages to install.

It is a separate crate from the shell (`launcher/`, a member of the root Cargo workspace) and ships as
the release asset `grafida-launcher-linux-{x86_64,aarch64}`.

## Why it exists

The shell links WebKitGTK 4.1, GTK 3 and glibc 2.35 or later. When one of them is missing or too old,
the dynamic loader refuses to start the shell before `main()` runs. The only trace is a line on
stderr, and a launch from a desktop menu shows no stderr. The user clicks the icon and nothing happens.

The shell cannot report this itself, because it never gets to run. So the launcher is a second binary
that links nothing the system could lack: it is statically linked against musl, with no dependencies
beyond `std` and `serde_json`. It runs on any Linux of the right architecture, asks whether the shell
would load, and explains the problem in a dialog if it would not.

The launcher knows only the shell's own requirements (WebKitGTK 4.1, GTK 3, glibc 2.35). An app may
need more, so every alert ends by asking the user to read the app's documentation for its minimum
system requirements. The README's "System requirements" section is the text apps copy.

## Platforms

**Linux only.** Packagers ship it in the tarball (as the app's executable), in the AppImage (as
`AppRun`) and in the deb and rpm packages.

- **macOS** has no equivalent. The shell links only system frameworks, and WKWebView is part of the OS.
- **Windows** has none either. The shell links only system DLLs and checks for WebView2 itself
  (grafida/grafida#91).

The crate still compiles on Windows, so the workspace builds there. That binary only prints a notice
and exits with 69, i.e. `EX_UNAVAILABLE` from BSD `sysexits.h`). Nothing ships it.

## How it works

1. **Find the shell.** It resolves its own absolute path (`current_exe()`, which follows symlinks, so a
   link such as `~/.local/bin/myapp` still works) and takes `grafida-shell` from the same directory.
   It refuses to continue if it cannot get an absolute path. The shell is never searched for on `PATH`.
2. **Ask the system loader** whether the shell would load. It runs the shell with
   `LD_TRACE_LOADED_OBJECTS=1`, `LD_BIND_NOW=1`, `LD_WARN=1` and `LC_ALL=C` (the parser reads English), which is what `ldd -r` does. glibc's
   loader then prints the libraries, the missing ones and the unresolved symbols, and exits without
   running the program. This takes about 30 ms.
3. **Read the verdict** (`classify`, a pure function):
   - A `lib… => not found` line means a library is missing.
   - A `version 'GLIBC_x' not found` line means glibc is too old.
   - An `undefined symbol` line means a library is too old. It counts only when it names the shell
     itself. The shell is linked `-z now`, but a library's own unresolved references bind lazily and
     may never be used.
4. **Start the shell.** If the check passes, the launcher `exec`s the shell with the same arguments, so
   it replaces itself and leaves no extra process.
5. **Or explain.** If a check fails, `explain` builds the message, with the packages to install for the
   distribution family it reads from `/etc/os-release` (Debian, Fedora, Arch, openSUSE, or generic
   advice). `alert` prints it to stderr, then tries `zenity`, `kdialog`, `notify-send` and `xmessage`
   in turn, until one reports that it showed the dialog. The window title is the `title` from the
   `shell.json` beside the shell. The launcher exits with 69 (`EX_UNAVAILABLE`).

Two rules keep the check from doing harm:

- **It fails open.** Output the launcher does not recognise lets the shell start anyway. Refusing to
  start on a system it merely failed to read is worse than the crash it is trying to explain.
- **It probes only a glibc shell.** It reads the shell's ELF `PT_INTERP` first and probes only when the
  program interpreter is glibc's `ld-linux-*`. Under any other loader, or a static binary, trace mode
  would run the shell's `main()` for real, and then run it a second time on the `exec`. A glibc loader
  that is absent altogether (a musl system) is still probed, because the kernel then refuses the exec
  before anything runs, and the launcher reports that.

The title comes from the manifest, and the libraries and version floors are the shell's.

## How it is built

Always as a static musl binary. `scripts/release/linux-container-build.sh` runs inside an Ubuntu 22.04
container (the glibc 2.35 floor), natively for aarch64 and emulated for x86_64:

```bash
cargo build --release --locked -p grafida-launcher --target x86_64-unknown-linux-musl   # or aarch64-…
```

`scripts/release/check-linux-binaries.sh` then fails the build if the launcher has a program
interpreter or any `NEEDED` library. `scripts/release/local-release.sh` builds and publishes all of it,
and the launcher's version in `launcher/Cargo.toml` is kept equal to the shell's.

A plain `cargo build` at the root also builds it, dynamically, for development. Use the musl
target for anything you ship.

**Keep it free of GTK, WebKit and C dependencies.** A dependency that links native libraries defeats
its purpose. It would fail on exactly the systems it exists for.

## How it is tested

```bash
cargo test -p grafida-launcher
```

- **Unit tests** (`src/lib.rs`) cover the pure parts: the ELF reader, `classify` against captured
  glibc trace output, distribution detection, the messages, title sanitising and the alert command
  lines.
- **`tests/loader.rs`** compiles tiny C programs with `cc`, then breaks their libraries (a missing
  library, a missing symbol, an unsatisfied version) and runs the real check against the real system
  loader. It pins the exact formats the parser depends on. It skips with a note when there is no
  `cc`, and it checks that a shell with a non-glibc interpreter is never probed.
- **`tests/alerts.rs`** runs the launcher binary with no shell beside it, a fake display, and stub
  `zenity`, `kdialog`, `notify-send` and `xmessage` programs on `PATH` that record being run. It checks
  the fall-through order and which exit statuses count as "shown".
- **Release smoke test** (`scripts/release/smoke-linux.sh`) installs the real launcher and shell in a
  packaged layout under Xvfb and starts the launcher. A clean exit means the check passed, the shell
  came up and shut down, and the worker answered.
- **By hand:** the graphical alert needs a real desktop and a machine without WebKitGTK. See
  `docs/dev/linux-test-vms.md`.

The loader's trace output is glibc-specific, so the tests that run it need a glibc Linux host. The
release container provides one, and `scripts/release/local-release.sh check` runs the tests there.
