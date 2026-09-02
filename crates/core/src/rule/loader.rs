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
        "vocab" | "pattern" | "literal-ban" | "metric" | "repetition"
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
            match (group.threshold_gt, group.threshold_lt) {
                (Some(_), Some(_)) => push(
                    None,
                    "metric rule takes one of `threshold_gt` or `threshold_lt`, not both".into(),
                ),
                (None, None) => push(
                    None,
                    "metric rule requires `threshold_gt` or `threshold_lt`".into(),
                ),
                _ => {}
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
        "repetition" => {
            if group.variant.is_none() {
                push(None, "repetition rule requires `variant`".into());
            } else if crate::rule::RepetitionVariant::parse(
                group.variant.as_deref().unwrap_or_default(),
            )
            .is_none()
            {
                push(None, format!("unknown variant {:?}", group.variant));
            }
            match group.threshold {
                Some(t) if !(0.0 < t && t <= 1.0) => {
                    push(None, format!("repetition threshold {t} is not in (0, 1]"));
                }
                None => push(None, "repetition rule requires `threshold`".into()),
                _ => {}
            }
            if !group.entries.is_empty() {
                push(None, "repetition rules must not carry [[entries]]".into());
            }
            if group.max_distance == Some(0) {
                // A zero cap filters every pair (distinct units are >= 1
                // token apart), silently disabling the lint.
                push(None, "repetition max-distance must be at least 1".into());
            }
            // Metric-only keys are illegal here: a typo'd kind must refuse,
            // not fall through to defaults.
            if group.stat.is_some()
                || group.per_words.is_some()
                || group.threshold_gt.is_some()
                || group.threshold_lt.is_some()
                || group.window.is_some()
                || group.terms.is_some()
            {
                push(
                    None,
                    "`stat`/`per_words`/`threshold-gt`/`threshold-lt`/`window`/`terms` only apply to metric rules".into(),
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

    // Fixtures are mandatory for every group except metrics and repetitions
    // (aggregate-only: no per-instance matching exists to fixture).
    if matches!(group.kind.as_str(), "metric" | "repetition") {
        // Group-level templates still validate: placeholders must fit the
        // kind's grammar (metric: value/per_words/stat/window; repetition:
        // count).
        let allowed: &[&str] = match group.kind.as_str() {
            "metric" => &["value", "per_words", "stat", "window"],
            _ => &["count"],
        };
        for (field, template) in [("advice", &group.advice), ("message", &group.message)] {
            if let Some(text) = template {
                if let Err(e) = crate::rule::template::validate(text, allowed) {
                    push(None, format!("group {field} template: {e}"));
                }
            }
        }
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
/// name files or directories, relative to `rules_root`, used as-is.
///
/// `models_dir` is the embedding-model root (`None` in tests, which skips
/// the model-presence check entirely). When a repetition group needs the
/// model and the directory lacks it, that pack fails to load.
///
/// Never panics on bad data: problems land in [`Loaded::errors`].
pub fn load(cfg: &Config, rules_root: &Utf8Path, models_dir: Option<&Utf8Path>) -> Loaded {
    load_split(
        cfg,
        rules_root.join("rules").as_path(),
        rules_root,
        models_dir,
    )
}

/// Like [`load`], but the pack directory and the extras root are given
/// separately. `packs_dir` holds `<stem>.toml` files; `extras_root` anchors
/// config `extra_paths`. The CLI's `--rules-dir` uses this so an explicitly
/// named pack directory works regardless of its name.
pub fn load_split(
    cfg: &Config,
    packs_dir: &Utf8Path,
    extras_root: &Utf8Path,
    models_dir: Option<&Utf8Path>,
) -> Loaded {
    let mut loaded = Loaded::default();

    let mut pack_files = Vec::new();
    for name in &cfg.packs.builtin {
        pack_files.push(packs_dir.join(format!("{name}.toml")));
    }
    for extra in &cfg.packs.extra_paths {
        let path = extras_root.join(extra);
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
    // Model-dependent repetition groups need their model files present.
    // Only packs that ARE installed are probed: with no repetition pack in
    // the effective set, a missing model is invisible.
    check_model_availability(
        rules_needed_models(&loaded.rule_set),
        models_dir,
        &mut loaded.errors,
    );
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

/// Model files a propositional repetition rule requires, under
/// `<models_dir>/all-MiniLM-L6-v2/`.
const MODEL_DIR_NAME: &str = "all-MiniLM-L6-v2";
const MODEL_FILES: [&str; 5] = [
    "model.safetensors",
    "config.json",
    "tokenizer.json",
    "tokenizer_config.json",
    "special_tokens_map.json",
];

/// Every (group id-base, variant) pair in `rules` whose variant requires
/// the embedding model. Group-scoped: disabled groups and `[lints]`-allowed
/// groups do not force the check.
fn rules_needed_models(rules: &RuleSet) -> Vec<(String, crate::rule::RepetitionVariant)> {
    let mut out = Vec::new();
    for group in &rules.groups {
        let Some(spec) = &group.repetition else {
            continue;
        };
        if group.enabled && spec.variant.needs_model() {
            out.push((group.id_base.clone(), spec.variant));
        }
    }
    out
}

/// Record a load error per group whose model files are missing. A `None`
/// models_dir (tests, hermetic harnesses) skips the check entirely.
fn check_model_availability(
    needed: Vec<(String, crate::rule::RepetitionVariant)>,
    models_dir: Option<&Utf8Path>,
    errors: &mut Vec<LoadError>,
) {
    let Some(models_dir) = models_dir else {
        return;
    };
    if needed.is_empty() {
        return;
    }
    let model_dir = models_dir.join(MODEL_DIR_NAME);
    let missing: Vec<&str> = MODEL_FILES
        .iter()
        .copied()
        .filter(|f| !model_dir.join(f).is_file())
        .collect();
    if missing.is_empty() {
        return;
    }
    for (id_base, variant) in needed {
        errors.push(LoadError {
            path: model_dir.to_string(),
            line: None,
            message: format!(
                "repetition rule `{id_base}` needs the {MODEL_DIR_NAME} model ({}) for the `{}` variant - expected files in {}",
                missing.join(", "),
                variant.name(),
                model_dir
            ),
        });
    }
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

    let has_group_level_rule = matches!(group.kind.as_str(), "metric" | "repetition");
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
                    let threshold = match (group.threshold_gt, group.threshold_lt) {
                        // Validation guarantees exactly one is present.
                        (Some(gt), _) => crate::rule::MetricThreshold::AtLeast(gt),
                        (None, Some(lt)) => crate::rule::MetricThreshold::AtMost(lt),
                        (None, None) => crate::rule::MetricThreshold::AtLeast(0.0),
                    };
                    let (terms, term_lemmas) = expand_metric_terms(&group.terms);
                    crate::rule::MetricSpec {
                        stat,
                        per_words: group.per_words.unwrap_or(1000),
                        threshold,
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
        repetition: if group.kind == "repetition" && group_was_valid {
            crate::rule::RepetitionVariant::parse(group.variant.as_deref().unwrap_or_default()).map(
                |variant| crate::rule::RepetitionSpec {
                    variant,
                    threshold: group.threshold.unwrap_or_default(),
                    min_members: group
                        .min_members
                        .unwrap_or_else(|| variant.default_min_members()),
                    max_distance: group
                        .max_distance
                        .unwrap_or(crate::rule::DEFAULT_MAX_DISTANCE),
                },
            )
        } else {
            None
        },
    });
}

/// Byte offset -> 1-based line number.
fn line_of(text: &str, byte: usize) -> usize {
    let byte = crate::boundary::floor(text, byte.min(text.len()));
    text[..byte].bytes().filter(|&b| b == b'\n').count() + 1
}

/// Allowed template placeholders for a non-pattern kind.
fn group_allowed_for(kind: &str) -> &'static [&'static str] {
    match kind {
        "vocab" => &["match"],
        "literal-ban" => &["match"],
        "metric" => &["value", "per_words", "stat", "window"],
        "repetition" => &["count"],
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

#[cfg(test)]
mod tests {
    use super::*;

    fn utf8(path: &std::path::Path) -> camino::Utf8PathBuf {
        camino::Utf8PathBuf::from_path_buf(path.to_path_buf()).expect("utf8")
    }

    fn pack_dir(toml: &str) -> (tempfile::TempDir, camino::Utf8PathBuf) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let packs = tmp.path().join("rules");
        std::fs::create_dir_all(&packs).expect("mkdir");
        std::fs::write(packs.join("pack.toml"), toml).expect("write");
        let root = camino::Utf8PathBuf::from_path_buf(tmp.path().to_path_buf()).expect("utf8");
        (tmp, root)
    }

    fn cfg(pack: &str) -> Config {
        Config {
            packs: crate::config::Packs {
                builtin: vec![pack.to_owned()],
                extra_paths: vec![],
            },
            ..Config::default()
        }
    }

    const REP: &str = r#"
[[group]]
id-base = "REP"
kind = "repetition"
tier = 2
category = "repetition"
message = "Repeated {count} times"
variant = "propositional"
threshold = 0.8

[group.fixtures]
must_match = []
"#;

    #[test]
    fn repetition_group_with_valid_fields_loads() {
        // Given a repetition group with every required field.
        let (tmp, root) = pack_dir(REP);

        // When loading with no models_dir (check skipped).
        let loaded = load(&cfg("pack"), &root, None);

        // Then the group lands with its spec materialized.
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        let spec = loaded.rule_set.groups[0]
            .repetition
            .as_ref()
            .expect("repetition spec");
        assert_eq!(spec.variant.name(), "propositional");
        assert!((spec.threshold - 0.8).abs() < f64::EPSILON);
        // And the pair-variant default min_members is 2.
        assert_eq!(spec.min_members, 2);
        // And the distance cap defaults to 200.
        assert_eq!(spec.max_distance, crate::rule::DEFAULT_MAX_DISTANCE);
        drop(tmp);
    }

    #[test]
    fn repetition_explicit_max_distance_round_trips() {
        // Given a repetition group with an explicit max-distance.
        let toml = REP.replace("threshold = 0.8", "threshold = 0.8\nmax-distance = 42");
        let (tmp, root) = pack_dir(&toml);

        // When loading.
        let loaded = load(&cfg("pack"), &root, None);

        // Then the explicit cap wins.
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        let spec = loaded.rule_set.groups[0]
            .repetition
            .as_ref()
            .expect("repetition spec");
        assert_eq!(spec.max_distance, 42);
        drop(tmp);
    }

    #[test]
    fn repetition_zero_max_distance_is_refused() {
        // Given a repetition group whose max-distance is zero.
        let toml = REP.replace("threshold = 0.8", "threshold = 0.8\nmax-distance = 0");
        let (tmp, root) = pack_dir(&toml);

        // When loading.
        let loaded = load(&cfg("pack"), &root, None);

        // Then the pack refuses with a pointing message.
        assert!(
            loaded
                .errors
                .iter()
                .any(|e| e.message.contains("max-distance")),
            "{:?}",
            loaded.errors
        );
        drop(tmp);
    }

    #[test]
    fn repetition_group_with_unknown_variant_is_refused() {
        // Given a repetition group naming a variant that does not exist.
        let toml = REP.replace("propositional", "semantic-foo");
        let (tmp, root) = pack_dir(&toml);

        // When loading.
        let loaded = load(&cfg("pack"), &root, None);

        // Then the pack is refused naming the variant.
        assert!(
            loaded
                .errors
                .iter()
                .any(|e| e.message.contains("unknown variant")),
            "{:?}",
            loaded.errors
        );
        drop(tmp);
    }

    #[test]
    fn repetition_group_without_threshold_is_refused() {
        // Given a repetition group missing its threshold.
        let toml = REP.replace("threshold = 0.8\n", "");
        let (tmp, root) = pack_dir(&toml);

        // When loading.
        let loaded = load(&cfg("pack"), &root, None);

        // Then the pack is refused requiring threshold.
        assert!(
            loaded
                .errors
                .iter()
                .any(|e| e.message.contains("requires `threshold`")),
            "{:?}",
            loaded.errors
        );
        drop(tmp);
    }

    #[test]
    fn repetition_group_with_out_of_range_threshold_is_refused() {
        // Given a repetition group whose threshold exceeds 1.
        let toml = REP.replace("threshold = 0.8", "threshold = 1.5");
        let (tmp, root) = pack_dir(&toml);

        // When loading.
        let loaded = load(&cfg("pack"), &root, None);

        // Then the pack is refused with the range in the message.
        assert!(
            loaded
                .errors
                .iter()
                .any(|e| e.message.contains("not in (0, 1]")),
            "{:?}",
            loaded.errors
        );
        drop(tmp);
    }

    #[test]
    fn repetition_group_with_metric_keys_is_refused() {
        // Given a repetition group carrying a metric-only key.
        let toml = REP.replace(
            "threshold = 0.8",
            "threshold = 0.8\nstat = \"em_dash_rate\"",
        );
        let (tmp, root) = pack_dir(&toml);

        // When loading.
        let loaded = load(&cfg("pack"), &root, None);

        // Then the pack is refused: metric keys are not repetition keys.
        assert!(
            loaded
                .errors
                .iter()
                .any(|e| e.message.contains("only apply to metric rules")),
            "{:?}",
            loaded.errors
        );
        drop(tmp);
    }

    #[test]
    fn repetition_group_requiring_model_fails_load_when_model_missing() {
        // Given a propositional repetition pack and an EMPTY models dir.
        let (tmp, root) = pack_dir(REP);
        let models = tempfile::tempdir().expect("models");

        // When loading with models_dir pointed at the empty dir.
        let loaded = load(&cfg("pack"), &root, Some(utf8(models.path()).as_path()));

        // Then the pack fails to load naming the model directory and file.
        let err = loaded
            .errors
            .iter()
            .find(|e| e.message.contains("all-MiniLM-L6-v2"))
            .expect("model error");
        assert!(err.message.contains("model.safetensors"), "{err:?}");
        drop(tmp);
        drop(models);
    }

    #[test]
    fn content_family_group_does_not_require_model() {
        // Given a content-family repetition pack (no model needed).
        let toml = REP
            .replace("propositional", "content-family")
            .replace("threshold = 0.8", "threshold = 0.4\nmin-members = 3");
        let (tmp, root) = pack_dir(&toml);
        let models = tempfile::tempdir().expect("models");

        // When loading with an empty models dir.
        let loaded = load(&cfg("pack"), &root, Some(utf8(models.path()).as_path()));

        // Then the pack loads: the content-family variant is model-free.
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        let spec = loaded.rule_set.groups[0].repetition.expect("spec");
        assert_eq!(spec.min_members, 3);
        drop(tmp);
        drop(models);
    }

    #[test]
    fn disabled_repetition_group_skips_model_check() {
        // Given a DISABLED propositional repetition pack.
        let toml = REP.replace("id-base = \"REP\"", "id-base = \"REP\"\nenabled = false");
        let (tmp, root) = pack_dir(&toml);
        let models = tempfile::tempdir().expect("models");

        // When loading with an empty models dir.
        let loaded = load(&cfg("pack"), &root, Some(utf8(models.path()).as_path()));

        // Then nothing is probed: a disabled pack never demands the model.
        assert!(loaded.errors.is_empty(), "{:?}", loaded.errors);
        drop(tmp);
        drop(models);
    }
}
