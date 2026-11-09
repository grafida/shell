//! The application manifest, `shell.json`: everything app-specific the shell needs.
//!
//! The JSON Schema in `schema/shell.schema.json` is the contract. A manifest is validated against
//! it **first**, so a broken manifest is reported in the schema's own terms (which member, what is
//! wrong with it) rather than as a serde type error, and only then deserialised into [`Manifest`].
//! The schema's `default` annotations and the defaults here must agree;
//! `tests::schema_defaults_match_the_shell_defaults` keeps them in step.

use std::{fmt, fs, io, path::Path};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The schema, compiled into the binary so a manifest is always checked against the schema of the
/// shell that runs it.
pub const SCHEMA: &str = include_str!("../schema/shell.schema.json");

/// File name the shell looks for.
pub const FILE_NAME: &str = "shell.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Manifest {
    #[serde(rename = "$schema", default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub id: String,
    pub title: String,
    pub scheme: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    data_dir: Option<String>,
    #[serde(default = "default_start_path")]
    pub start_path: String,
    #[serde(default)]
    pub no_store_paths: Vec<String>,
    pub window: Window,
    pub worker: Worker,
    #[serde(default)]
    pub dev: Dev,
    #[serde(default)]
    pub webview: Webview,
    #[serde(default)]
    pub menu: Menu,
    #[serde(default = "default_log_dir")]
    pub log_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Window {
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_height: Option<u32>,
    #[serde(default)]
    pub theme: Theme,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    Light,
    Dark,
    #[default]
    System,
}

