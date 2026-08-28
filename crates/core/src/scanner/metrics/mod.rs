//! Document-level metrics + the closed STAT registry.
//!
//! One submodule per stat under `scanner/metrics/`; each exposes a
//! `measure()` that fills its field of [`DocStats`]. The coordinator
//! [`compute()`] runs them all in one pass over the prepared document —
//! adding a stat means a new submodule, a `DocStats` field, and one
//! delegate line here.
//!
//! Stats compute over the MASKED, USE-MENTION-CLEANED text so code, URLs
//! and quoted mentions never pollute the numbers. Floors prevent small
//! docs from producing explosive nonsense rates; they live in
//! [`DocStats::get`], next to the values they guard.

mod bold_density;
mod bullet_boldlead;
mod curly_double_ratio;
mod em_dash_rate;
mod emoji_decoration;
mod heading_titlecase;
mod opening_ngram_repeat;
mod sent_len_cv;
mod term_cluster_max;
mod tricolon_streak;

/// Canonical stat identifiers (closed set — adding one is a deliberate PR).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stat {
    EmDashRate,
    CurlyDoubleRatio,
    BoldDensity,
    HeadingTitlecaseFraction,
    EmojiDecorationCount,
    BulletBoldleadFraction,
    TricolonMaxStreak,
    SentLenCv,
    OpeningNgramRepeat,
    /// Max DISTINCT cluster terms found in one window (paragraph default).
    TermClusterMax,
}

impl Stat {
    /// Parse a TOML `stat` string; unknown names are loader errors upstream.
    pub fn parse(name: &str) -> Option<Stat> {
        Some(match name {
            "em_dash_rate" => Stat::EmDashRate,
            "curly_double_ratio" => Stat::CurlyDoubleRatio,
            "bold_density" => Stat::BoldDensity,
            "heading_titlecase_fraction" => Stat::HeadingTitlecaseFraction,
            "emoji_decoration_count" => Stat::EmojiDecorationCount,
            "bullet_boldlead_fraction" => Stat::BulletBoldleadFraction,
            "tricolon_max_streak" => Stat::TricolonMaxStreak,
            "sent_len_cv" => Stat::SentLenCv,
            "opening_ngram_repeat" => Stat::OpeningNgramRepeat,
            "term_cluster_max" => Stat::TermClusterMax,
            _ => return None,
        })
    }

    pub fn name(&self) -> &'static str {
        match self {
            Stat::EmDashRate => "em_dash_rate",
            Stat::CurlyDoubleRatio => "curly_double_ratio",
            Stat::BoldDensity => "bold_density",
            Stat::HeadingTitlecaseFraction => "heading_titlecase_fraction",
            Stat::EmojiDecorationCount => "emoji_decoration_count",
            Stat::BulletBoldleadFraction => "bullet_boldlead_fraction",
            Stat::TricolonMaxStreak => "tricolon_max_streak",
            Stat::SentLenCv => "sent_len_cv",
            Stat::OpeningNgramRepeat => "opening_ngram_repeat",
            Stat::TermClusterMax => "term_cluster_max",
        }
    }
}

/// Computed values for one document.
#[derive(Debug, Clone, Default)]
pub struct DocStats {
    pub word_count: usize,
    pub bullet_count: usize,
    pub sentence_count: usize,
    pub em_dash_rate: f64,
    pub curly_double_ratio: f64,
    pub bold_density: f64,
    pub heading_titlecase_fraction: f64,
    pub emoji_decoration_count: usize,
    pub bullet_boldlead_fraction: f64,
    pub tricolon_max_streak: usize,
    pub sent_len_cv: f64,
    pub opening_ngram_repeat: usize,
}

impl DocStats {
    /// Value for a stat; `None` = floor not met (silence, per spec).
    pub fn get(&self, stat: Stat) -> Option<f64> {
        match stat {
            Stat::EmDashRate if self.word_count >= 250 => Some(self.em_dash_rate),
            Stat::CurlyDoubleRatio if self.word_count >= 250 => Some(self.curly_double_ratio),
            Stat::BoldDensity if self.word_count >= 250 => Some(self.bold_density),
            Stat::HeadingTitlecaseFraction if self.word_count >= 250 => {
                Some(self.heading_titlecase_fraction)
            }
            Stat::EmojiDecorationCount => Some(self.emoji_decoration_count as f64),
            Stat::BulletBoldleadFraction if self.bullets() >= 4 => {
                Some(self.bullet_boldlead_fraction)
            }
            Stat::TricolonMaxStreak if self.word_count >= 250 => {
                Some(self.tricolon_max_streak as f64)
            }
            Stat::SentLenCv if self.sentences() >= 6 => Some(self.sent_len_cv),
            Stat::OpeningNgramRepeat if self.word_count >= 250 => {
                Some(self.opening_ngram_repeat as f64)
            }
            _ => None,
        }
    }

