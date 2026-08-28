//! Scan orchestration: normalize -> regions -> use-mention -> four scanners
//! -> finding assembly -> deterministic sort.
//!
//! Offsets run on the normalized text; because masking preserves byte
//! length, region-map positions are normalized-text positions, translated
//! to ORIGINAL coordinates only at finding assembly via the EOL remap.

pub mod literal_scan;
pub mod metrics;
pub mod pattern_scan;
pub mod regions;
pub mod use_mention;
pub mod vocab_scan;

use crate::eol::normalize;
use crate::finding::{Finding, KindTag, Span, Tier};
use crate::rule::{RuleSet, fixtures::Matcher};

/// Options affecting scan behavior (subset of config; resolved by CLI).
#[derive(Debug, Clone, Default)]
pub struct LintSettings {
    /// Tier filter; None = all tiers.
    pub max_tier: Option<u8>,
    /// `[lints]` overrides keyed `GROUP` or `GROUP#slug` (slug wins).
    pub levels: std::collections::BTreeMap<String, crate::config::LintLevel>,
}

impl LintSettings {
    /// Effective override for a lint id: exact `GROUP#slug` first, then
    /// `GROUP`. `None` = use the rule's tier.
    pub fn level_for(&self, group: &str, id: &str) -> Option<crate::config::LintLevel> {
        self.levels
            .get(id)
            .or_else(|| self.levels.get(group))
            .copied()
    }
}

/// One document's lint result set, sorted deterministically.
pub fn scan(src: &str, rules: &RuleSet, settings: &LintSettings) -> Vec<Finding> {
    let norm = normalize(src);
    let text = &norm.text;

    let map = regions::build_regions(text);
    let dict = build_dictionary(rules);
    let map = use_mention::mask_quoted_terms(&map, &dict);

    let mut findings = Vec::new();

    // Pattern fan-out table: ONE scan per unique regex source string, one
    // finding per surviving owner per hit. Built per-document because the
    // `[lints]`/tier gates decide which owners participate.
    let pattern_groups = pattern_owners(rules, settings);

    // ONE shared vocab index for the whole ruleset: tokenization happens
    // once per document, not once per entry.
    let shared_vocab = {
        let mut pairs: Vec<(String, String)> = Vec::new();
        for g in &rules.groups {
            if !g.enabled {
                continue;
            }
            for e in &g.entries {
                if settings.level_for(&g.id_base, &e.id) == Some(crate::config::LintLevel::Allow) {
                    continue;
                }
                if let Matcher::Vocab { terms, .. } = &e.matcher {
                    for t in terms {
                        pairs.push((t.clone(), e.id.clone()));
                    }
                }
            }
        }
        vocab_scan::VocabIndex::build(pairs)
    };

    for group in &rules.groups {
        if !group.enabled {
            continue;
        }
        // Group-level override: allow prunes the whole group; other levels
        // demote/promote its findings' severity via entry tier resolution.
        let group_level = settings.level_for(&group.id_base, "");
        if group_level == Some(crate::config::LintLevel::Allow) {
            continue;
        }
        if let Some(max) = settings.max_tier {
            if group.tier > max {
                continue;
            }
        }
        for entry in &group.entries {
            let entry_level = settings.level_for(&group.id_base, &entry.id);
            if entry_level == Some(crate::config::LintLevel::Allow) {
                continue;
            }
            match &entry.matcher {
                Matcher::Vocab { .. } => {}
                Matcher::Pattern(_) => {
                    // Handled by the fan-out pass below; entries are visited
                    // there once per unique regex string.
                }
                Matcher::Literal { needles } => {
                    let compiled: Vec<(String, Vec<crate::rule::literals::Segment>)> = needles
                        .iter()
                        .filter_map(|n| {
                            crate::rule::literals::compile(n)
                                .ok()
                                .map(|s| (n.clone(), s))
                        })
                        .collect();
                    for hit in literal_scan::scan(&map, &compiled) {
                        findings.push(make_finding(
                            group,
                            entry.id.as_str(),
                            KindTag::LiteralBan,
                            effective_tier(settings, group, &entry.id),
                            hit.start,
                            hit.end,
                            hit.term.clone(),
                            &[],
                            entry
                                .message_override
                                .as_deref()
                                .or(group.message.as_deref()),
                            entry.advice_override.as_deref().or(group.advice.as_deref()),
                            entry
                                .category_override
                                .as_deref()
                                .or(Some(group.category.as_str())),
                            None,
                            &norm,
                            src,
                        ));
                    }
                }
            }
        }
    }

    // Pattern fan-out: one scan per unique regex string, one finding per
    // owner per hit. Tier/`[lints]` gates were applied when building the
    // owner table, so a suppressed owner simply doesn't appear here.
    for (re, owners) in &pattern_groups {
        for hit in pattern_scan::scan(re, text, &map) {
            for (group, entry) in owners {
                findings.push(make_finding(
                    group,
                    entry.id.as_str(),
                    KindTag::Pattern,
                    effective_tier(settings, group, &entry.id),
                    hit.start,
                    hit.end,
                    text[hit.start..hit.end].to_string(),
                    &hit.captures,
                    entry
                        .message_override
                        .as_deref()
                        .or(group.message.as_deref()),
                    entry.advice_override.as_deref().or(group.advice.as_deref()),
                    entry
                        .category_override
                        .as_deref()
                        .or(Some(group.category.as_str())),
                    None,
                    &norm,
                    src,
                ));
            }
        }
    }

    // Shared vocab scan: one pass, then per-hit entry resolution.
    {
        let scope_allow = scope_predicate("prose");
        for hit in shared_vocab.scan(text, &map, &scope_allow) {
            let Some((group, entry)) = resolve_entry(rules, &hit.entry_slug) else {
                continue;
            };
            findings.push(make_finding(
                group,
                entry.id.as_str(),
                KindTag::Vocab,
                effective_tier(settings, group, &entry.id),
                hit.start,
                hit.end,
                hit.matched.clone(),
                &[],
                entry
                    .message_override
                    .as_deref()
                    .or(group.message.as_deref()),
                entry.advice_override.as_deref().or(group.advice.as_deref()),
                entry
                    .category_override
                    .as_deref()
                    .or(Some(group.category.as_str())),
                entry.replacement.clone(),
                &norm,
                src,
            ));
        }
    }

    metric_findings(src, text, &map, rules, settings, &norm, &mut findings);

    sort_findings(findings)
}

