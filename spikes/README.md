# Phase 0 spikes

Throwaway code kept for reproducibility of [ADR 0001](../docs/adr/0001-shell-stack.md) (Phase 0, and its macOS section from Phase 4). Not part of
the shell; not maintained.

- `common/index.html` — the test page both spikes serve at `boson://app/`.
- `rust-wry/` — Rust + wry/tao/rfd. `src/win.rs` is the Windows real-`boson://` approach.
  - Linux: `cargo build --release` (needs `libwebkit2gtk-4.1-dev`).
  - Windows from Linux/macOS: `RUSTFLAGS="-C target-feature=+crt-static" cargo xwin build --release --target x86_64-pc-windows-msvc`.
- `cpp-saucer/` — C++23 + saucer (Linux): `cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release && cmake --build build`.
- `macos/` — Phase 4 (grafida/grafida#88): the shell owns WKWebView + `WKURLSchemeHandler` through
  objc2. `MACOSX_DEPLOYMENT_TARGET=12.0 cargo build --release`; it serves `common/index.html`, then
  `macos/extra.html` (an aborted request, the body forms WebKit hands over).
  `macos/measure-macos.sh <label> <binary> [settle_s] [measure_s]` is the macOS `measure.sh` (results
  also in `results/<label>-extra.json`). In a VM, run it in the desktop session:
  `sudo launchctl asuser $(id -u) sudo -u tester ./measure-macos.sh …`.
- `windows/run.ps1` — the Windows counterpart of `measure.sh`, run inside the VM in the logged-on
  session (`schtasks /Create … /IT`, then `/Run`); writes `C:\spike\result.json`.
  `windows/dialog.ps1 -Tag <t>` runs it once with the real dialog and waits for it to exit.
- `measure.sh <label> <binary> [settle_s] [measure_s]` — runs a spike with `SPIKE_AUTOPICK` set,
  collects its `REPORT` line into `results/<label>.json`, then measures idle CPU of the whole
  process tree. Opens a window on the current desktop.

Env: `SPIKE_AUTOPICK=<file>` bypasses the native dialog; `GRAFIDA_DEBUG=1` enables DevTools.
