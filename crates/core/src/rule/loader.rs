//! Pack loading: discovery -> strict parse -> validation -> [`RuleSet`].
//!
//! All errors accumulate (never first-fail) so one bad rule file reports
//! every problem at once, rendered later via codespan with file:line.

use crate::config::Config;
use crate::finding::Tier;
use crate::rule::RuleSet;
use crate::rule::schema::{EntryToml, GroupToml, RulesFileToml};
use camino::Utf8Path;

/// One validation failure, tied to the file (and where possible, line).
#[derive(Debug, Clone)]
pub struct LoadError {
    pub path: String,
    pub line: Option<usize>,
    pub message: String,
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(l) => write!(f, "{}:{l}: {}", self.path, self.message),
            None => write!(f, "{}: {}", self.path, self.message),
        }
    }
}

/// Outcome of a load attempt: rules plus every error found.
#[derive(Debug, Default)]
pub struct Loaded {
    pub rule_set: RuleSet,
    pub errors: Vec<LoadError>,
    /// `dedup:` diagnostics from the compile stage (single-owner collisions).
    pub dedup_warnings: Vec<String>,
}

/// Validate one parsed group; appends to `errors` rather than early-returning.
fn validate_group(path: &str, group: &GroupToml, errors: &mut Vec<LoadError>) {
    let mut push = |line: Option<usize>, message: String| {
        errors.push(LoadError {
            path: path.to_owned(),
            line,
            message,
        });
    };

    // Tier must be a real tier.
    if Tier::from_number(group.tier).is_none() {
        push(None, format!("tier {} is not 1, 2 or 3", group.tier));
    }

    // Known kind?
    if !matches!(
        group.kind.as_str(),
        "vocab" | "pattern" | "literal-ban" | "metric"
    ) {
        push(None, format!("unknown kind {:?}", group.kind));
    }

    match group.kind.as_str() {
        "metric" => {
            if group.stat.is_none() {
                push(None, "metric rule requires `stat`".into());
            } else if crate::metric_stats::Stat::from_name(
                group.stat.as_deref().unwrap_or_default(),
            )
            .is_none()
            {
                push(None, format!("unknown stat {:?}", group.stat));
            }
            if group.threshold_gt.is_none() {
                push(None, "metric rule requires `threshold_gt`".into());
            }
            if !group.entries.is_empty() {
                push(None, "metric rules must not carry [[entries]]".into());
            }
            let is_cluster = group.stat.as_deref() == Some("term_cluster_max");
            if is_cluster {
                if group.terms.as_ref().is_none_or(Vec::is_empty) {
                    push(None, "term_cluster_max requires `terms`".into());
                }
                if let Some(w) = &group.window {
                    if crate::rule::ClusterWindow::parse(w).is_none() {
                        push(None, format!("unknown window {w:?}"));
                    }
                }
            } else if group.window.is_some() || group.terms.is_some() {
                push(
                    None,
                    "`window`/`terms` only apply to term_cluster_max".into(),
                );
            }
        }
        _ => {
            if group.entries.is_empty() {
                push(
                    None,
                    "kind {:?} requires at least one [[entries]] block".into(),
                );
            }
        }
    }

    // Per-entry checks: slugs present and unique within the file.
    let mut seen = std::collections::BTreeSet::new();
    for (idx, entry) in group.entries.iter().enumerate() {
        let slug = entry.slug.clone().or_else(|| entry.id.clone());
        let slug = match slug {
            Some(s) if !s.is_empty() => s,
            _ => {
                push(None, format!("entries[{idx}] needs a non-empty `slug`"));
                continue;
            }
        };
        if !seen.insert(slug.clone()) {
            push(None, format!("duplicate entry slug `{slug}`"));
        }
        match group.kind.as_str() {
            "vocab" => {
                if entry.terms.is_empty() && entry.regex.is_none() {
                    push(None, format!("entry `{slug}`: vocab entries need `terms`"));
                }
                if entry.replacement.is_some() && entry.terms.len() != 1 {
                    // Replacement semantics require a single base term so
                    // generated inflections rewrite consistently.
                    push(
                        None,
                        format!("entry `{slug}`: `replacement` requires exactly one term"),
                    );
                }
            }
            "pattern" if entry.regex.is_none() => {
                push(
                    None,
                    format!("entry `{slug}`: pattern entries need `regex`"),
                );
            }
            "literal-ban" if entry.terms.is_empty() => {
                push(
                    None,
                    format!("entry `{slug}`: literal-ban entries need `terms`"),
                );
            }
            _ => {}
        }
    }

    // Fixtures are mandatory for every group except metrics (aggregate-only,
    // no per-instance matching exists).
    if group.kind == "metric" {
        return;
    }
    // Template validation: placeholders must fit the kind's grammar.
    for entry in &group.entries {
        let slug = entry
            .slug
            .clone()
            .or_else(|| entry.id.clone())
            .unwrap_or_default();
        let allowed: Vec<String> = if group.kind == "pattern" {
            entry
                .regex
                .as_deref()
                .map(str::trim)
                .and_then(|src| regex::Regex::new(src).ok())
                .map(|re| re.capture_names().flatten().map(String::from).collect())
                .unwrap_or_default()
        } else {
            group_allowed_for(&group.kind)
                .iter()
                .map(|s| (*s).to_owned())
                .collect()
        };
        let allowed: Vec<&str> = allowed.iter().map(String::as_str).collect();

        // Pattern entries must compile (policy + engine) to load.
        if group.kind == "pattern" {
            if let Some(src) = &entry.regex {
                let trimmed = src.trim();
                if let Err(violation) = crate::rule::policy::check(trimmed) {
                    push(None, format!("entry `{slug}` regex policy: {violation}"));
                }
                if let Err(e) = regex::Regex::new(trimmed) {
                    push(None, format!("entry `{slug}` invalid regex: {e}"));
                }
            }
        }
        for (field, template) in [("advice", &entry.advice), ("message", &entry.message)] {
            if let Some(text) = template {
                if let Err(e) = crate::rule::template::validate(text, &allowed) {
                    push(None, format!("entry `{slug}` {field} template: {e}"));
                }
            }
        }
    }

    // Fixture gate: every entry must prove hit/miss behavior.
    for failure in crate::rule::fixtures::evaluate(group) {
        push(
            None,
            format!(
                "entry `{}` fixture failure: {} - sample: {:?}",
                failure.slug, failure.problem, failure.fixture
            ),
        );
    }
}

