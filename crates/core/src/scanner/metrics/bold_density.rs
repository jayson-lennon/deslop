//! `bold_density` — strong-emphasis spans per 100 words.
//!
//! Spans come from the region map (`**...**` runs over the masked text);
//! this module only applies the formula.

use crate::scanner::metrics::{DocStats, Inputs};

/// Fill the `bold_density` field: `spans * 100 / words`.
pub fn measure(inputs: &Inputs<'_>, stats: &mut DocStats) {
    // Bold density per 100 words.
    stats.bold_density = inputs.bold_spans.len() as f64 * 100.0 / stats.word_count.max(1) as f64;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::metrics::testutil;

    #[test]
    fn spans_scale_per_hundred_words() {
        // Given 100 words and 4 bold spans -> 4.0 per 100 words.
        let prose = "word ".repeat(100);
        let bold: Vec<(usize, usize)> = vec![(0, 2), (10, 12), (20, 22), (30, 32)];
        let mut stats = DocStats::default();
        stats.word_count = crate::scanner::metrics::testutil::word_count(&prose);

        // When measuring with those spans.
        measure(
            &Inputs {
                prose: &prose,
                heading_ranges: &[],
                bold_spans: &bold,
                list_items: &[],
            },
            &mut stats,
        );

        // Then density is exactly 4.
        assert!((stats.bold_density - 4.0).abs() < 1e-9);
    }

    #[test]
    fn no_spans_read_zero() {
        // Given any prose without bold spans.
        let prose = "word ".repeat(50);
        let mut stats = DocStats::default();
        stats.word_count = crate::scanner::metrics::testutil::word_count(&prose);

        // When measuring.
        measure(
            &Inputs {
                prose: &prose,
                heading_ranges: &[],
                bold_spans: &[],
                list_items: &[],
            },
            &mut stats,
        );

        // Then density is 0.
        assert_eq!(stats.bold_density, 0.0);
    }
}
