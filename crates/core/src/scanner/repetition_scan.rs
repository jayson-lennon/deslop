//! The repetition pass: group-level repetition lints over visible prose.
//!
//! Units are sentences (`metrics::sentences`) for near-verbatim and
//! propositional variants, blank-line paragraphs for content-family.
//! Similar pairs merge into connected components; each component with at
//! least `min_members` members becomes ONE anchorless finding whose context
//! lists every member as `line {n}: {text}` in ORIGINAL coordinates.
//!
//! The propositional variant needs an [`Embedder`]; when none is available
//! (model missing at runtime, embedder build failed) the variant emits one
//! stderr warning per document set and skips instead of failing the run.

use super::metrics;
use tracing::{debug, info_span};

use super::repetition::{
    components, content_words, jaccard, lcs_ratio, line_index, line_of, overlap_coef,
    shingles_adaptive, words_lower,
};
use crate::embedder::Embedder;
use crate::finding::{Finding, KindTag};
use crate::rule::{RepetitionSpec, RepetitionVariant, RuleGroup, RuleSet};

/// Minimum words for a sentence unit to participate in pair comparisons.
/// One-liners, interjections, and rhetorical fragments ("Nope.", "That was
/// the whole point.") are noise for every variant: below this bar,
/// order-blind similarity over a handful of function words matches by
/// accident in themed prose.
const MIN_UNIT_WORDS: usize = 7;

/// Minimum words for a sentence unit in the propositional variant. Embedding
/// similarity is meaning-level, not string-level, so shorter rhetorical
/// twins ("X did not write the books.") are safe to compare here even
/// though the order-blind near-verbatim bar excludes them.
const MIN_UNIT_WORDS_PROPOSITIONAL: usize = 6;

/// Minimum content words for a paragraph unit to participate in the
/// content-family variant. Headings ("The piracy part") and stubs have
/// tiny word sets that overlap trivially with everything.
const MIN_PARA_CONTENT: usize = 6;

/// Minimum absolute word overlap for a content-family pair, regardless of
/// the coefficient: one shared word is not a family.
const MIN_OVERLAP_WORDS: usize = 2;

/// Jaccard bar at which a sentence pair counts as near-verbatim for the
/// propositional suppression rule. Pinned here because dedup may drop the
/// near-verbatim group itself; suppression must not depend on config order.
const NEAR_VERBATIM_BAR: f64 = 0.55;

/// LCS-ratio bar for the near-verbatim variant's order-preserving path.
/// Tuned on youtube-script prose: twin sentences score 0.6-0.85, unrelated
/// sentences in the same document stay under 0.25.
const CONTAINMENT_BAR: f64 = 0.6;

/// A similarity pair over unit indices.
type Pair = (usize, usize);

/// The document as the repetition pass sees it: original source plus the
/// normalized/masked view and the offset remap between them.
pub struct DocumentView<'a> {
    pub src: &'a str,
    pub norm_text: &'a str,
    pub map: &'a super::regions::RegionMap,
    pub norm: &'a crate::eol::Normalized,
}

/// One repetition hit ready for finding assembly: unit spans (in `prose`
/// coordinates) of a connected component, sorted by position.
type Component = Vec<(usize, usize)>;