/// Load packs declared in `cfg`.
///
/// Builtin pack stems resolve to `<rules_root>/rules/<stem>.toml` - one
/// flat file per pack, any number of `[[group]]` tables inside. `extra_paths`
/// name files or directories, used as-is.
///
/// Never panics on bad data: problems land in [`Loaded::errors`].
pub fn load(cfg: &Config, rules_root: &Utf8Path) -> Loaded {
    let mut loaded = Loaded::default();

    let mut pack_files = Vec::new();
    for name in &cfg.packs.builtin {
        pack_files.push(rules_root.join("rules").join(format!("{name}.toml")));
    }
    for extra in &cfg.packs.extra_paths {
        let path = rules_root.join(extra);
        if path.is_dir() {
            pack_files.extend(crate::sorted_toml_files(&path));
        } else {
            pack_files.push(path);
        }
    }

    // GROUP id-base uniqueness spans the whole effective ruleset; entry ids
    // too. Entry SLUGS stay group-scoped (two groups may both have `main`).
    let mut seen_ids = std::collections::BTreeMap::new();
    let mut entry_ids = std::collections::BTreeMap::new();

    for file in pack_files {
        let text = match std::fs::read_to_string(&file) {
            Ok(t) => t,
            Err(e) => {
                loaded.errors.push(LoadError {
                    path: file.to_string(),
                    line: None,
                    message: format!("unreadable rule pack: {e}"),
                });
                continue;
            }
        };
        parse_pack_file(&file, &text, &mut seen_ids, &mut entry_ids, &mut loaded);
    }
    // Compile stage: one owner per term (highest tier, then config order),
    // metric conflicts resolved to the strictest threshold.
    loaded.dedup_warnings = crate::rule::dedup::dedup(&mut loaded.rule_set);
    // Deterministic listing order: by id-base then file-derived insertion.
    loaded
        .rule_set
        .groups
        .sort_by(|a, b| a.id_base.cmp(&b.id_base));
    loaded
}

/// Expansion result: surface forms plus the lemma each belongs to.
pub struct MetricTerms {
    pub terms: Vec<String>,
    pub term_lemmas: Vec<u32>,
}

/// Expand metric terms to surface forms tagged with their lemma index:
/// each listed term is one lemma, so `delve` + `delves` count as ONE
/// distinct term in cluster scoring regardless of inflection.
fn expand_metric_terms(terms: &Option<Vec<String>>) -> (Vec<String>, Vec<u32>) {
    let mut forms: Vec<(String, u32)> = terms
        .clone()
        .unwrap_or_default()
        .iter()
        .enumerate()
        .flat_map(|(i, t)| {
            crate::rule::stems::expand(t.trim())
                .into_iter()
                .map(move |form| (form, i as u32))
        })
        .collect();
    forms.sort();
    forms.dedup();
    let lemmas = forms.iter().map(|(_, l)| *l).collect();
    let surface = forms.into_iter().map(|(t, _)| t).collect();
    (surface, lemmas)
}

fn parse_pack_file(
    path: &Utf8Path,
    text: &str,
    seen_ids: &mut std::collections::BTreeMap<String, String>,
    seen_entry_ids: &mut std::collections::BTreeMap<String, ()>,
    loaded: &mut Loaded,
) {
    let parsed: Result<RulesFileToml, toml::de::Error> = toml::from_str(text);

    let file = match parsed {
        Ok(f) => f,
        Err(e) => {
            loaded.errors.push(LoadError {
                path: path.to_string(),
                line: e.span().map(|s| line_of(text, s.start)),
                message: format!("invalid rule TOML: {e}"),
            });
            return;
        }
    };

    for group in &file.groups {
        parse_group(path, group, seen_ids, seen_entry_ids, loaded);
    }
}

