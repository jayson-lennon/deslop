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
                builtin: [
                    "aatell",
                    "slop",
                    "wsc",
                    "aisigns",
                    "cluster-terms",
                    "hedging",
                ]
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
/// Bare `wasm` file names resolve against the user plugin install dir,
/// `<XDG data dir>/deslop/plugins` (e.g. `~/.local/share/deslop/plugins`).
/// `data_dir` is the platform data directory; `None` leaves bare names
/// unresolved (tests, or hosts without a data dir).
///
/// # Errors
///
/// Returns [`ConfigError`] when an existing config cannot be read or parsed;
/// a missing discovered config means defaults apply.
pub fn discover(
    start: &camino::Utf8Path,
    data_dir: Option<&camino::Utf8Path>,
    user_config: Option<&camino::Utf8Path>,
) -> Result<Config, error_stack::Report<ConfigError>> {
    if start.is_file() {
        return parse_config_file(start, data_dir);
    }

    let found = start
        .ancestors()
        .map(|dir| dir.join(".deslop.toml"))
        .find(|candidate| candidate.is_file());

    match found {
        Some(path) => parse_config_file(&path, data_dir),
        // No project config anywhere up the tree: fall back to the
        // user-global config (~/.config/deslop/deslop.toml), mirroring how
        // rule packs honor ~/.config/deslop/rules. A project config, when
        // present, always wins; `--config` beats both.
        None => match user_config.filter(|path| path.is_file()) {
            Some(path) => parse_config_file(path, data_dir),
            None => Ok(Config::default()),
        },
    }
}

fn parse_config_file(
    path: &camino::Utf8Path,
    data_dir: Option<&camino::Utf8Path>,
) -> Result<Config, error_stack::Report<ConfigError>> {
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
        .and_then(|cfg| resolve_plugin_paths(cfg, path, data_dir))
}

