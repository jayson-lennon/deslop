//! Merge policy: union across sources, MAX(tier) on conflict,
//! replacement by source priority, deterministic ordering.

use crate::slop_json::RawTerm;

/// Normalized single vocabulary entry ready for emission.
#[derive(Debug, Clone)]
pub struct MergedTerm {
    pub term: String,
    /// Higher = softer (fewer false positives win ties).
    pub tier: u8,
    pub replacement: Option<String>,
    /// Source of the chosen replacement (priority tracking).
    pub replacement_src: Option<String>,
    /// Concatenated evidence notes from contributing sources.
    pub evidence: Vec<String>,
    /// anti-ai-tell class rank, strongest wins: `hard_ban`=3,
    /// `strong_flag`=2, `density_watch`=1, none=0. Drives the group split
    /// into hard-ban / strong-flag / watch files.
    pub severity: u8,
}

/// Severity rank for a raw class label.
pub fn severity_rank(label: Option<&str>) -> u8 {
    match label {
        Some("hard_ban") => 3,
        Some("strong_flag") => 2,
        Some("density_watch") => 1,
        _ => 0,
    }
}

/// Group id-base for a severity rank.
pub fn group_for_severity(rank: u8) -> &'static str {
    match rank {
        3 => "MODERN-VOCAB-HARD-BAN",
        2 => "MODERN-VOCAB-STRONG-FLAG",
        1 => "MODERN-VOCAB-WATCH",
        _ => "MODERN-VOCAB",
    }
}

/// Source priority for replacement selection (index = strength).
const PRIORITY: [&str; 4] = ["anti-ai-slop", "stop-slop", "anti-ai-tell", "wsc"];

/// Tier ceilings per source (conservative: most sources are advisory).
fn source_tier(source: &str) -> u8 {
    match source {
        // wsc's reason texts are strong signals — Tier 2.
        "wsc" => 2,
        // everything else lands at Tier 2 as well (tell-level);
        // density_watch-ish lists would be Tier 3 but merged union can't tell.
        _ => 2,
    }
}

/// Union all raw terms. Case-insensitive keying on the trimmed term.
pub fn merge_vocab(all: Vec<RawTerm>) -> Vec<MergedTerm> {
    let mut map: std::collections::BTreeMap<String, MergedTerm> = std::collections::BTreeMap::new();
    for term in all {
        let key = term.term.trim().to_lowercase();
        if key.chars().count() < 3 {
            continue;
        }
        let entry = map.entry(key.clone()).or_insert_with(|| MergedTerm {
            term: key,
            tier: source_tier(&term.source),
            replacement: None,
            replacement_src: None,
            evidence: Vec::new(),
            severity: severity_rank(term.severity),
        });
        entry.tier = entry.tier.max(source_tier(&term.source));
        entry.severity = entry.severity.max(severity_rank(term.severity));
        if !entry.evidence.contains(&term.evidence) && !term.evidence.is_empty() {
            entry.evidence.push(term.evidence.clone());
        }
        // Replacement only from higher-priority source when not yet set.
        let this_prio = crate::merge::best_priority(Some(&term.source));
        let cur_prio = crate::merge::best_priority(entry.replacement_src.as_deref());
        if entry.replacement.is_none() || cur_prio > this_prio {
            if let Some(r) = term.replacement {
                entry.replacement = Some(r);
                entry.replacement_src = Some(term.source.clone());
            }
        }
    }
    let mut out: Vec<MergedTerm> = map.into_values().collect();
    out.sort_by(|a, b| a.term.cmp(&b.term));
    out
}

/// Lower number = higher priority; missing source sorts last.
pub fn best_priority(src: Option<&str>) -> usize {
    src.and_then(|s| PRIORITY.iter().position(|p| *p == s))
        .unwrap_or(PRIORITY.len() + 1)
}

#[cfg(test)]
mod tests {
    use super::merge_vocab;
    use crate::slop_json::RawTerm;

    fn raw(term: &str, source: &str, replacement: Option<&str>) -> RawTerm {
        RawTerm {
            term: term.into(),
            replacement: replacement.map(Into::into),
            evidence: format!("evidence from {source}"),
            source: source.into(),
            severity: None,
        }
    }

    #[test]
    fn union_keeps_first_replacement_by_priority() {
        // Given the same term from two sources, slop first in priority.
        let merged = merge_vocab(vec![
            raw("delve", "stop-slop", Some("dig")),
            raw("Delve", "anti-ai-slop", Some("examine")),
        ]);
        // When merged.
        assert_eq!(merged.len(), 1);
        // Then the higher-priority source's replacement wins.
        assert_eq!(merged[0].replacement.as_deref(), Some("examine"));
        assert_eq!(merged[0].replacement_src.as_deref(), Some("anti-ai-slop"));
    }

    #[test]
    fn replacement_not_clobbered_by_lower_priority() {
        let merged = merge_vocab(vec![
            raw("delve", "anti-ai-slop", Some("examine")),
            raw("delve", "wsc", None),
            raw("delve", "wsc", Some("look into")),
        ]);
        assert_eq!(merged[0].replacement.as_deref(), Some("examine"));
    }

    #[test]
    fn evidence_concatenates_and_dedupes() {
        let merged = merge_vocab(vec![
            raw("delve", "anti-ai-slop", None),
            raw("delve", "anti-ai-slop", None),
            raw("delve", "anti-ai-tell", None),
        ]);
        assert_eq!(merged[0].evidence.len(), 2);
    }

    #[test]
    fn short_terms_are_dropped() {
        let merged = merge_vocab(vec![raw("ok", "anti-ai-slop", None)]);
        assert!(merged.is_empty());
    }
}
