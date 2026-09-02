//! Document-level metrics + the closed STAT registry.
//!
//! One submodule per stat under `scanner/metrics/`; each exposes a
//! `measure()` that fills its field of [`DocStats`]. The coordinator
//! [`compute()`] runs them all in one pass over the prepared document -
//! adding a stat means a new submodule, a `DocStats` field, and one
//! delegate line here.
//!
//! Stats compute over the MASKED, USE-MENTION-CLEANED text so code, URLs
//! and quoted mentions never pollute the numbers. Floors prevent small
//! docs from producing explosive nonsense rates; they live in
//! [`DocStats::get`], next to the values they guard.

use icu_segmenter::SentenceSegmenter;
use icu_segmenter::options::SentenceBreakInvariantOptions;

mod anchors;
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

/// Canonical stat identifiers (closed set - adding one is a deliberate PR).
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
    let mut stats = DocStats {
        word_count: words_count(inputs.prose),
        ..Default::default()
    };

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

pub use term_cluster_max::{ClusterWindowHit, first_words, windows as cluster_windows};

/// Abbreviations the Unicode rules miss: each ends in a dot that the
/// segmenter reads as a sentence end before a capitalized next word.
/// Post-segment, spans whose last word is one of these are merged with
/// their successor (iteratively - `e.g. e.g. foo` chains).
const SENTENCE_GUARDS: [&str; 6] = ["e.g", "i.e", "Dr", "vs", "etc", "Mr"];

fn last_word_is_guard(span_text: &str) -> bool {
    let last_word = span_text
        .split_whitespace()
        .next_back()
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_alphanumeric());
    SENTENCE_GUARDS
        .iter()
        .any(|g| last_word.trim_end_matches('.').eq_ignore_ascii_case(g))
}

