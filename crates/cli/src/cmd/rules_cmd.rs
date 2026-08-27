//! `deslop rules`: list the effective merged ruleset.

/// One flattened entry for display.
pub struct RuleRow {
    pub id: String,
    pub tier: u8,
    pub kind: String,
    pub category: String,
    pub enabled: bool,
    pub has_advice: bool,
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
        "{:<idw$}  {:<4}  {:<12}  {:<catw$}  {:<8}  advice\n",
        "ID",
        "tier",
        "kind",
        "category",
        "enabled",
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
            if r.enabled { "yes" } else { "no" },
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
}

impl RulesCmd<'_> {
    /// # Errors
    ///
    /// Fails when packs cannot be located on disk.
    pub fn run(&mut self) -> Result<i32, ()> {
        let loaded = load_rules(self.cfg);
        if !loaded.errors.is_empty() {
            for err in &loaded.errors {
                eprintln!("deslop: {err}");
            }
            return Err(());
        }

        let rows = flatten(&loaded.rule_set);
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
pub fn load_for_lint(cfg: &deslop_core::config::Config) -> deslop_core::rule::loader::Loaded {
    load_rules(cfg)
}

fn load_rules(cfg: &deslop_core::config::Config) -> deslop_core::rule::loader::Loaded {
    // Builtin names resolve under ./rules; extra_paths are absolute or
    // cwd-relative already.
    let loaded = deslop_core::rule::loader::load(cfg, camino::Utf8Path::new("."));
    if std::env::var_os("DESLOP_DEBUG_LOAD").is_some() {
        eprintln!(
            "debug: errors={:?} groups={}",
            loaded.errors,
            loaded.rule_set.groups.len()
        );
    }
    loaded
}

fn flatten(rule_set: &deslop_core::rule::RuleSet) -> Vec<RuleRow> {
    let mut rows = Vec::new();
    for group in &rule_set.groups {
        // Metric rules live at group level (no entries).
        if group.entries.is_empty() {
            rows.push(RuleRow {
                id: group.id_base.clone(),
                tier: group.tier,
                kind: group.kind.clone(),
                category: group.category.clone(),
                enabled: group.enabled,
                has_advice: group.advice.is_some(),
            });
            continue;
        }
        for entry in &group.entries {
            rows.push(RuleRow {
                id: entry.id.clone(),
                tier: group.tier,
                kind: group.kind.clone(),
                category: group.category.clone(),
                enabled: group.enabled,
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
            "  {{\"id\":{},\"tier\":{},\"kind\":{},\"enabled\":{}}}{}",
            serde_json::to_string(&r.id).expect("str"),
            r.tier,
            serde_json::to_string(&r.kind).expect("str"),
            r.enabled,
            comma
        );
    }
    let _ = writeln!(out, "]}}");
}
