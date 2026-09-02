//! Findings: the unit of lint output.

use std::fmt;

/// Severity tier. Ordered by false-positive risk, not by count: Tier 1
/// artifacts are unambiguous; Tier 3 density signals matter only in aggregate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Tier {
    /// Chatbot markup artifacts and placeholders.
    Artifact = 1,
    /// Per-instance prose tells.
    Tell = 2,
    /// Document-level density/statistical hints.
    Density = 3,
}

impl Tier {
    pub const ALL: [Tier; 3] = [Tier::Artifact, Tier::Tell, Tier::Density];

    /// Numeric value used on the CLI (`--tier 2`) and in TOML data.
    pub const fn number(self) -> u8 {
        self as u8
    }

    /// Parse from a rule/config integer.
    pub fn from_number(n: u8) -> Option<Tier> {
        match n {
            1 => Some(Tier::Artifact),
            2 => Some(Tier::Tell),
            3 => Some(Tier::Density),
            _ => None,
        }
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Tier::Artifact => "artifact",
            Tier::Tell => "tell",
            Tier::Density => "density",
        };
        write!(f, "{name}")
    }
}

/// Byte-offset span into the ORIGINAL source text (never masked coordinates).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Span {
        Span { start, end }
    }

    /// The exact source substring this span covers.
    ///
    /// Returns `None` if the span does not lie on char boundaries or exceeds
    /// the input length - callers surface that as an internal error rather
    /// than panicking on multibyte documents.
    pub fn slice(self, src: &str) -> Option<&str> {
        src.get(self.start..self.end)
    }
}

/// What kind of rule produced a finding. Mirrors [`crate::rule::Kind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KindTag {
    Vocab,
    Pattern,
    LiteralBan,
    Metric,
    /// Findings from a `[plugins]` WASM module; identity comes from the
    /// module's own manifest rather than the rule loader.
    Plugin,
    /// Group-level repetition clusters (near-verbatim, propositional,
    /// content-family); one finding per repeated group of sentences or
    /// paragraphs, with per-member line evidence in `context`.
    Repetition,
}

/// A single lint result, ready for rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Globally unique rule identity, e.g. `PROSE-PAT-NEGPAR#not-merely`.
    pub entry_id: String,
    pub kind: KindTag,
    pub tier: Tier,
    pub category: String,
    pub message: String,
    /// Actionable rewrite advice, already interpolated. Optional only during
    /// the staged rollout; CI gates it to `Some` for every enabled entry.
    pub advice: Option<String>,
    pub span: Span,
    /// Exact `src[span]` copy; asserted equal to the slice in tests.
    pub excerpt: String,
    /// `(text, href)` reference note rendered after the diagnostic body.
    pub url: Option<(String, String)>,
    /// Extra evidence line rendered between help and url - the cluster
    /// metric's "which words, where" chain. `None` for every other kind.
    pub context: Option<String>,
    /// Vocab-only mechanical rewrite; `None` = report-only.
    pub replacement: Option<String>,
    /// True only for window-spanned cluster findings: the span covers a
    /// whole sentence/paragraph/document, so a caret over it is noise. The
    /// human renderer draws the excerpt block (gutter + numbered source
    /// lines) WITHOUT any underline or arrow marks; other renderers ignore
    /// this flag.
    pub anchorless: bool,
}
