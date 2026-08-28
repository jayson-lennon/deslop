//! `deslop rules`: list the effective merged ruleset.

/// One flattened entry for display.
pub struct RuleRow {
    pub id: String,
    pub tier: u8,
    pub kind: String,
    pub category: String,
    /// Effective clippy-style level after `[lints]` overrides.
    pub level: &'static str,
    pub has_advice: bool,
}

/// One row per loaded plugin: id, tier, kind, category, effective level.
///
/// Loaded separately from packs (plugins warn-and-skip on failure), so a
/// broken plugin file shows zero rows here rather than failing the listing.
fn plugin_rows(cfg: &deslop_core::config::Config) -> Vec<RuleRow> {
    let settings = deslop_core::scanner::LintSettings {
        max_tier: None,
        levels: cfg.lint.clone(),
    };
    let (plugins, warnings) = deslop_core::plugin::load_plugins(&cfg.plugins);
    for warning in &warnings {
        eprintln!("{warning}");
    }
    let mut rows = Vec::new();
    for plugin in &plugins {
        let meta = plugin.meta();
        let level = settings
            .level_for(&meta.id, "")
            .map(|l| l.name())
            .unwrap_or_else(|| tier_level(meta.tier));
        rows.push(RuleRow {
            id: meta.id.clone(),
            tier: meta.tier,
            kind: "plugin".into(),
            category: meta.category.clone(),
            level,
            has_advice: false,
        });
    }
    rows
}

/// Tier number -> default level name.
fn tier_level(tier: u8) -> &'static str {
    match tier {
        1 => "error",
        2 => "warn",
        _ => "note",
    }
}

/// Render rows as an aligned text table.
pub fn render_table(rows: &[RuleRow]) -> String {
    let id_w = rows.iter().map(|r| r.id.len()).max().unwrap_or(2).max(2);
    let cat_w = rows
        .iter()
        .map(|r| r.category.len())
        .max()
        .unwrap_or(8)
        .max(8);
    let mut out = String::new();
    out.push_str(&format!(
        "{:<idw$}  {:<4}  {:<12}  {:<catw$}  {:<5}  advice\n",
        "ID",
        "tier",
        "kind",
        "category",
        "level",
        idw = id_w,
        catw = cat_w
    ));
    for r in rows {
        out.push_str(&format!(
            "{:<idw$}  {:<4}  {:<12}  {:<catw$}  {:<8}  {}\n",
            r.id,
            r.tier,
            r.kind,
            r.category,
            r.level,
            if r.has_advice { "yes" } else { "-" },
            idw = id_w,
            catw = cat_w
        ));
    }
    out
}

use std::io::Write as _;

/// Context for the listing run.
pub struct RulesCmd<'a> {
    pub cfg: &'a deslop_core::config::Config,
    pub json: bool,
    /// `--rules-dir` override: the directory containing pack TOMLs.
    pub rules_dir: Option<camino::Utf8PathBuf>,
}

impl RulesCmd<'_> {
    /// # Errors
    ///
    /// Fails when packs cannot be located on disk.
    pub fn run(&mut self) -> Result<i32, error_stack::Report<super::CmdError>> {
        let loaded = load_rules(self.cfg, self.rules_dir.clone());
        if !loaded.errors.is_empty() {
            for err in &loaded.errors {
                eprintln!("deslop: {err}");
            }
            return Err(super::fail("rules listing failed"));
        }

        let rows = {
            let mut rows = flatten(&loaded.rule_set, self.cfg);
            rows.extend(plugin_rows(self.cfg));
            rows
        };
        let stdout = std::io::stdout();
        let mut out = stdout.lock();
        if self.json {
            render_json(&rows, &mut out);
        } else {
            let _ = write!(out, "{}", render_table(&rows));
        }
        Ok(0)
    }
}

/// Loader access for other commands; pub(crate) within the binary.
pub fn load_for_lint(
    cfg: &deslop_core::config::Config,
    rules_dir: Option<camino::Utf8PathBuf>,
) -> deslop_core::rule::loader::Loaded {
    load_rules(cfg, rules_dir)
}

