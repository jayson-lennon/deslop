//! anti-ai-slop converter: patterns.json words+phrases -> vocab entries.

use serde::Deserialize;

#[derive(Deserialize)]
pub struct Patterns {
    #[serde(default)]
    pub words: Vec<Entry>,
    #[serde(default)]
    pub phrases: Vec<Entry>,
}

#[derive(Deserialize)]
pub struct Entry {
    pub text: String,
    #[serde(default)]
    pub replace: Option<String>,
    #[serde(default)]
    pub severity: Option<String>,
}

/// Read and normalize every entry.
///
/// # Errors
///
/// Fails if the file cannot be read or parsed.
pub fn read(path: &std::path::Path) -> Result<Vec<RawTerm>, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let data: Patterns = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for e in data.words.into_iter().chain(data.phrases) {
        out.push(RawTerm {
            term: e.text,
            replacement: e.replace,
            evidence: format!("severity={}", e.severity.unwrap_or_else(|| "n/a".into())),
            source: "anti-ai-slop".into(),
        });
    }
    Ok(out)
}

/// A normalized vocabulary entry shared across all converters.
#[derive(Debug, Clone)]
pub struct RawTerm {
    pub term: String,
    pub replacement: Option<String>,
    pub evidence: String,
    pub source: String,
}
