//! `bullet_boldlead_fraction` — bullets that OPEN with a bold span.
//!
//! AI lists love "**Bold claim:** supporting words"; human lists rarely
//! start every bullet with emphasis. A lead counts when a bold span starts
//! within the first ~3 bytes of the item. The doc floor (>=4 bullets)
//! lives in [`crate::scanner::metrics::DocStats::get`].

use crate::scanner::metrics::{DocStats, Inputs};

/// Fill the `bullet_boldlead_fraction` field.
pub fn measure(inputs: &Inputs<'_>, stats: &mut DocStats) {
    stats.bullet_count = inputs.list_items.len();
    let bullets = inputs.list_items.len();
    let bold_leads = inputs
        .list_items
        .iter()
        .filter(|(s, _)| bold_lead_at(inputs.bold_spans, *s))
        .count();
    stats.bullet_boldlead_fraction = if bullets >= 4 {
        bold_leads as f64 / bullets as f64
    } else {
        0.0
    };
}

/// Does a bold span begin at this bullet's head?
fn bold_lead_at(bold_spans: &[(usize, usize)], item_start: usize) -> bool {
    bold_spans
        .iter()
        .any(|(s, _)| *s <= item_start + 3 && *s >= item_start.saturating_sub(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bold_leads_fraction_over_four_bullets() {
        // Given 4 bullets where 2 open with a bold span.
        let mut stats = DocStats::default();

        // When measuring.
        measure(
            &Inputs {
                prose: "",
                heading_ranges: &[],
                bold_spans: &[(10, 14), (30, 34)],
                list_items: &[(10, 20), (30, 40), (50, 60), (70, 80)],
            },
            &mut stats,
        );

        // Then half the bullets are bold-led.
        assert!((stats.bullet_boldlead_fraction - 0.5).abs() < 1e-9);
        assert_eq!(stats.bullet_count, 4);
    }

    #[test]
    fn under_four_bullets_reads_zero() {
        // Given only 3 bullets, all bold-led.
        let mut stats = DocStats::default();

        // When measuring.
        measure(
            &Inputs {
                prose: "",
                heading_ranges: &[],
                bold_spans: &[(10, 14), (30, 34), (50, 54)],
                list_items: &[(10, 20), (30, 40), (50, 60)],
            },
            &mut stats,
        );

        // Then the fraction stays 0.0 (floor handles silencing upstream).
        assert_eq!(stats.bullet_boldlead_fraction, 0.0);
    }
}
