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
#[derive(Debug, Clone, Copy, Default)]
pub struct LintSettings {
    /// Tier filter; None = all tiers.
    pub max_tier: Option<u8>,
}

/// One document's lint result set, sorted deterministically.
pub fn scan(src: &str, rules: &RuleSet, settings: &LintSettings) -> Vec<Finding> {
    let norm = normalize(src);
    let text = &norm.text;

    let map = regions::build_regions(text);
    let dict = build_dictionary(rules);
    let map = use_mention::mask_quoted_terms(&map, &dict);

    let mut findings = Vec::new();

    for group in &rules.groups {
        if !group.enabled {
            continue;
        }
        if let Some(max) = settings.max_tier {
            if group.tier > max {
                continue;
            }
        }
        for entry in &group.entries {
            match &entry.matcher {
                Matcher::Vocab { terms, .. } => {
                    let index = vocab_scan::VocabIndex::build(
                        terms.iter().map(|t| (t.clone(), entry.id.clone())),
                    );
                    let scope_allow = scope_predicate(&group.scope);
                    for hit in index.scan(text, &map, &scope_allow) {
                        findings.push(make_finding(
                            group,
                            entry.id.as_str(),
                            KindTag::Vocab,
                            hit.start,
                            hit.end,
                            hit.matched.clone(),
                            &[],
                            entry
                                .message_override
                                .as_deref()
                                .or(group.message.as_deref()),
                            entry.advice_override.as_deref().or(group.advice.as_deref()),
                            entry.replacement.clone(),
                            &norm,
                            src,
                        ));
                    }
                }
                Matcher::Pattern(re) => {
                    for hit in pattern_scan::scan(re, text, &map) {
                        findings.push(make_finding(
                            group,
                            entry.id.as_str(),
                            KindTag::Pattern,
                            hit.start,
                            hit.end,
                            text[hit.start..hit.end].to_string(),
                            &hit.captures,
                            entry
                                .message_override
                                .as_deref()
                                .or(group.message.as_deref()),
                            entry.advice_override.as_deref().or(group.advice.as_deref()),
                            None,
                            &norm,
                            src,
                        ));
                    }
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
                            hit.start,
                            hit.end,
                            hit.term.clone(),
                            &[],
                            entry
                                .message_override
                                .as_deref()
                                .or(group.message.as_deref()),
                            entry.advice_override.as_deref().or(group.advice.as_deref()),
                            None,
                            &norm,
                            src,
                        ));
                    }
                }
            }
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
        if let Some(max) = settings.max_tier {
            if group.tier > max {
                continue;
            }
        }
        let Some(spec) = &group.metric else { continue };
        let local_stat = metrics::Stat::parse(spec.stat.name()).expect("same registry");
        let Some(value) = stats.get(local_stat) else {
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
        let anchor = 0;
        let (o_start, o_end) = norm.span_to_orig(anchor, anchor);
        findings.push(Finding {
            entry_id: group.id_base.clone(),
            kind: KindTag::Metric,
            tier: Tier::from_number(group.tier).unwrap_or(Tier::Density),
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
    start: usize,
    end: usize,
    matched: String,
    captures: &[(String, String)],
    message_t: Option<&str>,
    advice_t: Option<&str>,
    replacement: Option<String>,
    norm: &crate::eol::Normalized,
    orig_src: &str,
) -> Finding {
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
        tier: tier_of(group.tier),
        category: group.category.clone(),
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

/// Sort: (offset, tier number, entry id).
fn sort_findings(mut f: Vec<Finding>) -> Vec<Finding> {
    f.sort_by_key(|x| (x.span.start, x.span.end, x.tier, x.entry_id.clone()));
    f
}

fn tier_of(n: u8) -> Tier {
    Tier::from_number(n).unwrap_or(Tier::Tell)
}
