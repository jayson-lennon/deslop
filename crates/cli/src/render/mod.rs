//! Output rendering: human (codespan), GitHub annotations, JSON.

pub mod github;
pub mod human;
pub mod json;
mod truncate;

use std::io::Write;

use deslop_core::boundary;

/// A finding bound to its source file for rendering.
#[derive(Debug, Clone)]
pub struct FiledFinding<'a> {
    pub path: &'a str,
    pub src: &'a str,
    pub finding: &'a deslop_core::finding::Finding,
}

/// 1-based char column for a byte offset within its line (shared by the
/// JSON and GitHub formatters so both count chars identically).
///
/// `offset` is floored to the enclosing char boundary first: caller
/// arithmetic (e.g. a span-end minus one) can land mid-character on
/// multibyte source, and a column is only defined for a whole character.
pub(crate) fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let clamped = boundary::floor(src, offset);
    let prefix = &src.as_bytes()[..clamped];
    let line_start = prefix
        .iter()
        .rposition(|b| *b == b'\n')
        .map_or(0, |pos| pos + 1);
    let line = prefix.iter().filter(|b| **b == b'\n').count() + 1;
    let col = src[line_start..clamped].chars().count() + 1;
    (line, col)
}

/// Render findings in the configured format.
///
/// `width` applies to the human format only (0 = untruncated).
///
/// # Errors
///
/// Fails when writing to the destination fails.
pub fn render(
    format: deslop_core::config::FormatName,
    color: deslop_core::config::ColorChoice,
    width: usize,
    filed: &[FiledFinding<'_>],
    out: &mut dyn Write,
) -> std::io::Result<()> {
    match format {
        deslop_core::config::FormatName::Human => human::render_human(filed, color, width, out),
        deslop_core::config::FormatName::Github => github::render_github(filed, out),
        deslop_core::config::FormatName::Json => json::render_json(filed, out),
    }
}
