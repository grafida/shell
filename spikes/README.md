# Phase 0 spikes

Throwaway code kept for reproducibility of [ADR 0001](../docs/adr/0001-shell-stack.md). Not part of
the shell; not maintained.

- `common/index.html` — the test page both spikes serve at `boson://app/`.
- `rust-wry/` — Rust + wry/tao/rfd. `src/win.rs` is the Windows real-`boson://` approach.
  - Linux: `cargo build --release` (needs `libwebkit2gtk-4.1-dev`).
  - Windows from Linux/macOS: `RUSTFLAGS="-C target-feature=+crt-static" cargo xwin build --release --target x86_64-pc-windows-msvc`.
- `cpp-saucer/` — C++23 + saucer (Linux): `cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release && cmake --build build`.
- `windows/run.ps1` — the Windows counterpart of `measure.sh`, run inside the VM in the logged-on
  session (`schtasks /Create … /IT`, then `/Run`); writes `C:\spike\result.json`.
  `windows/dialog.ps1 -Tag <t>` runs it once with the real dialog and waits for it to exit.
- `measure.sh <label> <binary> [settle_s] [measure_s]` — runs a spike with `SPIKE_AUTOPICK` set,
  collects its `REPORT` line into `results/<label>.json`, then measures idle CPU of the whole
  process tree. Opens a window on the current desktop.

Env: `SPIKE_AUTOPICK=<file>` bypasses the native dialog; `GRAFIDA_DEBUG=1` enables DevTools.