fn parse_group(
    path: &Utf8Path,
    group: &GroupToml,
    seen_ids: &mut std::collections::BTreeMap<String, String>,
    seen_entry_ids: &mut std::collections::BTreeMap<String, ()>,
    loaded: &mut Loaded,
) {
    let active_entry_counter = 0usize;

    let err_before = loaded.errors.len();
    validate_group(path.as_str(), group, &mut loaded.errors);
    let group_valid = loaded.errors.len() == err_before;

    // Group id-base is GLOBAL: two files both defining the same id_base is a
    // conflict regardless of pack.
    match seen_ids.get(&group.id_base) {
        Some(first) => {
            loaded.errors.push(LoadError {
                path: path.to_string(),
                line: None,
                message: format!(
                    "group id-base `{}` already defined in {first}",
                    group.id_base
                ),
            });
        }
        None => {
            seen_ids.insert(group.id_base.clone(), path.to_string());
        }
    }

    let group_enabled = group.enabled.unwrap_or(true);

    // Build ACTIVE entries with compiled matchers; compile failures were
    // recorded above, so failure here just skips that entry silently.
    let mut active_entries = Vec::new();
    for entry in &group.entries {
        let slug = entry_slug(entry, group, active_entry_counter);
        let id = format!("{}#{slug}", group.id_base);
        if seen_entry_ids.insert(id.clone(), ()).is_some() {
            push_error(loaded, path, None, format!("duplicate entry id `{id}`"));
            continue;
        }
        if !group_enabled || !entry_enabled(entry) {
            continue;
        }
        if let Ok(matcher) = crate::rule::fixtures::Matcher::build(&group.kind, entry) {
            active_entries.push(crate::rule::ActiveEntry {
                id,
                message_override: entry.message.clone(),
                advice_override: entry.advice.clone(),
                category_override: entry.category.clone(),
                matcher,
                replacement: entry.replacement.clone(),
            });
        }
    }

    let has_group_level_rule = group.kind == "metric";
    if active_entries.is_empty() && !has_group_level_rule {
        return;
    }
    // Group-scoped error accounting: was THIS group valid? (loaded.errors is
    // global across the run, so this is the snapshot taken at validation.)
    let group_was_valid = group_valid;
    loaded.rule_set.groups.push(crate::rule::RuleGroup {
        id_base: group.id_base.clone(),
        tier: group.tier,
        kind: group.kind.clone(),
        category: group.category.clone(),
        message: group.message.clone(),
        advice: group.advice.clone(),
        enabled: group_enabled,
        scope: group
            .scope
            .clone()
            .unwrap_or_else(|| default_scope(&group.kind)),
        url: group.url.as_ref().map(|u| (u.text.clone(), u.href.clone())),
        entries: active_entries,
        // Only materialize when THIS group validated; broken groups already
        // recorded errors and abort the run before scanning.
        metric: if group.kind == "metric" && group_was_valid {
            crate::metric_stats::Stat::from_name(group.stat.as_deref().unwrap_or_default()).map(
                |stat| {
                    let (terms, term_lemmas) = expand_metric_terms(&group.terms);
                    crate::rule::MetricSpec {
                        stat,
                        per_words: group.per_words.unwrap_or(1000),
                        threshold_gt: group.threshold_gt.unwrap_or(0.0),
                        window: group
                            .window
                            .as_deref()
                            .and_then(crate::rule::ClusterWindow::parse)
                            .unwrap_or(crate::rule::ClusterWindow::Paragraph),
                        terms,
                        term_lemmas,
                    }
                },
            )
        } else {
            None
        },
    });
}

/// Byte offset -> 1-based line number.
fn line_of(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())]
        .bytes()
        .filter(|&b| b == b'\n')
        .count()
        + 1
}

/// Allowed template placeholders for a non-pattern kind.
fn group_allowed_for(kind: &str) -> &'static [&'static str] {
    match kind {
        "vocab" => &["match"],
        "literal-ban" => &["match"],
        "metric" => &["value", "per_words"],
        _ => &[],
    }
}

fn entry_slug(entry: &EntryToml, group: &GroupToml, counter: usize) -> String {
    entry
        .slug
        .clone()
        .or_else(|| entry.id.clone())
        .unwrap_or_else(|| format!("{}-e{}", group.id_base.to_lowercase(), counter))
}

fn entry_enabled(entry: &EntryToml) -> bool {
    let _ = entry; // v1: entries are enabled unless the group is disabled
    true
}

fn default_scope(kind: &str) -> String {
    match kind {
        "literal-ban" => "anywhere".into(),
        _ => "prose".into(),
    }
}

pub(crate) fn push_error(
    loaded: &mut Loaded,
    path: &Utf8Path,
    line: Option<usize>,
    message: String,
) {
    loaded.errors.push(LoadError {
        path: path.to_string(),
        line,
        message,
    });
}
