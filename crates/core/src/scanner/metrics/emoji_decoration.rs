//! `emoji_decoration_count` - decorative emoji in headings and bullet leads.
//!
//! Counting is restricted to heading bodies and list items because emoji in
//! running prose is legitimate voice; emoji as UI decoration clusters where
//! structure is.

use crate::scanner::metrics::{DocStats, Inputs};

/// Fill the `emoji_decoration_count` field.
pub fn measure(inputs: &Inputs<'_>, stats: &mut DocStats) {
    stats.emoji_decoration_count = emoji_count(inputs.prose, inputs);
}

fn emoji_count(prose: &str, inputs: &Inputs<'_>) -> usize {
    let is_emoji = |c: char| {
        ('\u{1F300}'..='\u{1FAFF}').contains(&c) || ('\u{2600}'..='\u{27BF}').contains(&c)
    };
    inputs
        .heading_ranges
        .iter()
        .chain(inputs.list_items.iter())
        .filter_map(|(s, e)| prose.get(*s..(*e).min(prose.len())))
        .map(|seg| seg.chars().filter(|c| is_emoji(*c)).count())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_emoji_in_structural_positions() {
        // Given a heading and a bullet each carrying one emoji.
        let prose = "\u{1F680} Launch plan\nregular prose here\n- \u{2728} shiny item";
        let mut stats = DocStats::default();

        // When measuring with those ranges.
        measure(
            &Inputs {
                prose,
                heading_ranges: &[(0, 15)],
                bold_spans: &[],
                list_items: &[(30, 44)],
            },
            &mut stats,
        );

        // Then both emoji count.
        assert_eq!(stats.emoji_decoration_count, 2);
    }

    #[test]
    fn emoji_in_plain_prose_ignored() {
        // Given emoji only in running text (no structural ranges).
        let prose = "great \u{1F389} stuff happening everywhere in prose";
        let mut stats = DocStats::default();

        // When measuring.
        measure(
            &Inputs {
                prose,
                heading_ranges: &[],
                bold_spans: &[],
                list_items: &[],
            },
            &mut stats,
        );

        // Then nothing counts.
        assert_eq!(stats.emoji_decoration_count, 0);
    }
}
