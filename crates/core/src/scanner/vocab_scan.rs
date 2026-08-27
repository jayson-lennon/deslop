//! Vocab scanner: word-index lookups against a term HashMap.
//!
//! The dictionary maps lowercased term -> the group's entry slug so findings
//! identify exactly which entry fired. Multi-word terms are looked up over
//! whitespace-collapsed word runs; stems were expanded at load time.

use std::collections::HashMap;

use super::regions::{RegionMap, Scope};

/// A hit awaiting finding assembly.
#[derive(Debug, Clone)]
pub struct VocabHit {
    pub start: usize,
    pub end: usize,
    pub matched: String,
    pub entry_slug: String,
}

/// Compiled dictionary for one ruleset snapshot.
#[derive(Debug, Default, Clone)]
pub struct VocabIndex {
    /// lowercase surface form (single word or phrase) -> entry slug.
    pub terms: HashMap<String, String>,
}

impl VocabIndex {
    /// Build from (term, slug) pairs — already stem-expanded upstream.
    pub fn build<I: IntoIterator<Item = (String, String)>>(pairs: I) -> VocabIndex {
        let mut terms = HashMap::new();
        for (term, slug) in pairs {
            terms.insert(term.to_lowercase(), slug);
        }
        VocabIndex { terms }
    }

    /// Find all visible whole-word occurrences in scannable scopes.
    ///
    /// Scope routing: headings + prose scan; if `heading_terms_only` some
    /// entries declare heading-only scopes they'd be filtered here (v1: no
    /// per-entry scope yet — kind-level default applies).
    pub fn scan(&self, src: &str, map: &RegionMap, allow: &dyn Fn(Scope) -> bool) -> Vec<VocabHit> {
        let words = tokenize_visible(src, map);
        let mut hits = Vec::new();
        let mut i = 0;
        while i < words.len() {
            // Try longest run of up to 4 words.
            let matched = (1..=4usize)
                .rev()
                .find_map(|len| self.try_run(src, map, allow, &words[i..], len));
            match matched {
                Some(hit) => {
                    // Advance past every word the matched phrase consumed.
                    let span_words = phrase_word_count(src, &words[i..], &hit);
                    i += span_words.max(1);
                    hits.push(hit);
                }
                None => i += 1,
            }
        }
        hits
    }

