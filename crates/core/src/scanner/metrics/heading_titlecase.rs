//! `heading_titlecase_fraction` - share of headings in Title Case.
//!
//! Title Case = >=80% of content words (non-minor, excluding the first
//! word) capitalized, with at least 3 words. Headings under 3 words
//! never qualify.

use crate::scanner::metrics::{DocStats, Inputs};

/// Fill the `heading_titlecase_fraction` field.
pub fn measure(inputs: &Inputs<'_>, stats: &mut DocStats) {
    let prose = inputs.prose;
    stats.heading_titlecase_fraction = if inputs.heading_ranges.is_empty() {
        0.0
    } else {
        let tc = inputs
            .heading_ranges
            .iter()
            .filter(|(s, e)| is_titlecase(&prose[*s..(*e).min(prose.len())]))
            .count();
        tc as f64 / inputs.heading_ranges.len() as f64
    };
}

/// Title Case decision for one heading body.
pub(crate) fn is_titlecase(text: &str) -> bool {
    let minor = ["of", "the", "and", "in", "on", "to", "a", "an", "for"];
    let words: Vec<&str> = text
        .split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .collect();
    if words.len() < 3 {
        return false;
    }
    let content: Vec<&str> = words
        .iter()
        .skip(1)
        .filter(|w| !minor.contains(&w.to_lowercase().as_str()))
        .copied()
        .collect();
    if content.is_empty() {
        return false;
    }
    let capitalized = content
        .iter()
        .filter(|w| w.chars().next().is_some_and(|c| c.is_uppercase()))
        .count();
    capitalized * 100 >= content.len() * 80
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fully_titlecased_heading_scores_one() {
        // Given a heading in Title Case.
        let heading = "Impact of Technology And Digitalization";
        let prose = format!("{heading} plain filler words repeat here. ");
        let mut stats = DocStats::default();

        // When measuring over that one heading range.
        measure(
            &Inputs {
                prose: &prose,
                heading_ranges: &[(0, heading.len())],
                bold_spans: &[],
                list_items: &[],
            },
            &mut stats,
        );

        // Then the fraction is 1.0.
        assert!((stats.heading_titlecase_fraction - 1.0).abs() < 1e-9);
    }

    #[test]
    fn sentence_case_heading_scores_zero() {
        // Given a lowercase heading.
        let heading = "how things work nowadays";
        let prose = format!("{heading} plain filler words repeat here. ");
        let mut stats = DocStats::default();

        // When measuring.
        measure(
            &Inputs {
                prose: &prose,
                heading_ranges: &[(0, heading.len())],
                bold_spans: &[],
                list_items: &[],
            },
            &mut stats,
        );

        // Then the fraction is 0.0.
        assert_eq!(stats.heading_titlecase_fraction, 0.0);
    }
}
