//! Serde mirror of the rule-file format (one file = many `[[group]]` tables).
//!
//! `deny_unknown_fields` rejects typos outright: data is code here.
//! Per-kind legality (a `metric` group carrying `[[entries]]`, say) is
//! enforced by the loader, which understands kinds; serde alone cannot.

/// Raw mirror of one rule file: a sequence of group tables.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulesFileToml {
    #[serde(default, rename = "group")]
    pub groups: Vec<GroupToml>,
}

/// Raw mirror of one `[[group]]` table.
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
    #[serde(default, alias = "threshold-lt")]
    pub threshold_lt: Option<f64>,
    /// metric cluster: paragraph | sentence | document (default paragraph).
    #[serde(default)]
    pub window: Option<String>,
    /// metric cluster: distinct terms counted within the window.
    #[serde(default)]
    pub terms: Option<Vec<String>>,
    // --- kind = repetition (loader enforces presence iff kind == "repetition") ---
    /// near-verbatim | propositional | content-family.
    #[serde(default)]
    pub variant: Option<String>,
    /// Similarity cutoff in (0, 1]; pairs at or above it cluster together.
    #[serde(default)]
    pub threshold: Option<f64>,
    /// Minimum members before a repetition cluster is reported.
    #[serde(default, alias = "min-members")]
    pub min_members: Option<usize>,
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

/// Mandatory self-tests embedded in every rule group.
///
/// Note: the loader rejects an empty `must_match` - a rule that matches
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
    /// Overrides the group's `category` on this entry's findings.
    #[serde(default)]
    pub category: Option<String>,

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
    fn parses_a_multi_group_file() {
        // Given a rule file with two groups and distinct fixtures.
        let text = r#"
[[group]]
id-base = "AATELL"
kind = "vocab"
tier = 2
category = "delve-era"
message = "AI-register vocabulary: {match}"
advice = "prefer plain wording"
enabled = true

[group.url]
text = "Signs of AI writing"
href = "https://example.org/aisigns"

[group.fixtures]
must_match = ["we must delve deeper"]
must_not_match = ["the word delve in quotes"]

[[group.entries]]
terms = ["delve"]
stems = true
replacement = "examine"

[[group]]
id-base = "CLUSTER"
kind = "metric"
tier = 3
category = "watch-list-density"
message = "{value} distinct watch-list words cluster in one {window}"
stat = "term_cluster_max"
threshold_gt = 4.0
window = "paragraph"
terms = ["delve", "tapestry"]

[group.fixtures]
must_match = []
"#;

        // When parsing.
        let file: RulesFileToml = toml::from_str(text).expect("parses");

        // Then both groups land with their own fixtures and entries.
        assert_eq!(file.groups.len(), 2);
        let vocab = &file.groups[0];
        let metric = &file.groups[1];
        assert_eq!(vocab.id_base, "AATELL");
        assert_eq!(vocab.entries.len(), 1);
        assert_eq!(vocab.entries[0].terms, vec!["delve"]);
        assert!(vocab.entries[0].stems);
        assert_eq!(vocab.fixtures.must_match, vec!["we must delve deeper"]);
        assert_eq!(metric.id_base, "CLUSTER");
        assert_eq!(metric.stat.as_deref(), Some("term_cluster_max"));
        assert_eq!(metric.threshold_gt, Some(4.0));
        assert!(metric.entries.is_empty());
        // And the metric's group-level terms land on the metric, not vocab.
        assert_eq!(
            metric.terms.as_deref(),
            Some(vec!["delve".to_owned(), "tapestry".to_owned()].as_slice())
        );
        assert!(metric.fixtures.must_match.is_empty());
    }

    #[test]
    fn unknown_fields_are_rejected() {
        // Given a group table with a misspelled key.
        let text = r#"
[[group]]
id-base = "X"
kind = "vocab"
tier = 2
category = "c"
stems = true
"#;

        // When parsing.
        let result: Result<RulesFileToml, _> = toml::from_str(text);

        // Then it fails (stems belongs to entries, not the group envelope).
        assert!(result.is_err());
    }

    #[test]
    fn legacy_single_group_files_are_rejected() {
        // Given a pre-unfuck file whose envelope sits at top level.
        let text = r#"
id-base = "OLD"
kind = "vocab"
tier = 2
category = "c"

[[entries]]
terms = ["delve"]
"#;

        // When parsing.
        let result: Result<RulesFileToml, _> = toml::from_str(text);

        // Then it fails (no dual syntax).
        assert!(result.is_err());
    }

    #[test]
    fn repetition_group_fields_round_trip() {
        // Given a repetition group with variant, threshold and min-members.
        let text = r#"
[[group]]
id-base = "REPETITION"
kind = "repetition"
tier = 2
category = "repetition"
message = "Near-verbatim repetition across {count} sentences"
variant = "near-verbatim"
threshold = 0.55

[group.fixtures]
must_match = []

[[group]]
id-base = "REPETITION-CF"
kind = "repetition"
tier = 3
category = "repetition"
message = "Content family spans {count} paragraphs"
variant = "content-family"
threshold = 0.35
min-members = 3

[group.fixtures]
must_match = []
"#;

        // When parsing.
        let file: RulesFileToml = toml::from_str(text).expect("parses");

        // Then both repetition groups carry their fields.
        let nv = &file.groups[0];
        assert_eq!(nv.variant.as_deref(), Some("near-verbatim"));
        assert_eq!(nv.threshold, Some(0.55));
        assert_eq!(nv.min_members, None);
        let cf = &file.groups[1];
        assert_eq!(cf.variant.as_deref(), Some("content-family"));
        assert_eq!(cf.min_members, Some(3));
    }

    #[test]
    fn metric_group_round_trips() {
        // Given a metric group with its stat keys.
        let text = r#"
[[group]]
id-base = "AISIGNS-METRIC-EMDASH"
kind = "metric"
tier = 3
category = "punctuation-density"
message = "Em-dash density {value}"
stat = "em_dash_rate"
per_words = 1000
threshold_gt = 6.0

[group.fixtures]
must_match = []
must_not_match = []
"#;

        // When parsing.
        let file: RulesFileToml = toml::from_str(text).expect("parses");

        // Then the three metric fields land.
        let group = &file.groups[0];
        assert_eq!(group.stat.as_deref(), Some("em_dash_rate"));
        assert_eq!(group.per_words, Some(1000));
        assert_eq!(group.threshold_gt, Some(6.0));
    }
}