/// Document-level stats -> one finding per crossed threshold (Tier 3).
/// Anchored near the densest spot for the stat where sensible; em-dash-rate
/// anchors at its densest line, others anchor at document start.
fn metric_findings(
    orig_src: &str,
    norm_text: &str,
    map: &regions::RegionMap,
    rules: &RuleSet,
    settings: &LintSettings,
    norm: &crate::eol::Normalized,
    findings: &mut Vec<Finding>,
) {
    let Some((_, prose)) = metrics::visible_prose(norm_text, map) else {
        return;
    };
    let heading_ranges = metrics::scope_ranges(map, regions::Scope::is_heading_like);
    let bold_spans = metrics::bold_ranges(map);
    let list_items = metrics::list_item_ranges(map);
    let inputs = metrics::Inputs {
        prose: &prose,
        heading_ranges: &heading_ranges,
        bold_spans: &bold_spans,
        list_items: &list_items,
    };
    let stats = metrics::compute(&inputs);

    for group in &rules.groups {
        if !group.enabled {
            continue;
        }
        if settings.level_for(&group.id_base, &group.id_base)
            == Some(crate::config::LintLevel::Allow)
        {
            continue;
        }
        if let Some(max) = settings.max_tier {
            if group.tier > max {
                continue;
            }
        }
        let Some(spec) = &group.metric else { continue };
        let local_stat = metrics::Stat::parse(spec.stat.name()).expect("same registry");
        // Cluster stat is per-rule (terms vary); compute directly on masked.
        // (value, anchor span in norm text). Cluster stat is per-rule
        // (terms vary); other stats anchor at zero (whole-doc finding).
        let measured: Option<(f64, metrics::ClusterHit)> = match local_stat {
            metrics::Stat::TermClusterMax => {
                metrics::term_cluster_max(norm_text, &spec.terms, &spec.term_lemmas, spec.window)
                    .map(|(n, hit)| (n as f64, hit))
            }
            _ => stats
                .get(local_stat)
                .map(|v| (v, metrics::ClusterHit { start: 0, end: 0 })),
        };
        let Some((value, hit)) = measured else {
            continue;
        };
        if value <= spec.threshold_gt {
            continue;
        }
        // Denominator aware message: value is already "per per_words".
        let per_words = spec.per_words.max(1);
        let lookup = |name: &str| match name {
            "value" => Some(format!("{value:.1}")),
            "per_words" => Some(per_words.to_string()),
            "stat" => Some(spec.stat.name().to_string()),
            "window" => Some(spec.window.name().to_string()),
            _ => None,
        };
        let message = group
            .message
            .as_deref()
            .map(|t| crate::rule::template::render(t, &lookup))
            .unwrap_or_else(|| default_message(KindTag::Metric));
        let advice = group
            .advice
            .as_deref()
            .map(|t| crate::rule::template::render(t, &lookup));
        let (o_start, o_end) = norm.span_to_orig(hit.start, hit.end);
        findings.push(Finding {
            entry_id: group.id_base.clone(),
            kind: KindTag::Metric,
            tier: effective_tier(settings, group, &group.id_base),
            category: group.category.clone(),
            message,
            advice,
            span: Span::new(o_start, o_end),
            excerpt: orig_src[o_start..o_end].to_string(),
            url: group.url.clone(),
            replacement: None,
        });
    }
}

fn scope_predicate(scope: &str) -> impl Fn(regions::Scope) -> bool + '_ {
    move |s| match scope {
        "heading" => matches!(s, regions::Scope::Heading(_)),
        "list-item" => s == regions::Scope::ListItem,
        // prose AND anywhere both allow plain prose; heading/list scopes are
        // additive for "anywhere".
        _ => true,
    }
}