/// Run every enabled repetition group over one document.
pub fn repetition_findings(
    doc: &DocumentView<'_>,
    rules: &RuleSet,
    settings: &LintSettings,
    embedder: Option<&dyn Embedder>,
    findings: &mut Vec<Finding>,
    warnings: &mut Vec<String>,
) {
    let Some((_, prose)) = metrics::visible_prose(doc.norm_text, doc.map) else {
        return;
    };
    let lines = line_index(doc.src);

    for group in &rules.groups {
        if !group.enabled {
            continue;
        }
        if settings.level_for(&group.id_base, &group.id_base)
            == Some(crate::config::LintLevel::Allow)
        {
            continue;
        }
        if let Some(max) = settings.max_tier {
            if group.tier > max {
                continue;
            }
        }
        let Some(spec) = &group.repetition else {
            continue;
        };

        let _pass =
            info_span!("repetition_pass", rule = %group.id_base, variant = spec.variant.name());
        let all_components = match spec.variant {
            RepetitionVariant::NearVerbatim => {
                let comps = cluster(
                    &prose,
                    near_verbatim_pairs(&prose, spec.threshold),
                    spec.min_members,
                    MIN_UNIT_WORDS,
                );
                debug!(rule = %group.id_base, components = comps.len(), "near-verbatim pass");
                comps
            }
            RepetitionVariant::Propositional => {
                let Some(embedder) = embedder else {
                    warnings.push(
                        "deslop: propositional repetition skipped - the all-MiniLM-L6-v2 model is not available".into(),
                    );
                    continue;
                };
                match propositional_pairs(&prose, spec.threshold, embedder) {
                    Ok(pairs) => {
                        debug!(rule = %group.id_base, pairs = pairs.len(), "propositional pairs");
                        let comps = cluster(
                            &prose,
                            pairs,
                            spec.min_members,
                            MIN_UNIT_WORDS_PROPOSITIONAL,
                        );
                        // Suppression: a propositional component whose members
                        // all sit inside one near-verbatim component is that
                        // lint's report already; saying it twice is noise.
                        // The near-verbatim similarity bar is its own
                        // canonical constant, not another group's threshold.
                        let nv = near_verbatim_set(&prose, NEAR_VERBATIM_BAR);
                        // Map each propositional component back to unit indices.
                        let units = sentence_units(&prose, MIN_UNIT_WORDS_PROPOSITIONAL);
                        comps
                            .into_iter()
                            .filter(|comp| {
                                let idxs: Vec<usize> = comp
                                    .iter()
                                    .filter_map(|span| units.iter().position(|u| u == span))
                                    .collect();
                                !contained_in_one(&idxs, &nv)
                            })
                            .collect()
                    }
                    Err(e) => {
                        warnings.push(format!("deslop: embedding failed: {e}"));
                        continue;
                    }
                }
            }
            RepetitionVariant::ContentFamily => {
                let paragraphs = paragraph_bounds(&prose);
                let pairs = content_family_pairs(&prose, spec.threshold);
                debug!(rule = %group.id_base, paragraphs = paragraphs.len(), pairs = pairs.len(), "content-family pass");
                components(paragraphs.len(), &pairs)
                    .into_iter()
                    .filter(|comp| comp.len() >= spec.min_members)
                    .map(|comp| comp.into_iter().map(|i| paragraphs[i]).collect())
                    .collect()
            }
        };

        for comp in all_components {
            findings.push(repetition_finding(
                group, spec, &comp, &prose, &lines, doc.norm, doc.src, settings,
            ));
        }
    }
}

use crate::scanner::LintSettings;

/// Sentence units (>= [`MIN_UNIT_WORDS`] words) as prose spans. Pure
/// bracketed stage cues ("[SCREEN: ...]") are production notes, not prose,
/// and pair with each other trivially.
fn sentence_units(prose: &str, min_words: usize) -> Vec<(usize, usize)> {
    metrics::sentences(prose)
        .into_iter()
        .filter(|&(s, e)| {
            let text = prose[s..e].trim();
            let cue = text.starts_with('[') && text.ends_with(']');
            !cue && words_count(text) >= min_words
        })
        .collect()
}

fn words_count(text: &str) -> usize {
    super::metrics::words_count(text)
}

/// Near-verbatim pairs: sentence pairs whose adaptive-shingle Jaccard
/// reaches `threshold`.
fn near_verbatim_pairs(prose: &str, threshold: f64) -> Vec<Pair> {
    let units = sentence_units(prose, MIN_UNIT_WORDS);
    let shingle_sets: Vec<_> = units
        .iter()
        .map(|&(s, e)| shingles_adaptive(&prose[s..e]))
        .collect();
    // LCS-over-words catches the mid-length regime where k-gram shingles
    // are all-or-nothing: "X did not write the books" vs "X did not write
    // these books" shares no 8-gram but is unmistakably the same sentence
    // with one substitution.
    let word_sets: Vec<Vec<String>> = units
        .iter()
        .map(|&(s, e)| words_lower(&prose[s..e]))
        .collect();
    let mut pairs = Vec::new();
    for i in 0..units.len() {
        for j in (i + 1)..units.len() {
            let shingle_sim = jaccard(&shingle_sets[i], &shingle_sets[j]);
            let lcs_sim = lcs_ratio(&word_sets[i], &word_sets[j]);
            if shingle_sim >= threshold || lcs_sim >= CONTAINMENT_BAR {
                pairs.push((i, j));
            }
        }
    }
    pairs
}

