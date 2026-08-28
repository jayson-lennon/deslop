//! `.deslop.toml` configuration: discovery + typed model.

use std::collections::BTreeMap;

/// Fully resolved deslop configuration.
///
/// `BTreeMap` everywhere so iteration order is deterministic regardless of
/// insertion order — lint output must be byte-stable run to run.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub packs: Packs,
    pub scan: Scan,
    pub output: OutputFormatSection,
    /// Per-entry / per-group level overrides: `[lints] ID = "level"`.
    pub lint: BTreeMap<String, LintLevel>,
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
    parse_config_str(&text).map_err(|report| {
        report.change_context(ConfigError::Read {
            path: path.to_owned(),
            source: std::io::Error::other("invalid config syntax"),
        })
    })
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

impl RawConfig {
    /// Convert with level validation; `Err` carries the offending key+level.
    fn into_config(self) -> Result<Config, String> {
        let mut lint = std::collections::BTreeMap::new();
        for (key, level) in self.lints {
            let level = level.into_level().map_err(|e| format!("{key}: {e}"))?;
            lint.insert(key, level);
        }
        let mut cfg = Config {
            lint,
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
}