#[allow(clippy::too_many_arguments)]
fn make_finding(
    group: &crate::rule::RuleGroup,
    entry_id: &str,
    kind: KindTag,
    tier: Tier,
    start: usize,
    end: usize,
    matched: String,
    captures: &[(String, String)],
    message_t: Option<&str>,
    advice_t: Option<&str>,
    category_t: Option<&str>,
    replacement: Option<String>,
    norm: &crate::eol::Normalized,
    orig_src: &str,
) -> Finding {
    let category = category_t.unwrap_or(&group.category);
    let (o_start, o_end) = norm.span_to_orig(start, end);
    let mut vars: Vec<(String, String)> = vec![("match".into(), matched)];
    for (n, v) in captures {
        vars.push((n.clone(), v.clone()));
    }
    let lookup = |name: &str| vars.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
    let message = message_t
        .map(|t| crate::rule::template::render(t, &lookup))
        .unwrap_or_else(|| default_message(kind));
    let advice = advice_t.map(|t| crate::rule::template::render(t, &lookup));
    let excerpt = orig_src[o_start..o_end].to_string();
    Finding {
        entry_id: entry_id.to_string(),
        kind,
        tier,
        category: category.to_string(),
        message,
        advice,
        span: Span::new(o_start, o_end),
        excerpt,
        url: group.url.clone(),
        replacement,
    }
}

fn default_message(kind: KindTag) -> String {
    match kind {
        KindTag::Vocab => "AI-tell vocabulary".into(),
        KindTag::Pattern => "AI writing construction".into(),
        KindTag::LiteralBan => "chatbot markup artifact".into(),
        KindTag::Metric => "document-level signal".into(),
    }
}

/// Combined dictionary for the use-mention pass.
fn build_dictionary(rules: &RuleSet) -> Vec<String> {
    let mut out = Vec::new();
    for g in &rules.groups {
        for e in &g.entries {
            if let Matcher::Vocab { terms, .. } = &e.matcher {
                out.extend(terms.iter().cloned());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Find (group, entry) by globally-unique entry id.
fn resolve_entry<'a>(
    rules: &'a crate::rule::RuleSet,
    id: &str,
) -> Option<(&'a crate::rule::RuleGroup, &'a crate::rule::ActiveEntry)> {
    for g in &rules.groups {
        if let Some(e) = g.entries.iter().find(|e| e.id == id) {
            return Some((g, e));
        }
    }
    None
}

/// Group pattern entries by their compiled regex's source string: each unique
/// string is scanned ONCE, and every surviving owner emits its own finding
/// per hit. Same gates as the per-entry loop (enabled, allow, max tier).
fn pattern_owners<'a>(
    rules: &'a RuleSet,
    settings: &LintSettings,
) -> Vec<(&'a regex::Regex, Vec<(&'a crate::rule::RuleGroup, &'a crate::rule::ActiveEntry)>)> {
    let mut order: Vec<&'a regex::Regex> = Vec::new();
    let mut owners: std::collections::HashMap<
        &'a str,
        Vec<(&'a crate::rule::RuleGroup, &'a crate::rule::ActiveEntry)>,
    > = std::collections::HashMap::new();
    for group in &rules.groups {
        if !group.enabled {
            continue;
        }
        if settings.level_for(&group.id_base, "") == Some(crate::config::LintLevel::Allow) {
            continue;
        }
        if let Some(max) = settings.max_tier {
            if group.tier > max {
                continue;
            }
        }
        for entry in &group.entries {
            if settings.level_for(&group.id_base, &entry.id)
                == Some(crate::config::LintLevel::Allow)
            {
                continue;
            }
            if let Matcher::Pattern(re) = &entry.matcher {
                match owners.get_mut(re.as_str()) {
                    Some(v) => v.push((group, entry)),
                    None => {
                        order.push(re);
                        owners.insert(re.as_str(), vec![(group, entry)]);
                    }
                }
            }
        }
    }
    order
        .into_iter()
        .filter_map(|re| {
            let group = owners.remove(re.as_str())?;
            Some((re, group))
        })
        .collect()
}

/// Sort: (offset, tier number, entry id).
fn sort_findings(mut f: Vec<Finding>) -> Vec<Finding> {
    f.sort_by_key(|x| (x.span.start, x.span.end, x.tier, x.entry_id.clone()));
    f
}

fn tier_of(n: u8) -> Tier {
    Tier::from_number(n).unwrap_or(Tier::Tell)
}

/// Tier after `[lints]` override; `None` = suppressed at the group gate.
fn effective_tier(settings: &LintSettings, group: &crate::rule::RuleGroup, entry_id: &str) -> Tier {
    let base = tier_of(group.tier);
    let level = settings.level_for(&group.id_base, entry_id);
    match level {
        Some(crate::config::LintLevel::Error) => Tier::Artifact,
        Some(crate::config::LintLevel::Warn) => Tier::Tell,
        Some(crate::config::LintLevel::Note) => Tier::Density,
        _ => base,
    }
}
