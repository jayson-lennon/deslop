//! Pack loading: discovery -> strict parse -> validation -> [`RuleSet`].
//!
//! All errors accumulate (never first-fail) so one bad rule file reports
//! every problem at once, rendered later via codespan with file:line.
use crate::config::Config;
use crate::finding::Tier;
use crate::rule::schema::{EntryToml, GroupToml};

pub mod fixtures;
pub mod literals;
pub mod loader;
pub mod notice;
pub mod policy;
pub mod schema;
pub mod stems;
pub mod template;

use camino::Utf8Path;

/// A rule file = one group: shared envelope + `[[entries]]`.
#[derive(Debug, Clone)]
pub struct RuleGroup {
    pub id_base: String,
    /// 1=artifact(error) 2=tell(warning) 3=density(hint).
    pub tier: u8,
    /// vocab | pattern | literal-ban | metric.
    pub kind: String,
    pub category: String,
    pub message: Option<String>,
    pub advice: Option<String>,
    pub enabled: bool,
    /// prose | heading | list-item | anywhere (kind defaults applied).
    pub scope: String,
    pub url: Option<(String, String)>,
    pub entries: Vec<ActiveEntry>,
    /// metric-only fields (kind == "metric").
    pub metric: Option<MetricSpec>,
}

/// Threshold spec for a document-level metric rule.
#[derive(Debug, Clone)]
pub struct MetricSpec {
    pub stat: crate::metric_stats::Stat,
    pub per_words: u32,
    pub threshold_gt: f64,
    /// term_cluster_max: granularity of the counted window.
    pub window: ClusterWindow,
    /// term_cluster_max: distinct terms counted per window (lowercased).
    pub terms: Vec<String>,
}

/// Window granularity for cluster stats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClusterWindow {
    Paragraph,
    Sentence,
    Document,
}

impl ClusterWindow {
    pub fn parse(name: &str) -> Option<ClusterWindow> {
        Some(match name {
            "paragraph" => ClusterWindow::Paragraph,
            "sentence" => ClusterWindow::Sentence,
            "document" => ClusterWindow::Document,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            ClusterWindow::Paragraph => "paragraph",
            ClusterWindow::Sentence => "sentence",
            ClusterWindow::Document => "document",
        }
    }
}

/// One scannable entry with its compiled matcher.
#[derive(Debug, Clone)]
pub struct ActiveEntry {
    /// Globally unique "GROUP#slug".
    pub id: String,
    pub message_override: Option<String>,
    pub advice_override: Option<String>,
    pub matcher: fixtures::Matcher,
    /// vocab only: mechanical rewrite when present.
    pub replacement: Option<String>,
}

/// Everything loaded and validated at startup; consumed by the scanner.
#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    pub groups: Vec<RuleGroup>,
}

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
                .and_then(|src| fancy_regex::Regex::new(src).ok())
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
                if let Err(violation) = crate::rule::policy::check(src) {
                    push(None, format!("entry `{slug}` regex policy: {violation}"));
                }
                if let Err(e) = fancy_regex::Regex::new(src) {
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
                "entry `{}` fixture failure: {} — sample: {:?}",
                failure.slug, failure.problem, failure.fixture
            ),
        );
    }
}

