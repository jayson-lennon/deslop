//! Fixture evaluation: rules prove they work before they may load.
//!
//! A miniature, masking-free matcher (scope = anywhere) shared with the
//! scanner primitives; every enabled group's `must_match` / `must_not_match`
//! strings are run through it at load time.

use crate::rule::schema::{EntryToml, GroupToml};

/// Compiled per-entry matcher.
pub enum Matcher {
    /// Any term present as a whole-word, case-insensitive match.
    Vocab {
        terms: Vec<String>,
        word_boundary: bool,
    },
    /// Regex engine match.
    Pattern(fancy_regex::Regex),
    /// Plain case-insensitive substring hunt.
    Literal { needles: Vec<String> },
}

impl Clone for Matcher {
    fn clone(&self) -> Self {
        match self {
            Matcher::Vocab {
                terms,
                word_boundary,
            } => Matcher::Vocab {
                terms: terms.clone(),
                word_boundary: *word_boundary,
            },
            Matcher::Pattern(re) => {
                Matcher::Pattern(fancy_regex::Regex::new(re.as_str()).unwrap_or_else(|_| {
                    // Compiled once already; unreachable in practice.
                    fancy_regex::Regex::new(r"(?!)").expect("never fails")
                }))
            }
            Matcher::Literal { needles } => Matcher::Literal {
                needles: needles.clone(),
            },
        }
    }
}

impl std::fmt::Debug for Matcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Matcher::Vocab { terms, .. } => write!(f, "Vocab({terms:?})"),
            Matcher::Pattern(_) => write!(f, "Pattern(<compiled>)"),
            Matcher::Literal { needles } => write!(
                f,
                "Literal({needies_len} needles)",
                needies_len = needles.len()
            ),
        }
    }
}

impl Matcher {
    /// Build from an entry within its group kind context.
    ///
    /// Metric kinds carry their logic at GROUP level; a sentinel matcher is
    /// returned so loaders can treat metric groups uniformly.
    ///
    /// # Errors
    ///
    /// Pattern entries whose regex fails to compile return the engine error.
    pub fn build(kind: &str, entry: &EntryToml) -> Result<Matcher, String> {
        match kind {
            "metric" => Ok(Matcher::Literal {
                needles: Vec::new(),
            }),
            "pattern" => {
                let source = entry
                    .regex
                    .as_deref()
                    .ok_or_else(|| "pattern entry missing `regex`".to_string())?;
                let re = fancy_regex::Regex::new(source)
                    .map_err(|e| format!("regex `{source}`: {e}"))?;
                Ok(Matcher::Pattern(re))
            }
            "literal-ban" => {
                // Compile BEFORE any casing: `{N}` tokens are case-sensitive;
                // matching itself stays case-insensitive at find time.
                let mut needles = Vec::with_capacity(entry.terms.len());
                for term in &entry.terms {
                    if crate::rule::literals::compile(term).is_err() {
                        return Err(format!("bad literal-ban term {term:?}"));
                    }
                    needles.push(term.clone());
                }
                Ok(Matcher::Literal { needles })
            }
            // vocab (and the tolerant default) share word-list semantics;
            // stems=true mechanically expands inflections here so every
            // consumer (fixtures, scanner, use-mention dict) sees one set.
            _ => {
                let mut terms = Vec::new();
                for term in &entry.terms {
                    terms.push(term.to_lowercase());
                    if entry.stems && term.chars().count() >= 3 {
                        for form in crate::rule::stems::expand(term) {
                            terms.push(form.to_lowercase());
                        }
                    }
                }
                terms.sort();
                terms.dedup();
                Ok(Matcher::Vocab {
                    terms,
                    word_boundary: entry.word_boundary.unwrap_or(true),
                })
            }
        }
    }

    /// Does this matcher hit anywhere in `text`?
    pub fn matches(&self, text: &str) -> bool {
        match self {
            Matcher::Pattern(re) => re.is_match(text).unwrap_or(false),
            Matcher::Literal { needles } => {
                // Segment-aware: honors {N} digit runs; case-insensitive via
                // find()'s internal lowercasing. Uncompilable terms were
                // rejected at build, so compile().ok() is safe here.
                needles.iter().any(|n| {
                    crate::rule::literals::compile(n)
                        .map(|segs| crate::rule::literals::find(text, &segs).is_some())
                        .unwrap_or(false)
                })
            }
            Matcher::Vocab {
                terms,
                word_boundary,
            } => {
                let lower = text.to_lowercase();
                terms.iter().any(|term| {
                    if *word_boundary {
                        word_boundary_contains(&lower, term)
                    } else {
                        lower.contains(term)
                    }
                })
            }
        }
    }
}

/// Whole-word containment (ASCII letter/digit boundaries).
fn word_boundary_contains(haystack_lower: &str, needle_lower: &str) -> bool {
    let mut start = 0;
    while let Some(pos) = haystack_lower[start..].find(needle_lower) {
        let abs = start + pos;
        let end = abs + needle_lower.len();
        let before_ok = haystack_lower[..abs]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
        let after_ok = haystack_lower[end..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
        if before_ok && after_ok {
            return true;
        }
        start = abs + 1;
    }
    false
}

/// Failure of one fixture string for one entry.
#[derive(Debug)]
pub struct FixtureFailure {
    pub slug: String,
    pub fixture: String,
    pub problem: FixtureProblem,
}

#[derive(Debug)]
pub enum FixtureProblem {
    /// A must_match string produced no hit.
    MissedPositive,
    /// A must_not_match string produced a hit.
    FalsePositive,
}

impl std::fmt::Display for FixtureProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FixtureProblem::MissedPositive => {
                write!(f, "must_match sample did not match")
            }
            FixtureProblem::FalsePositive => {
                write!(f, "must_not_match sample unexpectedly matched")
            }
        }
    }
}

/// Evaluate every entry of `group` against its fixtures.
///
/// Entries are matched independently so a failure names the exact slug;
/// a group passes only when ALL entries hit all positives and none of the
/// negatives.
pub fn evaluate(group: &GroupToml) -> Vec<FixtureFailure> {
    let mut failures = Vec::new();
    for entry in &group.entries {
        let slug = entry
            .slug
            .clone()
            .or_else(|| entry.id.clone())
            .unwrap_or_else(|| "<unnamed>".into());
        let matcher = match Matcher::build(&group.kind, entry) {
            Ok(m) => m,
            Err(_) => continue, // compile errors surface in the loader
        };
        for positive in &group.fixtures.must_match {
            if !matcher.matches(positive) {
                failures.push(FixtureFailure {
                    slug: slug.clone(),
                    fixture: positive.clone(),
                    problem: FixtureProblem::MissedPositive,
                });
            }
        }
        for negative in &group.fixtures.must_not_match {
            if matcher.matches(negative) {
                failures.push(FixtureFailure {
                    slug: slug.clone(),
                    fixture: negative.clone(),
                    problem: FixtureProblem::FalsePositive,
                });
            }
        }
    }
    failures
}
