//! Document-level metrics + the closed STAT registry.
//!
//! Stats compute over the MASKED, USE-MENTION-CLEANED text so code, URLs and
//! quoted mentions never pollute the numbers. Floors prevent small docs from
//! producing explosive nonsense rates.

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

/// Anchor for a metric finding (densest spot approximation).
pub type Anchor = (usize, usize);

/// Max DISTINCT `terms` in any `window`, plus the norm-offset of that
/// window's first term hit (anchor). Windows are split on blank lines
/// (paragraph) or sentence enders (sentence); document = whole text.
pub fn term_cluster_max(
    masked: &str,
    terms: &[String],
    window: crate::rule::ClusterWindow,
) -> Option<(usize, usize)> {
    use crate::rule::ClusterWindow as W;
    if terms.is_empty() {
        return None;
    }
    // First term occurrence per term (norm offsets). Terms are ASCII
    // (sanitized at load), so byte scanning is char-boundary safe.
    let lower = masked.to_lowercase();
    let lb = lower.as_bytes();
    let mut hits: Vec<(usize, usize)> = Vec::new(); // (start, term_idx)
    for (ti, t) in terms.iter().enumerate() {
        if t.is_empty() {
            continue;
        }
        let mut from = 0usize;
        while let Some(rel) = lower[from..].find(t.as_str()) {
            let at = from + rel;
            let end = at + t.len();
            let before_ok = at == 0 || !lb[at - 1].is_ascii_alphanumeric();
            let after_ok = end >= lb.len() || !lb[end].is_ascii_alphanumeric();
            if before_ok && after_ok {
                hits.push((at, ti));
            }
            from = end.max(at + 1);
        }
    }
    // No hits is a valid measurement of zero (never exceeds a threshold).
    if hits.is_empty() {
        return Some((0, 0));
    }
    hits.sort_unstable();
    // Window boundaries as byte ranges.
    let bounds: Vec<usize> = match window {
        W::Document => vec![0, masked.len()],
        W::Paragraph => {
            let mut b = vec![0usize];
            let bytes = masked.as_bytes();
            for i in 0..bytes.len() {
                if bytes[i] == b'\n' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
                    b.push(i + 1);
                }
            }
            b.push(masked.len());
            b
        }
        W::Sentence => sentences(masked)
            .into_iter()
            .flat_map(|(s, e)| [s, e])
            .collect(),
    };
    // Distinct per window: walk hits, reset at each boundary.
    let mut best = 0usize;
    let mut best_at = hits[0].0;
    let mut bi = 0usize;
    let mut seen = std::collections::BTreeSet::new();
    for &(at, ti) in &hits {
        while bi + 1 < bounds.len() && bounds[bi + 1] <= at {
            bi += 1;
            seen.clear();
        }
        seen.insert(ti);
        if seen.len() > best {
            best = seen.len();
            best_at = at;
        }
    }
    Some((best, best_at))
}

/// Compute every stat over a prepared document.
///
/// `prose` is the masked text; `heading_ranges` are visible heading bodies;
/// `bold_spans` the strong-emphasis ranges; `list_item_starts` bullet starts.
pub struct Inputs<'a> {
    pub prose: &'a str,
    pub heading_ranges: &'a [(usize, usize)],
    pub bold_spans: &'a [(usize, usize)],
    pub list_items: &'a [(usize, usize)],
}

