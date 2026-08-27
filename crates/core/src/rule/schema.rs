//! Serde mirror of the rule-file format (one file = one group).
//!
//! `deny_unknown_fields` rejects typos outright: data is code here.
//! Per-kind legality (a `metric` file carrying `[[entries]]`, say) is
//! enforced by the loader, which understands kinds; serde alone cannot.

/// Raw mirror of one whole group file.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GroupToml {
    /// Group prefix for ids; entries append `#slug`.
    #[serde(rename = "id-base")]
    pub id_base: String,
    /// vocab | pattern | literal-ban | metric
    pub kind: String,
    /// 1 = artifact (error), 2 = tell (warning), 3 = density (hint).
    pub tier: u8,
    pub category: String,
    /// Interpolable: vocab allows {match}; patterns allow named captures.
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub advice: Option<String>,
    #[serde(default)]
    pub enabled: Option<bool>,
    /// prose | heading | list-item | anywhere. Kind defaults apply.
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub url: Option<UrlToml>,
    #[serde(default)]
    pub origin: Option<OriginToml>,
    #[serde(default)]
    pub fixtures: FixturesToml,
    #[serde(default)]
    pub entries: Vec<EntryToml>,
    // --- kind = metric (loader enforces presence iff kind == "metric") ---
    /// Name from the canonical STAT registry (`crate::metric_stats::Stat`).
    #[serde(default)]
    pub stat: Option<String>,
    #[serde(default, alias = "per-words")]
    pub per_words: Option<u32>,
    #[serde(default, alias = "threshold-gt")]
    pub threshold_gt: Option<f64>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct UrlToml {
    pub text: String,
    pub href: String,
}

impl Default for UrlToml {
    fn default() -> Self {
        UrlToml {
            text: "learn more".into(),
            href: String::new(),
        }
    }
}

/// Provenance for converted rules; cross-checked against NOTICE.toml.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OriginToml {
    pub repo: String,
    /// Full 40-hex commit SHA of the source snapshot.
    pub commit: String,
}

/// Mandatory self-tests embedded in every rule group.
///
/// Note: the loader rejects an empty `must_match` — a rule that matches
/// nothing is not a rule.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixturesToml {
    #[serde(default)]
    pub must_match: Vec<String>,
    #[serde(default)]
    pub must_not_match: Vec<String>,
}

/// One `[[entries]]` block. Exactly one body field is legal, chosen by the
/// group's `kind`; the loader rejects mismatches.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntryToml {
    /// Entry slug within the group; generated when absent.
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub advice: Option<String>,
    #[serde(default)]
    pub message: Option<String>,

    /// Match list. Shared by vocab (word list) and literal-ban (markers);
    /// meaning depends on the group's kind.
    #[serde(default)]
    pub terms: Vec<String>,
    #[serde(default)]
    pub stems: bool,
    #[serde(default)]
    pub word_boundary: Option<bool>,
    #[serde(default)]
    pub replacement: Option<String>,

    // --- kind = pattern ---
    #[serde(default)]
    pub regex: Option<String>,
    /// Default "echo": named captures surface in message/advice rendering.
    #[serde(default)]
    pub captures: Option<String>,
    #[serde(default)]
    pub engine: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_complete_vocab_group() {
        // Given a full vocab group document.
        let text = r#"
id-base = "MODERN-VOCAB"
kind = "vocab"
tier = 2
category = "delve-era"
message = "AI-register vocabulary: {match}"
advice = "prefer plain wording"
enabled = true

[url]
text = "Signs of AI writing"
href = "https://example.org/aisigns"

[origin]
repo = "https://github.com/walidboulanouar/anti-ai-slop"
commit = "37d1175523f1880aff8f3c4230905177e75dd183"

[fixtures]
must_match = ["we must delve deeper"]
must_not_match = ["the word delve in quotes"]

[[entries]]
terms = ["delve"]
stems = true
replacement = "examine"
"#;

        // When parsing.
        let group: GroupToml = toml::from_str(text).expect("parses");

        // Then envelope and entry bodies land in typed slots.
        assert_eq!(group.id_base, "MODERN-VOCAB");
        assert_eq!(group.tier, 2);
        assert!(group.enabled.unwrap_or(false));
        assert_eq!(group.entries.len(), 1);
        assert_eq!(group.entries[0].terms, vec!["delve"]);
        assert_eq!(group.entries[0].replacement.as_deref(), Some("examine"));
    }

    #[test]
    fn unknown_fields_are_rejected() {
        // Given a document with a misspelled key.
        let text = r#"
id-base = "X"
kind = "vocab"
tier = 2
category = "c"
stems = true
"#;

        // When parsing.
        let result: Result<GroupToml, _> = toml::from_str(text);

        // Then it fails (stems belongs to entries, not the envelope).
        assert!(result.is_err());
    }

    #[test]
    fn metric_group_round_trips() {
        // Given a metric rule with top-level stat keys.
        let text = r#"
id-base = "DOC-METRIC-EMDASH"
kind = "metric"
tier = 3
category = "punctuation-density"
message = "Em-dash density {value}"
stat = "em_dash_rate"
per_words = 1000
threshold_gt = 6.0

[fixtures]
must_match = []
must_not_match = []
"#;

        // When parsing.
        let group: GroupToml = toml::from_str(text).expect("parses");

        // Then the three metric fields land.
        assert_eq!(group.stat.as_deref(), Some("em_dash_rate"));
        assert_eq!(group.per_words, Some(1000));
        assert_eq!(group.threshold_gt, Some(6.0));
    }
}
