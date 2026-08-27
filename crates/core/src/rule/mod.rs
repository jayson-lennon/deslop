//! Rule model: data-only definitions loaded from TOML packs.

use crate::finding::Tier;

/// Globally unique rule identity: `<GROUP>#<slug>`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EntryId(pub String);

/// A rule file = one group: shared envelope + `[[entries]]`.
#[derive(Debug, Clone)]
pub struct RuleGroup {
    pub id_base: String,
    pub tier: Tier,
    pub kind: Kind,
}

/// Everything loaded and validated at startup; consumed by the scanner.
#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    pub groups: Vec<RuleGroup>,
}

/// Which scanner runs, carrying its per-kind entry payload.
#[derive(Debug, Clone)]
pub enum Kind {
    Vocab(VocabBody),
    Pattern(PatternBody),
    LiteralBan(LitBody),
    Metric(MetricBody),
}

/// Vocabulary entry body (term-list matching).
#[derive(Debug, Clone, Default)]
pub struct VocabBody {
    pub stems: bool,
}

/// Regex pattern entry body.
#[derive(Debug, Clone, Default)]
pub struct PatternBody {
    pub engine_checked: bool,
}

/// Substring ban entry body.
#[derive(Debug, Clone, Default)]
pub struct LitBody {
    pub placeholder_expanded: bool,
}

/// Document-metric rule body.
#[derive(Debug, Clone, Default)]
pub struct MetricBody {
    pub wired_to_registry: bool,
}
