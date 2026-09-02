//! Repetition detection primitives: deterministic similarity math over
//! sentence and paragraph units.
//!
//! This module is pure: no I/O, no model, no config — every function takes
//! text or slices and returns plain data. The [`super`] pass assembles
//! findings from these primitives; embedding-based similarity lives behind
//! [`crate::embedder::Embedder`].
//!
//! Determinism contract: fingerprints use fixed-seed FNV-1a (std's
//! `RandomState` is per-process random and would break byte-stable runs);
//! pair iteration and component output are position-sorted.

use std::collections::HashSet;

/// Byte offset of each line's start (line 0 starts at 0), original source.
/// CRLF-safe: `\r` never terminates a line, only `\n` does.
pub fn line_index(src: &str) -> Vec<usize> {
    let mut out = vec![0usize];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            out.push(i + 1);
        }
    }
    out
}

/// 1-based line number containing byte offset `off`.
pub fn line_of(index: &[usize], off: usize) -> usize {
    index.partition_point(|&start| start <= off)
}

/// Byte offset of each whitespace token's start, in scan order.
pub fn token_positions(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut in_token = false;
    for (i, ch) in text.char_indices() {
        let ws = ch.is_whitespace();
        if !ws && !in_token {
            out.push(i);
        }
        in_token = !ws;
    }
    out
}

/// Token-index distance between two byte offsets `from` <= `to` (both
/// should land at token starts): the number of token starts in `[from,
/// to)`, i.e. the difference of their token indices. Adjacent tokens are
/// distance 1; identical offsets are 0.
pub fn tokens_between(positions: &[usize], from: usize, to: usize) -> usize {
    let from_idx = positions.partition_point(|&p| p < from);
    let to_idx = positions.partition_point(|&p| p < to);
    to_idx - from_idx
}

/// FNV-1a 64-bit with a fixed seed; stable across processes and runs.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Lowercased alphanumeric runs (a word must carry at least one).
/// Char-boundary safe: multibyte alphanumerics survive intact.
pub fn words(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut run = None;
    for (i, c) in text.char_indices() {
        match c.is_alphanumeric() {
            true if run.is_none() => run = Some(i),
            false => {
                if let Some(start) = run.take() {
                    out.push(&text[start..i]);
                }
            }
            true => {}
        }
    }
    if let Some(start) = run {
        out.push(&text[start..]);
    }
    out
}

/// Lowercased words (the unit for shingling and content sets).
pub fn words_lower(text: &str) -> Vec<String> {
    words(text).into_iter().map(str::to_lowercase).collect()
}

/// Adaptive shingle length: 8-grams, falling back to 4 when the text is
/// too short for an 8-gram (fewer than 8 words yield zero shingles).
const SHINGLE_K: usize = 8;
const SHINGLE_K_FALLBACK: usize = 4;

/// FNV-hashed k-gram shingles of the text's lowercased words. A text with
/// fewer than k words has an empty set.
fn shingles_at(words: &[String], k: usize) -> HashSet<u64> {
    if words.len() < k {
        return HashSet::new();
    }
    words
        .windows(k)
        .map(|w| fnv1a64(w.join(" ").as_bytes()))
        .collect()
}

/// Adaptive shingles: 8-grams, or 4-grams when the text has fewer than 8
/// words. A 4-gram fallback still needs 4 words; shorter texts are empty.
pub fn shingles_adaptive(text: &str) -> HashSet<u64> {
    let ws = words_lower(text);
    match ws.len() >= SHINGLE_K {
        true => shingles_at(&ws, SHINGLE_K),
        false => shingles_at(&ws, SHINGLE_K_FALLBACK),
    }
}

/// Jaccard similarity of two shingle sets: |A∩B| / |A∪B|.
/// Both empty -> 0.0 (no evidence of repetition), one empty -> 0.0.
pub fn jaccard(a: &HashSet<u64>, b: &HashSet<u64>) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count();
    let union = a.len() + b.len() - inter;
    inter as f64 / union as f64
}

