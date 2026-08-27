//! literal-ban scanner: compiled markers over the masked text.
//!
//! Raw inline HTML stays visible (it IS visible text when pasted), so a
//! `<ref>{{cite web...}}</ref>` blob hits the placeholder family correctly.

use super::regions::RegionMap;
use crate::rule::literals::{self, Segment};

/// One hit to lift into a Finding by the caller.
#[derive(Debug, Clone)]
pub struct LiteralHit {
    pub start: usize,
    pub end: usize,
    /// The marker term as authored (for message text).
    pub term: String,
}

/// Find every unmasked occurrence of any compiled marker.
///
/// `compiled` pairs the authored term with its segments. Overlaps between
/// DIFFERENT terms are all reported; identical-term repeats report each span.
pub fn scan(map: &RegionMap, compiled: &[(String, Vec<Segment>)]) -> Vec<LiteralHit> {
    let mut hits = Vec::new();
    for (term, segs) in compiled {
        let mut from = 0;
        while let Some((rel_start, rel_end)) = find_from(&map.masked[from..], segs) {
            let start = from + rel_start;
            let end = from + rel_end;
            // A hit counts only if it BEGINS on visible text; if its first
            // byte is inside a masked region we skip the whole match.
            if !map.is_masked(start) {
                hits.push(LiteralHit {
                    start,
                    end,
                    term: term.clone(),
                });
                from = end.max(start + 1);
            } else {
                from = start + 1;
            }
        }
    }
    hits.sort_by_key(|h| (h.start, h.end));
    hits.dedup_by_key(|h| (h.start.clone(), h.end.clone()));
    hits
}

fn find_from(haystack: &str, segs: &[Segment]) -> Option<(usize, usize)> {
    literals::find(haystack, segs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::regions::build_regions;

    fn compiled() -> Vec<(String, Vec<Segment>)> {
        vec![
            (
                "contentReference[oaicite:{N}]{{index={N}}}".to_string(),
                literals::compile("contentReference[oaicite:{N}]{{index={N}}}").expect("compiles"),
            ),
            (
                "utm_source=chatgpt.com".to_string(),
                literals::compile("utm_source=chatgpt.com").expect("compiles"),
            ),
        ]
    }

    #[test]
    fn chatgpt_artifact_in_prose_is_found() {
        // Given a pasted ChatGPT citation blob in prose.
        let src = "see contentReference[oaicite:16]{index=16} thanks";
        let map = build_regions(src);

        // When scanning.
        let hits = scan(&map, &compiled());

        // Then exactly one hit spans the artifact.
        assert_eq!(hits.len(), 1);
        assert_eq!(
            &src[hits[0].start..hits[0].end],
            "contentReference[oaicite:16]{index=16}"
        );
    }

    #[test]
    fn same_artifact_in_code_fence_is_ignored() {
        // Given the artifact inside a fenced block.
        let src = "```\ncontentReference[oaicite:16]{index=16}\n```";
        let map = build_regions(src);

        // When scanning.
        let hits = scan(&map, &compiled());

        // Then nothing fires.
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn raw_html_citation_paste_still_fires() {
        // Given the artifact wrapped in inline HTML (visible when rendered).
        let src = "<ref>contentReference[oaicite:3]{index=3}</ref>";
        let map = build_regions(src);

        // When scanning.
        let hits = scan(&map, &compiled());

        // Then it still hits (raw HTML is visible text).
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn multiple_terms_yield_sorted_deduped_hits() {
        // Given two different artifacts on one line.
        let src = "utm_source=chatgpt.com then utm_source=chatgpt.com again";
        let map = build_regions(src);

        // When scanning.
        let hits = scan(&map, &compiled());

        // Then both occurrences appear in order.
        assert_eq!(hits.len(), 2);
        assert!(hits[0].start < hits[1].start);
    }
}
