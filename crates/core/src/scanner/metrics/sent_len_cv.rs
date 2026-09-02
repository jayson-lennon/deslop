//! `sent_len_cv` - coefficient of variation of sentence word counts.
//!
//! Monotone rhythm (every sentence ~15 words) is an AI tell; human prose
//! varies. CV = stddev / mean over the document's sentences. The floor
//! (>= 6 sentences) lives in [`crate::scanner::metrics::DocStats::get`].

use crate::scanner::metrics::{DocStats, Inputs};

/// Fill the `sent_len_cv` field.
pub fn measure(inputs: &Inputs<'_>, stats: &mut DocStats) {
    let sents = crate::scanner::metrics::sentences(inputs.prose);
    stats.sentence_count = sents.len();
    let lens: Vec<f64> = sents
        .iter()
        .map(|(s, e)| {
            crate::scanner::metrics::words_count(&inputs.prose[*s..(*e).min(inputs.prose.len())])
                as f64
        })
        .collect();
    stats.sent_len_cv = if lens.len() >= 6 && lens.iter().sum::<f64>() > 0.0 {
        let mean = lens.iter().sum::<f64>() / lens.len() as f64;
        let var = lens.iter().map(|l| (l - mean).powi(2)).sum::<f64>() / lens.len() as f64;
        if mean > 0.0 { var.sqrt() / mean } else { 0.0 }
    } else {
        0.0
    };
}

#[cfg(test)]
mod tests {
    use crate::scanner::metrics::{compute, testutil};

    #[test]
    fn uniform_sentences_yield_low_cv() {
        // Given six sentences of identical length. Each opens with a
        // numeral ("0. ..."), which UAX #29 treats as an unambiguous
        // sentence break (a lowercase word after ". " would suppress it).
        let doc = "0. one two three. 1. one two three. 2. one two three. 3. one two three. 4. one two three. 5. one two three.";

        // When computing.
        let s = compute(&testutil::inputs(doc));

        // Then CV is exactly zero.
        assert!(s.sent_len_cv.abs() < 1e-9);
    }

    #[test]
    fn varied_sentences_yield_higher_cv_than_uniform() {
        // Given alternating short and long sentences (numbered so every
        // boundary is unambiguous to the segmenter).
        let doc = "0. one two. 1. one two three four five six seven eight nine ten eleven twelve. \
                   2. one two. 3. one two three four five six seven eight nine ten eleven twelve. \
                   4. one two. 5. one two three four five six seven eight nine ten eleven twelve.";

        // When computing.
        let s = compute(&testutil::inputs(doc));

        // Then CV is meaningfully positive.
        assert!(s.sent_len_cv > 0.3);
    }

    #[test]
    fn paragraphed_uniform_sentences_have_low_cv() {
        // Given six paragraphs, each a single uniform-length sentence
        // (the blank-line merge defect fused these into one "sentence").
        let doc = "One two three four.\n\nFive six seven eight.\n\nNine ten eleven twelve.\n\n\
                   Thirteen fourteen fifteen sixteen.\n\nSeventeen eighteen nineteen twenty.\n\n\
                   Twenty one two three.";

        // When computing.
        let s = compute(&testutil::inputs(doc));

        // Then every paragraph measures as its own sentence, so the
        // rhythm reads as uniform (CV near zero), not as one chunk.
        assert_eq!(s.sentence_count, 6);
        assert!(s.sent_len_cv < 0.2);
    }

    #[test]
    fn wildly_varied_paragraphs_have_high_cv() {
        // Given paragraphs whose sentences alternate between 3 and 20 words.
        let doc = "One two three.\n\nFour five six seven eight nine ten eleven twelve thirteen \
                   fourteen fifteen sixteen seventeen eighteen nineteen twenty.\n\nOne two \
                   three.\n\nFour five six seven eight nine ten eleven twelve thirteen fourteen \
                   fifteen sixteen seventeen eighteen nineteen twenty.\n\nOne two three.\n\n\
                   Four five six seven eight nine ten eleven twelve thirteen fourteen fifteen \
                   sixteen seventeen eighteen nineteen twenty.";

        // When computing.
        let s = compute(&testutil::inputs(doc));

        // Then every sentence is its own unit and the CV matches the closed
        // form for alternating lengths, (b - a) / (a + b) = 14/20.
        assert_eq!(s.sentence_count, 6);
        let expected = 14.0 / 20.0;
        assert!((s.sent_len_cv - expected).abs() < 1e-9);
    }
}
