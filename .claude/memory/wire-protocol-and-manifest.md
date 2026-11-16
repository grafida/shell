# Wire protocol and manifest contract

Read before changing `protocol/v1.md`, the fixtures, `src/protocol.rs`, or `schema/shell.schema.json` / `src/manifest.rs`.

- **The protocol is specified here** (`protocol/v1.md`), with fixtures both sides run
  (`tests/protocol_fixtures.rs` here; a vendored, checksummed copy in grafida/shell-php). A failing
  fixture is a question about which side is wrong — never a file to regenerate.
  `scripts/protocol/make-fixtures.py` is the fixtures' **independent** writer (it must never call
  into either codec) and only ever *adds* cases; changing an existing `.bin`/`.json` is a protocol
  change that needs its reason in the commit message. `PROTOCOL_VERSION` (`src/protocol.rs`) must
  equal `Grafida\Shell\Protocol::VERSION` in grafida/shell-php.
- **The order of checks in v1.md §4 is normative**, because it decides which error code a byte
  sequence produces on both sides. The header is validated *before* `B` is read; a body on
  `ready`/`shutdown` is `bad_header` *before* the body limit applies; both length limits are
  enforced before reading what they announce. Never scan for the next magic after a bad frame.
- **Every integer on the wire is within 0 … 2^53 − 1**, not merely ids: PHP has no `u64`, so an
  integer Rust would accept and PHP would turn into a float is a split-brain fixture waiting to
  happen.
- **The manifest's contract is `schema/shell.schema.json`**, compiled into the binary. `Manifest::parse()`
  validates against it *before* serde sees the JSON, so errors name the member in the schema's terms.
  The schema's `default` annotations must equal the serde defaults in `src/manifest.rs`;
  `schema_defaults_match_the_shell_defaults` walks the schema and fails on any drift. Every member is
  `additionalProperties: false`: a typo is an error, never a silently ignored setting.
