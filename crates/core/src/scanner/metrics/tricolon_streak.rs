//! `tricolon_max_streak` — consecutive sentences containing "x, y, and z".
//!
//! The tricolon itself is one sentence; the SIGNAL is strings of them back
//! to back, so the metric tracks the longest run.

use crate::scanner::metrics::{DocStats, Inputs};

/// Fill the `tricolon_max_streak` field.
pub fn measure(inputs: &Inputs<'_>, stats: &mut DocStats) {
    stats.tricolon_max_streak = tricolon_streak(inputs.prose);
}

fn tricolon_streak(prose: &str) -> usize {
    const TRICOLON: &str = r"\b[a-z]+, [a-z]+, and [a-z]+\b";
    let re = regex::Regex::new(TRICOLON).ok();
    let sents = crate::scanner::metrics::sentences(prose);
    let mut streak = 0;
    let mut best = 0;
    for (s, e) in &sents {
        let hit = re.as_ref().is_some_and(|re| re.is_match(&prose[*s..*e]));
        if hit {
            streak += 1;
            best = best.max(streak);
        } else {
            streak = 0;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use crate::scanner::metrics::{compute, testutil};

    #[test]
    fn consecutive_tricolons_stack_the_streak() {
        // Given two adjacent sentences each containing a tricolon.
        let doc = "we ship speed, scale, and soul. we love rust, cargo, and crabs.";

        // When computing.
        let s = compute(&testutil::inputs(doc));

        // Then the max streak is two.
        assert_eq!(s.tricolon_max_streak, 2);
    }

    #[test]
    fn separated_tricolons_reset_the_streak() {
        // Given tricolons split by a plain sentence.
        let doc = "a, b, and c here. plain sentence. d, e, and f there.";

        // When computing.
        let s = compute(&testutil::inputs(doc));

        // Then the streak never exceeds one.
        assert_eq!(s.tricolon_max_streak, 1);
    }
}
