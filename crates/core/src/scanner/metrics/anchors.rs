//! Whole-doc metric anchoring.
//!
//! Concrete-signal stats (curly quotes, em dashes, bold spans, Title Case
//! headings, structural emoji, bold-led bullets, tricolons) anchor at the
//! FIRST occurrence of their signal — the evidence exists at a place.
//!
//! Distributional stats (sentence-length CV, repeated openers) have no
//! single spot of evidence — the signal is the whole document's rhythm —
//! so they report `None` and their findings render as document-level notes
//! with no span. `term_cluster_max` already carries its own window anchor
//! from the cluster pass.

use super::Stat;

/// First-signal span for one stat: `(start, end)` inside `text`.
///
/// `None` = document-level stat (or signal not locatable); the caller
/// keeps the `(0, 0)` span, which the human renderer treats as doc-level.
pub(crate) fn first_signal_span(
    stat: Stat,
    text: &str,
    bold_spans: &[(usize, usize)],
    heading_ranges: &[(usize, usize)],
    list_items: &[(usize, usize)],
) -> (usize, usize) {
    // One probe per stat; the match arms are small enough to inline.
    let found: Option<(usize, usize)> = match stat {
        Stat::CurlyDoubleRatio => {
            let curly = ['\u{201C}', '\u{201D}'];
            text.char_indices()
                .find(|(_, c)| curly.contains(c))
                .map(|(i, c)| (i, i + c.len_utf8()))
        }
        Stat::EmDashRate => em_dash_first(text),
        Stat::BoldDensity => bold_spans.first().copied(),
        Stat::HeadingTitlecaseFraction => {
            let tc: Vec<&(usize, usize)> = heading_ranges
                .iter()
                .filter(|(s, e)| {
                    text.get(*s..(*e).min(text.len()))
                        .is_some_and(super::heading_titlecase::is_titlecase)
                })
                .collect();
            tc.first()
                .map(|(s, e)| (*s, *e))
                .or_else(|| heading_ranges.first().copied())
        }
        Stat::EmojiDecorationCount => {
            let is_emoji = |c: char| {
                ('\u{1F300}'..='\u{1FAFF}').contains(&c) || ('\u{2600}'..='\u{27BF}').contains(&c)
            };
            let mut best: Option<(usize, usize)> = None;
            for (s, e) in heading_ranges.iter().chain(list_items.iter()) {
                let seg = match text.get(*s..(*e).min(text.len())) {
                    Some(seg) => seg,
                    None => continue,
                };
                for (off, c) in seg.char_indices() {
                    if is_emoji(c) {
                        best = Some((*s + off, *s + off + c.len_utf8()));
                        break;
                    }
                }
                if best.is_some() {
                    break;
                }
            }
            best
        }
        Stat::BulletBoldleadFraction => list_items
            .iter()
            .find(|(s, _)| bold_spans.iter().any(|(b, _)| *b <= *s + 3))
            .map(|(s, e)| (*s, *e)),
        Stat::TricolonMaxStreak => tricolon_first(text),
        // Distributional stats: the evidence IS the whole document (a
        // rhythm, not a spot). One sentence's span would be a fake
        // anchor, so report None and let the finding render doc-level.
        Stat::SentLenCv | Stat::OpeningNgramRepeat => None,
        // Cluster findings arrive with their own anchor; never routed here.
        Stat::TermClusterMax => None,
    };
    found.unwrap_or((0, 0))
}

/// First U+2014 in text, as a byte span.
fn em_dash_first(text: &str) -> Option<(usize, usize)> {
    text.char_indices()
        .find(|&(_, c)| c == '\u{2014}')
        .map(|(i, c)| (i, i + c.len_utf8()))
}

/// First "x, y, and z" match in text.
fn tricolon_first(text: &str) -> Option<(usize, usize)> {
    let re = regex::Regex::new(r"\b[a-z]+, [a-z]+, and [a-z]+\b").ok()?;
    re.find(text).map(|m| (m.start(), m.end()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curly_ratio_anchors_at_first_curly_quote() {
        // Given a doc whose first curly quote sits after a heading.
        let text = "# Title\n\nsome \u{201C}quoted\u{201D} words";

        // When finding the anchor.
        let (s, e) = first_signal_span(Stat::CurlyDoubleRatio, text, &[], &[], &[]);

        // Then it covers the opening curly quote, not byte 0.
        assert_eq!(&text[s..e], "\u{201C}");
    }

    #[test]
    fn titlecase_anchors_at_first_qualifying_heading() {
        // Given a sentence-case heading and then a Title Case one.
        let h1 = "how things work";
        let h2 = "Impact of Technology And Digitalization";
        let text = format!("{h1}\n{h2}\n");
        let ranges = vec![(0, h1.len()), (h1.len() + 1, h1.len() + 1 + h2.len())];

        // When finding the anchor.
        let (s, e) = first_signal_span(Stat::HeadingTitlecaseFraction, &text, &[], &ranges, &[]);

        // Then it covers the TITLE CASE heading.
        assert_eq!(&text[s..e], h2);
    }

    #[test]
    fn bold_density_anchors_at_first_bold_span() {
        // Given two bold spans.
        let bold = vec![(10, 20), (30, 40)];

        // When finding the anchor.
        let (s, e) = first_signal_span(Stat::BoldDensity, "filler text here", &bold, &[], &[]);

        // Then it covers the first span.
        assert_eq!((s, e), (10, 20));
    }

    #[test]
    fn missing_signal_falls_back_to_document_start() {
        // Given text with no curly quotes at all.
        // When finding the anchor.
        let (s, e) = first_signal_span(Stat::CurlyDoubleRatio, "no quotes", &[], &[], &[]);

        // Then the span is the honest empty document anchor.
        assert_eq!((s, e), (0, 0));
    }

    #[test]
    fn distributional_stats_report_no_span() {
        // Given a distributional stat (evidence is the whole doc).
        // When finding the anchor.
        let (s, e) = first_signal_span(Stat::SentLenCv, "some sentence.", &[], &[], &[]);

        // Then there is no fake single-sentence anchor.
        assert_eq!((s, e), (0, 0));
    }

    #[test]
    fn em_dash_anchors_at_first_dash() {
        // Given text where an em dash appears mid-sentence.
        let text = "plain words \u{2014} then more";
        let (s, e) = first_signal_span(Stat::EmDashRate, text, &[], &[], &[]);

        // Then the anchor covers the dash.
        assert_eq!(&text[s..e], "\u{2014}");
    }
}