/// Re-anchor plugin `wasm` paths now that the config file's location is
/// known. All resolution happens here so `parse_config_str` stays
/// text-only: absolute paths pass through exactly; `./`/`../` forms join
/// onto the config file's directory (repo-local modules); bare names join
/// onto the user plugin install dir `<data_dir>/deslop/plugins` (left
/// as-given when `data_dir` is unavailable).
fn resolve_plugin_paths(
    mut cfg: Config,
    config_path: &camino::Utf8Path,
    data_dir: Option<&camino::Utf8Path>,
) -> Result<Config, error_stack::Report<ConfigError>> {
    let config_dir = config_path.parent();
    let install_dir: Option<camino::Utf8PathBuf> =
        data_dir.map(|dir| dir.join("deslop").join("plugins"));
    for decl in cfg.plugins.plugins.values_mut() {
        let wasm = decl.path.as_str();
        // Normalize `./x` to `x` so joins render `<dir>/x`, not `<dir>/./x`.
        let relative = wasm.strip_prefix("./").unwrap_or(wasm);
        let anchored: Option<&camino::Utf8Path> = match () {
            _ if decl.path.is_absolute() => None,
            _ if wasm.starts_with("./") || wasm.starts_with("../") => config_dir,
            _ => install_dir.as_deref(),
        };
        if let Some(dir) = anchored {
            decl.path = dir.join(relative);
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
    /// TOML section `[plugin.<id>]` (singular); the typed config keeps the
    /// plural field name since it holds every declaration.
    #[serde(default, rename = "plugin")]
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

/// Raw `[plugin.<id>]` section: one table per plugin, carrying the module
/// path, an optional enable switch, host knobs in `.runtime`, and opaque
/// params everywhere else.
///
/// Decoded as a raw `toml::Table` (not serde-flattened structs) because the
/// table keys are plugin ids we must pass through untouched, and two of the
/// inner keys (`wasm`, `runtime`) are host-owned. Hand-rolling keeps the
/// "everything under `[plugin.<id>]` except `wasm`/`enabled`/`runtime` is
/// opaque" rule explicit.
#[derive(Debug, Default, serde::Deserialize)]
struct RawPlugins(toml::Table);

/// The `.wasm` extension every `wasm` value must carry; lookup is literal.
const WASM_EXT: &str = "wasm";

impl RawPlugins {
    /// Validate and split into typed declarations + params.
    ///
    /// Paths are kept AS WRITTEN here; anchoring happens later in
    /// `resolve_plugin_paths`, once the config file's directory and the
    /// data dir are known.
    fn into_plugin_config(self) -> Result<crate::plugin::PluginConfig, String> {
        let mut out = crate::plugin::PluginConfig::default();
        for (key, value) in &self.0 {
            match key.as_str() {
                "lints" | "packs" | "scan" | "output" => {
                    return Err(format!(
                        "unexpected key [plugin.{key}] (this belongs at the top level)"
                    ));
                }
                other => {
                    let table = value
                        .as_table()
                        .ok_or_else(|| format!("plugin.{other} must be a table"))?;
                    // Reserved keys are host-consumed; the rest is opaque.
                    let mut params = table.clone();
                    let wasm = params
                        .remove("wasm")
                        .ok_or_else(|| format!("plugin.{other} is missing the wasm key"))?;
                    let wasm = wasm
                        .as_str()
                        .ok_or_else(|| format!("plugin.{other}.wasm must be a string"))?;
                    if !wasm.ends_with(WASM_EXT) {
                        return Err(format!(
                            "plugin.{other}.wasm must end in .wasm (literal file lookup), \
                             got {wasm:?}"
                        ));
                    }
                    let enabled = match params.remove("enabled") {
                        Some(v) => v
                            .as_bool()
                            .ok_or_else(|| format!("plugin.{other}.enabled must be a boolean"))?,
                        None => true,
                    };
                    let mut runtime = crate::plugin::PluginRuntime::default();
                    if let Some(rt) = params.remove("runtime") {
                        let rt = rt
                            .as_table()
                            .ok_or_else(|| format!("plugin.{other}.runtime must be a table"))?;
                        parse_runtime(other, rt, &mut runtime)?;
                    }
                    let id = other.to_uppercase();
                    let decl = crate::plugin::PluginDeclaration {
                        key: other.to_owned(),
                        path: camino::Utf8PathBuf::from(wasm),
                        enabled,
                    };
                    if out.plugins.insert(id.clone(), decl).is_some() {
                        return Err(format!(
                            "plugin.{other} declared twice (keys differing only by case collide)"
                        ));
                    }
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

/// Parse one plugin's `.runtime` table into host knobs.
fn parse_runtime(
    id: &str,
    rt: &toml::Table,
    runtime: &mut crate::plugin::PluginRuntime,
) -> Result<(), String> {
    for (rk, rv) in rt {
        match rk.as_str() {
            "fuel" => {
                let fuel = rv
                    .as_integer()
                    .ok_or_else(|| format!("plugin.{id}.runtime.fuel must be an integer"))?;
                if fuel < 0 {
                    return Err(format!("plugin.{id}.runtime.fuel must be >= 0"));
                }
                runtime.fuel = Some(fuel as u64);
            }
            unknown => {
                return Err(format!(
                    "unknown plugin.{id}.runtime key {unknown:?} (known: fuel)"
                ));
            }
        }
    }
    Ok(())
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
        let mut lint = std::collections::BTreeMap::new();
        for (key, level) in self.lints {
            let level = level.into_level().map_err(|e| format!("{key}: {e}"))?;
            lint.insert(key, level);
        }
        let plugins = self
            .plugins
            .into_plugin_config()
            .map_err(|e| format!("[plugin] {e}"))?;
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
        let cfg = discover(
            camino::Utf8Path::from_path(root.join("b").as_path()).expect("utf8"),
            None,
            None,
        )
        .expect("discover");

        // Then the parent's config applies.
        assert_eq!(cfg.scan.tiers, vec![1]);
    }

    #[test]
    fn discover_without_any_config_falls_back_to_defaults() {
        // Given an empty temp tree.
        let dir = tempfile::tempdir().expect("tempdir");

        // When discovering from inside it.
        let cfg = discover(
            camino::Utf8Path::from_path(dir.path()).expect("utf8"),
            None,
            None,
        )
        .expect("discover");

        // Then defaults apply (all six builtin packs).
        assert_eq!(cfg.packs.builtin.len(), 6);
    }

    #[test]
    fn discover_falls_back_to_the_user_global_config() {
        // Given no project config and a user-global file with a marker value.
        let dir = tempfile::tempdir().expect("tempdir");
        let user_dir = tempfile::tempdir().expect("tempdir");
        let user_config = user_dir.path().join("deslop.toml");
        std::fs::write(&user_config, "[scan]\ntiers = [1]\n").expect("write");

        // When discovering from inside the empty tree.
        let cfg = discover(
            camino::Utf8Path::from_path(dir.path()).expect("utf8"),
            None,
            Some(camino::Utf8Path::from_path(&user_config).expect("utf8")),
        )
        .expect("discover");

        // Then the user-global config applies.
        assert_eq!(cfg.scan.tiers, vec![1]);
    }

    #[test]
    fn project_config_wins_over_the_user_global_config() {
        // Given both a project config and a user-global config.
        let dir = tempfile::tempdir().expect("tempdir");
        let user_dir = tempfile::tempdir().expect("tempdir");
        let user_config = user_dir.path().join("deslop.toml");
        std::fs::write(&user_config, "[scan]\ntiers = [1]\n").expect("write");
        std::fs::write(dir.path().join(".deslop.toml"), "[scan]\ntiers = [2]\n").expect("write");

        // When discovering from the project dir.
        let cfg = discover(
            camino::Utf8Path::from_path(dir.path()).expect("utf8"),
            None,
            Some(camino::Utf8Path::from_path(&user_config).expect("utf8")),
        )
        .expect("discover");

        // Then the project config wins.
        assert_eq!(cfg.scan.tiers, vec![2]);
    }

    #[test]
    fn a_missing_user_global_config_still_falls_back_to_defaults() {
        // Given no project config and a user config path that does not exist.
        let dir = tempfile::tempdir().expect("tempdir");

        // When discovering.
        let cfg = discover(
            camino::Utf8Path::from_path(dir.path()).expect("utf8"),
            None,
            Some(camino::Utf8Path::new("/nonexistent/deslop.toml")),
        )
        .expect("discover");

        // Then defaults apply.
        assert_eq!(cfg.packs.builtin.len(), 6);
    }

    #[test]
    fn plugin_block_parses_wasm_params_and_runtime() {
        // Given a config declaring one plugin with params and a fuel knob.
        let text = r#"
[plugin.exclaim]
wasm = "exclaim.wasm"
threshold_gt = 2.5

[plugin.exclaim.runtime]
fuel = 123456
"#;

        // When parsing.
        let cfg = parse_config_str(text).expect("parse");

        // Then the wasm value stays as-written (text mode) under the
        // upper-cased key, and params are opaque JSON.
        let decl = cfg.plugins.plugins.get("EXCLAIM").expect("declaration");
        assert_eq!(decl.path, camino::Utf8PathBuf::from("exclaim.wasm"));
        assert!(decl.enabled);
        let params = cfg.plugins.params.get("EXCLAIM").expect("params");
        assert_eq!(params["threshold_gt"], serde_json::json!(2.5));
        // And the reserved keys never leak into params.
        assert!(params.get("wasm").is_none());
        assert!(params.get("runtime").is_none());
        // And the runtime knob landed in its own map.
        assert_eq!(cfg.plugins.runtime["EXCLAIM"].fuel, Some(123_456));
    }

    #[test]
    fn plugin_enabled_false_marks_the_declaration() {
        // Given a plugin switched off at the load level.
        let text = r#"
[plugin.p]
wasm = "p.wasm"
enabled = false
"#;

        // When parsing.
        let cfg = parse_config_str(text).expect("parse");

        // Then the declaration carries enabled = false.
        assert!(!cfg.plugins.plugins["P"].enabled);
    }

    #[test]
    fn plugin_table_keys_become_uppercase_ids() {
        // Given a config using lowercase plugin ids.
        let text = r#"
[plugin.myplug]
wasm = "myplug.wasm"
flag = true
"#;

        // When parsing.
        let cfg = parse_config_str(text).expect("parse");

        // Then the params key is upper-cased to match manifest ids.
        assert!(cfg.plugins.params.contains_key("MYPLUG"));
    }

    #[test]
    fn plugin_section_absent_yields_default() {
        // Given a config without any [plugin.*] table.
        let cfg = parse_config_str("[lints]\nFOO = \"allow\"\n").expect("parse");

        // Then the plugin config is empty.
        assert!(cfg.plugins.plugins.is_empty());
        assert!(cfg.plugins.params.is_empty());
        assert!(cfg.plugins.runtime.is_empty());
    }

    #[test]
    fn plugin_missing_wasm_key_is_a_config_error() {
        // Given a plugin table with no wasm path.
        let text = "[plugin.p]\nthreshold = 1\n";

        // When parsing.
        let result = parse_config_str(text);

        // Then it fails naming the missing key.
        let err = format!("{:?}", result.expect_err("must fail"));
        assert!(err.contains("missing the wasm key"), "{err}");
    }

    #[test]
    fn plugin_wasm_without_wasm_extension_is_a_config_error() {
        // Given a wasm path that is not a .wasm file.
        let text = "[plugin.p]\nwasm = \"p.wat\"\n";

        // When parsing.
        let result = parse_config_str(text);

        // Then it fails naming the extension requirement.
        let err = format!("{:?}", result.expect_err("must fail"));
        assert!(err.contains(".wasm"), "{err}");
    }

    #[test]
    fn plugin_unknown_runtime_key_is_a_config_error() {
        // Given a runtime table with an unrecognized knob.
        let text = r#"
[plugin.p]
wasm = "p.wasm"

[plugin.p.runtime]
fual = 10
"#;

        // When parsing.
        let result = parse_config_str(text);

        // Then it fails naming the bad key.
        let err = result.expect_err("must fail");
        assert!(format!("{err}").contains("fual"));
    }

    #[test]
    fn plugin_negative_fuel_is_a_config_error() {
        // Given a negative fuel value.
        let text = r#"
[plugin.p]
wasm = "p.wasm"

[plugin.p.runtime]
fuel = -5
"#;

        // When parsing.
        let result = parse_config_str(text);

        // Then it fails.
        assert!(result.is_err());
    }

    #[test]
    fn plugin_scalar_entry_is_a_config_error() {
        // Given a plugin entry that is not a table.
        let text = "plugin = 3\n";

        // When parsing.
        let result = parse_config_str(text);

        // Then it fails with a type error.
        assert!(result.is_err());
    }

    #[test]
    fn wasm_paths_resolve_by_form_against_the_right_anchor() {
        // Given a config using all three path forms and a known data dir.
        let text = r#"
[plugin.installed]
wasm = "installed.wasm"

[plugin.repolocal]
wasm = "./modules/repo.wasm"

[plugin.absolute]
wasm = "/opt/lints/abs.wasm"
"#;
        let raw: RawConfig = toml::from_str(text).expect("toml");
        let mut cfg = raw.into_config().expect("convert");

        // When resolving with both anchors known.
        let config_path = camino::Utf8PathBuf::from("/repo/.deslop.toml");
        let data_dir = camino::Utf8PathBuf::from("/home/u/.local/share");
        cfg = resolve_plugin_paths(cfg, &config_path, Some(&data_dir)).expect("resolve");

        // Then each form lands at its anchor: bare → install dir,
        // ./ → config dir, absolute → untouched.
        assert_eq!(
            cfg.plugins.plugins["INSTALLED"].path,
            camino::Utf8PathBuf::from("/home/u/.local/share/deslop/plugins/installed.wasm")
        );
        assert_eq!(
            cfg.plugins.plugins["REPOLOCAL"].path,
            camino::Utf8PathBuf::from("/repo/modules/repo.wasm")
        );
        assert_eq!(
            cfg.plugins.plugins["ABSOLUTE"].path,
            camino::Utf8PathBuf::from("/opt/lints/abs.wasm")
        );
    }

    #[test]
    fn case_colliding_plugin_keys_are_a_config_error() {
        // Given two tables whose keys differ only by case.
        let text = r#"
[plugin.Ex]
wasm = "a.wasm"

[plugin.ex]
wasm = "b.wasm"
"#;

        // When parsing.
        let result = parse_config_str(text);

        // Then the collision is rejected (one id, two declarations).
        assert!(result.is_err());
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
}
