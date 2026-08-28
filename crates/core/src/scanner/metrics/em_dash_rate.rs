//! `em_dash_rate` - em dashes per 1000 words.
//!
//! Spaced and attached dashes count identically; the floor (250 words)
//! lives in [`crate::scanner::metrics::DocStats::get`], not here.

use crate::scanner::metrics::DocStats;

/// Fill the `em_dash_rate` field: `dashes / words * 1000`.
pub fn measure(prose: &str, stats: &mut DocStats) {
    // Em dashes (spaced or attached count the same).
    let em_dashes = prose.matches('\u{2014}').count();
    stats.em_dash_rate = em_dashes as f64 * 1000.0 / stats.word_count.max(1) as f64;
}

#[cfg(test)]
mod tests {
    use crate::scanner::metrics::{compute, testutil};

    #[test]
    fn counts_dashes_per_thousand_words() {
        // Given a 260-word doc with two em dashes.
        let doc = format!("{}— end — done", "word ".repeat(260));

        // When computing.
        let s = compute(&testutil::inputs(&doc));

        // Then the rate equals 2/words*1000 exactly.
        let expected = 2.0 * 1000.0 / s.word_count as f64;
        assert!((s.em_dash_rate - expected).abs() < 1e-9);
    }
}
