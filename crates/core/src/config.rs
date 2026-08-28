//! `.deslop.toml` configuration: discovery + typed model.

use std::collections::BTreeMap;

/// Fully resolved deslop configuration.
///
/// `BTreeMap` everywhere so iteration order is deterministic regardless of
/// insertion order - lint output must be byte-stable run to run.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub packs: Packs,
    pub scan: Scan,
    pub output: OutputFormatSection,
    /// Per-entry / per-group level overrides: `[lints] ID = "level"`.
    pub lint: BTreeMap<String, LintLevel>,
    /// WASM plugin declarations: `[plugins]`.
    pub plugins: crate::plugin::PluginConfig,
}

/// Pack selection: which builtin packs load, plus extra user packs.
#[derive(Debug, Clone, PartialEq)]
pub struct Packs {
    pub builtin: Vec<String>,
    pub extra_paths: Vec<camino::Utf8PathBuf>,
}

/// Scanner-level switches.
#[derive(Debug, Clone, PartialEq)]
pub struct Scan {
    /// Enabled tier numbers, ascending; see [`crate::finding::Tier`].
    pub tiers: Vec<u8>,
    pub respect_gitignore: bool,
    pub extra_globs: Vec<String>,
}

/// Output formatting preferences.
///
/// Named to avoid collision with renderer-side enums that gain behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputFormatSection {
    pub format: FormatName,
    pub color: ColorChoice,
}

