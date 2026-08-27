//! anti-ai-tell converter: vocabulary.json tiered lists + copulative set.

use serde_json::Value;

use crate::slop_json::RawTerm;

/// Read hard_ban / strong_flag / density_watch / copulative phrases.
///
/// # Errors
///
/// Fails if the file is missing or not the expected shape.
pub fn read(path: &std::path::Path) -> Result<Vec<RawTerm>, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let data: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;

    let mut out = Vec::new();
    for (key, label) in [
        ("hard_ban", "hard_ban"),
        ("strong_flag", "strong_flag"),
        ("density_watch", "density_watch"),
    ] {
        if let Some(words) = data[&key]["words"].as_array() {
            for w in words.iter().filter_map(Value::as_str) {
                let severity = match key {
                    "hard_ban" => Some("hard_ban"),
                    "strong_flag" => Some("strong_flag"),
                    _ => Some("density_watch"),
                };
                out.push(RawTerm {
                    term: w.to_string(),
                    replacement: None,
                    evidence: format!("anti-ai-tell tier={label}"),
                    source: "anti-ai-tell".into(),
                    severity,
                });
            }
        }
    }
    if let Some(phrases) = data["copulative_avoidance"]["phrases"].as_array() {
        for p in phrases.iter().filter_map(Value::as_str) {
            out.push(RawTerm {
                term: p.to_string(),
                replacement: None,
                evidence: "anti-ai-tell copulative_avoidance".into(),
                source: "anti-ai-tell".into(),
                severity: Some("strong_flag"),
            });
        }
    }
    Ok(out)
}
