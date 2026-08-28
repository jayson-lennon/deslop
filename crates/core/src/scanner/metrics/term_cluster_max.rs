//! `term_cluster_max` — max DISTINCT watch terms inside one window.
//!
//! Identity is the LEMMA (`term_lemmas` parallel array), so `delve` and
//! `delves` never count as two. Windows: blank-line paragraphs, sentence
//! enders, or the whole document. The anchor span rides the window's
//! final hit for rendering.

use crate::scanner::metrics::{ClusterHit, sentences};

/// Max DISTINCT `terms` in any `window`, plus the span of that window's
/// last-counted hit (rendering anchor).
pub fn measure(
    masked: &str,
    terms: &[String],
    term_lemmas: &[u32],
    window: crate::rule::ClusterWindow,
) -> Option<(usize, ClusterHit)> {
    use crate::rule::ClusterWindow as W;
    if terms.is_empty() {
        return None;
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
    // No terms or no hits: no measurement (nothing can exceed threshold).
    if hits.is_empty() {
        return None;
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
    let mut best_hit = ClusterHit {
        start: hits[0].0,
        end: hits[0].1,
    };
    let mut bi = 0usize;
    let mut seen = std::collections::BTreeSet::new();
    for &(at, end, ti) in &hits {
        while bi + 1 < bounds.len() && bounds[bi + 1] <= at {
            bi += 1;
            seen.clear();
        }
        seen.insert(ti);
        if seen.len() > best {
            best = seen.len();
            best_hit = ClusterHit { start: at, end };
        }
    }
    Some((best, best_hit))
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
        let (n, hit) = measure(text, &terms(), &lemmas(), ClusterWindow::Paragraph).unwrap();

        // Then all three distinct terms count.
        assert_eq!(n, 3);
        // And the anchor covers the window's final hit word.
        assert_eq!(&text[hit.start..hit.end], "notably");
    }

    #[test]
    fn repeated_same_term_counts_once() {
        // Given a paragraph repeating a single term.
        let text = "crucial crucial crucial crucial";

        // When measuring the cluster.
        let (n, hit) = measure(text, &terms(), &lemmas(), ClusterWindow::Paragraph).unwrap();

        // Then only one distinct term counts.
        assert_eq!(n, 1);
        // And the anchor covers the first "crucial".
        assert_eq!(&text[hit.start..hit.end], "crucial");
    }

    #[test]
    fn inflected_forms_share_one_lemma_slot() {
        // Given a terms list where two forms map to one lemma.
        let t = vec!["delve".to_string(), "delves".to_string()];
        let l = vec![0u32, 0u32];
        let text = "we delve and she delves";

        // When measuring.
        let (n, _) = measure(text, &t, &l, ClusterWindow::Document).unwrap();

        // Then both forms count as ONE distinct term.
        assert_eq!(n, 1);
    }

    #[test]
    fn separate_paragraphs_do_not_pool() {
        // Given terms spread across two paragraphs.
        let text = "crucial here.\n\nrobust and notably there";

        // When measuring per-paragraph.
        let (n, hit) = measure(text, &terms(), &lemmas(), ClusterWindow::Paragraph).unwrap();

        // Then no paragraph exceeds two.
        assert_eq!(n, 2);
        // And the anchor sits in the densest paragraph (earliest hit there).
        assert_eq!(&text[hit.start..hit.end], "notably");
    }

    #[test]
    fn document_window_pools_everything() {
        // Given the same two-paragraph spread.
        let text = "crucial here.\n\nrobust and notably there";

        // When measuring over the whole document.
        let (n, _) = measure(text, &terms(), &lemmas(), ClusterWindow::Document).unwrap();

        // Then all three pool to three.
        assert_eq!(n, 3);
    }

    #[test]
    fn no_hits_yields_no_measurement() {
        // Given a text without any watch terms.
        // When measuring.
        let got = measure(
            "plain words only",
            &terms(),
            &lemmas(),
            ClusterWindow::Paragraph,
        );

        // Then there is no measurement (and no anchor).
        assert!(got.is_none());
    }

    #[test]
    fn subsequence_words_do_not_match() {
        // Given text where terms appear only as substrings of other words.
        let text = "crucially robustly";

        // When measuring.
        let got = measure(text, &terms(), &lemmas(), ClusterWindow::Paragraph);

        // Then nothing counts: no hits means no measurement at all.
        assert!(got.is_none());
    }
}