impl Default for OutputFormatSection {
    fn default() -> Self {
        OutputFormatSection {
            format: FormatName::Human,
            color: ColorChoice::Auto,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatName {
    Human,
    Json,
    Github,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorChoice {
    Auto,
    Always,
    Never,
}

/// Effective level for a lint, clippy-style. Default is the rule's tier;
/// config overrides per GROUP or GROUP#slug (slug wins).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LintLevel {
    Allow,
    Note,
    Warn,
    Error,
}

impl LintLevel {
    /// Parse a `[lints]` value string; `None` = unknown level (config error).
    pub fn parse(s: &str) -> Option<LintLevel> {
        Some(match s {
            "allow" => LintLevel::Allow,
            "note" => LintLevel::Note,
            "warn" => LintLevel::Warn,
            "error" => LintLevel::Error,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            LintLevel::Allow => "allow",
            LintLevel::Note => "note",
            LintLevel::Warn => "warn",
            LintLevel::Error => "error",
        }
    }
}

impl Default for Config {
    fn default() -> Config {
        Config {
            packs: Packs {
                builtin: ["aatell", "slop", "wsc", "aisigns", "cluster-terms"]
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect(),
                extra_paths: Vec::new(),
            },
            scan: Scan {
                tiers: vec![1, 2, 3],
                respect_gitignore: true,
                extra_globs: Vec::new(),
            },
            output: OutputFormatSection::default(),
            lint: BTreeMap::new(),
            plugins: crate::plugin::PluginConfig::default(),
        }
    }
}

/// Errors surfaced while locating or reading configuration.
#[derive(Debug, wherror::Error)]
#[error(debug)]
pub enum ConfigError {
    #[error("config file {path} is not valid UTF-8")]
    NotUtf8 { path: camino::Utf8PathBuf },
    #[error("failed to process config file {path}: {source}")]
    Read {
        path: camino::Utf8PathBuf,
        source: std::io::Error,
    },
    #[error("invalid [lints] entry: {detail}")]
    LintLevel { detail: String },
}

/// Discover the nearest `.deslop.toml` walking up from `start`, falling back
/// to [`Config::default`] when none exists. An explicit `start` pointing at a
/// file is honored as THE config location (`--config PATH` semantics).
///
/// # Errors
///
/// Returns [`ConfigError`] when an existing config cannot be read or parsed;
/// a missing discovered config means defaults apply.
pub fn discover(start: &camino::Utf8Path) -> Result<Config, error_stack::Report<ConfigError>> {
    if start.is_file() {
        return parse_config_file(start);
    }

    let found = start
        .ancestors()
        .map(|dir| dir.join(".deslop.toml"))
        .find(|candidate| candidate.is_file());

    match found {
        Some(path) => parse_config_file(&path),
        None => Ok(Config::default()),
    }
}

fn parse_config_file(path: &camino::Utf8Path) -> Result<Config, error_stack::Report<ConfigError>> {
    let text = std::fs::read_to_string(path).map_err(|source| {
        error_stack::Report::new(ConfigError::Read {
            path: path.to_owned(),
            source,
        })
    })?;
    // Position-free until phase 2 introduces spanned pack diagnostics.
    parse_config_str(&text)
        .map_err(|report| {
            report.change_context(ConfigError::Read {
                path: path.to_owned(),
                source: std::io::Error::other("invalid config syntax"),
            })
        })
        .and_then(|cfg| resolve_plugin_paths(cfg, path))
}

/// Re-anchor relative `plugins.paths` entries against the config file's
/// directory. Done post-parse so `parse_config_str` stays text-only (the
/// test path leaves paths cwd-relative).
fn resolve_plugin_paths(
    mut cfg: Config,
    config_path: &camino::Utf8Path,
) -> Result<Config, error_stack::Report<ConfigError>> {
    if let Some(dir) = config_path.parent() {
        for path in &mut cfg.plugins.paths {
            if path.is_relative() {
                *path = dir.join(&*path);
            }
        }
    }
    Ok(cfg)
}

/// Parse configuration text (exposed for tests).
///
/// # Errors
///
/// Invalid TOML yields a report carrying the toml error detail.
pub fn parse_config_str(text: &str) -> Result<Config, error_stack::Report<ConfigError>> {
    let raw: RawConfig = toml::from_str(text).map_err(|e| {
        error_stack::Report::new(ConfigError::Read {
            path: "".into(),
            source: std::io::Error::other(e.to_string()),
        })
    })?;
    raw.into_config()
        .map_err(|detail| error_stack::Report::new(ConfigError::LintLevel { detail }))
}

/// Raw serde mirror of the file format.
#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    #[serde(default)]
    packs: RawPacks,
    #[serde(default)]
    scan: RawScan,
    #[serde(default)]
    output: RawOutput,
    #[serde(default)]
    lints: BTreeMap<String, RawLintLevel>,
    #[serde(default)]
    plugins: RawPlugins,
}

/// String-newtype enabling validation of level values at parse time.
#[derive(Debug, serde::Deserialize)]
struct RawLintLevel(String);

impl RawLintLevel {
    /// Convert to a level; `Err` names the bad value.
    fn into_level(self) -> Result<LintLevel, String> {
        LintLevel::parse(&self.0).ok_or_else(|| {
            format!(
                "unknown lint level {:?} (expected allow|note|warn|error)",
                self.0
            )
        })
    }
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPacks {
    builtin: Option<Vec<String>>,
    extra_paths: Option<Vec<String>>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawScan {
    tiers: Option<Vec<u8>>,
    respect_gitignore: Option<bool>,
    extra_globs: Option<Vec<String>>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RawOutput {
    format: Option<String>,
    color: Option<String>,
}

/// Raw `[plugins]` section: a list of files plus opaque per-plugin tables.
///
/// Decoded as a raw `toml::Table` (not serde-flattened structs) because the
/// table keys are plugin ids we must pass through untouched, and one of
/// them (`runtime`) is host-owned. Hand-rolling keeps the "everything under
/// `[plugins.<id>]` except `.runtime` is opaque" rule explicit.
#[derive(Debug, Default, serde::Deserialize)]
struct RawPlugins(toml::Table);

impl RawPlugins {
    /// Resolve into the typed plugin config.
    ///
    /// `config_dir` anchors relative `paths` entries; `None` leaves them
    /// as-given (the `parse_config_str` test path).
    fn into_plugin_config(
        self,
        config_dir: Option<&camino::Utf8Path>,
    ) -> Result<crate::plugin::PluginConfig, String> {
        let mut out = crate::plugin::PluginConfig::default();
        for (key, value) in &self.0 {
            match key.as_str() {
                "paths" => {
                    let items = value
                        .as_array()
                        .ok_or_else(|| "plugins.paths must be an array of strings".to_string())?;
                    for item in items {
                        let raw = item
                            .as_str()
                            .ok_or_else(|| "plugins.paths entries must be strings".to_string())?;
                        let path = camino::Utf8PathBuf::from(raw);
                        out.paths.push(match config_dir {
                            Some(dir) if path.is_relative() => dir.join(path),
                            _ => path,
                        });
                    }
                }
                "lints" | "packs" | "scan" | "output" => {
                    return Err(format!(
                        "unexpected key [plugins.{key}] (this belongs at the top level)"
                    ));
                }
                other => {
                    let table = value
                        .as_table()
                        .ok_or_else(|| format!("plugins.{other} must be a table"))?;
                    // `.runtime` is host-owned; everything else is opaque params.
                    let mut runtime = crate::plugin::PluginRuntime::default();
                    let mut params = table.clone();
                    if let Some(rt) = params.remove("runtime") {
                        let rt = rt
                            .as_table()
                            .ok_or_else(|| format!("plugins.{other}.runtime must be a table"))?;
                        for (rk, rv) in rt {
                            match rk.as_str() {
                                "fuel" => {
                                    let fuel = rv.as_integer().ok_or_else(|| {
                                        format!("plugins.{other}.runtime.fuel must be an integer")
                                    })?;
                                    if fuel < 0 {
                                        return Err(format!(
                                            "plugins.{other}.runtime.fuel must be >= 0"
                                        ));
                                    }
                                    runtime.fuel = Some(fuel as u64);
                                }
                                unknown => {
                                    return Err(format!(
                                        "unknown plugins.{other}.runtime key {unknown:?} \
                                         (known: fuel)"
                                    ));
                                }
                            }
                        }
                    }
                    let id = other.to_uppercase();
                    if runtime != crate::plugin::PluginRuntime::default() {
                        out.runtime.insert(id.clone(), runtime);
                    }
                    // An empty params table is left unregistered: the plugin
                    // then receives `{}`, matching "nothing declared".
                    if !params.is_empty() {
                        let json = toml_to_json(&toml::Value::Table(params));
                        out.params.insert(id, json);
                    }
                }
            }
        }
        Ok(out)
    }
}

/// Convert a toml value to JSON, preserving table contents verbatim.
///
/// Numbers become integers or floats; datetimes stringify (a plugin asking
/// for a TOML datetime as a config value is far outside normal use).
fn toml_to_json(value: &toml::Value) -> serde_json::Value {
    match value {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => serde_json::Value::Number((*i).into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        toml::Value::Boolean(b) => serde_json::Value::Bool(*b),
        toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
        toml::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(toml_to_json).collect())
        }
        toml::Value::Table(map) => serde_json::Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect(),
        ),
    }
}

impl RawConfig {
    /// Convert with level validation; `Err` carries the offending key+level.
    fn into_config(self) -> Result<Config, String> {
        self.into_config_in_dir(None)
    }

    /// Like [`Self::into_config`] but anchors relative plugin paths at
    /// `config_dir` when provided.
    fn into_config_in_dir(
        self,
        config_dir: Option<&camino::Utf8Path>,
    ) -> Result<Config, String> {
        let mut lint = std::collections::BTreeMap::new();
        for (key, level) in self.lints {
            let level = level.into_level().map_err(|e| format!("{key}: {e}"))?;
            lint.insert(key, level);
        }
        let plugins = self
            .plugins
            .into_plugin_config(config_dir)
            .map_err(|e| format!("[plugins] {e}"))?;
        let mut cfg = Config {
            lint,
            plugins,
            ..Config::default()
        };
        if let Some(builtin) = self.packs.builtin {
            cfg.packs.builtin = builtin;
        }
        if let Some(extra) = self.packs.extra_paths {
            cfg.packs.extra_paths = extra.into_iter().map(camino::Utf8PathBuf::from).collect();
        }
        if let Some(tiers) = self.scan.tiers {
            cfg.scan.tiers = tiers;
        }
        if let Some(gi) = self.scan.respect_gitignore {
            cfg.scan.respect_gitignore = gi;
        }
        if let Some(globs) = self.scan.extra_globs {
            cfg.scan.extra_globs = globs;
        }
        if let Some(fmt) = self.output.format {
            cfg.output.format = match fmt.as_str() {
                "json" => FormatName::Json,
                "github" => FormatName::Github,
                _ => FormatName::Human,
            };
        }
        if let Some(color) = self.output.color {
            cfg.output.color = match color.as_str() {
                "always" => ColorChoice::Always,
                "never" => ColorChoice::Never,
                _ => ColorChoice::Auto,
            };
        }
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_yields_default_lint_map() {
        // Given an empty config document.
        let text = "";

        // When parsing.
        let result = parse_config_str(text);

        // Then it equals the built-in defaults' overrides (none).
        assert_eq!(result.expect("parse").lint, Config::default().lint);
    }

    #[test]
    fn unknown_lint_keys_are_preserved_for_tolerance() {
        // Given a lint override naming a rule that may not exist yet.
        let text = r#"
[lints]
"MODERN-VOCAB#showcase" = "allow"
"#;

        // When parsing.
        let cfg = parse_config_str(text).expect("parse");

        // Then the override survives under its exact key.
        assert_eq!(
            cfg.lint.get("MODERN-VOCAB#showcase").copied(),
            Some(LintLevel::Allow)
        );
    }

    #[test]
    fn unknown_lint_level_is_rejected() {
        // Given a misspelled level value.
        let text = "[lints]\nfoo = \"silent\"\n";

        // When parsing.
        let result = parse_config_str(text);

        // Then it is a lint-level error mentioning the bad value.
        let err = format!("{:?}", result.expect_err("reject"));
        assert!(err.contains("silent"));
    }

    #[test]
    fn lint_levels_parse_from_config() {
        // Given every supported level.
        let text = "[lints]\na = \"allow\"\nb = \"note\"\nc = \"warn\"\nd = \"error\"\n";

        // When parsing.
        let cfg = parse_config_str(text).expect("parse");

        // Then each key maps to its level.
        assert_eq!(cfg.lint.get("a"), Some(&LintLevel::Allow));
        assert_eq!(cfg.lint.get("b"), Some(&LintLevel::Note));
        assert_eq!(cfg.lint.get("c"), Some(&LintLevel::Warn));
        assert_eq!(cfg.lint.get("d"), Some(&LintLevel::Error));
    }

    #[test]
    fn unknown_fields_are_rejected() {
        // Given a document containing a misspelled section.
        let text = "[scanx]\ntiers = [1]\n";

        // When parsing.
        let result = parse_config_str(text);

        // Then parsing fails rather than silently ignoring.
        assert!(result.is_err());
    }

    #[test]
    fn explicit_overrides_reach_typed_config() {
        // Given a config overriding every section.
        let text = r#"
[packs]
builtin = ["artifacts"]
extra_paths = ["/tmp/team-pack"]

[scan]
tiers = [2]
respect_gitignore = false

[output]
format = "json"
color = "never"
"#;

        // When parsing.
        let cfg = parse_config_str(text).expect("parse");

        // Then each override lands in its typed slot.
        assert_eq!(cfg.packs.builtin, vec!["artifacts"]);
        assert_eq!(cfg.packs.extra_paths.len(), 1);
        assert_eq!(cfg.scan.tiers, vec![2]);
        assert!(!cfg.scan.respect_gitignore);
        assert_eq!(cfg.output.format, FormatName::Json);
        assert_eq!(cfg.output.color, ColorChoice::Never);
    }

    #[test]
    fn discover_walks_up_to_nearest_config() {
        // Given a directory tree with a config two levels up.
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("a");
        std::fs::create_dir_all(root.join("b")).expect("mkdir");
        std::fs::write(root.join(".deslop.toml"), "[scan]\ntiers = [1]\n").expect("write");

        // When discovering from the deep child.
        let cfg = discover(camino::Utf8Path::from_path(root.join("b").as_path()).expect("utf8"))
            .expect("discover");

        // Then the parent's config applies.
        assert_eq!(cfg.scan.tiers, vec![1]);
    }

    #[test]
    fn discover_without_any_config_falls_back_to_defaults() {
        // Given an empty temp tree.
        let dir = tempfile::tempdir().expect("tempdir");

        // When discovering from inside it.
        let cfg =
            discover(camino::Utf8Path::from_path(dir.path()).expect("utf8")).expect("discover");

        // Then defaults apply (all five builtin packs).
        assert_eq!(cfg.packs.builtin.len(), 5);
    }

    #[test]
    fn plugins_section_parses_paths_and_params() {
        // Given a config declaring one plugin file and one params table.
        let text = r#"
[plugins]
paths = ["plug.wasm"]

[plugins.exclaim]
threshold_gt = 2.5

[plugins.exclaim.runtime]
fuel = 123456
"#;

        // When parsing.
        let cfg = parse_config_str(text).expect("parse");

        // Then paths stay relative (text mode) and params are opaque JSON.
        assert_eq!(
            cfg.plugins.paths,
            vec![camino::Utf8PathBuf::from("plug.wasm")]
        );
        let params = cfg.plugins.params.get("EXCLAIM").expect("params");
        assert_eq!(params["threshold_gt"], serde_json::json!(2.5));
        // And the runtime table never leaks into params.
        assert!(params.get("runtime").is_none());
        // And the runtime knob landed in its own map.
        assert_eq!(cfg.plugins.runtime["EXCLAIM"].fuel, Some(123_456));
    }

    #[test]
    fn plugin_table_keys_become_uppercase_ids() {
        // Given a config using lowercase plugin ids.
        let text = r#"
[plugins.myplug]
flag = true
"#;

        // When parsing.
        let cfg = parse_config_str(text).expect("parse");

        // Then the params key is upper-cased to match manifest ids.
        assert!(cfg.plugins.params.contains_key("MYPLUG"));
    }

    #[test]
    fn plugins_section_absent_yields_default() {
        // Given a config without [plugins].
        let cfg = parse_config_str("[lints]\nFOO = \"allow\"\n").expect("parse");

        // Then the plugin config is empty.
        assert!(cfg.plugins.paths.is_empty());
        assert!(cfg.plugins.params.is_empty());
        assert!(cfg.plugins.runtime.is_empty());
    }

    #[test]
    fn plugins_unknown_runtime_key_is_a_config_error() {
        // Given a runtime table with an unrecognized knob.
        let text = r#"
[plugins.p.runtime]
fual = 10
"#;

        // When parsing.
        let result = parse_config_str(text);

        // Then it fails naming the bad key.
        let err = result.expect_err("must fail");
        assert!(format!("{err}").contains("fual"));
    }

    #[test]
    fn plugins_negative_fuel_is_a_config_error() {
        // Given a negative fuel value.
        let text = r#"
[plugins.p.runtime]
fuel = -5
"#;

        // When parsing.
        let result = parse_config_str(text);

        // Then it fails.
        assert!(result.is_err());
    }

    #[test]
    fn plugins_scalar_entry_is_a_config_error() {
        // Given a plugin entry that is not a table.
        let text = "[plugins]\np = 3\n";

        // When parsing.
        let result = parse_config_str(text);

        // Then it fails with a type error.
        assert!(result.is_err());
    }
}
