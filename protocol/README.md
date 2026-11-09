# Shell ↔ worker wire protocol

- **[`v1.md`](v1.md)**: the specification (protocol version 1). It covers the frame layout,
  header types, limits, the normative order of checks, error codes and the session rules.
- **[`fixtures/`](fixtures/)**: the cross-language conformance fixtures. Each case is a `NAME.bin`
  byte stream with a `NAME.json` saying what a receiver must make of it. `SHA256SUMS` lists every
  file. The format is in v1.md §8.

Who runs the fixtures:

| Implementation | Test |
|---|---|
| Rust, `src/protocol.rs` (this repository) | `tests/protocol_fixtures.rs`: whole-buffer reads, one-byte reads, an open pipe that must never be waited on, encoder round trip, checksum manifest |
| PHP, [grafida/shell-php](https://github.com/grafida/shell-php) | vendors a copy of `fixtures/`, pinned to a commit of this repository in its `protocol/FIXTURES_REF`, and checks the copy against `SHA256SUMS` |

**A failing fixture is a question about which side is wrong, never a file to regenerate.**
`scripts/protocol/make-fixtures.py` is the fixtures' independent writer. It is neither codec under
test, so a bug a codec shares with its fixtures cannot hide. It only ever **adds** fixtures: it
refuses to touch an existing one and fails if a listed file no longer matches `SHA256SUMS`. To add
a case, define it in the script and run it from the repository root.