/// Sentence segmentation over Unicode sentence boundaries, shared by the
/// sentence-window stats and the cluster windows.
///
/// A lone newline in prose is a SOFT wrap (markdown renders it as a
/// space), so segmentation runs over a joined view where lone newlines
/// become spaces; newlines adjacent to another newline are paragraph
/// breaks and stay. This keeps wrapped lines inside one sentence while
/// blank lines still end one — a paragraph break can NEVER merge two
/// sentences into a single window.
///
/// Returns `(start, end)` byte spans into `prose` (view offsets are
/// remapped back). Spans carry trailing whitespace (e.g. `"Okay. "`),
/// and zero-word segments (the blank line between paragraphs arrives as
/// its own segment) are dropped. `Dr. Smith` stays one sentence via the
/// guard merge.
pub(crate) fn sentences(prose: &str) -> Vec<(usize, usize)> {
    // Soft-wrap join: view text plus view-byte -> prose-byte map.
    let mut view = String::with_capacity(prose.len());
    let mut map: Vec<usize> = Vec::with_capacity(prose.len() + 1);
    let mut prev_newline = false;
    for (i, c) in prose.char_indices() {
        let next_newline = prose[i + c.len_utf8()..].starts_with('\n');
        let keep_newline = c == '\n' && (prev_newline || next_newline);
        match keep_newline {
            true => view.push('\n'),
            false if c == '\n' => view.push(' '),
            false => view.push(c),
        }
        for _ in 0..c.len_utf8() {
            map.push(i);
        }
        prev_newline = c == '\n';
    }
    map.push(prose.len());

    // Const-built from baked data (a cheap Copy of static refs); the
    // segmenter holds no per-document state and needs no caching.
    let segmenter = SentenceSegmenter::new(SentenceBreakInvariantOptions::default());
    // Segmenter boundaries: byte offsets into the VIEW, starting at 0.
    let mut bounds: Vec<usize> = segmenter.segment_str(&view).collect();
    if bounds.first() != Some(&0) {
        bounds.insert(0, 0);
    }
    if *bounds.last().unwrap_or(&0) != view.len() {
        bounds.push(view.len());
    }

    // Consecutive boundary pairs, minus zero-word segments (spaces in the
    // view are whitespace, so word counts match the prose slices) and with
    // trailing paragraph-break whitespace trimmed off each span.
    let units: Vec<(usize, usize)> = bounds
        .windows(2)
        .map(|w| (w[0], w[1]))
        .filter(|(s, e)| words_count(&view[*s..*e]) > 0)
        .map(|(s, e)| (s, s + view[s..e].trim_end().len()))
        .collect();

    // Guard-merge: an abbreviation-final span rejoins its successor,
    // repeating until no merged span itself ends in a guard.
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(units.len());
    for (start, end) in units {
        match out.last_mut() {
            Some(prev) if last_word_is_guard(&view[prev.0..prev.1]) => prev.1 = end,
            _ => out.push((start, end)),
        }
    }
    // Remap view coordinates back to prose coordinates.
    out.iter_mut().for_each(|(s, e)| {
        *s = map[*s];
        *e = map[*e];
    });
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

/// Where a whole-doc metric finding anchors: the first occurrence of the
/// stat's underlying signal (first curly quote, first Title Case heading,
/// ...), so the caret sits on evidence instead of byte 0.
pub(crate) fn first_signal_span(
    stat: Stat,
    text: &str,
    bold_spans: &[(usize, usize)],
    heading_ranges: &[(usize, usize)],
    list_items: &[(usize, usize)],
) -> (usize, usize) {
    anchors::first_signal_span(stat, text, bold_spans, heading_ranges, list_items)
}

/// Shared test scaffolding for the per-stat submodules.
#[cfg(test)]
pub(crate) mod testutil {
    use super::Inputs;

    /// Inputs with no structural ranges - prose-only docs.
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
    fn blank_line_breaks_sentences() {
        // Given two sentences separated by a blank line.
        let doc = "One two three.\n\nMembers arrive here.";

        // When segmenting.
        let got = sentences(doc);

        // Then the sentences are separate spans, not one merged chunk.
        assert_eq!(got.len(), 2);
        assert_eq!(&doc[got[0].0..got[0].1], "One two three.");
        assert_eq!(&doc[got[1].0..got[1].1], "Members arrive here.");
    }

    #[test]
    fn guard_merges_across_segments() {
        // Given "Mr." at a segment boundary (a UAX #29 miss).
        let doc = "Mr. Jones left.";

        // When segmenting.
        let got = sentences(doc);

        // Then the guard merges the segments into one sentence.
        assert_eq!(got.len(), 1);
        assert_eq!(&doc[got[0].0..got[0].1], doc);
    }

    #[test]
    fn decimal_and_abbrev_do_not_split() {
        // Given a decimal and an example abbreviation.
        let doc = "It cost $5.50 total. E.g. this.";

        // When segmenting.
        let got = sentences(doc);

        // Then the decimal stays inside one span.
        assert_eq!(got.len(), 2);
        assert_eq!(&doc[got[0].0..got[0].1], "It cost $5.50 total.");
        // And the abbreviation does not start a new sentence.
        assert_eq!(&doc[got[1].0..got[1].1], "E.g. this.");
    }

    #[test]
    fn zero_word_segments_are_dropped() {
        // Given a sentence followed only by blank lines.
        let doc = "Ends here.\n\n";

        // When segmenting.
        let got = sentences(doc);

        // Then no trailing zero-word span is emitted.
        assert_eq!(got.len(), 1);
        assert_eq!(&doc[got[0].0..got[0].1], "Ends here.");
    }

    #[test]
    fn lowercase_continuation_does_not_split() {
        // Given a lowercase continuation after a period (SB8 suppression).
        let doc = "Ends with etc. then lowercase continues. Next real one.";

        // When segmenting.
        let got = sentences(doc);

        // Then the abbreviation + lowercase continuation stays one sentence,
        // and the capitalized sentence after it separates.
        assert_eq!(got.len(), 2);
        assert_eq!(
            &doc[got[0].0..got[0].1],
            "Ends with etc. then lowercase continues."
        );
        assert_eq!(&doc[got[1].0..got[1].1], "Next real one.");
    }

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
