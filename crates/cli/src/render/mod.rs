//! Output rendering: human (codespan), GitHub annotations, JSON.

pub mod github;
pub mod human;
pub mod json;

use std::io::Write;

/// A finding bound to its source file for rendering.
#[derive(Debug, Clone)]
pub struct FiledFinding<'a> {
    pub path: &'a str,
    pub src: &'a str,
    pub finding: &'a deslop_core::finding::Finding,
}

/// Render findings in the configured format.
///
/// # Errors
///
/// Fails when writing to the destination fails.
pub fn render(
    format: deslop_core::config::FormatName,
    color: deslop_core::config::ColorChoice,
    filed: &[FiledFinding<'_>],
    out: &mut dyn Write,
) -> std::io::Result<()> {
    match format {
        deslop_core::config::FormatName::Human => human::render_human(filed, color, out),
        deslop_core::config::FormatName::Github => github::render_github(filed, out),
        deslop_core::config::FormatName::Json => json::render_json(filed, out),
    }
}
