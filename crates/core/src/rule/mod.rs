//! Rule types: [`RuleGroup`], [`ActiveEntry`], [`RuleSet`] — the scanner
//! contract. Loading lives in [`loader`]; schema parsing in [`schema`].

pub mod dedup;
pub mod fixtures;
pub mod literals;
pub mod loader;
pub mod policy;
pub mod schema;
pub mod stems;
pub mod template;

/// A rule file = many `[[group]]` tables; one table = one [`RuleGroup`].
#[derive(Debug, Clone)]
pub struct RuleGroup {
    pub id_base: String,
    /// 1=artifact(error) 2=tell(warning) 3=density(hint).
    pub tier: u8,
    /// vocab | pattern | literal-ban | metric.
    pub kind: String,
    pub category: String,
    pub message: Option<String>,
    pub advice: Option<String>,
    pub enabled: bool,
    /// prose | heading | list-item | anywhere (kind defaults applied).
    pub scope: String,
    pub url: Option<(String, String)>,
    pub entries: Vec<ActiveEntry>,
    /// metric-only fields (kind == "metric").
    pub metric: Option<MetricSpec>,
}

/// Threshold spec for a document-level metric rule.
#[derive(Debug, Clone)]
pub struct MetricSpec {
    pub stat: crate::metric_stats::Stat,
    pub per_words: u32,
    pub threshold_gt: f64,
    /// term_cluster_max: granularity of the counted window.
    pub window: ClusterWindow,
    /// term_cluster_max: surface forms counted per window (lowercased,
    /// lemma-expanded).
    pub terms: Vec<String>,
    /// Parallel to `terms`: the lemma each form belongs to. Distinct-lemma
    /// counting, so inflections never inflate the cluster score.
    pub term_lemmas: Vec<u32>,
}

/// Window granularity for cluster stats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClusterWindow {
    Paragraph,
    Sentence,
    Document,
}

impl ClusterWindow {
    pub fn parse(name: &str) -> Option<ClusterWindow> {
        Some(match name {
            "paragraph" => ClusterWindow::Paragraph,
            "sentence" => ClusterWindow::Sentence,
            "document" => ClusterWindow::Document,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            ClusterWindow::Paragraph => "paragraph",
            ClusterWindow::Sentence => "sentence",
            ClusterWindow::Document => "document",
        }
    }
}

/// One scannable entry with its compiled matcher.
#[derive(Debug, Clone)]
pub struct ActiveEntry {
    /// Globally unique "GROUP#slug".
    pub id: String,
    pub message_override: Option<String>,
    pub advice_override: Option<String>,
    /// Overrides the group's category on this entry's findings.
    pub category_override: Option<String>,
    pub matcher: fixtures::Matcher,
    /// vocab only: mechanical rewrite when present.
    pub replacement: Option<String>,
}

/// Everything loaded and validated at startup; consumed by the scanner.
#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    pub groups: Vec<RuleGroup>,
}

/// One validation failure, tied to the file (and where possible, line).
#[derive(Debug, Clone)]
pub struct LoadError {
    pub path: String,
    pub line: Option<usize>,
    pub message: String,
}

impl std::fmt::Display for LoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.line {
            Some(l) => write!(f, "{}:{l}: {}", self.path, self.message),
            None => write!(f, "{}: {}", self.path, self.message),
        }
    }
}

/// Outcome of a load attempt: rules plus every error found.
#[derive(Debug, Default)]
pub struct Loaded {
    pub rule_set: RuleSet,
    pub errors: Vec<LoadError>,
}

