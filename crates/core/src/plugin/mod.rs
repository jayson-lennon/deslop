//! Plugin support: out-of-process-style lint logic loaded from WASM files.
//!
//! A plugin is a `.wasm` module (declared under `[plugins]` in
//! `.deslop.toml`) that turns a [`PluginInput`] document envelope into
//! finished [`PluginFinding`]s. The host owns everything mechanical — ABI
//! marshalling, validation, coordinate remapping, `[lints]` overrides,
//! deterministic merge — while the plugin owns its entire detection pipeline
//! (metrics, thresholds, wording).
//!
//! The stable seam is the runtime-agnostic [`LintPlugin`] trait:
//! [`super::plugin::wasmi_host::WasmiPlugin`] is the production
//! implementation, [`fake::FakePlugin`] is an in-memory implementation for
//! tests. Everything above the trait (scanner, CLI) never knows which one it
//! is talking to.

use std::collections::{BTreeMap, HashSet};
use std::fmt;

use deslop_plugin_protocol::{PluginFinding, PluginInput, PluginManifest, PROTOCOL_ABI};

pub mod fake;
pub mod wasmi_host;

/// Fuel budget baseline for a single plugin call, independent of input size.
///
/// Sized so a bare instantiation plus small-input scans (roughly a second of
/// interpreter work) always succeed; the per-byte term below scales the rest.
pub const FUEL_BASE: u64 = 50_000_000;

/// Fuel charged per byte of serialized input on top of [`FUEL_BASE`].
///
/// High enough that honest linear work (scanning a big document) never trips
/// the budget; low enough that a runaway loop dies quickly instead of
/// hanging the lint run.
pub const FUEL_PER_BYTE: u64 = 1_000;

/// Upper bound on findings a single plugin may emit per document. A plugin
/// past this limit is treated as a protocol violation (a lint rule that
/// matches everything is a bug, not a feature).
pub const MAX_FINDINGS: usize = 1_000;

/// Hard cap on a plugin instance's linear memory (host-owned, not
/// configurable). Plugins use a bump allocator with no `dealloc` export, so
/// this limit is what bounds runaway guest allocation.
pub const MAX_MEMORY_BYTES: u64 = 16 * 1024 * 1024;

/// The seam between deslop and plugin implementations.
///
/// One instance per loaded plugin, reused across every scanned document:
/// `scan` is stateless, so only the fuel budget varies per call.
pub trait LintPlugin: fmt::Debug + Send + Sync {
    /// Static identity declared by the plugin itself (id, tier, category,
    /// ABI version). Validated at load time, then trusted hereafter.
    fn meta(&self) -> &PluginManifest;

    /// Analyze one document.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError`] when the plugin fails (trap, fuel exhaustion,
    /// protocol violation). Callers must treat a failure as "no findings
    /// from this plugin for this document" plus a warning — never as a fatal
    /// scan error.
    fn scan(&self, input: &PluginInput) -> Result<Vec<PluginFinding>, PluginError>;
}

/// Failures a plugin can produce, each of which means "skip this plugin's
/// findings and warn" — never a fatal error.
#[derive(Debug, Clone, wherror::Error)]
#[error(debug)]
pub enum PluginError {
    #[error("plugin {id}: {detail}")]
    Load { id: String, detail: String },
    #[error("plugin {id}: trapped: {detail}")]
    Trap { id: String, detail: String },
    #[error("plugin {id}: exhausted its fuel budget")]
    Fuel { id: String },
    #[error("plugin {id}: protocol violation: {detail}")]
    Protocol { id: String, detail: String },
}

/// Host-owned knobs for one plugin, from `[plugins.<id>.runtime]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginRuntime {
    /// Explicit fuel budget replacing the size-scaled default.
    pub fuel: Option<u64>,
}

/// Resolved `[plugins]` configuration.
///
/// `params` and `runtime` are keyed by the plugin id in UPPER-CASE so that
/// config table names match manifests case-insensitively
/// (`[plugins.exclaim]` configures the `EXCLAIM` plugin).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PluginConfig {
    /// Absolute paths of plugin files to load.
    pub paths: Vec<camino::Utf8PathBuf>,
    /// Opaque `[plugins.<id>]` tables, passed to plugins verbatim as
    /// [`PluginInput::config`]. The host never interprets them.
    pub params: BTreeMap<String, serde_json::Value>,
    /// Host knobs per plugin id (`[plugins.<id>.runtime]`).
    pub runtime: BTreeMap<String, PluginRuntime>,
}

/// Fuel budget for one plugin call: the explicit override when set,
/// otherwise the size-scaled default (baseline + per-byte of input).
pub fn fuel_for(runtime: Option<&PluginRuntime>, input_len: usize) -> u64 {
    match runtime.and_then(|rt| rt.fuel) {
        Some(fuel) => fuel,
        None => FUEL_BASE + FUEL_PER_BYTE * input_len as u64,
    }
}

/// Validate a manifest at load time. `Err` carries the human-readable detail.
pub fn validate_manifest(manifest: &PluginManifest) -> Result<(), String> {
    if manifest.abi != PROTOCOL_ABI {
        return Err(format!(
            "manifest abi {} does not match host abi {PROTOCOL_ABI}",
            manifest.abi
        ));
    }
    if crate::finding::Tier::from_number(manifest.tier).is_none() {
        return Err(format!("manifest tier {} is not in 1..=3", manifest.tier));
    }
    id_problems(&manifest.id)?;
    Ok(())
}

