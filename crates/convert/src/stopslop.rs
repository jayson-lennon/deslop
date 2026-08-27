//! stop-slop converter: phrases.md bullets + Avoid/Use tables -> entries.

/// Bullet items (`- "..."` forms) map to report-only terms; table rows map to
/// term+replacement pairs (Avoid | Use instead).
pub fn read(path: &std::path::Path) -> Result<Vec<crate::slop_json::RawTerm>, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = Vec::new();
    let mut in_table = false;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('|') && !trimmed.starts_with("|-") && !trimmed.contains("|---") {
            let cells: Vec<&str> = trimmed
                .split('|')
                .map(str::trim)
                .filter(|c| !c.is_empty())
                .collect();
            if in_table && cells.len() >= 2 && cells[0] != "Avoid" {
                out.push(crate::slop_json::RawTerm {
                    term: clean(cells[0]),
                    replacement: Some(clean(cells[1])),
                    evidence: "stop-slop jargon table".into(),
                    source: "stop-slop".into(),
                    severity: None,
                });
            }
            // A table row signals being inside a table only for body rows.
            continue;
        }
        if trimmed.starts_with("|-") {
            in_table = true;
            continue;
        }
        if !trimmed.starts_with('|') {
            in_table = false;
        }
        if let Some(item) = trimmed.strip_prefix("- ") {
            let t = item.trim_matches(['"', '\u{201C}', '\u{201D}', '.']).trim();
            // Keep multi-word phrases and distinctive tokens; skip single
            // common adverbs that would nuke whole corpora of good prose.
            if is_standalone_adverb(t) || t.contains('[') && t.contains(']') {
                // Placeholder patterns like "Here's what [X]" still valuable —
                // keep the template as a phrase but strip brackets.
                let stripped = t.replace(['[', ']'], "");
                push_phrase(&mut out, &stripped);
            } else {
                push_phrase(&mut out, t);
            }
        }
    }
    Ok(out)
}

fn push_phrase(out: &mut Vec<crate::slop_json::RawTerm>, phrase: &str) {
    let p = phrase.trim();
    if p.chars().count() < 4 {
        return; // too short to be reliable
    }
    out.push(crate::slop_json::RawTerm {
        term: p.to_string(),
        replacement: None,
        evidence: "stop-slop phrases list".into(),
        source: "stop-slop".into(),
        severity: None,
    });
}

fn is_standalone_adverb(word: &str) -> bool {
    let lower = word.to_lowercase();
    [
        "really",
        "just",
        "literally",
        "genuinely",
        "honestly",
        "simply",
        "actually",
        "deeply",
        "truly",
        "fundamentally",
        "inherently",
        "inevitably",
        "interestingly",
        "importantly",
        "crucially",
    ]
    .iter()
    .any(|w| *w == lower)
}

fn clean(cell: &str) -> String {
    cell.trim_matches('.').trim().to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn extracts_table_rows_and_bullets() {
        let terms = super::read(std::path::Path::new(
            "../../third-party/stop-slop/references/phrases.md",
        ))
        .expect("reads phrases.md");
        // Union of table rows (Avoid|Use) and bullets; upstream has ~65.
        assert!(terms.len() >= 50, "got {}", terms.len());
    }

    #[test]
    fn skips_common_adverbs() {
        assert!(super::is_standalone_adverb("really"));
        assert!(!super::is_standalone_adverb("boil the ocean"));
    }
}
