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
use crate::plugin::{LintPlugin, PluginFinding, PluginInput};
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
///
/// Convenience wrapper with no plugins; see [`scan_with_plugins`].
pub fn scan(src: &str, rules: &RuleSet, settings: &LintSettings) -> Vec<Finding> {
    scan_with_plugins(src, rules, settings, &[]).findings
}

/// [`scan`] with a plugin pass appended. The only difference besides the
/// extra findings is the warning list: one entry per plugin failure.
pub fn scan_with_plugins(
    src: &str,
    rules: &RuleSet,
    settings: &LintSettings,
    plugins: &[Box<dyn LintPlugin>],
) -> ScanWithPlugins {
    let norm = normalize(src);
    let text = &norm.text;

    let map = regions::build_regions(text);
    let dict = build_dictionary(rules);
    let map = use_mention::mask_quoted_terms(&map, &dict);

    let mut findings = Vec::new();
    let mut warnings = Vec::new();

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

    plugin_findings(
        src,
        text,
        &map,
        &map.masked,
        &norm,
        plugins,
        settings,
        &mut findings,
        &mut warnings,
    );

    ScanWithPlugins {
        findings: sort_findings(findings),
        warnings,
    }
}

/// Result of a scan that includes plugins: findings plus non-fatal warnings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScanWithPlugins {
    /// All findings, sorted deterministically.
    pub findings: Vec<Finding>,
    /// One warning per plugin failure (trap, fuel, protocol violation).
    /// Rendered on stderr; they never affect the exit code.
    pub warnings: Vec<String>,
}

/// Run every plugin over one document and assemble its findings.
///
/// Gating mirrors native groups: a plugin whose GROUP is `allow`ed or whose
/// tier exceeds the max is not called at all. Per finding, the `GROUP#slug`
/// key gets the same allow/note/warn/error treatment. A plugin failure drops
/// that plugin's findings for this document and records a warning.
#[allow(clippy::too_many_arguments)]
fn plugin_findings(
    src: &str,
    _norm_text: &str,
    _map: &regions::RegionMap,
    masked: &str,
    norm: &crate::eol::Normalized,
    plugins: &[Box<dyn LintPlugin>],
    settings: &LintSettings,
    findings: &mut Vec<Finding>,
    warnings: &mut Vec<String>,
) {
    if plugins.is_empty() {
        return;
    }
    // Document envelope: the masked text plus the same range sources the
    // metric scanner uses. Masking is byte-length-preserving, so all these
    // coordinates are directly usable by the plugin. Built once, cloned per
    // plugin (each gets its own `config` in phase 4 wiring).
    let envelope = PluginInput {
        text: masked.to_string(),
        heading_ranges: ranges_u64(metrics::scope_ranges(
            _map,
            regions::Scope::is_heading_like,
        )),
        bold_spans: ranges_u64(metrics::bold_ranges(_map)),
        list_items: ranges_u64(metrics::list_item_ranges(_map)),
        config: serde_json::json!({}),
    };

    for plugin in plugins {
        let manifest = plugin.meta();
        let id = &manifest.id;
        if settings.level_for(id, "") == Some(crate::config::LintLevel::Allow) {
            continue;
        }
        if let Some(max) = settings.max_tier {
            if manifest.tier > max {
                continue;
            }
        }
        let input = PluginInput {
            config: plugin.params(),
            ..envelope.clone()
        };
        let produced = match plugin.scan(&input) {
            Ok(produced) => produced,
            Err(error) => {
                warnings.push(format!("deslop: plugin {id} failed: {error}"));
                continue;
            }
        };
        let tier = tier_of(manifest.tier);
        for pf in dedupe_by_slug(produced, id) {
            let entry_id = format!("{id}#{}", pf.slug);
            // Per-finding gate: allow/note/warn/error by GROUP#slug, then
            // GROUP. Native code passes the full entry id as the exact key;
            // plugins follow the same shape.
            let level = settings.level_for(id, &entry_id);
            if level == Some(crate::config::LintLevel::Allow) {
                continue;
            }
            let effective = match level {
                Some(crate::config::LintLevel::Error) => Tier::Artifact,
                Some(crate::config::LintLevel::Warn) => Tier::Tell,
                Some(crate::config::LintLevel::Note) => Tier::Density,
                _ => tier,
            };
            // Validate: spans are guest-supplied, so never trust them.
            let (start, end) = (pf.span.0 as usize, pf.span.1 as usize);
            let valid = start < end
                && end <= masked.len()
                && masked.is_char_boundary(start)
                && masked.is_char_boundary(end);
            if !valid {
                warnings.push(format!(
                    "deslop: plugin {id} finding {} has invalid span [{start}..{end}); dropped",
                    pf.slug
                ));
                continue;
            }
            let (o_start, o_end) = norm.span_to_orig(start, end);
            findings.push(Finding {
                entry_id,
                kind: KindTag::Plugin,
                tier: effective,
                category: manifest.category.clone(),
                message: pf.message,
                advice: pf.advice,
                span: Span::new(o_start, o_end),
                excerpt: src[o_start..o_end].to_string(),
                url: None,
                context: None,
                replacement: None,
                anchorless: false,
            });
        }
    }
}

