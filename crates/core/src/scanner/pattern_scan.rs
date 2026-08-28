//! Pattern scanner: `regex` (linear-time DFA/NFA) over VISIBLE runs of the masked text.
//!
//! Matches never span masked holes: we iterate visible character runs and
//! run the engine per-run, so a regex can't accidentally bridge a code
//! fence's NULs. Named captures flow out for message/advice interpolation.

use regex::Regex;

use super::regions::RegionMap;

/// A match plus its named captures (capture text only, not offsets).
#[derive(Debug, Clone)]
pub struct PatternHit {
    pub start: usize,
    pub end: usize,
    /// Ordered named captures: (name, text).
    pub captures: Vec<(String, String)>,
}

/// Extract visible (start,end) runs from the masked map.
fn visible_runs(map: &RegionMap) -> Vec<(usize, usize)> {
    let bytes = map.masked.as_bytes();
    let mut runs = Vec::new();
    let mut start: Option<usize> = None;
    for (i, &b) in bytes.iter().enumerate() {
        if b == 0 {
            if let Some(s) = start.take() {
                runs.push((s, i));
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(s) = start {
        runs.push((s, bytes.len()));
    }
    runs
}

/// Run one compiled regex across all visible runs.
pub fn scan(re: &Regex, src: &str, map: &RegionMap) -> Vec<PatternHit> {
    let mut hits = Vec::new();
    let names: Vec<Option<&str>> = re.capture_names().collect();
    let runs = visible_runs(map);
    for (run_start, run_end) in runs {
        let hay = &src[run_start..run_end];
        for m in re.captures_iter(hay) {
            let Some(whole) = m.get(0) else {
                continue;
            };
            let mut captures = Vec::new();
            for (idx, name) in names.iter().enumerate().skip(1) {
                if let Some(n) = name {
                    if let Some(c) = m.get(idx) {
                        captures.push(((*n).to_string(), c.as_str().to_string()));
                    }
                }
            }
            hits.push(PatternHit {
                start: run_start + whole.start(),
                end: run_start + whole.end(),
                captures,
            });
        }
    }
    hits.sort_by_key(|h| (h.start, h.end));
    hits.dedup_by_key(|h| (h.start, h.end));
    hits
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::regions::build_regions;

    fn scan1(pattern: &str, src: &str) -> Vec<PatternHit> {
        let re = Regex::new(pattern).expect("compiles");
        let map = build_regions(src);
        scan(&re, src, &map)
    }

    #[test]
    fn negative_parallelism_example_hits() {
        // Given the wsc-family construction.
        let hits = scan1(
            r"\bn(?:ot|['’]t) (?:just|only|merely) [^.!?\n]{1,120}?[,;:.] (?:it(?:'s| is)|this is)",
            "It is not just faster; it is transformative.",
        );

        // Then exactly one hit with a sane span.
        assert_eq!(hits.len(), 1);
        assert!(hits[0].end > hits[0].start + 10);
    }

    #[test]
    fn named_captures_flow_through() {
        // Given a pattern with (?P<payload>...).
        let hits = scan1(
            r"\bdelve into (?P<payload>[a-z ]+)",
            "we delve into hidden things here",
        );

        // Then the named capture carries its text.
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0]
                .captures
                .iter()
                .find(|(n, _)| n == "payload")
                .map(|(_, v)| v.as_str()),
            Some("hidden things here")
        );
    }

    #[test]
    fn match_cannot_bridge_masked_hole() {
        // Given two visible halves separated by an inline-code hole; any hit
        // would have to span masked NULs, which per-run scanning forbids.
        let re = Regex::new(r"alpha.*omega").expect("compiles");
        let src = "alpha `bridge` omega";
        let map = build_regions(src);

        // When scanning.
        let hits = scan(&re, src, &map);

        // Then nothing matched across the hole (the run split breaks it).
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn overlapping_matches_dedupe_to_first_span() {
        // Given a pattern that can match at overlapping offsets.
        let hits = scan1(r"aba", "abababab");

        // Then non-overlapping greedy-left results only.
        assert_eq!(hits.len(), 2);
    }
}
