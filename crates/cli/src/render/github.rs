//! GitHub Actions annotation output: `::error` / `::warning` / `::notice`.
//!
//! Spans are byte offsets; positions are emitted as 1-based char-derived
//! line/col pairs computed with char_indices (never slicing at non-char
//! boundaries).

use std::io::Write;

use super::FiledFinding;
use deslop_core::finding::Tier;

/// Percent-encode per the workflow-command spec.
fn escape_data(s: &str) -> String {
    s.replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// Property values additionally escape the two property separators.
fn escape_property(s: &str) -> String {
    escape_data(s).replace(':', "%3A").replace(',', "%2C")
}

fn command(tier: Tier) -> &'static str {
    match tier {
        Tier::Artifact => "error",
        Tier::Tell => "warning",
        Tier::Density => "notice",
    }
}

use super::line_col;

/// Render all findings as workflow commands to `out`.
///
/// # Errors
///
/// Fails when writing fails.
pub fn render_github(filed: &[FiledFinding<'_>], out: &mut dyn Write) -> std::io::Result<()> {
    for f in filed {
        let (line, col) = line_col(f.src, f.finding.span.start);
        let end_off = f.finding.span.end.clamp(f.finding.span.start, f.src.len());
        let (end_line, end_col) = if end_off > f.finding.span.start {
            line_col(f.src, end_off)
        } else {
            (line, col)
        };
        writeln!(
            out,
            "::{} file={},line={line},col={col},endLine={end_line},endColumn={end_col},title={}::{}",
            command(f.finding.tier),
            escape_property(f.path),
            escape_property(&f.finding.entry_id),
            escape_data(&f.finding.message),
        )?;
        if let Some(advice) = &f.finding.advice {
            // Advice rides along as a second notice-free line, kept on the
            // same annotation stream but out of the title.
            writeln!(out, "help: {advice}")?;
        }
    }
    Ok(())
}
