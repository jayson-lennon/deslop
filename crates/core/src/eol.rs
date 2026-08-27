//! EOL normalization: `\r\n` -> `\n` with a bidirectional offset remap.
//!
//! Scanning runs on normalized text (regexes want `\n`-clean input); findings
//! and fixes apply to ORIGINAL bytes. `normalize` returns the map so every
//! normalized offset can translate back exactly.

/// Normalized text plus a remap: `orig[norm_map[i]] ..=` yields the original
/// byte that produced normalized byte `i`.
///
/// The vector length equals the normalized length. Since removals only ever
/// delete the `\r` of a `\r\n`, mapping is monotone; fixes can therefore
/// translate spans with two lookups.
#[derive(Debug, Clone)]
pub struct Normalized {
    pub text: String,
    /// norm byte offset -> original byte offset.
    pub norm_to_orig: Vec<usize>,
    /// Original byte length (for exclusive-end translation at EOF).
    pub orig_len: usize,
}

/// Normalize CRLF (and lone CR for robustness) to LF, recording origins.
pub fn normalize(src: &str) -> Normalized {
    let mut text = String::with_capacity(src.len());
    let mut norm_to_orig = Vec::with_capacity(src.len());
    let mut orig = 0usize;
    let bytes = src.as_bytes();
    while orig < src.len() {
        match bytes[orig] {
            b'\r' if bytes.get(orig + 1) == Some(&b'\n') => {
                text.push('\n');
                // The normalized \n corresponds to the ORIGINAL \n position.
                norm_to_orig.push(orig + 1);
                orig += 2;
            }
            b'\r' => {
                text.push('\n');
                norm_to_orig.push(orig);
                orig += 1;
            }
            _ => {
                // Copy one full UTF-8 char.
                let ch = src[orig..].chars().next().expect("char at boundary");
                for k in 0..ch.len_utf8() {
                    norm_to_orig.push(orig + k);
                }
                text.push(ch);
                orig += ch.len_utf8();
            }
        }
    }
    debug_assert_eq!(text.len(), norm_to_orig.len());
    Normalized {
        text,
        norm_to_orig,
        orig_len: src.len(),
    }
}

impl Normalized {
    /// Translate a normalized span to original coordinates.
    ///
    /// Ends are exclusive: `orig_end` is the origin of the first normalized
    /// byte AFTER the span (or original length at EOF).
    ///
    /// # Panics
    ///
    /// Only if `start` exceeds normalized length — a programming error.
    pub fn span_to_orig(&self, start: usize, end: usize) -> (usize, usize) {
        let orig_start = self.norm_to_orig[start.min(self.norm_to_orig.len())];
        let orig_end = if end >= self.norm_to_orig.len() {
            self.orig_len
        } else {
            self.norm_to_orig[end]
        };
        (orig_start, orig_end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_becomes_lf_with_exact_remap() {
        // Given a CRLF document.
        let src = "one\r\ntwo\r\nthree";
        let n = normalize(src);

        // Then normalization removes both CRs.
        assert_eq!(n.text, "one\ntwo\nthree");
        assert_eq!(n.text.len() + 2, n.orig_len);
    }

    #[test]
    fn spans_translate_back_through_removed_crs() {
        // Given a hit that sits after two CRLFs.
        let src = "a\r\nb\r\ndelve";
        let n = normalize(src);
        let start = n.text.find("delve").expect("present");
        let (o0, o1) = n.span_to_orig(start, start + "delve".len());

        // Then the ORIGINAL bytes are exactly "delve".
        assert_eq!(&src[o0..o1], "delve");
    }

    #[test]
    fn crlf_span_hitting_the_line_break_itself_translates() {
        // Given a match covering one line ending.
        let src = "x\r\ny";
        let n = normalize(src);

        // When translating the whole normalized text.
        let (o0, o1) = n.span_to_orig(0, n.text.len());

        // Then we get the full original doc back.
        assert_eq!(&src[o0..o1], src);
    }

    #[test]
    fn lone_cr_normalizes_like_crlf() {
        // Given an old-Mac style lone CR.
        let src = "x\ry";
        let n = normalize(src);

        // Then it becomes \n and round-trips.
        assert_eq!(n.text, "x\ny");
        let (o0, o1) = n.span_to_orig(0, n.text.len());
        assert_eq!(&src[o0..o1], src);
    }

    #[test]
    fn lf_only_docs_remap_identity() {
        // Given an already-normalized doc.
        let src = "héllo\nwörld";
        let n = normalize(src);

        // Then remap is identity across multibyte chars.
        assert_eq!(n.text, src);
        for (norm_i, orig_i) in n.norm_to_orig.iter().enumerate() {
            assert_eq!(norm_i, *orig_i);
        }
    }
}