    fn bullets(&self) -> usize {
        self.bullet_count
    }

    fn sentences(&self) -> usize {
        self.sentence_count
    }
}

/// Where the densest window's term hit sits in the text (rendering anchor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClusterHit {
    /// Byte offset of the matching term.
    pub start: usize,
    /// Byte offset just past the matching term.
    pub end: usize,
}

/// Prepared per-document inputs: the visible prose plus the structural
/// ranges the region map extracted.
pub struct Inputs<'a> {
    pub prose: &'a str,
    pub heading_ranges: &'a [(usize, usize)],
    pub bold_spans: &'a [(usize, usize)],
    pub list_items: &'a [(usize, usize)],
}

/// Compute every stat. Deterministic; one pass, each stat in its submodule.
pub fn compute(inputs: &Inputs<'_>) -> DocStats {
    let mut stats = DocStats::default();
    stats.word_count = words_count(inputs.prose);

    em_dash_rate::measure(inputs.prose, &mut stats);
    curly_double_ratio::measure(inputs.prose, &mut stats);
    bold_density::measure(inputs, &mut stats);
    heading_titlecase::measure(inputs, &mut stats);
    emoji_decoration::measure(inputs, &mut stats);
    bullet_boldlead::measure(inputs, &mut stats);
    tricolon_streak::measure(inputs, &mut stats);
    sent_len_cv::measure(inputs, &mut stats);
    opening_ngram_repeat::measure(inputs, &mut stats);
    stats
}

pub use term_cluster_max::measure as term_cluster_max;

