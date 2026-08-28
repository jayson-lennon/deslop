//! `sent_len_cv` — coefficient of variation of sentence word counts.
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
        // Given six sentences of identical length.
        let doc = "one two three. one two three. one two three. one two three. one two three. one two three.";

        // When computing.
        let s = compute(&testutil::inputs(doc));

        // Then CV is exactly zero.
        assert!(s.sent_len_cv.abs() < 1e-9);
    }

    #[test]
    fn varied_sentences_yield_higher_cv_than_uniform() {
        // Given alternating short and long sentences.
        let short = "one two.";
        let long = "one two three four five six seven eight nine ten eleven twelve.";
        let doc = format!("{short} {long} {short} {long} {short} {long}");

        // When computing.
        let s = compute(&testutil::inputs(&doc));

        // Then CV is meaningfully positive.
        assert!(s.sent_len_cv > 0.3);
    }
}