impl Theme {
    pub fn parse(value: &str) -> Option<Theme> {
        match value {
            "light" => Some(Theme::Light),
            "dark" => Some(Theme::Dark),
            "system" => Some(Theme::System),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Worker {
    pub binary: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "default_ready_timeout_ms")]
    pub ready_timeout_ms: u64,
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: u64,
    #[serde(default)]
    pub restart: Restart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Restart {
    #[serde(default = "default_restart_max")]
    pub max: u32,
    #[serde(default = "default_restart_window")]
    pub window_seconds: u64,
}

impl Default for Restart {
    fn default() -> Self {
        Restart {
            max: default_restart_max(),
            window_seconds: default_restart_window(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Dev {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Webview {
    #[serde(default = "yes")]
    pub context_menu: bool,
    #[serde(default)]
    pub devtools: Devtools,
    #[serde(default = "yes")]
    pub spell_check: bool,
}

impl Default for Webview {
    fn default() -> Self {
        Webview {
            context_menu: true,
            devtools: Devtools::default(),
            spell_check: true,
        }
    }
}

/// `true`, `false`, or `"env:NAME"`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Devtools {
    Fixed(bool),
    Env(String),
}

impl Default for Devtools {
    fn default() -> Self {
        Devtools::Fixed(false)
    }
}

impl Devtools {
    /// Whether DevTools are on, given a way to read the environment. An `env:` switch is on when
    /// the variable is set to anything but the empty string or `0`.
    pub fn enabled(&self, env: impl Fn(&str) -> Option<String>) -> bool {
        match self {
            Devtools::Fixed(on) => *on,
            Devtools::Env(spec) => {
                let name = spec.strip_prefix("env:").unwrap_or(spec);
                env(name).is_some_and(|v| !v.is_empty() && v != "0")
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Menu {
    #[serde(default)]
    pub preset: MenuPreset,
    #[serde(default)]
    pub reserved_chords: Vec<String>,
}

impl Default for Menu {
    fn default() -> Self {
        Menu {
            preset: MenuPreset::Standard,
            reserved_chords: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MenuPreset {
    #[default]
    Standard,
    None,
}

fn default_start_path() -> String {
    "/".into()
}
fn default_log_dir() -> String {
    "auto".into()
}
fn default_ready_timeout_ms() -> u64 {
    15_000
}
fn default_max_body_bytes() -> u64 {
    crate::protocol::DEFAULT_MAX_BODY_BYTES
}
fn default_restart_max() -> u32 {
    3
}
fn default_restart_window() -> u64 {
    60
}
fn yes() -> bool {
    true
}

impl Manifest {
    /// The page origin, e.g. `boson://app`.
    pub fn origin(&self) -> String {
        format!("{}://app", self.scheme)
    }

    /// The first URL the webview loads.
    pub fn start_url(&self) -> String {
        format!("{}{}", self.origin(), self.start_path)
    }

    /// The per-user data directory name (defaults to the id).
    pub fn data_dir(&self) -> &str {
        self.data_dir.as_deref().unwrap_or(&self.id)
    }

    /// Reads, validates and parses a manifest file.
    pub fn load(path: &Path) -> Result<Manifest, ManifestError> {
        let text = fs::read_to_string(path).map_err(|e| ManifestError::Read(e.to_string()))?;
        Manifest::parse(&text)
    }

    /// Validates and parses manifest text.
    pub fn parse(text: &str) -> Result<Manifest, ManifestError> {
        let instance: Value = serde_json::from_str(text).map_err(|e| ManifestError::Json {
            line: e.line(),
            column: e.column(),
            message: e.to_string(),
        })?;

        let problems = validate(&instance);

        if !problems.is_empty() {
            return Err(ManifestError::Invalid(problems));
        }

        // Anything the schema accepts must parse; if it does not, the two have drifted.
        serde_json::from_value(instance).map_err(|e| ManifestError::Invalid(vec![e.to_string()]))
    }
}

/// Every schema violation in `instance`, each as `"<json pointer>: <what is wrong>"`.
pub fn validate(instance: &Value) -> Vec<String> {
    let schema: Value = serde_json::from_str(SCHEMA).expect("the embedded schema is valid JSON");
    let validator = jsonschema::validator_for(&schema).expect("the embedded schema compiles");

    let mut problems: Vec<String> = validator
        .iter_errors(instance)
        .map(|error| {
            let at = error.instance_path().to_string();
            let at = if at.is_empty() { "/".to_string() } else { at };
            format!("{at}: {error}")
        })
        .collect();
    problems.sort();
    problems.dedup();
    problems
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    Read(String),
    Json {
        line: usize,
        column: usize,
        message: String,
    },
    Invalid(Vec<String>),
}

impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ManifestError::Read(e) => write!(f, "cannot be read: {e}"),
            ManifestError::Json {
                line,
                column,
                message,
            } => write!(
                f,
                "is not valid JSON (line {line}, column {column}): {message}"
            ),
            ManifestError::Invalid(problems) => {
                write!(f, "does not match the manifest schema:")?;
                for p in problems {
                    write!(f, "\n  • {p}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ManifestError {}

impl From<io::Error> for ManifestError {
    fn from(e: io::Error) -> Self {
        ManifestError::Read(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn minimal() -> Value {
        json!({
            "id": "app.example",
            "title": "Example",
            "scheme": "boson",
            "window": {"width": 800, "height": 600},
            "worker": {"binary": "example-worker"}
        })
    }

    #[test]
    fn a_minimal_manifest_gets_the_defaults() {
        let m = Manifest::parse(&minimal().to_string()).unwrap();
        assert_eq!(m.origin(), "boson://app");
        assert_eq!(m.start_url(), "boson://app/");
        assert_eq!(m.data_dir(), "app.example");
        assert_eq!(m.worker.ready_timeout_ms, 15_000);
        assert_eq!(m.worker.max_body_bytes, 128 << 20);
        assert_eq!(
            m.worker.restart,
            Restart {
                max: 3,
                window_seconds: 60
            }
        );
        assert_eq!(m.window.theme, Theme::System);
        assert!(m.webview.context_menu);
        assert!(m.webview.spell_check);
        assert!(!m.webview.devtools.enabled(|_| Some("1".into())));
        assert_eq!(m.menu.preset, MenuPreset::Standard);
        assert_eq!(m.log_dir, "auto");
        assert_eq!(m.dev.worker, None);
    }

    #[test]
    fn errors_name_every_offending_member() {
        let mut v = minimal();
        v["window"]["width"] = json!(0);
        v["scheme"] = json!("https");
        v["worker"]["restart"] = json!({"max": 3, "windowSecs": 60});
        v["colour"] = json!("blue");

        let ManifestError::Invalid(problems) = Manifest::parse(&v.to_string()).unwrap_err() else {
            panic!("expected a schema error");
        };
        let all = problems.join("\n");
        assert!(all.contains("/window/width"), "{all}");
        assert!(all.contains("/scheme"), "{all}");
        assert!(
            all.contains("/worker/restart") && all.contains("windowSecs"),
            "{all}"
        );
        assert!(all.contains("colour"), "{all}");
    }

    #[test]
    fn missing_required_members_are_reported() {
        let ManifestError::Invalid(problems) = Manifest::parse("{}").unwrap_err() else {
            panic!("expected a schema error");
        };
        let all = problems.join("\n");
        for member in ["id", "title", "scheme", "window", "worker"] {
            assert!(all.contains(&format!("\"{member}\"")), "{member}: {all}");
        }
    }

    #[test]
    fn a_json_syntax_error_has_a_position() {
        let err = Manifest::parse("{\n  \"id\": \"x\",,\n}").unwrap_err();
        assert!(
            matches!(err, ManifestError::Json { line: 2, .. }),
            "{err:?}"
        );
        assert!(err.to_string().contains("line 2"));
    }

    #[test]
    fn devtools_follow_an_environment_switch() {
        let mut v = minimal();
        v["webview"] = json!({"devtools": "env:APP_DEBUG"});
        let m = Manifest::parse(&v.to_string()).unwrap();
        let env = |value: Option<&str>| {
            let value = value.map(str::to_owned);
            move |name: &str| {
                assert_eq!(name, "APP_DEBUG");
                value.clone()
            }
        };
        assert!(m.webview.devtools.enabled(env(Some("1"))));
        assert!(m.webview.devtools.enabled(env(Some("yes"))));
        assert!(!m.webview.devtools.enabled(env(Some("0"))));
        assert!(!m.webview.devtools.enabled(env(Some(""))));
        assert!(!m.webview.devtools.enabled(env(None)));

        v["webview"] = json!({"devtools": "APP_DEBUG"});
        assert!(Manifest::parse(&v.to_string()).is_err());
    }

    #[test]
    fn web_schemes_cannot_be_the_app_scheme() {
        for scheme in ["http", "https", "file", "data", "javascript", "Boson", "1x"] {
            let mut v = minimal();
            v["scheme"] = json!(scheme);
            assert!(Manifest::parse(&v.to_string()).is_err(), "{scheme}");
        }
    }

    #[test]
    fn integers_stay_inside_the_wire_range() {
        let mut v = minimal();
        v["worker"]["maxBodyBytes"] = json!(9_007_199_254_740_992_u64);
        assert!(Manifest::parse(&v.to_string()).is_err());
        v["worker"]["maxBodyBytes"] = json!(9_007_199_254_740_991_u64);
        assert!(Manifest::parse(&v.to_string()).is_ok());
    }

    /// Every `default` the schema advertises is the default the shell applies.
    #[test]
    fn schema_defaults_match_the_shell_defaults() {
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        let applied =
            serde_json::to_value(Manifest::parse(&minimal().to_string()).unwrap()).unwrap();

        let mut checked = 0;
        walk_defaults(&schema, &schema, "", &applied, &mut checked);
        assert!(
            checked >= 12,
            "only {checked} defaults found; did the walk break?"
        );
    }

    fn walk_defaults(
        root: &Value,
        node: &Value,
        pointer: &str,
        applied: &Value,
        checked: &mut usize,
    ) {
        let node = match node.get("$ref").and_then(Value::as_str) {
            Some(r) => root.pointer(r.trim_start_matches('#')).unwrap(),
            None => node,
        };
        if let Some(default) = node.get("default") {
            let actual = applied
                .pointer(pointer)
                .unwrap_or_else(|| panic!("{pointer} not applied"));
            assert_eq!(actual, default, "default of {pointer}");
            *checked += 1;
        }
        if let Some(props) = node.get("properties").and_then(Value::as_object) {
            for (name, sub) in props {
                let child = format!("{pointer}/{name}");
                // Only descend into objects the minimal manifest (with defaults) actually has.
                if applied.pointer(&child).is_some() || sub.get("default").is_some() {
                    walk_defaults(root, sub, &child, applied, checked);
                }
            }
        }
    }
}
