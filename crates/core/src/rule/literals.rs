//! literal-ban term compilation.
//!
//! Terms are plain substrings EXCEPT for the `{N}` token, which matches one
//! or more digits (chatbot citation ids). Real braces double as escapes,
//! mirroring [`crate::rule::template`]:
//!
//! - `turn0search{N}`               -> `turn0search` + digits
//! - `contentReference[oaicite:{N}]{{index={N}}}` -> digits inside both
//! - `[cite: {N}]`, `url=PASTE_SPOTIFY_TRACK_URL_HERE`
//!
//! Compilation lowers to a sequence of segments so matching can run as a
//! plain substring hunt per segment with a digit-scan between them - no
//! regex escaping hazards.

/// One compiled piece of a ban marker.
#[derive(Debug, Clone, PartialEq)]
pub enum Segment {
    /// Literal text, matched case-insensitively downstream.
    Text(String),
    /// `{N}`: one or more ASCII digits.
    Digits,
}

/// Compile a ban-marker term into segments.
///
/// # Errors
///
/// A single unmatched `{` that is not part of `{{`/`{N}` is an error; the
/// author almost certainly meant to double it.
pub fn compile(term: &str) -> Result<Vec<Segment>, String> {
    let mut segments = Vec::new();
    let mut text = String::new();
    let bytes = term.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => match bytes.get(i + 1) {
                Some(b'{') => {
                    text.push('{');
                    i += 2;
                }
                Some(b'N') if bytes.get(i + 2) == Some(&b'}') => {
                    if !text.is_empty() {
                        segments.push(Segment::Text(std::mem::take(&mut text)));
                    }
                    segments.push(Segment::Digits);
                    i += 3;
                }
                _ => {
                    return Err(format!(
                        "single `{{` in literal-ban term {term:?}; double it ({{{{) or use {{N}}"
                    ));
                }
            },
            b'}' if bytes.get(i + 1) == Some(&b'}') => {
                text.push('}');
                i += 2;
            }
            _ => {
                let ch = term[i..].chars().next().expect("non-empty char");
                text.push(ch);
                i += ch.len_utf8();
            }
        }
    }
    if !text.is_empty() {
        segments.push(Segment::Text(text));
    }
    Ok(segments)
}

/// Does `haystack` contain this compiled marker? Case-insensitive on text
/// segments; digit segments scan forward greedily-minimally.
pub fn find(haystack: &str, marker: &[Segment]) -> Option<(usize, usize)> {
    if marker.is_empty() {
        return None;
    }
    let lower = haystack.to_lowercase();
    let first_text = match &marker[0] {
        Segment::Text(t) => t.to_lowercase(),
        Segment::Digits => return find_digit_led(&lower, marker),
    };
    let mut search_from = 0;
    while let Some(rel) = lower[search_from..].find(&first_text) {
        let abs = search_from + rel;
        if let Some(end) = continue_match(&lower, abs + first_text.len(), &marker[1..]) {
            return Some((abs, end));
        }
        search_from = abs + 1;
    }
    None
}

fn find_digit_led(lower: &str, marker: &[Segment]) -> Option<(usize, usize)> {
    let bytes = lower.as_bytes();
    for (idx, b) in bytes.iter().enumerate() {
        if b.is_ascii_digit() {
            let end = skip_digits(bytes, idx);
            if let Some(done) = continue_match(lower, end, &marker[1..]) {
                return Some((idx, done));
            }
        }
    }
    None
}

fn continue_match(lower: &str, mut pos: usize, rest: &[Segment]) -> Option<usize> {
    for seg in rest {
        match seg {
            Segment::Text(t) => {
                let t_low = t.to_lowercase();
                if !lower[pos..].starts_with(&t_low) {
                    return None;
                }
                pos += t_low.len();
            }
            Segment::Digits => {
                let bytes = lower.as_bytes();
                if bytes.get(pos).is_none_or(|b| !b.is_ascii_digit()) {
                    return None;
                }
                pos = skip_digits(bytes, pos);
            }
        }
    }
    Some(pos)
}

fn skip_digits(bytes: &[u8], mut i: usize) -> usize {
    while bytes.get(i).is_some_and(|b| b.is_ascii_digit()) {
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rstest::rstest]
    #[case("turn0search{N}", "see turn0search42 results", true)]
    #[case("[cite: {N}]", "gemini says [cite: 3, 12]", false)] // digits+commas: {N}=one run; comma breaks match
    #[case("{{index={N}}}", "contentReference[oaicite:16]{index=16}", true)]
    #[case("utm_source=chatgpt.com", "read at utm_source=chatgpt.com", true)]
    fn compile_and_find_cases(#[case] marker: &str, #[case] hay: &str, #[case] expect_hit: bool) {
        // Given a compiled ban marker.
        let segs = compile(marker).expect("compiles");

        // When hunting the sample.
        let hit = find(hay, &segs).is_some();

        // Then the hit expectation holds.
        assert_eq!(hit, expect_hit, "{marker:?} in {hay:?}");
    }

    #[test]
    fn real_brace_markers_compile_without_error() {
        // Given the ChatGPT contentReference marker (doubled real braces).
        let segs = compile("contentReference[oaicite:{N}]{{index={N}}}").expect("compiles");

        // Then segments interleave text and digit runs correctly.
        assert!(matches!(segs[0], Segment::Text(_)));
        assert!(segs.iter().filter(|s| matches!(s, Segment::Digits)).count() == 2);
    }

    #[test]
    fn unmatched_brace_is_an_error_pointing_at_author_intent() {
        // Given a term with a single un-doubled brace that is not {N}.
        let result = compile("[oaicite:16{index=16]");

        // Then compilation refuses with guidance.
        let err = result.expect_err("should error");
        assert!(err.contains("double it"), "{err}");
    }

    #[test]
    fn find_returns_char_safe_spans() {
        // Given unicode before a hit.
        let segs = compile("turn0search{N}").expect("compiles");
        let hay = "«é» turn0search7!";

        // When matching.
        let span = find(hay, &segs).expect("hit");

        // Then the slice is clean and exact.
        assert_eq!(&hay[span.0..span.1], "turn0search7");
    }
}