/// The full set of near-verbatim components at a given threshold, as UNIT
/// INDEX groups, used for the propositional suppression rule.
fn near_verbatim_set(prose: &str, threshold: f64) -> Vec<Vec<usize>> {
    let units = sentence_units(prose, MIN_UNIT_WORDS);
    let pairs = near_verbatim_pairs(prose, threshold);
    components(units.len(), &pairs)
}

/// Propositional pairs: sentence pairs whose embedding cosine reaches
/// `threshold`. Requires the model; errors surface as a run warning.
#[allow(clippy::needless_pass_by_value)]
fn propositional_pairs(
    prose: &str,
    threshold: f64,
    embedder: &dyn Embedder,
) -> Result<Vec<Pair>, error_stack::Report<crate::embedder::EmbedError>> {
    let units = sentence_units(prose, MIN_UNIT_WORDS_PROPOSITIONAL);
    let texts: Vec<String> = units
        .iter()
        .map(|&(s, e)| prose[s..e].to_string())
        .collect();
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    debug!(units = texts.len(), "embedding sentence units");
    let vectors = embedder.embed(&texts)?;
    let mut pairs = Vec::new();
    for i in 0..vectors.len() {
        for j in (i + 1)..vectors.len() {
            // Embeddings are L2-normalized, so the dot product IS cosine.
            let dot: f32 = vectors[i].iter().zip(&vectors[j]).map(|(x, y)| x * y).sum();
            if f64::from(dot) >= threshold {
                pairs.push((i, j));
            }
        }
    }
    Ok(pairs)
}

/// Content-family pairs: paragraph pairs whose content-word overlap
/// coefficient reaches `threshold`.
fn content_family_pairs(prose: &str, threshold: f64) -> Vec<Pair> {
    let paragraphs = paragraph_bounds(prose);
    let sets: Vec<Vec<String>> = paragraphs
        .iter()
        .map(|&(s, e)| {
            let own = content_words(&prose[s..e]);
            // Doc-adaptive weighting: in a LARGE document, a word appearing
            // in more than half the paragraphs is the DOCUMENT's topic, not
            // a repetition signal — drop it. Small docs keep everything:
            // at five paragraphs the filter would erase real families.
            if paragraphs.len() >= 8 {
                let majority = paragraphs.len() / 2;
                own.into_iter()
                    .filter(|w| {
                        paragraphs
                            .iter()
                            .filter(|&&(s2, e2)| words_lower(&prose[s2..e2]).contains(w))
                            .count()
                            <= majority
                    })
                    .collect()
            } else {
                own
            }
        })
        .collect();
    let mut pairs = Vec::new();
    for i in 0..paragraphs.len() {
        if sets[i].len() < MIN_PARA_CONTENT {
            continue;
        }
        for j in (i + 1)..paragraphs.len() {
            if sets[j].len() < MIN_PARA_CONTENT {
                continue;
            }
            let shared = sets[i].iter().filter(|w| sets[j].contains(*w)).count();
            if shared >= MIN_OVERLAP_WORDS && overlap_coef(&sets[i], &sets[j]) >= threshold {
                pairs.push((i, j));
            }
        }
    }
    pairs
}

/// Paragraph units as prose spans, like the cluster windows. Paragraphs
/// below [`MIN_PARA_CONTENT`] content words do not participate: headings
/// and stub lines have tiny word sets that overlap trivially.
fn paragraph_bounds(prose: &str) -> Vec<(usize, usize)> {
    let bytes = prose.as_bytes();
    let mut bounds = vec![0usize];
    for i in 0..bytes.len() {
        if bytes[i] == b'\n' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
            bounds.push(i + 1);
        }
    }
    bounds.push(prose.len());
    bounds
        .windows(2)
        .map(|w| (w[0], w[1]))
        .filter(|&(s, e)| words_count(&prose[s..e]) > 0)
        .map(|(s, e)| {
            let start = s + prose[s..e].len() - prose[s..e].trim_start().len();
            let end = s + prose[s..e].trim_end().len();
            (start, end)
        })
        .collect()
}

