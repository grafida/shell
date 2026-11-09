shell 0.1 (unreleased)
================================================================================
+ Repository skeleton: crate, CI, licence, protocol and schema placeholders
+ ADR 0001: Rust + wry/tao/rfd/muda; deployment floors; per-arch builds; MIT
+ Phase 0 spikes (wry, saucer) with the measurement harness
+ ADR 0001: Windows runtime results; the real boson:// scheme works on WebView2 without a wry fork
+ ADR 0001: native dialogs verified on KDE Plasma (Wayland) and Windows; no open items
+ Developer docs: Windows 11 test VM on a Linux host, with `scripts/dev/winvm-unattend.py`
+ Wire protocol v1 specification (`protocol/v1.md`) with 97 conformance fixtures and their independent writer
+ Rust protocol codec (`src/protocol.rs`), tested against every fixture
+ `shell.json` manifest with a JSON Schema; an invalid manifest is refused with every offending member named
+ Worker supervisor: `ready` handshake, 502 `worker_restarted` and respawn on a crash, gives up after too many