/// English function words excluded from content-word families. Part of the
/// algorithm (like the `DocStats` floors), not lint behavior.
pub(crate) const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "but", "if", "of", "to", "in", "on", "at", "for", "with", "is",
    "are", "was", "were", "be", "been", "being", "it", "its", "this", "that", "these", "those",
    "they", "them", "their", "we", "our", "us", "you", "your", "i", "my", "me", "he", "she", "his",
    "her", "as", "by", "from", "has", "have", "had", "do", "does", "did", "not", "no", "so",
    "than", "then", "there", "what", "which", "who", "will", "would", "can", "could", "should",
    "just", "about", "into", "over", "after", "before",
];

/// Minimum content-word length; shorter tokens are structural noise.
const MIN_CONTENT_LEN: usize = 3;

/// Distinct non-stopword words of length >= 3, sorted. The unit for
/// overlap-coefficient paragraph similarity.
pub fn content_words(text: &str) -> Vec<String> {
    let mut out: Vec<String> = words_lower(text)
        .into_iter()
        .filter(|w| w.len() >= MIN_CONTENT_LEN && !STOPWORDS.contains(&w.as_str()))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Longest-common-subsequence ratio over word lists, normalized by the
/// shorter sentence. Order-preserving containment: near-verbatim pairs
/// (same sentence, small edits/insertions) score high even when no full
/// k-gram survives the edit, while topically-similar but differently
/// structured sentences stay low.
pub fn lcs_ratio(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    // O(n*m) table; sentence units are short, and comparisons are
    // O(units^2) per document anyway.
    let mut prev = vec![0u32; b.len() + 1];
    let mut curr = vec![0u32; b.len() + 1];
    for aw in a {
        for (j, bw) in b.iter().enumerate() {
            curr[j + 1] = if aw == bw {
                prev[j] + 1
            } else {
                prev[j + 1].max(curr[j])
            };
        }
        std::mem::swap(&mut prev, &mut curr);
        curr.fill(0);
    }
    let lcs = prev[b.len()];
    f64::from(lcs) / a.len().min(b.len()) as f64
}

/// Overlap coefficient: |A∩B| / min(|A|,|B|). Both non-empty by contract;
/// an empty set makes the coefficient 0.0.
pub fn overlap_coef(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let bset: HashSet<&String> = b.iter().collect();
    let inter = a.iter().filter(|w| bset.contains(*w)).count();
    inter as f64 / a.len().min(b.len()) as f64
}

/// Union-find over `0..n`; `union(a, b)` merges their components.
struct DisjointSet {
    parent: Vec<usize>,
}

impl DisjointSet {
    fn new(n: usize) -> Self {
        DisjointSet {
            parent: (0..n).collect(),
        }
    }

    fn find(&mut self, x: usize) -> usize {
        match self.parent[x] {
            root if root == x => root,
            _ => {
                let root = self.find(self.parent[x]);
                self.parent[x] = root;
                root
            }
        }
    }

    fn union(&mut self, a: usize, b: usize) {
        let (ra, rb) = (self.find(a), self.find(b));
        match ra <= rb {
            true => self.parent[rb] = ra,
            false => self.parent[ra] = rb,
        }
    }
}

/// Connected components induced by `(a, b)` pairs over `0..n`, each sorted
/// ascending, components sorted by first member. Singletons are excluded.
/// Deterministic regardless of pair order.
pub fn components(n: usize, pairs: &[(usize, usize)]) -> Vec<Vec<usize>> {
    let mut ds = DisjointSet::new(n);
    for &(a, b) in pairs {
        ds.union(a, b);
    }
    let mut grouped: std::collections::BTreeMap<usize, Vec<usize>> =
        std::collections::BTreeMap::new();
    for i in 0..n {
        grouped.entry(ds.find(i)).or_default().push(i);
    }
    let mut out: Vec<Vec<usize>> = grouped.into_values().filter(|c| c.len() >= 2).collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1a64_matches_known_vectors() {
        // Given the canonical FNV-1a test vectors.
        // When hashing.
        // Then the fixed-seed digests match independent computations.
        assert_eq!(fnv1a64(b""), 0xcbf29ce484222325);
        assert_eq!(fnv1a64(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x85944171f73967e8);
    }

    #[test]
    fn fnv1a64_is_stable_across_calls() {
        // Given the same bytes hashed twice.
        // When comparing digests.
        // Then they are identical (no per-process seed).
        assert_eq!(fnv1a64(b"delve into"), fnv1a64(b"delve into"));
    }

    #[test]
    fn line_index_marks_every_line_start() {
        // Given a three-line document.
        let src = "alpha\nbeta\ngamma";

        // When indexing line starts.
        let index = line_index(src);

        // Then line 2 starts after the first newline and line 3 after the second.
        assert_eq!(index, vec![0, 6, 11]);
    }

    #[test]
    fn line_of_reports_one_based_lines() {
        // Given a two-line document index.
        let index = line_index("alpha\nbeta");

        // When locating offsets on each line.
        // Then 1-based numbers come back.
        assert_eq!(line_of(&index, 0), 1);
        assert_eq!(line_of(&index, 5), 1);
        assert_eq!(line_of(&index, 6), 2);
        assert_eq!(line_of(&index, 9), 2);
    }

    #[test]
    fn token_positions_marks_every_token_start() {
        // Given a document with runs of whitespace between tokens.
        let text = "alpha  beta\n\tgamma";

        // When indexing token starts.
        let positions = token_positions(text);

        // Then each token's first byte is recorded, whitespace runs skipped.
        assert_eq!(positions, vec![0, 7, 13]);
    }

    #[test]
    fn tokens_between_counts_token_advances() {
        // Given a token index over a short document.
        let positions = token_positions("one two three four");

        // When measuring token distance between "one" (0) and "four" (14).
        // Then the index difference is 3 (one -> two -> three -> four).
        assert_eq!(tokens_between(&positions, 0, 14), 3);
        // And the distance from "two" (4) to "three" (8) is 1 (adjacent).
        assert_eq!(tokens_between(&positions, 4, 8), 1);
        // And a range to itself is zero.
        assert_eq!(tokens_between(&positions, 4, 4), 0);
    }

    #[test]
    fn tokens_between_is_multibyte_safe() {
        // Given a document whose first token is multibyte ("héllo" = 6 bytes).
        let positions = token_positions("héllo world");

        // When measuring from the second token's start (6) to the end (11).
        // Then only "world" counts.
        assert_eq!(tokens_between(&positions, 6, 11), 1);
    }

    #[test]
    fn crlf_source_still_counts_lines() {
        // Given a CRLF document.
        let src = "alpha\r\nbeta\r\ngamma";

        // When indexing and locating the third line's start.
        let index = line_index(src);

        // Then \r never terminates a line; the offset lands on line 3.
        assert_eq!(index, vec![0, 7, 13]);
        assert_eq!(line_of(&index, 13), 3);
    }

    #[test]
    fn multibyte_offsets_map_to_lines() {
        // Given a document whose second line opens with multibyte text.
        let src = "héllo\nwörld ✓";

        // When indexing and locating the check mark.
        let index = line_index(src);
        let off = src.find('✓').expect("present");

        // Then the byte offset resolves to line 2.
        assert_eq!(line_of(&index, off), 2);
    }

    #[test]
    fn words_extracts_lowercased_alphanumeric_runs() {
        // Given mixed-case text with punctuation and curly quotes.
        // When tokenizing.
        // Then words are lowercase and punctuation-free.
        assert_eq!(
            words_lower("Don't stop—the “Books” are GONE!"),
            vec!["don", "t", "stop", "the", "books", "are", "gone"]
        );
    }

    #[test]
    fn words_keeps_multibyte_letters() {
        // Given multibyte alphabetic characters.
        // When tokenizing.
        // Then they survive as whole words.
        assert_eq!(words_lower("héllo wörld"), vec!["héllo", "wörld"]);
    }

    #[test]
    fn shingles_use_eight_word_grams() {
        // Given an eight-word sentence.
        let text = "the court describes anthropic buying copyrighted books today";

        // When shingling.
        let sh = shingles_adaptive(text);

        // Then exactly one 8-gram exists.
        assert_eq!(sh.len(), 1);
    }

    #[test]
    fn short_text_falls_back_to_four_word_shingles() {
        // Given a seven-word sentence.
        let text = "they bought the book and scanned it";
        let full = shingles_at(&words_lower(text), SHINGLE_K_FALLBACK);

        // When shingling adaptively.
        let sh = shingles_adaptive(text);

        // Then the 4-gram fallback produced every 4-word window.
        assert_eq!(sh, full);
        assert_eq!(sh.len(), 4);
    }

    #[test]
    fn tiny_text_yields_no_shingles() {
        // Given fewer than four words.
        // When shingling.
        // Then the set is empty.
        assert!(shingles_adaptive("nope not now").is_empty());
    }

    #[test]
    fn jaccard_of_identical_sets_is_one() {
        // Given one shingle set.
        let a = shingles_adaptive("they bought the book and scanned every page");
        let b = a.clone();

        // When comparing.
        // Then similarity is exactly 1.
        assert_eq!(jaccard(&a, &b), 1.0);
    }

    #[test]
    fn jaccard_of_disjoint_sets_is_zero() {
        // Given shingle sets sharing nothing.
        let a = shingles_adaptive("they bought the book and scanned every page");
        let b = shingles_adaptive("nobody ever reads the license terms anymore");

        // When comparing.
        // Then similarity is exactly 0.
        assert_eq!(jaccard(&a, &b), 0.0);
    }

    #[test]
    fn jaccard_with_empty_side_is_zero() {
        // Given an empty set.
        let a = shingles_adaptive("they bought the book and scanned every page");
        let empty = HashSet::new();

        // When comparing.
        // Then no similarity evidence exists.
        assert_eq!(jaccard(&a, &empty), 0.0);
        assert_eq!(jaccard(&empty, &empty), 0.0);
    }

    #[test]
    fn stopwords_and_short_tokens_are_excluded() {
        // Given a paragraph mixing function words and content words.
        // When extracting content words.
        // Then stopwords and sub-3-letter tokens vanish.
        assert_eq!(
            content_words("The AI can read the old books, and it will buy them"),
            vec!["books", "buy", "old", "read"]
        );
    }

    #[test]
    fn content_words_are_sorted_and_deduped() {
        // Given repeated content words.
        // When extracting.
        // Then each appears once, sorted.
        assert_eq!(
            content_words("books books Books warehouses scan"),
            vec!["books", "scan", "warehouses"]
        );
    }

    #[test]
    fn overlap_coefficient_measures_containment() {
        // Given a set fully contained in a larger one.
        let a = vec!["books".to_owned(), "scanned".to_owned()];
        let b = vec![
            "books".to_owned(),
            "pages".to_owned(),
            "scanned".to_owned(),
            "warehouses".to_owned(),
        ];

        // When comparing.
        // Then the coefficient is 1 (a is a subset of b).
        assert_eq!(overlap_coef(&a, &b), 1.0);
    }

    #[test]
    fn overlap_coefficient_is_symmetric() {
        // Given two partially overlapping sets.
        let a = vec!["books".to_owned(), "pages".to_owned(), "scanned".to_owned()];
        let b = vec![
            "pages".to_owned(),
            "warehouses".to_owned(),
            "scanned".to_owned(),
            "shelves".to_owned(),
        ];

        // When comparing both directions.
        // Then both directions report the same value.
        let ab = overlap_coef(&a, &b);
        let ba = overlap_coef(&b, &a);
        assert_eq!(ab, ba);
        assert!((ab - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn overlap_coefficient_with_empty_side_is_zero() {
        // Given an empty set.
        let a = vec!["books".to_owned()];
        // When comparing.
        // Then the coefficient is 0.
        assert_eq!(overlap_coef(&a, &[]), 0.0);
    }

    #[test]
    fn transitive_pairs_form_one_component() {
        // Given a chain a~b and b~c.
        let pairs = vec![(1, 2), (2, 4)];

        // When finding components over six units.
        // Then a b c d land in one sorted component; singles are excluded.
        let comps = components(6, &pairs);
        assert_eq!(comps, vec![vec![1, 2, 4]]);
    }

    #[test]
    fn disjoint_pairs_form_separate_components() {
        // Given two independent pairs.
        let pairs = vec![(0, 3), (2, 5)];

        // When finding components.
        // Then two components come back, sorted by first member.
        let comps = components(6, &pairs);
        assert_eq!(comps, vec![vec![0, 3], vec![2, 5]]);
    }

    #[test]
    fn pair_order_does_not_change_components() {
        // Given the same graph edges in different orders.
        let a = components(5, &[(0, 1), (1, 2), (3, 4)]);
        let b = components(5, &[(3, 4), (2, 1), (1, 0)]);

        // When comparing results.
        // Then components are identical (determinism).
        assert_eq!(a, b);
    }
}