/// Load packs declared in `cfg`.
///
/// Builtin names resolve under `<rules_root>/rules/builtin/<name>`;
/// `extra_paths` are used as-is.
///
/// Never panics on bad data: problems land in [`Loaded::errors`].
pub fn load(cfg: &Config, rules_root: &Utf8Path) -> Loaded {
    let mut loaded = Loaded::default();

    let mut pack_dirs = Vec::new();
    for name in &cfg.packs.builtin {
        pack_dirs.push(rules_root.join("rules").join("builtin").join(name));
    }
    for extra in &cfg.packs.extra_paths {
        pack_dirs.push(extra.clone());
    }

    // Entry-id uniqueness spans the whole effective ruleset, not one file.
    let mut seen_ids = std::collections::BTreeMap::new();

    for dir in pack_dirs {
        let notice = load_notice(&dir);
        for file in crate::sorted_toml_files(&dir) {
            if file.file_name() == Some("NOTICE.toml") {
                continue;
            }
            let text = match std::fs::read_to_string(&file) {
                Ok(t) => t,
                Err(e) => {
                    loaded.errors.push(LoadError {
                        path: file.to_string(),
                        line: None,
                        message: format!("unreadable rule file: {e}"),
                    });
                    continue;
                }
            };
            let mut entry_ids = std::collections::BTreeMap::new();
            parse_group_file(
                &file,
                &text,
                notice.as_ref(),
                &mut seen_ids,
                &mut entry_ids,
                &mut loaded,
            );
        }
    }
    loaded
}

#[allow(clippy::too_many_arguments)]
fn parse_group_file(
    path: &Utf8Path,
    text: &str,
    notice: Option<&crate::rule::notice::Notice>,
    seen_ids: &mut std::collections::BTreeMap<String, String>,
    seen_entry_ids: &mut std::collections::BTreeMap<String, ()>,
    loaded: &mut Loaded,
) {
    let active_entry_counter = 0usize;

    let parsed: Result<GroupToml, toml::de::Error> = toml::from_str(text);

    let group = match parsed {
        Ok(g) => g,
        Err(e) => {
            loaded.errors.push(LoadError {
                path: path.to_string(),
                line: e.span().map(|s| line_of(text, s.start)),
                message: format!("invalid rule TOML: {e}"),
            });
            return;
        }
    };

    validate_group(path.as_str(), &group, &mut loaded.errors);

    // Attribution cross-check: an [origin] in a rule file must be covered by
    // the pack's NOTICE.toml; a converted (origin-bearing) rule with no
    // NOTICE at all is likewise refused.
    if let Some(origin) = &group.origin {
        match notice {
            None => loaded.errors.push(LoadError {
                path: path.to_string(),
                line: None,
                message: format!(
                    "rule declares [origin] but {} has no NOTICE.toml",
                    path.parent().unwrap_or(path)
                ),
            }),
            Some(n) if !n.covers(&origin.repo, &origin.commit) => {
                loaded.errors.push(LoadError {
                    path: path.to_string(),
                    line: None,
                    message: format!(
                        "origin {}/{} not listed in pack NOTICE.toml",
                        origin.repo, origin.commit
                    ),
                });
            }
            _ => {}
        }
    }

    // Detect cross-file duplicate GROUP ids by recording id_base -> first path.
    if let Some(first) = seen_ids.get(&group.id_base) {
        loaded.errors.push(LoadError {
            path: path.to_string(),
            line: None,
            message: format!(
                "duplicate group id-base `{}` (first defined in {first})",
                group.id_base
            ),
        });
    } else {
        seen_ids.insert(group.id_base.clone(), path.to_string());
    }

    let group_enabled = group.enabled.unwrap_or(true);

    // Build ACTIVE entries with compiled matchers; compile failures were
    // recorded above, so failure here just skips that entry silently.
    let mut active_entries = Vec::new();
    for entry in &group.entries {
        let slug = entry_slug(entry, &group, active_entry_counter);
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
                matcher,
                replacement: entry.replacement.clone(),
            });
        }
    }

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
        metric: None,
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

/// Read + parse `NOTICE.toml` from a pack dir; `None` when absent or broken.
fn load_notice(dir: &Utf8Path) -> Option<crate::rule::notice::Notice> {
    let path = dir.join("NOTICE.toml");
    let text = std::fs::read_to_string(&path).ok()?;
    crate::rule::notice::Notice::parse(&text).ok()
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
        .unwrap_or_else(|| format!("e{}-{counter}", group.id_base.to_lowercase()))
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