/// Check a plugin/finding identifier: non-empty, no `'#'` (which would break
/// the `ID#slug` entry-id convention), no whitespace (breaks config keys).
fn id_problems(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("id is empty".into());
    }
    if id.contains('#') {
        return Err("id contains '#'".into());
    }
    if id.chars().any(char::is_whitespace) {
        return Err("id contains whitespace".into());
    }
    Ok(())
}

/// Check a finding slug the same way; slugs become the `#slug` half of an
/// entry id, so they share the id character rules.
pub fn validate_finding_slug(slug: &str) -> Result<(), String> {
    id_problems(slug)
}

/// Load every plugin file declared in `cfg`.
///
/// Warnings-not-failures policy: a plugin that fails to load is skipped with
/// a returned warning (rendered on stderr by the CLI) and never aborts the
/// run. A duplicate id keeps the first plugin and warns about the rest.
///
/// # Errors
///
/// Never returns `Err`; all load problems surface as warnings.
pub fn load_plugins(cfg: &PluginConfig) -> (Vec<Box<dyn LintPlugin>>, Vec<String>) {
    let mut loaded: Vec<Box<dyn LintPlugin>> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for path in &cfg.paths {
        match wasmi_host::instantiate(path) {
            Ok(mut plugin) => {
                let id = plugin.meta().id.clone();
                let key = id.to_uppercase();
                if let Some(runtime) = cfg.runtime.get(&key) {
                    plugin.set_fuel_override(runtime.fuel);
                }
                if seen.insert(key) {
                    loaded.push(Box::new(plugin));
                } else {
                    warnings.push(format!(
                        "deslop: duplicate plugin id {id} at {path}; keeping the first"
                    ));
                }
            }
            Err(error) => {
                warnings.push(format!("deslop: skipping plugin from {path}: {error}"));
            }
        }
    }

    (loaded, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(id: &str, tier: u8, abi: u32) -> PluginManifest {
        PluginManifest {
            id: id.into(),
            tier,
            category: "test".into(),
            abi,
        }
    }

    #[test]
    fn validate_manifest_accepts_current_protocol() {
        // Given a manifest matching the host's ABI and a legal tier.
        // When validating.
        // Then no problems are reported.
        assert_eq!(validate_manifest(&manifest("EXCLAIM", 3, PROTOCOL_ABI)), Ok(()));
    }

    #[test]
    fn validate_manifest_rejects_wrong_abi() {
        // Given a manifest built against a future ABI.
        // When validating.
        // Then the mismatch is named.
        assert!(
            validate_manifest(&manifest("X", 1, PROTOCOL_ABI + 1))
                .expect_err("must reject")
                .contains("abi")
        );
    }

    #[test]
    fn validate_manifest_rejects_out_of_range_tier() {
        // Given a manifest with tier 9.
        // When validating.
        // Then the tier is rejected.
        assert!(validate_manifest(&manifest("X", 9, PROTOCOL_ABI)).is_err());
    }

    #[test]
    fn validate_manifest_rejects_empty_id() {
        // Given a manifest with an empty id.
        // When validating.
        // Then the id is rejected.
        assert!(validate_manifest(&manifest("", 1, PROTOCOL_ABI)).is_err());
    }

    #[test]
    fn manifest_id_rejects_hash_and_whitespace() {
        // Given ids that would break entry-id or config-key syntax.
        // When validating each.
        // Then both are rejected.
        assert!(validate_manifest(&manifest("A#B", 1, PROTOCOL_ABI)).is_err());
        assert!(validate_manifest(&manifest("A B", 1, PROTOCOL_ABI)).is_err());
    }

    #[test]
    fn finding_slug_follows_id_rules() {
        // Given a legal and an illegal slug.
        // When validating.
        // Then only the legal one passes.
        assert_eq!(validate_finding_slug("exclamania"), Ok(()));
        assert!(validate_finding_slug("").is_err());
        assert!(validate_finding_slug("a#b").is_err());
    }

    #[test]
    fn fuel_for_defaults_to_size_scaled_budget() {
        // Given no runtime override and a 200-byte input.
        // When computing fuel.
        // Then the baseline plus per-byte charge applies.
        assert_eq!(fuel_for(None, 200), FUEL_BASE + 200 * FUEL_PER_BYTE);
    }

    #[test]
    fn fuel_for_override_replaces_the_formula() {
        // Given an explicit fuel override.
        // When computing fuel.
        // Then the override is used verbatim, not added to the formula.
        let rt = PluginRuntime {
            fuel: Some(1234),
        };
        assert_eq!(fuel_for(Some(&rt), 10_000), 1234);
    }

    #[test]
    fn load_plugins_skips_missing_files_with_a_warning() {
        // Given config pointing at a file that does not exist.
        let cfg = PluginConfig {
            paths: vec!["/nonexistent/plugin.wasm".into()],
            ..PluginConfig::default()
        };

        // When loading.
        let (plugins, warnings) = load_plugins(&cfg);

        // Then no plugins load, a warning names the path, and the run is not aborted.
        assert!(plugins.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("/nonexistent/plugin.wasm"));
        assert!(warnings[0].starts_with("deslop: skipping plugin"));
    }

    #[test]
    fn load_plugins_with_no_paths_loads_nothing_and_warns_nothing() {
        // Given empty plugin config.
        // When loading.
        // Then the result is empty and silent.
        let (plugins, warnings) = load_plugins(&PluginConfig::default());
        assert!(plugins.is_empty());
        assert!(warnings.is_empty());
    }
}
