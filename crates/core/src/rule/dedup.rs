//! Load-time compile stage: deduplicate the parsed rule set.
//!
//! Vocab and literal-ban terms get ONE owner each - the highest-severity
//! claim wins, config order breaks same-tier ties (the first claim seen in
//! group/entry order keeps the term; only a strictly lower tier number
//! (higher severity) steals it).
//! Loser terms are dropped from the loser's matcher; an entry that loses
//! every term disappears, and an emptied group goes with it. Every drop is
//! recorded so the CLI can print a `dedup:` line naming winner and loser.
//!
//! Patterns are never dropped here: identical regex strings fan out to every
//! owning rule at scan time (the scanner groups them by `Regex::as_str()`).
//! Metrics deduplicate on (stat, window, normalized terms, direction); the
//! STRICTest threshold survives within a direction so a config conflict
//! cannot weaken a check. Opposite directions on one key are different
//! predicates (fire above vs fire below) and both survive.

use std::collections::HashMap;

use crate::rule::{RuleGroup, RuleSet, fixtures::Matcher};

/// Metric dedup identity: (stat, window, sorted normalized terms, direction).
type MetricKey = (String, String, Vec<String>, u8);

/// One dropped claim, for `dedup:` diagnostics.
struct DroppedClaim {
    /// The losing entry id (`GROUP#slug`).
    loser_id: String,
    /// The winner's id.
    winner_id: String,
    /// The contested normalized term.
    term: String,
}

/// Who owns a term: group index (for matcher pruning) + identity for logs.
#[derive(Clone)]
struct Claim {
    group_idx: usize,
    entry_id: String,
    tier: u8,
}

/// A surviving metric claim: (group index, threshold, id-base).
type MetricClaim = (usize, crate::rule::MetricThreshold, String);

/// A surviving repetition claim: (group index, threshold, id-base).
type RepetitionClaim = (usize, f64, String);

/// Stable direction tag for the dedup key (deterministic ordering).
fn direction(t: crate::rule::MetricThreshold) -> u8 {
    match t {
        crate::rule::MetricThreshold::AtLeast(_) => 0,
        crate::rule::MetricThreshold::AtMost(_) => 1,
    }
}

/// Human word for the fire direction, for `dedup:` diagnostics.
fn direction_word(t: crate::rule::MetricThreshold) -> &'static str {
    match t {
        crate::rule::MetricThreshold::AtLeast(_) => "above",
        crate::rule::MetricThreshold::AtMost(_) => "below",
    }
}