/// Keep the first finding per slug; a plugin repeating a slug is a bug and
/// would produce colliding entry ids.
fn dedupe_by_slug(
    produced: Vec<PluginFinding>,
    id: &str,
) -> impl Iterator<Item = PluginFinding> {
    let mut seen = std::collections::HashSet::new();
    produced.into_iter().filter(move |pf| {
        let fresh = pf.slug.is_empty() || seen.insert(pf.slug.clone());
        if !fresh {
            eprintln!("deslop: plugin {id} repeated slug {}; dropped", pf.slug);
        }
        fresh
    })
}

/// Widen native `(usize, usize)` ranges to the wire protocol's u64 pairs.
fn ranges_u64(ranges: Vec<(usize, usize)>) -> Vec<(u64, u64)> {
    ranges
        .into_iter()
        .map(|(s, e)| (s as u64, e as u64))
        .collect()
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

        // Cluster: per-rule terms, one finding PER offending window. The
        // finding spans the whole WINDOW, not the last trigger word: the
        // message preview and the terms note carry the evidence, so a
        // caret on one arbitrary hit would misstate the finding.
        if local_stat == metrics::Stat::TermClusterMax {
            for w in
                metrics::cluster_windows(norm_text, &spec.terms, &spec.term_lemmas, spec.window)
            {
                if (w.distinct as f64) <= spec.threshold_gt {
                    continue;
                }
                // Trim the blank-line padding from paragraph/document window
                // bounds so the span (and excerpt) start and end on content.
                let bounds = trim_window_bounds(norm_text, w.bounds);
                let (o_start, o_end) = norm.span_to_orig(bounds.0, bounds.1);
                let context = cluster_context(&w);
                findings.push(metric_finding(
                    group,
                    spec,
                    w.distinct as f64,
                    (o_start, o_end),
                    orig_src,
                    settings,
                    Some(context),
                    true,
                ));
            }
            continue;
        }

        // Whole-doc stats: single value, anchored at the FIRST occurrence of
        // their signal so the caret sits on evidence, not byte 0.
        let Some((value, (s, e))) = stats.get(local_stat).map(|v| {
            let (s, e) = metrics::first_signal_span(
                local_stat,
                norm_text,
                &bold_spans,
                &heading_ranges,
                &list_items,
            );
            (v, (s, e))
        }) else {
            continue;
        };
        if value <= spec.threshold_gt {
            continue;
        }
        let (o_start, o_end) = norm.span_to_orig(s, e);
        findings.push(metric_finding(
            group,
            spec,
            value,
            (o_start, o_end),
            orig_src,
            settings,
            None,
            false,
        ));
    }
}

/// Build one metric Finding. `context` carries the cluster metric's
/// evidence (which words fired, indented under a header); `None` for
/// whole-doc stats. `anchorless` marks window-spanned cluster findings for
/// caret-free human rendering. The window's own count
/// drives `{value}` so a per-window finding reports ITS number, not the
/// document maximum.
#[allow(clippy::too_many_arguments)]
fn metric_finding(
    group: &crate::rule::RuleGroup,
    spec: &crate::rule::MetricSpec,
    value: f64,
    span: (usize, usize),
    orig_src: &str,
    settings: &LintSettings,
    context: Option<String>,
    anchorless: bool,
) -> Finding {
    let (o_start, o_end) = span;
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
    Finding {
        entry_id: group.id_base.clone(),
        kind: KindTag::Metric,
        tier: effective_tier(settings, group, &group.id_base),
        category: group.category.clone(),
        message,
        advice,
        span: Span::new(o_start, o_end),
        excerpt: orig_src[o_start..o_end].to_string(),
        url: group.url.clone(),
        context,
        replacement: None,
        anchorless,
    }
}

/// Hardcoded cluster evidence note: a `Clustered terms:` header, then one
/// line per DISTINCT trigger word in first-occurrence order, each indented
/// two spaces. Rendered as a note list by the human renderer and as a
/// single multi-line string by the other formats. Example:
/// `Clustered terms:\n  also\n  aptly\n  adept`
fn cluster_context(w: &metrics::ClusterWindowHit) -> String {
    let mut out = String::from("Clustered terms:");
    for term in &w.terms_in_order {
        out.push_str("\n  ");
        out.push_str(term);
    }
    out
}

/// Shrink raw window bounds to their content: paragraph windows include the
/// blank-line separator that opens them, and the trailing newline can ride
/// along too. Leading/trailing `\n` bytes are padding, never prose, so
/// trimming them cannot split a multibyte character.
fn trim_window_bounds(text: &str, bounds: (usize, usize)) -> (usize, usize) {
    let bytes = text.as_bytes();
    let mut start = bounds.0.min(text.len());
    let mut end = bounds.1.min(text.len());
    while start < end && bytes[start] == b'\n' {
        start += 1;
    }
    while end > start && bytes[end - 1] == b'\n' {
        end -= 1;
    }
    (start, end)
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
        context: None,
        replacement,
        anchorless: false,
    }
}

fn default_message(kind: KindTag) -> String {
    match kind {
        KindTag::Vocab => "AI-tell vocabulary".into(),
        KindTag::Pattern => "AI writing construction".into(),
        KindTag::LiteralBan => "chatbot markup artifact".into(),
        KindTag::Metric => "document-level signal".into(),
        // Plugins always provide their own message; the host never renders
        // templates on their behalf.
        KindTag::Plugin => "plugin finding".into(),
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
) -> Vec<(
    &'a regex::Regex,
    Vec<(&'a crate::rule::RuleGroup, &'a crate::rule::ActiveEntry)>,
)> {
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
