//! `term_cluster_max` - DISTINCT watch terms inside one window, per window.
//!
//! Identity is the LEMMA (`term_lemmas` parallel array), so `delve` and
//! `delves` never count as two. Windows: blank-line paragraphs, sentence
//! enders, or the whole document. [`windows`] returns EVERY window, each
//! with its own count, anchor span (final hit in that window) and the
//! trigger words in first-occurrence order for the finding's context line.

use crate::scanner::metrics::sentences;

/// One measured window: where it sits, what it counted.
pub struct ClusterWindowHit {
    /// Window byte range in the masked text.
    pub bounds: (usize, usize),
    /// DISTINCT lemma count inside the window.
    pub distinct: usize,
    /// Span of the window's LAST-counted hit (rendering anchor).
    pub last_hit: (usize, usize),
    /// Trigger words, deduped by lemma, first-occurrence order, surface
    /// form exactly as written in the text.
    pub terms_in_order: Vec<String>,
}

/// Measure every `window` in `masked` against the watched `terms`.
///
/// Returns one entry per window that contains at least one hit; windows
/// with zero hits are omitted (they can never exceed a threshold).
pub fn windows(
    masked: &str,
    terms: &[String],
    term_lemmas: &[u32],
    window: crate::rule::ClusterWindow,
) -> Vec<ClusterWindowHit> {
    use crate::rule::ClusterWindow as W;
    if terms.is_empty() {
        return Vec::new();
    }
    // First term occurrence per term (norm offsets). Terms are ASCII
    // (sanitized at load), so byte scanning is char-boundary safe.
    let lower = masked.to_lowercase();
    let lb = lower.as_bytes();
    let mut hits: Vec<(usize, usize, u32)> = Vec::new(); // (start, end, lemma)
    for (ti, t) in terms.iter().enumerate() {
        if t.is_empty() {
            continue;
        }
        // Identity = lemma index; forms of one word count once.
        let lemma = term_lemmas.get(ti).copied().unwrap_or(ti as u32);
        let mut from = 0usize;
        while let Some(rel) = lower[from..].find(t.as_str()) {
            let at = from + rel;
            let end = at + t.len();
            let before_ok = at == 0 || !lb[at - 1].is_ascii_alphanumeric();
            let after_ok = end >= lb.len() || !lb[end].is_ascii_alphanumeric();
            if before_ok && after_ok {
                hits.push((at, end, lemma));
            }
            from = end.max(at + 1);
        }
    }
    // No hits anywhere: every window would be empty.
    if hits.is_empty() {
        return Vec::new();
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
    // Walk hits, resetting state at each boundary; one record per window
    // that saw at least one hit.
    let mut out: Vec<ClusterWindowHit> = Vec::new();
    let mut bi = 0usize;
    let mut seen: Vec<(u32, String)> = Vec::new(); // (lemma, first surface form)
    let mut last = (0usize, 0usize);
    let flush = |out: &mut Vec<ClusterWindowHit>,
                 seen: &mut Vec<(u32, String)>,
                 last: &mut (usize, usize),
                 wb: (usize, usize)| {
        if seen.is_empty() {
            return;
        }
        out.push(ClusterWindowHit {
            bounds: wb,
            distinct: seen.len(),
            last_hit: *last,
            terms_in_order: seen.drain(..).map(|(_, s)| s).collect(),
        });
    };
    for &(at, end, lemma) in &hits {
        while bi + 1 < bounds.len() && bounds[bi + 1] <= at {
            flush(&mut out, &mut seen, &mut last, (bounds[bi], bounds[bi + 1]));
            bi += 1;
        }
        if seen.iter().any(|(l, _)| *l == lemma) {
            // Repeat of an already-counted lemma: count stays, anchor rides
            // the LAST hit, surface keeps the FIRST spelling.
            last = (at, end);
        } else {
            seen.push((lemma, masked[at..end].to_string()));
            last = (at, end);
        }
    }
    flush(
        &mut out,
        &mut seen,
        &mut last,
        (bounds[bi], *bounds.last().unwrap_or(&masked.len())),
    );
    out
}

/// First `n` whitespace-delimited words of the window, verbatim.
///
/// Used for the finding context line ("the paragraph started like this").
pub fn first_words(masked: &str, bounds: (usize, usize), n: usize) -> Vec<String> {
    masked[bounds.0..bounds.1]
        .split_whitespace()
        .take(n)
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule::ClusterWindow;

    fn terms() -> Vec<String> {
        vec!["crucial".into(), "robust".into(), "notably".into()]
    }

    fn lemmas() -> Vec<u32> {
        (0..terms().len() as u32).collect()
    }

    #[test]
    fn counts_distinct_terms_in_one_paragraph() {
        // Given one paragraph with three distinct watch terms.
        let text = "The crucial part is robust and notably quick.";

        // When measuring the cluster.
        let got = windows(text, &terms(), &lemmas(), ClusterWindow::Paragraph);

        // Then one window holds all three distinct terms.
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].distinct, 3);
        // And the anchor covers the window's final hit word.
        assert_eq!(&text[got[0].last_hit.0..got[0].last_hit.1], "notably");
    }

    #[test]
    fn repeated_same_term_counts_once() {
        // Given a paragraph repeating a single term.
        let text = "crucial crucial crucial crucial";

        // When measuring the cluster.
        let got = windows(text, &terms(), &lemmas(), ClusterWindow::Paragraph);

        // Then only one distinct term counts.
        assert_eq!(got[0].distinct, 1);
        // And the anchor covers the LAST "crucial" (anchor rides final hit).
        assert_eq!(&text[got[0].last_hit.0..got[0].last_hit.1], "crucial");
    }

    #[test]
    fn terms_in_order_keep_first_surface_form() {
        // Given a paragraph where an inflected form precedes the base form.
        let t = vec!["delve".to_string(), "delves".to_string()];
        let l = vec![0u32, 0u32];
        let text = "she delves, we delve, they delves again";

        // When measuring.
        let got = windows(text, &t, &l, ClusterWindow::Document);

        // Then the lemma appears once, spelled as it first occurred.
        assert_eq!(got[0].terms_in_order, vec!["delves".to_string()]);
    }

    #[test]
    fn inflected_forms_share_one_lemma_slot() {
        // Given a terms list where two forms map to one lemma.
        let t = vec!["delve".to_string(), "delves".to_string()];
        let l = vec![0u32, 0u32];
        let text = "we delve and she delves";

        // When measuring.
        let got = windows(text, &t, &l, ClusterWindow::Document);

        // Then both forms count as ONE distinct term.
        assert_eq!(got[0].distinct, 1);
    }

    #[test]
    fn each_offending_paragraph_is_its_own_window() {
        // Given terms spread across two paragraphs, both dense.
        let text = "crucial robust here.\n\nrobust notably crucial there";

        // When measuring per-paragraph.
        let got = windows(text, &terms(), &lemmas(), ClusterWindow::Paragraph);

        // Then BOTH windows are reported, each with its own count.
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].distinct, 2);
        assert_eq!(got[1].distinct, 3);
        // And each window's terms stay in that window's first-hit order.
        assert_eq!(got[0].terms_in_order, vec!["crucial", "robust"]);
        assert_eq!(got[1].terms_in_order, vec!["robust", "notably", "crucial"]);
        // And each anchor stays inside its own window.
        assert_eq!(&text[got[0].last_hit.0..got[0].last_hit.1], "robust");
        assert_eq!(&text[got[1].last_hit.0..got[1].last_hit.1], "crucial");
    }

    #[test]
    fn document_window_pools_everything() {
        // Given the same two-paragraph spread.
        let text = "crucial here.\n\nrobust and notably there";

        // When measuring over the whole document.
        let got = windows(text, &terms(), &lemmas(), ClusterWindow::Document);

        // Then all three pool into one window of three.
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].distinct, 3);
    }

    #[test]
    fn sentence_window_separates_siblings() {
        // Given two sentences, each dense on its own.
        let text = "crucial robust. notably crucial robust.";

        // When measuring per-sentence.
        let got = windows(text, &terms(), &lemmas(), ClusterWindow::Sentence);

        // Then each sentence is its own window.
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].distinct, 2);
        assert_eq!(got[1].distinct, 3);
    }

    #[test]
    fn no_hits_yields_no_windows() {
        // Given a text without any watch terms.
        // When measuring.
        let got = windows(
            "plain words only",
            &terms(),
            &lemmas(),
            ClusterWindow::Paragraph,
        );

        // Then there is nothing to report.
        assert!(got.is_empty());
    }

    #[test]
    fn subsequence_words_do_not_match() {
        // Given text where terms appear only as substrings of other words.
        let text = "crucially robustly";

        // When measuring.
        let got = windows(text, &terms(), &lemmas(), ClusterWindow::Paragraph);

        // Then nothing counts.
        assert!(got.is_empty());
    }

    #[test]
    fn first_words_takes_window_prefix_verbatim() {
        // Given a window bounds range over text with a leading sentence.
        let text = "In the end, we chose. crucial robust notably adept";
        let bounds = (0, text.len());

        // When taking the first four words.
        let got = first_words(text, bounds, 4);

        // Then punctuation and casing survive untouched.
        assert_eq!(got, vec!["In", "the", "end,", "we"]);
    }

    #[test]
    fn first_words_within_window_bounds_only() {
        // Given bounds that start mid-text (a second paragraph).
        let text = "prefix paragraph.\n\nsecond one starts here";
        let bounds = (text.find("second").unwrap(), text.len());

        // When taking words.
        let got = first_words(text, bounds, 2);

        // Then only the window's own words come back.
        assert_eq!(got, vec!["second", "one"]);
    }
}