/// Deduplicate `rules` in place; returns human-readable diagnostics lines.
pub fn dedup(rules: &mut RuleSet) -> Vec<String> {
    let mut warnings = Vec::new();
    let mut dropped: Vec<DroppedClaim> = Vec::new();

    let mut vocab_owners: HashMap<String, Claim> = HashMap::new();
    let mut literal_owners: HashMap<String, Claim> = HashMap::new();

    for (gi, group) in rules.groups.iter().enumerate() {
        let is_vocab = group.kind == "vocab";
        let is_literal = group.kind == "literal-ban";
        if !is_vocab && !is_literal {
            continue;
        }
        for entry in &group.entries {
            let (terms, owners): (Vec<String>, &mut HashMap<String, Claim>) = match &entry.matcher {
                Matcher::Vocab { terms, .. } => (terms.clone(), &mut vocab_owners),
                Matcher::Literal { needles } => (needles.clone(), &mut literal_owners),
                _ => continue,
            };
            for term in terms {
                match owners.get(&term) {
                    None => {
                        owners.insert(
                            term,
                            Claim {
                                group_idx: gi,
                                entry_id: entry.id.clone(),
                                tier: group.tier,
                            },
                        );
                    }
                    // Tier 1 (artifact) is the HIGHEST severity; a
                    // lower tier number always beats a higher one.
                    Some(existing) if group.tier < existing.tier => {
                        dropped.push(DroppedClaim {
                            loser_id: existing.entry_id.clone(),
                            winner_id: entry.id.clone(),
                            term: term.clone(),
                        });
                        owners.insert(
                            term,
                            Claim {
                                group_idx: gi,
                                entry_id: entry.id.clone(),
                                tier: group.tier,
                            },
                        );
                    }
                    Some(existing) => {
                        dropped.push(DroppedClaim {
                            loser_id: entry.id.clone(),
                            winner_id: existing.entry_id.clone(),
                            term: term.clone(),
                        });
                    }
                }
            }
        }
    }

    // Metric dedup: (stat, window, sorted terms, direction) -> strictest
    // threshold wins within one direction; opposite directions on the same
    // key are different predicates (fire above vs fire below) and both
    // survive. Value = (group index, threshold, id-base) of the survivor.
    let mut metric_owners: HashMap<MetricKey, MetricClaim> = HashMap::new();
    let mut remove_groups: Vec<usize> = Vec::new();
    for (gi, group) in rules.groups.iter().enumerate() {
        let Some(spec) = &group.metric else { continue };
        let key = (
            spec.stat.name().to_string(),
            spec.window.name().to_string(),
            {
                let mut t = spec.terms.clone();
                t.sort();
                t
            },
            direction(spec.threshold),
        );
        match metric_owners.get(&key).cloned() {
            Some((prev_gi, prev_threshold, prev_gid)) => {
                if spec.threshold.is_stricter_than(prev_threshold) {
                    // Strictest wins: this group supersedes the previous owner.
                    warnings.push(format!(
                        "dedup: metric conflict on {}/{} - {}/{} (fires {} {}) supersedes {}/{} (fires {} {})",
                        key.0, key.1,
                        group.id_base, key.0, direction_word(spec.threshold), spec.threshold.value(),
                        prev_gid, key.0, direction_word(prev_threshold), prev_threshold.value(),
                    ));
                    metric_owners.insert(key, (gi, spec.threshold, group.id_base.clone()));
                    remove_groups.push(prev_gi);
                } else {
                    warnings.push(format!(
                        "dedup: metric conflict on {}/{} - {}/{} (fires {} {}) dropped; {}/{} keeps the stricter fires {} {}",
                        key.0, key.1,
                        group.id_base, key.0, direction_word(spec.threshold), spec.threshold.value(),
                        prev_gid, key.0, direction_word(prev_threshold), prev_threshold.value(),
                    ));
                    remove_groups.push(gi);
                }
            }
            None => {
                metric_owners.insert(key, (gi, spec.threshold, group.id_base.clone()));
            }
        }
    }

    // Repetition dedup: one owner per VARIANT (each variant is a different
    // detector; two groups running the same detection is a conflict). The
    // STRICTEST (highest) threshold survives — a higher similarity cutoff
    // fires on fewer, more-similar clusters. Config order breaks ties.
    let mut repetition_owners: HashMap<String, RepetitionClaim> = HashMap::new();
    for (gi, group) in rules.groups.iter().enumerate() {
        let Some(spec) = &group.repetition else {
            continue;
        };
        let key = spec.variant.name().to_string();
        match repetition_owners.get(&key).cloned() {
            Some((prev_gi, prev_threshold, prev_gid)) => {
                if spec.threshold > prev_threshold {
                    warnings.push(format!(
                        "dedup: repetition conflict on {} - {}/{} (threshold {}) supersedes {}/{} (threshold {})",
                        key, group.id_base, key, spec.threshold, prev_gid, key, prev_threshold,
                    ));
                    repetition_owners.insert(key, (gi, spec.threshold, group.id_base.clone()));
                    remove_groups.push(prev_gi);
                } else {
                    warnings.push(format!(
                        "dedup: repetition conflict on {} - {}/{} (threshold {}) dropped; {}/{} keeps the stricter threshold {}",
                        key, group.id_base, key, spec.threshold, prev_gid, key, prev_threshold,
                    ));
                    remove_groups.push(gi);
                }
            }
            None => {
                repetition_owners.insert(key, (gi, spec.threshold, group.id_base.clone()));
            }
        }
    }

    // Prune losing terms from matchers; drop emptied entries and groups.
    let mut owners_by_group: Vec<HashMap<String, Claim>> = Vec::new();
    owners_by_group.resize_with(rules.groups.len(), HashMap::new);
    for owners in [&vocab_owners, &literal_owners] {
        for (term, claim) in owners {
            owners_by_group[claim.group_idx].insert(term.clone(), claim.clone());
        }
    }

    for (gi, group) in rules.groups.iter_mut().enumerate() {
        if group.kind != "vocab" && group.kind != "literal-ban" {
            continue;
        }
        let owners = &owners_by_group[gi];
        group.entries.retain_mut(
            |entry: &mut crate::rule::ActiveEntry| match &mut entry.matcher {
                Matcher::Vocab { terms, .. } => {
                    terms.retain(|t| owners.get(t).is_some_and(|c| c.entry_id == entry.id));
                    !terms.is_empty()
                }
                Matcher::Literal { needles } => {
                    needles.retain(|t| owners.get(t).is_some_and(|c| c.entry_id == entry.id));
                    !needles.is_empty()
                }
                _ => true,
            },
        );
    }

    // Remove conflicted metric groups, then emptied vocab/literal groups.
    remove_groups.sort_unstable();
    remove_groups.dedup();
    for gi in remove_groups.into_iter().rev() {
        rules.groups.remove(gi);
    }
    rules.groups.retain(|g: &RuleGroup| {
        g.metric.is_some() || g.repetition.is_some() || !g.entries.is_empty()
    });

    // Deterministic diagnostics.
    dropped.sort_by(|a, b| a.loser_id.cmp(&b.loser_id).then(a.term.cmp(&b.term)));
    for d in &dropped {
        warnings.push(format!(
            "dedup: `{}` owned by {} - dropped duplicate claim from {}",
            d.term, d.winner_id, d.loser_id
        ));
    }
    warnings
}