/// Sentence splitting with abbreviation guard. Shared: several stats and
/// the cluster windows all need the same segmentation.
pub(crate) fn sentences(prose: &str) -> Vec<(usize, usize)> {
    const GUARDS: [&str; 6] = ["e.g", "i.e", "Dr", "vs", "etc", "Mr"];
    let mut out = Vec::new();
    let mut start = 0usize;
    let bytes = prose.as_bytes();
    let mut i = 0;
    while i < prose.len() {
        if matches!(bytes[i], b'.' | b'!' | b'?') {
            // Consume runs like "!!!" / "..."
            let mut j = i + 1;
            while j < prose.len() && matches!(bytes[j], b'.' | b'!' | b'?') {
                j += 1;
            }
            // Guard: abbreviation like "e.g." — inspect only the final
            // word fragment before the dot (bounded, no allocation).
            let mut win_start = i.saturating_sub(12).max(start);
            while win_start < i && !prose.is_char_boundary(win_start) {
                win_start -= 1;
            }
            let last_word: String = prose[win_start..i]
                .chars()
                .rev()
                .take_while(|c| c.is_alphanumeric() || *c == '.')
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let is_guard = GUARDS.iter().any(|g| {
                let g = g.trim_end_matches('.');
                last_word.eq_ignore_ascii_case(g)
                    || last_word.eq_ignore_ascii_case(&format!("{g}."))
                    || last_word.to_lowercase().ends_with(&format!(".{g}"))
            });
            if !is_guard
                && bytes
                    .get(j)
                    .is_none_or(|&b| matches!(b, b'\n' | b' ' | b'-' | b')' | b'"' | b'\'' | b']'))
                && bytes.get(j + 1).is_none_or(|&b| b.is_ascii_alphabetic())
            {
                out.push((start, j));
                start = j;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    if start < prose.len() {
        out.push((start, prose.len()));
    }
    out
}

/// Whitespace-split word count over alphanumeric-bearing words.
pub(crate) fn words_count(prose: &str) -> usize {
    prose
        .split(|c: char| c.is_whitespace())
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count()
}

/// Prose text with code/inline-code stripped: the visible (non-NUL) runs
/// of the masked buffer, joined. Returns (byte span inside `text`, joined
/// prose). Newlines between runs are preserved so sentence math still works.
pub fn visible_prose(
    text: &str,
    map: &crate::scanner::regions::RegionMap,
) -> Option<(usize, String)> {
    let _ = text;
    let bytes = map.masked.as_bytes();
    if bytes.is_empty() {
        return Some((0, String::new()));
    }
    // Build a keep/drop filter; copy kept bytes verbatim.
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            0 => {
                // Masked hole: keep structural newlines only.
                while i < bytes.len() && (bytes[i] == 0 || bytes[i] == b'\n') {
                    if bytes[i] == b'\n' {
                        out.push(b'\n');
                    } else {
                        out.push(b' ');
                    }
                    i += 1;
                }
            }
            _ => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    let prose = String::from_utf8_lossy(&out).into_owned();
    Some((0, prose))
}

/// Ranges of one scope kind (headings etc.) as sorted pairs.
pub fn scope_ranges(
    map: &crate::scanner::regions::RegionMap,
    pred: impl Fn(&crate::scanner::regions::Scope) -> bool,
) -> Vec<(usize, usize)> {
    map.scopes
        .iter()
        .filter(|(_, _, sc)| pred(sc))
        .map(|(s, e, _)| (*s, *e))
        .collect()
}

/// Strong-emphasis spans, re-derived from the masked text (`**...**` runs);
/// the region map does not track emphasis separately.
pub fn bold_ranges(map: &crate::scanner::regions::RegionMap) -> Vec<(usize, usize)> {
    let masked = &map.masked;
    let mut out = Vec::new();
    let bytes = masked.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'*' && bytes[i + 1] == b'*' {
            if let Some(rel) = masked[i + 2..].find("**") {
                out.push((i, i + 2 + rel));
                i += 2 + rel + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

pub fn list_item_ranges(map: &crate::scanner::regions::RegionMap) -> Vec<(usize, usize)> {
    scope_ranges(map, |sc| {
        matches!(sc, crate::scanner::regions::Scope::ListItem)
    })
}

/// Where a metric finding anchors: at the densest spot of its signal when
/// meaningful, else document start.
#[allow(clippy::match_single_binding)]
pub fn anchor_for(stat: Stat, _text: &str, _map: &crate::scanner::regions::RegionMap) -> usize {
    match stat {
        // A zero-width anchor renders an empty span; diagnostics prefer a
        // caret on something real, so anchor line starts suffice. Keep 0
        // for all stats v1 (T9 says "near densest cluster" — refine in the
        // phase-6 triage window).
        _ => 0,
    }
}

/// Shared test scaffolding for the per-stat submodules.
#[cfg(test)]
pub(crate) mod testutil {
    use super::Inputs;

    /// Inputs with no structural ranges — prose-only docs.
    pub fn inputs(prose: &str) -> Inputs<'_> {
        Inputs {
            prose,
            heading_ranges: &[],
            bold_spans: &[],
            list_items: &[],
        }
    }

    /// Word count consistent with the coordinator's definition.
    pub fn word_count(prose: &str) -> usize {
        super::words_count(prose)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compute_fills_word_count_and_coordinator_fields() {
        // Given a plain prose document.
        let doc = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu";

        // When computing.
        let s = compute(&testutil::inputs(doc));

        // Then the word count matches and every stat field materialized.
        assert_eq!(s.word_count, 12);
        assert_eq!(s.tricolon_max_streak, 0);
        assert_eq!(s.opening_ngram_repeat, 1);
        assert_eq!(s.bold_density, 0.0);
    }

    #[test]
    fn stat_registry_names_round_trip() {
        // Given every variant of the closed stat set.
        let all = [
            Stat::EmDashRate,
            Stat::CurlyDoubleRatio,
            Stat::BoldDensity,
            Stat::HeadingTitlecaseFraction,
            Stat::EmojiDecorationCount,
            Stat::BulletBoldleadFraction,
            Stat::TricolonMaxStreak,
            Stat::SentLenCv,
            Stat::OpeningNgramRepeat,
            Stat::TermClusterMax,
        ];

        // When parsing each name back.
        // Then every name round-trips and unknown names fail.
        for stat in all {
            assert_eq!(Stat::parse(stat.name()), Some(stat), "{}", stat.name());
        }
        assert_eq!(Stat::parse("bold-density"), None);
        assert_eq!(Stat::parse("vibes_per_paragraph"), None);
    }
}
