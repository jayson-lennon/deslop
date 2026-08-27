//! Use-mention distinction: "delve" (mentioning the word) is not slop.
//!
//! After structural masking, scan remaining prose for QUOTED occurrences of
//! dictionary terms and mask the quoted phrase too. Only exact whole-term
//! matches count — quoting is deliberate commentary, near-misses are prose.

use super::regions::RegionMap;

/// Quote pairs considered: curly double/single, straight double/single,
/// guillemets. The inner text must equal a known term exactly
/// (case-insensitive) after trimming whitespace.
const OPENERS: [char; 6] = ['"', '\u{201C}', '\'', '\u{2018}', '\u{00AB}', '"'];
const CLOSERS: [char; 6] = ['"', '\u{201D}', '\'', '\u{2019}', '\u{00BB}', '"'];

/// Byte-level in-place replacement preserving String validity:
/// replacing ASCII bytes with NULs only (UTF-8 continuation untouched).
fn replace_range_bytes(text: &mut String, range: std::ops::Range<usize>, with: &[u8]) {
    let mut out = Vec::with_capacity(text.len());
    out.extend_from_slice(text.as_bytes()[..range.start].to_vec().as_slice());
    out.extend_from_slice(with);
    out.extend_from_slice(text.as_bytes()[range.end..].to_vec().as_slice());
    *text = String::from_utf8(out).expect("ascii-safe rewrite");
}

/// Mask `"term"` / 'term' / “term” / «term» spans whose content matches a
/// dictionary term. Returns a NEW RegionMap (fresh NUL buffer).
pub fn mask_quoted_terms(map: &RegionMap, dictionary: &[String]) -> RegionMap {
    if dictionary.is_empty() {
        return map.clone();
    }
    let mut masked = map.masked.clone();
    for opener_idx in 0..OPENERS.len() {
        let (open, close) = (OPENERS[opener_idx], CLOSERS[opener_idx]);
        let mut from = 0;
        while let Some(rel) = map.masked[from..].find(open) {
            let open_at = from + rel;
            // Skip bytes already masked.
            if !map.is_masked(open_at) {
                if let Some(close_rel) = map.masked[open_at + open.len_utf8()..].find(close) {
                    let close_at = open_at + open.len_utf8() + close_rel;
                    let inner = &map.masked[open_at + open.len_utf8()..close_at];
                    let trimmed = inner.trim();
                    if dictionary.iter().any(|t| t.eq_ignore_ascii_case(trimmed)) {
                        // Mask quotes + interior (byte-safe: region within
                        // this string is pure ASCII since both quote chars
                        // and the dict term matched ASCII case-insensitively;
                        // non-ASCII interiors are copied unchanged).
                        let zone = open_at..close_at + close.len_utf8();
                        let bytes: Vec<u8> = masked.as_bytes()[zone.clone()].to_vec();
                        let nulled: Vec<u8> = bytes
                            .iter()
                            .map(|&b| if b == b'\n' || b >= 0x80 { b } else { 0 })
                            .collect();
                        replace_range_bytes(&mut masked, zone, &nulled);
                    }
                }
            }
            from = open_at + open.len_utf8();
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
}