    fn try_run(
        &self,
        src: &str,
        map: &RegionMap,
        allow: &dyn Fn(Scope) -> bool,
        words: &[Word],
        len: usize,
    ) -> Option<VocabHit> {
        if words.len() < len {
            return None;
        }
        let run = &words[..len];
        // Phrases must be contiguous-ish: single spaces between words only.
        let candidate: String = collapse_phrase(src, run)?;
        let start = run.first()?.start;
        let end = run.get(len - 1)?.end;
        // Visibility: first+last bytes must be visible and inside same region.
        if map.is_masked(start) || map.is_masked(end - 1) {
            return None;
        }
        if !allow(map.scope_at(start)) {
            return None;
        }
        let slug = self.terms.get(&candidate)?.clone();
        Some(VocabHit {
            start,
            end,
            matched: candidate,
            entry_slug: slug,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct Word {
    start: usize,
    end: usize,
}

/// Count how many word tokens the hit's byte span actually covers.
fn phrase_word_count(src: &str, remaining: &[Word], hit: &VocabHit) -> usize {
    remaining
        .iter()
        .take_while(|w| w.start < hit.end && w.end > hit.start)
        .filter(|w| src[w.start..w.end].chars().any(char::is_alphanumeric))
        .count()
}

/// Split on non-alphanumeric boundaries, keeping spans; skip masked words.
fn tokenize_visible(src: &str, map: &RegionMap) -> Vec<Word> {
    let mut words = Vec::new();
    let mut start: Option<usize> = None;
    for (idx, ch) in src.char_indices() {
        let alnum = ch.is_alphanumeric();
        if alnum && start.is_none() {
            start = Some(idx);
        } else if !alnum {
            if let Some(s) = start.take() {
                push_word(src, map, s, idx, &mut words);
            }
        }
    }
    if let Some(s) = start {
        push_word(src, map, s, src.len(), &mut words);
    }
    words
}

fn push_word(src: &str, map: &RegionMap, s: usize, e: usize, words: &mut Vec<Word>) {
    // A word is usable only when entirely unmasked and its interior contains
    // no NULs (partially masked tokens can't match reliably).
    if map.is_masked(s) || map.is_masked(e - 1) {
        return;
    }
    if src[s..e].bytes().any(|b| b == 0) {
        return;
    }
    words.push(Word { start: s, end: e });
}

/// Single-space separator between words? Collapse to canonical phrase form.
fn collapse_phrase(src: &str, run: &[Word]) -> Option<String> {
    let mut out = String::new();
    for (idx, w) in run.iter().enumerate() {
        if idx > 0 {
            let gap = &src[run[idx - 1].end..w.start];
            if gap.len() != 1 || !gap.bytes().all(|b| b == b' ') {
                return None; // hyphens/newlines/multi-space break phrases
            }
            out.push(' ');
        }
        out.push_str(&src[w.start..w.end]);
    }
    Some(out.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::regions::build_regions;

    fn always(_: Scope) -> bool {
        true
    }

    fn idx(pairs: &[(&str, &str)]) -> VocabIndex {
        VocabIndex::build(
            pairs
                .iter()
                .map(|(t, s)| ((*t).to_string(), (*s).to_string())),
        )
    }

    #[test]
    fn single_word_hit_fires_with_span_and_slug() {
        // Given an index with delve->delve and a doc using it.
        let index = idx(&[("delve", "delve")]);
        let src = "we must DELVE deeper";
        let map = build_regions(src);

        // When scanning.
        let hits = index.scan(src, &map, &always);

        // Then one case-insensitive whole-word hit lands with exact span.
        assert_eq!(hits.len(), 1);
        assert_eq!(&src[hits[0].start..hits[0].end], "DELVE");
        assert_eq!(hits[0].entry_slug, "delve");
    }

    #[test]
    fn word_inside_larger_word_does_not_fire() {
        // Given "delve" inside "delver".
        let index = idx(&[("delve", "delve")]);
        let src = "the delver guild";
        let map = build_regions(src);

        // When scanning.
        let hits = index.scan(src, &map, &always);

        // Then no hit (tokenizer produces only whole words).
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn phrase_matches_across_single_space() {
        // Given a phrase term.
        let index = idx(&[("plays a role", "plays-a-role")]);
        let src = "it plays a role here";
        let map = build_regions(src);

        // When scanning.
        let hits = index.scan(src, &map, &always);

        // Then the phrase hits spanning four words.
        assert_eq!(hits.len(), 1);
        assert_eq!(&src[hits[0].start..hits[0].end], "plays a role");
    }

    #[test]
    fn masked_words_are_skipped() {
        // Given inline code hiding the term.
        let index = idx(&[("delve", "delve")]);
        let src = "run `delve` now";
        let map = build_regions(src);

        // When scanning.
        let hits = index.scan(src, &map, &always);

        // Then nothing fires.
        assert!(hits.is_empty());
    }

    #[test]
    fn quoted_mention_dict_terms_are_skipped_after_use_mention_pass() {
        // Given the use-mention pass already masked "delve".
        let index = idx(&[("delve", "delve")]);
        let src = r#"avoid "delve" but still delve"#;
        let mut map = build_regions(src);
        map = crate::scanner::use_mention::mask_quoted_terms(&map, &["delve".to_string()]);

        // When scanning.
        let hits = index.scan(src, &map, &always);

        // Then only the plain use fires.
        assert_eq!(hits.len(), 1);
        assert!(hits[0].start > src.find("but").expect("pos"));
    }
}
