//! Canonical registry of document-level statistics used by `metric` rules.
//!
//! Rules are logic-free data naming a stat + threshold; the formulas live
//! here. Adding a new stat is a PR to this module only.

/// Compute over masked prose; see `scanner::regions` for the masking contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stat {
    EmDashRate,
    CurlyDoubleRatio,
    BoldDensity,
    HeadingTitlecaseFraction,
    EmojiDecorationCount,
    BulletBoldleadFraction,
    TricolonMaxStreak,
    SentLenCv,
    OpeningNgramRepeat,
    TermClusterMax,
}

impl Stat {
    /// Closed set, deterministic order for validation messages.
    pub const ALL: [Stat; 10] = [
        Stat::EmDashRate,
        Stat::CurlyDoubleRatio,
        Stat::BoldDensity,
        Stat::HeadingTitlecaseFraction,
        Stat::EmojiDecorationCount,
        Stat::BulletBoldleadFraction,
        Stat::TricolonMaxStreak,
        Stat::SentLenCv,
        Stat::OpeningNgramRepeat,
        Stat::TermClusterMax,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Stat::EmDashRate => "em_dash_rate",
            Stat::CurlyDoubleRatio => "curly_double_ratio",
            Stat::BoldDensity => "bold_density",
            Stat::HeadingTitlecaseFraction => "heading_titlecase_fraction",
            Stat::EmojiDecorationCount => "emoji_decoration_count",
            Stat::BulletBoldleadFraction => "bullet_boldlead_fraction",
            Stat::TricolonMaxStreak => "tricolon_max_streak",
            Stat::SentLenCv => "sent_len_cv",
            Stat::OpeningNgramRepeat => "opening_ngram_repeat",
            Stat::TermClusterMax => "term_cluster_max",
        }
    }

    pub fn from_name(name: &str) -> Option<Stat> {
        Stat::ALL.into_iter().find(|s| s.name() == name)
    }
}
