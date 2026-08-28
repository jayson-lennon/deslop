//! `curly_double_ratio` — curly double quotes vs straight doubles.
//!
//! Needs at least 20 total doubles before the ratio means anything;
//! below that the field reads 0.0 and the doc floor silences the stat.

use crate::scanner::metrics::DocStats;

/// Fill the `curly_double_ratio` field.
pub fn measure(prose: &str, stats: &mut DocStats) {
    // Curly double-quote ratio vs straight doubles.
    let curly = prose.matches('\u{201C}').count() + prose.matches('\u{201D}').count();
    let straight = prose.matches('"').count();
    stats.curly_double_ratio = if curly + straight >= 20 {
        curly as f64 / (curly + straight).max(1) as f64
    } else {
        0.0
    };
}

#[cfg(test)]
mod tests {
    use crate::scanner::metrics::{compute, testutil};

    #[test]
    fn mixed_quotes_yield_fraction() {
        // Given >250 words and 3 curly / 1 straight double (20+ total via filler).
        let mut quotes = "\u{201C}a\u{201D} \u{201C}b\u{201D} \u{201C}c\u{201D} \"d\" ".repeat(5);
        quotes.push_str(&"word ".repeat(250));

        // When computing.
        let s = compute(&testutil::inputs(&quotes));

        // Then the ratio is curly over total.
        let expected = 15.0 / 20.0;
        assert!((s.curly_double_ratio - expected).abs() < 1e-9);
    }

    #[test]
    fn too_few_quotes_read_zero() {
        // Given plenty of words but only 4 doubles.
        let doc = format!("{} \u{201C}q\u{201D} \"s\" ", "word ".repeat(120));

        // When computing.
        let s = compute(&testutil::inputs(&doc));

        // Then the ratio stays 0.0 (sample too small).
        assert_eq!(s.curly_double_ratio, 0.0);
    }
}