/// Sentence splitting with abbreviation guard.
fn sentences(prose: &str) -> Vec<(usize, usize)> {
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
            // Guard: abbreviation like "e.g." — check preceding word.
            let before = &prose[start..i];
            let words_before: Vec<&str> = before.rsplit(|c: char| !c.is_alphanumeric()).collect();
            let mut is_guard = false;
            if let Some(first) = words_before.first() {
                // Dotted abbreviations leave the last fragment ("g" of e.g.);
                // match either bare or with a preceding dotted component pair.
                let bare = first.eq_ignore_ascii_case("g")
                    || GUARDS.iter().any(|g| g.eq_ignore_ascii_case(first));
                let paired = words_before.len() >= 2
                    && format!("{}.{}", words_before[1], first).len() <= 5
                    && GUARDS.iter().any(|g| {
                        g.eq_ignore_ascii_case(&format!("{}.{}", words_before[1], first))
                            || g.replace('.', "")
                                .eq_ignore_ascii_case(&format!("{}{}", words_before[1], first))
                    });
                is_guard = bare || paired;
            }
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

fn words_count(prose: &str) -> usize {
    prose
        .split(|c: char| c.is_whitespace())
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count()
}

fn is_titlecase(text: &str) -> bool {
    let minor = ["of", "the", "and", "in", "on", "to", "a", "an", "for"];
    let words: Vec<&str> = text
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .collect();
    if words.len() < 3 {
        return false;
    }
    let content: Vec<&str> = words
        .iter()
        .skip(1)
        .filter(|w| !minor.contains(&w.to_lowercase().as_str()))
        .copied()
        .collect();
    if content.is_empty() {
        return false;
    }
    let capitalized = content
        .iter()
        .filter(|w| w.chars().next().is_some_and(|c| c.is_uppercase()))
        .count();
    capitalized * 100 >= content.len() * 80
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

/// Bold spans extracted from masked markers? The region map does not track
/// emphasis separately; re-derive from the ORIGINAL normalized text via a
/// lightweight pass is done here with regex over the plain text.
pub fn bold_ranges(map: &crate::scanner::regions::RegionMap) -> Vec<(usize, usize)> {
    bold_from_masked(&map.masked)
}

fn bold_from_masked(masked: &str) -> Vec<(usize, usize)> {
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

/// Compute stats. Deterministic; no allocation-heavy work.
pub fn compute(inputs: &Inputs<'_>) -> DocStats {
    let prose = inputs.prose;
    let wc = words_count(prose);
    let denom = wc.max(1) as f64;

    // Em dashes (spaced or attached count the same).
    let em_dashes = prose.matches('\u{2014}').count();
    let em_dash_rate = em_dashes as f64 * 1000.0 / denom;

    // Curly double-quote ratio vs straight doubles.
    let curly = prose.matches('\u{201C}').count() + prose.matches('\u{201D}').count();
    let straight = prose.matches('"').count();
    let curly_double_ratio = if curly + straight >= 20 {
        curly as f64 / (curly + straight).max(1) as f64
    } else {
        0.0
    };

    // Bold density per 100 words.
    let bold_density = inputs.bold_spans.len() as f64 * 100.0 / denom;

    // Heading title-case fraction.
    let heading_titlecase_fraction = if inputs.heading_ranges.is_empty() {
        0.0
    } else {
        let tc = inputs
            .heading_ranges
            .iter()
            .filter(|(s, e)| is_titlecase(&prose[*s..(*e).min(prose.len())]))
            .count();
        tc as f64 / inputs.heading_ranges.len() as f64
    };

    // Emoji decorations in headings / bullet leads.
    let emoji_decoration_count = emoji_count(prose, inputs);

    // Bullet bold-lead fraction.
    let bullets = inputs.list_items.len();
    let bold_leads = inputs
        .list_items
        .iter()
        .filter(|(s, _)| bold_lead_at(prose, inputs.bold_spans, *s))
        .count();
    let bullet_boldlead_fraction = if bullets >= 4 {
        bold_leads as f64 / bullets as f64
    } else {
        0.0
    };

    // Tricolon streak: "x, y, and z" occurrences in a row (adjacent commas-
    // free triple coordination); we track max consecutive sentences containing one.
    let tricolon_max_streak = tricolon_streak(prose);

    // Sentence length CV.
    let sents = sentences(prose);
    let lens: Vec<f64> = sents
        .iter()
        .map(|(s, e)| words_count(&prose[*s..*e]) as f64)
        .collect();
    let sent_len_cv = if lens.len() >= 6 && lens.iter().sum::<f64>() > 0.0 {
        let mean = lens.iter().sum::<f64>() / lens.len() as f64;
        let var = lens.iter().map(|l| (l - mean).powi(2)).sum::<f64>() / lens.len() as f64;
        if mean > 0.0 { var.sqrt() / mean } else { 0.0 }
    } else {
        0.0
    };

    // Opening bigram repeats (anaphora-ish).
    let opening_ngram_repeat = opening_bigram_max_repeat(prose, &sents);

    DocStats {
        word_count: wc,
        bullet_count: bullets,
        sentence_count: sents.len(),
        em_dash_rate,
        curly_double_ratio,
        bold_density,
        heading_titlecase_fraction,
        emoji_decoration_count,
        bullet_boldlead_fraction,
        tricolon_max_streak,
        sent_len_cv,
        opening_ngram_repeat,
    }
}

fn bold_lead_at(_prose: &str, bold_spans: &[(usize, usize)], item_start: usize) -> bool {
    bold_spans
        .iter()
        .any(|(s, _)| *s <= item_start + 3 && *s >= item_start.saturating_sub(1))
}

fn emoji_count(prose: &str, inputs: &Inputs<'_>) -> usize {
    let is_emoji = |c: char| {
        ('\u{1F300}'..='\u{1FAFF}').contains(&c) || ('\u{2600}'..='\u{27BF}').contains(&c)
    };
    inputs
        .heading_ranges
        .iter()
        .chain(inputs.list_items.iter())
        .filter_map(|(s, e)| prose.get(*s..(*e).min(prose.len())))
        .map(|seg| seg.chars().filter(|c| is_emoji(*c)).count())
        .sum()
}

fn tricolon_streak(prose: &str) -> usize {
    const TRICOLON: &str = r"\b[a-z]+, [a-z]+, and [a-z]+\b";
    let re = fancy_regex::Regex::new(TRICOLON).ok();
    let sents = sentences(prose);
    let mut streak = 0;
    let mut best = 0;
    for (s, e) in &sents {
        let hit = re
            .as_ref()
            .is_some_and(|re| re.is_match(&prose[*s..*e]).unwrap_or(false));
        if hit {
            streak += 1;
            best = best.max(streak);
        } else {
            streak = 0;
        }
    }
    best
}

fn opening_bigram_max_repeat(prose: &str, sents: &[(usize, usize)]) -> usize {
    let mut max_repeat = 0;
    let mut prev: Option<String> = None;
    let mut run = 0;
    for (s, e) in sents {
        let seg_end = (*e).min(prose.len());
        let first_words: Vec<&str> = prose[*s..seg_end.max(*s)]
            .split_whitespace()
            .take(2)
            .collect();
        if first_words.len() < 2 {
            prev = None;
            continue;
        }
        let bigram = first_words.join(" ").to_lowercase();
        match &prev {
            Some(p) if *p == bigram => {
                run += 1;
                max_repeat = max_repeat.max(run);
            }
            _ => {
                run = 1;
                max_repeat = max_repeat.max(1);
            }
        }
        prev = Some(bigram);
    }
    max_repeat
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(prose: &str) -> DocStats {
        // Harness treats everything up to the first period-run as one heading
        // only when the doc STARTS capitalized; else no headings (metrics that
        // need heading ranges set them explicitly in their own tests).
        let heading_ranges: Vec<(usize, usize)> = vec![(0, 0)]; // none by default
        let _ = prose;
        let empty: Vec<(usize, usize)> = Vec::new();
        compute(&Inputs {
            prose,
            heading_ranges: &heading_ranges,
            bold_spans: &empty,
            list_items: &empty,
        })
    }

    #[test]
    fn em_dash_rate_counts_per_thousand_words() {
        // Given a short doc with 2 em dashes and ~10 words (floor not met).
        let doc = "alpha beta gamma delta epsilon zeta eta theta iota — kappa — lambda";
        let s = stats(doc);

        // Then rate is computed but FLOORED OUT below 250 words.
        assert!(s.get(Stat::EmDashRate).is_none());
        // And the raw rate value is still stored.
        assert!(s.em_dash_rate > 0.0);
    }

    #[test]
    fn em_dash_rate_exposed_once_word_floor_met() {
        // Given >250 words with a few dashes.
        let filler = "word ".repeat(260);
        let doc = format!("{filler}— end — done");
        let doc = doc.as_str();
        let s = stats(doc);

        // When reading the stat.
        let v = s.get(Stat::EmDashRate).expect("above floor");

        // Then it equals count/words*1000 exactly.
        let expected = 2.0 * 1000.0 / s.word_count as f64;
        assert!((v - expected).abs() < 1e-9);
    }

    #[test]
    fn titlecase_heading_detected_above_floor() {
        // Given filler beyond the 250-word floor plus one Title Case heading
        // occupying the head range.
        let filler = "plain sentence words here and there in prose form. ".repeat(30);
        let heading_len = "Impact of Technology And Digitalization".len();
        let doc = format!(
            "{}\n\n{}",
            "Impact of Technology And Digitalization", filler
        );
        let empty: Vec<(usize, usize)> = Vec::new();
        let s = compute(&Inputs {
            prose: &doc,
            heading_ranges: &[(0, heading_len)],
            bold_spans: &empty,
            list_items: &empty,
        });

        // When checking the stat above the floor (filler pushes >250 words).
        let v = s.get(Stat::HeadingTitlecaseFraction).expect("over floor");

        // Then the heading qualifies as fully title-cased.
        assert!((v - 1.0).abs() < 1e-9);
    }

    #[test]
    fn sentence_cv_needs_six_sentences_minimum() {
        // Given five uniform sentences.
        let doc = "one two. three four. five six. seven eight. nine ten.";
        let s = stats(doc);

        // Then CV is floored out.
        assert!(s.get(Stat::SentLenCv).is_none());
        assert_eq!(s.sentence_count, 5);
    }

    #[test]
    fn abbreviation_guard_prevents_split() {
        // Given a sentence containing e.g. mid-flow.
        let doc = "use tools e.g. hammers. They work well today friend okay.";
        let sents = sentences(doc);

        // Then e.g. does NOT split; the trailing period does.
        assert_eq!(sents.len(), 2, "{sents:?}");
    }

    #[test]
    fn tricolon_streak_counts_consecutive() {
        // Given two adjacent sentences each containing a tricolon.
        let doc = "we ship speed, scale, and soul. we love rust, cargo, and crabs.";
        let s = stats(doc);

        // Then max streak is two even though floors mute the stat itself.
        assert_eq!(s.tricolon_max_streak, 2);
    }
}

#[cfg(test)]
mod cluster_tests {
    use super::term_cluster_max;
    use crate::rule::ClusterWindow;

    fn terms() -> Vec<String> {
        vec!["crucial".into(), "robust".into(), "notably".into()]
    }

    #[test]
    fn counts_distinct_terms_in_one_paragraph() {
        // Given one paragraph with three distinct watch terms.
        let text = "The crucial part is robust and notably quick.";
        // When measuring the cluster.
        let (n, _) = term_cluster_max(text, &terms(), ClusterWindow::Paragraph).unwrap();
        // Then all three distinct terms count.
        assert_eq!(n, 3);
    }

    #[test]
    fn repeated_same_term_counts_once() {
        // Given a paragraph repeating a single term.
        let text = "crucial crucial crucial crucial";
        // When measuring the cluster.
        let (n, _) = term_cluster_max(text, &terms(), ClusterWindow::Paragraph).unwrap();
        // Then only one distinct term counts.
        assert_eq!(n, 1);
    }

    #[test]
    fn separate_paragraphs_do_not_pool() {
        // Given terms spread across two paragraphs.
        let text = "crucial here.\n\nrobust and notably there";
        // When measuring per-paragraph.
        let (n, _) = term_cluster_max(text, &terms(), ClusterWindow::Paragraph).unwrap();
        // Then no paragraph exceeds two.
        assert_eq!(n, 2);
    }

    #[test]
    fn document_window_pools_everything() {
        // Given the same two-paragraph spread.
        let text = "crucial here.\n\nrobust and notably there";
        // When measuring over the whole document.
        let (n, _) = term_cluster_max(text, &terms(), ClusterWindow::Document).unwrap();
        // Then all three pool to three.
        assert_eq!(n, 3);
    }

    #[test]
    fn empty_terms_yields_none() {
        // Given no terms configured.
        // When measuring.
        let got = term_cluster_max("crucial", &[], ClusterWindow::Paragraph);
        // Then there is no measurement.
        assert!(got.is_none());
    }

    #[test]
    fn subsequence_words_do_not_match() {
        // Given text where terms appear only as substrings of other words.
        let text = "crucially robustly";
        // When measuring.
        let (n, _) = term_cluster_max(text, &terms(), ClusterWindow::Paragraph).unwrap();
        // Then nothing counts (space-padded whole-word match).
        assert_eq!(n, 0);
    }
}
