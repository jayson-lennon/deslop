//! `opening_ngram_repeat` - max run of sentences sharing their opening
//! two-word bigram.
//!
//! Anaphora-ish openers ("We ship X. We ship Y. We ship Z.") are a metric
//! sibling of the vocabulary tells.

use crate::scanner::metrics::{DocStats, Inputs};

/// Fill the `opening_ngram_repeat` field.
pub fn measure(inputs: &Inputs<'_>, stats: &mut DocStats) {
    let sents = crate::scanner::metrics::sentences(inputs.prose);
    stats.opening_ngram_repeat = opening_bigram_max_repeat(inputs.prose, &sents);
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
    use crate::scanner::metrics::{compute, testutil};

    #[test]
    fn repeated_openers_streak() {
        // Given three sentences opening with the same two words (each
        // sentence starts capitalized after the first, which UAX #29
        // requires to break after the period).
        let doc = "We ship speed. We ship quality. We ship both. Done here.";

        // When computing.
        let s = compute(&testutil::inputs(doc));

        // Then the max repeat is three.
        assert_eq!(s.opening_ngram_repeat, 3);
    }

    #[test]
    fn distinct_openers_yield_one() {
        // Given sentences that all start differently (each sentence starts
        // capitalized, which UAX #29 requires to break after the period).
        let doc = "We ship speed. They ship quality. You ship trust. Done here.";

        // When computing.
        let s = compute(&testutil::inputs(doc));

        // Then the max repeat is one.
        assert_eq!(s.opening_ngram_repeat, 1);
    }
}
