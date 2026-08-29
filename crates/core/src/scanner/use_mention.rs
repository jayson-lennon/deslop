//! Use-mention distinction: "delve" (mentioning the word) is not slop.
//!
//! After structural masking, scan remaining prose for QUOTED occurrences of
//! dictionary terms and mask the quoted phrase too. Only exact whole-term
//! matches count - quoting is deliberate commentary, near-misses are prose.

use super::regions::RegionMap;

/// Quote pairs considered: curly double/single, straight double/single,
/// guillemets. The inner text must equal a known term exactly
/// (case-insensitive) after trimming whitespace.
const OPENERS: [char; 6] = ['"', '\u{201C}', '\'', '\u{2018}', '\u{00AB}', '"'];
const CLOSERS: [char; 6] = ['"', '\u{201D}', '\'', '\u{2019}', '\u{00BB}', '"'];

/// Byte-level in-place replacement preserving String validity:
/// replacing ASCII bytes with NULs only (UTF-8 continuation untouched).
///
/// The range is expected to be char-boundary aligned (quote positions come
/// from `find` on the same buffer); on a misaligned range the rewrite is
/// skipped rather than repaired, leaving the zone visible to scanners.
fn replace_range_bytes(text: &mut String, range: std::ops::Range<usize>, with: &[u8]) {
    let (Some(head), Some(tail)) = (text.get(..range.start), text.get(range.end..)) else {
        return;
    };
    let mut out = Vec::with_capacity(text.len());
    out.extend_from_slice(head.as_bytes());
    out.extend_from_slice(with);
    out.extend_from_slice(tail.as_bytes());
    *text = String::from_utf8(out).expect("ascii-safe rewrite");
}

/// Mask `"term"` / 'term' / “term” / «term» spans whose content matches a
/// dictionary term. Returns a NEW RegionMap (fresh NUL buffer).
pub fn mask_quoted_terms(map: &RegionMap, dictionary: &[String]) -> RegionMap {
    if dictionary.is_empty() {
        return map.clone();
    }
    // HashSet for O(1) dictionary probes (pre-lowercased, trimmed).
    let dict: std::collections::HashSet<String> =
        dictionary.iter().map(|t| t.trim().to_lowercase()).collect();
    let mut masked = map.masked.clone();
    for opener_idx in 0..OPENERS.len() {
        let (open, close) = (OPENERS[opener_idx], CLOSERS[opener_idx]);
        let open_s = open.encode_utf8(&mut [0u8; 4]).to_string();
        let close_s = close.encode_utf8(&mut [0u8; 4]).to_string();
        let mut from = 0;
        while let Some(rel) = masked.get(from..).and_then(|s| s.find(&open_s)) {
            let open_at = from + rel;
            // Skip bytes already masked.
            if !map.is_masked(open_at) {
                let after_open = open_at + open_s.len();
                if let Some(close_rel) = masked.get(after_open..).and_then(|s| s.find(&close_s)) {
                    let close_at = after_open + close_rel;
                    if let Some(inner) = masked.get(after_open..close_at) {
                        let trimmed = inner.trim();
                        if dict.contains(&trimmed.to_lowercase()) {
                            // Mask quotes + interior (byte-safe: region within
                            // this string is pure ASCII since both quote chars
                            // and the dict term matched ASCII case-insensitively;
                            // non-ASCII interiors are copied unchanged).
                            let zone = open_at..close_at + close.len_utf8();
                            if let Some(bytes) = masked.get(zone.clone()).map(str::as_bytes) {
                                let nulled: Vec<u8> = bytes
                                    .iter()
                                    .map(|&b| if b == b'\n' || b >= 0x80 { b } else { 0 })
                                    .collect();
                                replace_range_bytes(&mut masked, zone, &nulled);
                            }
                        }
                    }
                }
            }
            from = open_at + open_s.len();
        }
    }
    RegionMap {
        masked,
        scopes: map.scopes.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scanner::regions::build_regions;

    fn dict() -> Vec<String> {
        vec!["delve".into(), "tapestry".into()]
    }

    #[test]
    fn straight_quoted_term_gets_masked() {
        // Given prose mentioning "delve" in quotes AND using it plainly.
        let src = r#"when writing, avoid "delve" yet we still delve here"#;
        let map = build_regions(src);
        let map2 = mask_quoted_terms(&map, &dict());

        // Then the quoted mention is NULed; the plain use survives.
        assert!(map2.masked.contains("we still delve"), "{:?}", map2.masked);
        let quoted_zone_nulled = !map2.masked.contains("\"delve\"");
        assert!(quoted_zone_nulled);
    }

    #[test]
    fn curly_quoted_term_gets_masked() {
        // Given typographic quotes.
        let src = "the word \u{201C}delve\u{201D} is loaded";
        let map = build_regions(src);
        let map2 = mask_quoted_terms(&map, &dict());

        // Then the mention disappears into NULs.
        assert!(!map2.masked.contains("delve"), "{:?}", map2.masked);
    }

    #[test]
    fn unquoted_use_stays_visible() {
        // Given plain unquoted usage.
        let src = "we must delve deeper";
        let map = build_regions(src);
        let map2 = mask_quoted_terms(&map, &dict());

        // Then nothing is masked.
        assert!(map2.masked.contains("delve"));
    }

    #[test]
    fn partial_quote_of_phrase_not_masked() {
        // Given quotes containing MORE than the term (not an exact mention).
        let src = "the so-called \"delve problem\" spreads";
        let map = build_regions(src);
        let map2 = mask_quoted_terms(&map, &dict());

        // Then content remains (exact-match only per spec).
        assert!(map2.masked.contains("delve problem"));
    }

    #[test]
    fn second_quoted_term_after_multibyte_text_is_still_masked() {
        // Given a masked quote, multibyte prose, then another quoted term.
        let src = "\"delve\" leads 汉字 \"tapestry\"";
        let map = build_regions(src);
        let map2 = mask_quoted_terms(&map, &dict());

        // Then BOTH mentions are NULed: scanning continues past multibyte
        // characters instead of stalling.
        let visible =
            map2.masked.matches("delve").count() + map2.masked.matches("tapestry").count();
        assert_eq!(visible, 0, "{:?}", map2.masked);
    }
}