/// Where the builtin packs live, resolved once per run:
/// 0. `--rules-dir DIR` when given - DIR itself is the pack directory and
///    the chain below is skipped (hermetic runs: CI, integration tests),
/// 1. `~/.config/deslop/rules` (via the `dirs` crate) when it exists -
///    user-installed packs,
/// 2. `./rules` when present (repo development layout, incl. tests),
/// 3. alongside the executable (`<exe_dir>/rules` - installed layout),
/// 4. Cargo target fallback (`target/debug` ancestor with a `rules/`),
/// 5. `.` as the last resort.
///
/// Returns `(packs_dir, extras_root)`: where `<stem>.toml` packs live and
/// where config `extra_paths` anchor, respectively.
fn rules_root(
    override_dir: Option<&camino::Utf8Path>,
) -> (camino::Utf8PathBuf, camino::Utf8PathBuf) {
    if let Some(dir) = override_dir {
        // The flag IS the pack directory; extras anchor at its parent so a
        // config's relative extra_paths keep working from the project root.
        let extras = dir
            .parent()
            .map(camino::Utf8Path::to_path_buf)
            .unwrap_or_else(|| camino::Utf8PathBuf::from("."));
        return (dir.to_path_buf(), extras);
    }
    if let Some(config_dir) = dirs::config_dir() {
        let user_rules = config_dir.join("deslop").join("rules");
        if user_rules.is_dir() {
            // The user dir IS a rules root: its packs sit directly in it
            // (<user_rules>/<stem>.toml), so hand back its PARENT with the
            // same "root + rules/" join the loader performs everywhere else.
            if let Some(root) = user_rules
                .parent()
                .and_then(|p| camino::Utf8PathBuf::from_path_buf(p.to_path_buf()).ok())
            {
                let packs = root.join("rules");
                return (packs, root);
            }
        }
    }
    if camino::Utf8Path::new("rules").is_dir() {
        return (
            camino::Utf8PathBuf::from("rules"),
            camino::Utf8PathBuf::from("."),
        );
    }
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf));
    if let Some(dir) = exe_dir {
        if let Ok(exe) = camino::Utf8PathBuf::from_path_buf(dir.clone()) {
            if exe.join("rules").is_dir() {
                return (exe.join("rules"), exe);
            }
        }
        // Cargo target layout: target/debug -> crate root two levels up.
        for ancestor in dir.ancestors().skip(1) {
            let candidate = ancestor.join("rules");
            if candidate.is_dir() {
                if let Ok(root) = camino::Utf8PathBuf::from_path_buf(ancestor.to_path_buf()) {
                    return (root.join("rules"), root);
                }
            }
        }
    }
    (
        camino::Utf8PathBuf::from("rules"),
        camino::Utf8PathBuf::from("."),
    )
}

fn load_rules(
    cfg: &deslop_core::config::Config,
    rules_dir: Option<camino::Utf8PathBuf>,
) -> deslop_core::rule::loader::Loaded {
    let (packs_dir, extras_root) = rules_root(rules_dir.as_deref());
    let loaded =
        deslop_core::rule::loader::load_split(cfg, packs_dir.as_path(), extras_root.as_path());
    if std::env::var_os("DESLOP_DEBUG_LOAD").is_some() {
        eprintln!(
            "debug: errors={:?} groups={}",
            loaded.errors,
            loaded.rule_set.groups.len()
        );
    }
    loaded
}

fn flatten(
    rule_set: &deslop_core::rule::RuleSet,
    cfg: &deslop_core::config::Config,
) -> Vec<RuleRow> {
    let settings = deslop_core::scanner::LintSettings {
        max_tier: None,
        levels: cfg.lint.clone(),
    };
    let mut rows = Vec::new();
    for group in &rule_set.groups {
        // Metric rules live at group level (no entries).
        if group.entries.is_empty() {
            let level = if group.enabled {
                settings
                    .level_for(&group.id_base, &group.id_base)
                    .map(|l| l.name())
                    .unwrap_or_else(|| tier_level(group.tier))
            } else {
                "allow"
            };
            rows.push(RuleRow {
                id: group.id_base.clone(),
                tier: group.tier,
                kind: group.kind.clone(),
                category: group.category.clone(),
                level,
                has_advice: group.advice.is_some(),
            });
            continue;
        }
        for entry in &group.entries {
            let level = if group.enabled {
                settings
                    .level_for(&group.id_base, &entry.id)
                    .map(|l| l.name())
                    .unwrap_or_else(|| tier_level(group.tier))
            } else {
                "allow"
            };
            rows.push(RuleRow {
                id: entry.id.clone(),
                tier: group.tier,
                kind: group.kind.clone(),
                category: group.category.clone(),
                level,
                has_advice: entry.advice_override.is_some() || group.advice.is_some(),
            });
        }
    }
    rows
}

fn render_json(rows: &[RuleRow], out: &mut impl std::io::Write) {
    let _ = writeln!(out, "{{\"rules\":[");
    for (idx, r) in rows.iter().enumerate() {
        let comma = if idx + 1 < rows.len() { "," } else { "" };
        let _ = writeln!(
            out,
            "  {{\"id\":{},\"tier\":{},\"kind\":{},\"level\":{}}}{}",
            serde_json::to_string(&r.id).expect("str"),
            r.tier,
            serde_json::to_string(&r.kind).expect("str"),
            serde_json::to_string(r.level).expect("str"),
            comma
        );
    }
    let _ = writeln!(out, "]}}");
}