/// Connected components over SENTENCE unit indices mapped back to prose
/// spans, filtered to `min_members`, members ascending. (Only used for the
/// sentence-unit variants; content-family clusters inline.)
fn cluster(prose: &str, pairs: Vec<Pair>, min_members: usize, min_words: usize) -> Vec<Component> {
    let units = sentence_units(prose, min_words);
    components(units.len(), &pairs)
        .into_iter()
        .filter(|comp| comp.len() >= min_members)
        .map(|comp| comp.into_iter().map(|i| units[i]).collect())
        .collect()
}

/// Whether `comp` lies inside one member of `groups` (by index coverage).
fn contained_in_one(comp: &[usize], groups: &[Vec<usize>]) -> bool {
    groups.iter().any(|g| comp.iter().all(|i| g.contains(i)))
}

/// Build one group-level repetition finding for a component.
///
/// The span covers the first member (in ORIGINAL coordinates); the context
/// lists every member as `  line {n}: {text80}` so the reader sees the
/// whole repetition group at a glance.
#[allow(clippy::too_many_arguments)]
fn repetition_finding(
    group: &RuleGroup,
    spec: &RepetitionSpec,
    comp: &Component,
    prose: &str,
    lines: &[usize],
    norm: &crate::eol::Normalized,
    orig_src: &str,
    settings: &LintSettings,
) -> Finding {
    let first = comp[0];
    let (o_start, o_end) = norm.span_to_orig(first.0, first.1);
    let count = comp.len();
    let lookup = |name: &str| match name {
        "count" => Some(count.to_string()),
        "variant" => Some(spec.variant.name().to_string()),
        _ => None,
    };
    let message = group
        .message
        .as_deref()
        .map(|t| crate::rule::template::render(t, &lookup))
        .unwrap_or_else(|| "repeated content".to_string());
    let advice = group
        .advice
        .as_deref()
        .map(|t| crate::rule::template::render(t, &lookup));

    let mut context = String::from("Repetition members:");
    for &(s, e) in comp {
        let line = line_of(lines, norm.span_to_orig(s, s).0);
        let text = member_excerpt(&prose[s..e]);
        context.push_str(&format!("\n  line {line}: {text}"));
    }

    Finding {
        entry_id: group.id_base.clone(),
        kind: KindTag::Repetition,
        tier: effective_tier(settings, group, &group.id_base),
        category: group.category.clone(),
        message,
        advice,
        span: crate::finding::Span::new(o_start, o_end),
        excerpt: super::excerpt_of(orig_src, o_start, o_end),
        url: group.url.clone(),
        context: Some(context),
        replacement: None,
        anchorless: true,
    }
}

/// One member's preview: whitespace-collapsed, truncated at 80 chars on a
/// char boundary with an ellipsis when cut.
fn member_excerpt(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    const LIMIT: usize = 80;
    if collapsed.chars().count() <= LIMIT {
        return collapsed;
    }
    let cut: String = collapsed.chars().take(LIMIT).collect();
    format!("{cut}…")
}

/// Group-level tier resolution honoring `[lints]` overrides.
fn effective_tier(
    settings: &LintSettings,
    group: &RuleGroup,
    entry_id: &str,
) -> crate::finding::Tier {
    let base =
        crate::finding::Tier::from_number(group.tier).unwrap_or(crate::finding::Tier::Density);
    match settings.level_for(&group.id_base, entry_id) {
        Some(crate::config::LintLevel::Error) => crate::finding::Tier::Artifact,
        Some(crate::config::LintLevel::Warn) => crate::finding::Tier::Tell,
        Some(crate::config::LintLevel::Note) => crate::finding::Tier::Density,
        _ => base,
    }
}
