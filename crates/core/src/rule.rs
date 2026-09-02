//! Rule types: [`RuleGroup`], [`ActiveEntry`], [`RuleSet`] - the scanner
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
    /// vocab | pattern | literal-ban | metric | repetition.
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
    /// repetition-only fields (kind == "repetition").
    pub repetition: Option<RepetitionSpec>,
}

/// Threshold spec for a document-level metric rule.
#[derive(Debug, Clone)]
pub struct MetricSpec {
    pub stat: crate::metric_stats::Stat,
    pub per_words: u32,
    /// Direction + cutoff the stat must cross to fire.
    pub threshold: MetricThreshold,
    /// term_cluster_max: granularity of the counted window.
    pub window: ClusterWindow,
    /// term_cluster_max: surface forms counted per window (lowercased,
    /// lemma-expanded).
    pub terms: Vec<String>,
    /// Parallel to `terms`: the lemma each form belongs to. Distinct-lemma
    /// counting, so inflections never inflate the cluster score.
    pub term_lemmas: Vec<u32>,
}

/// Which side of the cutoff a metric fires on. Chosen per rule in TOML
/// via `threshold-gt` ([`MetricThreshold::AtLeast`]) or `threshold-lt`
/// ([`MetricThreshold::AtMost`]); exactly one is present.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MetricThreshold {
    /// Fires when the value exceeds the cutoff (stat grows past it).
    AtLeast(f64),
    /// Fires when the value falls below the cutoff (stat shrinks under it).
    AtMost(f64),
}

impl MetricThreshold {
    /// Whether a measured value crosses this threshold. Comparisons are
    /// strict, matching the historical `value > threshold-gt` behavior.
    pub fn fires(self, value: f64) -> bool {
        match self {
            MetricThreshold::AtLeast(cutoff) => value > cutoff,
            MetricThreshold::AtMost(cutoff) => value < cutoff,
        }
    }

    /// The cutoff value (for dedup decisions and message interpolation).
    pub fn value(self) -> f64 {
        match self {
            MetricThreshold::AtLeast(cutoff) | MetricThreshold::AtMost(cutoff) => cutoff,
        }
    }

    /// Whether `self` fires on a strict subset of what `other` fires on.
    /// Only like directions compare: AtLeast gets stricter as the cutoff
    /// rises, AtMost as it falls. Opposite directions are different
    /// predicates and never compete (both survive dedup).
    pub fn is_stricter_than(self, other: MetricThreshold) -> bool {
        match (self, other) {
            (MetricThreshold::AtLeast(a), MetricThreshold::AtLeast(b)) => a > b,
            (MetricThreshold::AtMost(a), MetricThreshold::AtMost(b)) => a < b,
            _ => false,
        }
    }
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

/// Which repetition detector a group runs. Each variant has its own
/// similarity math and its own default minimum group size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepetitionVariant {
    /// Near-identical sentences (word shingle Jaccard).
    NearVerbatim,
    /// Same proposition reworded (embedding cosine).
    Propositional,
    /// One content-word family across many paragraphs (overlap coefficient).
    ContentFamily,
}

impl RepetitionVariant {
    pub fn parse(name: &str) -> Option<RepetitionVariant> {
        Some(match name {
            "near-verbatim" => RepetitionVariant::NearVerbatim,
            "propositional" => RepetitionVariant::Propositional,
            "content-family" => RepetitionVariant::ContentFamily,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            RepetitionVariant::NearVerbatim => "near-verbatim",
            RepetitionVariant::Propositional => "propositional",
            RepetitionVariant::ContentFamily => "content-family",
        }
    }

    /// Whether the variant needs the MiniLM embedding model.
    pub fn needs_model(self) -> bool {
        matches!(self, RepetitionVariant::Propositional)
    }

    /// Default minimum members before a cluster is worth reporting.
    pub fn default_min_members(self) -> usize {
        match self {
            RepetitionVariant::NearVerbatim | RepetitionVariant::Propositional => 2,
            RepetitionVariant::ContentFamily => 3,
        }
    }
}

/// Validated repetition settings for a group (kind == "repetition").
#[derive(Debug, Clone, Copy)]
pub struct RepetitionSpec {
    pub variant: RepetitionVariant,
    /// Similarity cutoff in (0, 1]; pairs at or above it join one cluster.
    pub threshold: f64,
    /// Minimum members before a cluster is reported.
    pub min_members: usize,
    /// Maximum whitespace-token distance between a pair's members; pairs
    /// farther apart never form, so they cannot bridge transitive chains.
    pub max_distance: usize,
}

/// Default `max-distance` for sentence-level repetition variants. Long-range
/// repeats are usually deliberate callbacks; the cap keeps incidental
/// restatements (which sit close together) as the signal.
pub const DEFAULT_MAX_DISTANCE: usize = 200;

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
