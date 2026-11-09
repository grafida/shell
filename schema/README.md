`shell.schema.json` is the contract for an application's `shell.json` manifest (JSON Schema
2020-12). The shell embeds it and validates the manifest against it at startup; see
`src/manifest.rs`, whose defaults must match the schema's `default` annotations.
