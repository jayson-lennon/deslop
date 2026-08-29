//! Boundary-safe byte-offset primitives for source-derived strings.
//!
//! All spans in deslop are byte offsets into ORIGINAL source text. Byte
//! arithmetic on those offsets can land mid-character on any multibyte
//! document; a raw `&src[start..end]` then panics. THE invariant of this
//! module: every offset produced or consumed here is a `char` boundary of
//! the string it indexes, so callers can never slice into the middle of a
//! character.
//!
//! These helpers never panic and never change behavior on aligned input:
//! on boundary-aligned offsets they are identity functions. The grapheme
//! segmenter (`unicode-segmentation`, see `render/truncate.rs`) remains the
//! tool for display work; this module is the tool for offset work.

/// Largest offset ≤ `off` that is a char boundary of `src`.
///
/// Offset 0 is always a boundary, so this terminates in at most three
/// steps (no valid UTF-8 character is longer than four bytes).
pub fn floor(src: &str, off: usize) -> usize {
    let mut o = off.min(src.len());
    while o > 0 && !src.is_char_boundary(o) {
        o -= 1;
    }
    o
}

/// The byte offset just before span `end`, floored to a boundary: the safe
/// replacement for `span.end - 1` (or `saturating_sub(1)`), which lands
/// mid-character whenever `end` sits just after a multibyte char.
pub fn end_boundary(src: &str, end: usize) -> usize {
    floor(src, end.saturating_sub(1))
}

/// Boundary-safe `src[start..end]`: both ends floored to char boundaries.
///
/// Returns `None` when the floored range is empty (including when both
/// ends collapse onto the same boundary) — callers must handle the miss
/// explicitly rather than index blindly.
pub fn slice_floor(src: &str, start: usize, end: usize) -> Option<&str> {
    let start = floor(src, start);
    let end = floor(src, end).max(start);
    src.get(start..end).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `héllo` is bytes `h(0) é(1..3) l(3) l(4) o(5)`, len 6; `ab…cd` is
    /// `a(0) b(1) …(2..5) c(5) d(6)`, so 3 and 4 are mid-character.
    #[test]
    fn floor_snaps_mid_char_offset_back_to_boundary() {
        // Given a string with a multibyte char at bytes 1..4.
        let src = "héllo";

        // When flooring an offset inside the `é`.
        // Then the enclosing boundary is returned.
        assert_eq!(floor(src, 2), 1);
    }

    #[test]
    fn floor_on_boundary_is_identity() {
        // Given an offset that is already a boundary.
        // When flooring.
        // Then nothing changes.
        assert_eq!(floor("héllo", 4), 4);
    }

    #[test]
    fn floor_beyond_len_clamps_to_len() {
        // Given an offset past the end of the string.
        // When flooring.
        // Then it clamps to len, which is always a boundary.
        assert_eq!(floor("héllo", 99), 6);
    }

    #[test]
    fn floor_at_zero_stays_zero() {
        // Given offset 0 on any string.
        // When flooring.
        // Then it stays 0 (empty strings included).
        assert_eq!(floor("", 0), 0);
        assert_eq!(floor("héllo", 0), 0);
    }

    #[test]
    fn end_boundary_is_end_minus_one_for_one_byte_char() {
        // Given a span ending after an ASCII char.
        // When taking the end boundary.
        // Then it is the byte before the end.
        assert_eq!(end_boundary("héllo", 6), 5);
    }

    #[test]
    fn end_boundary_snaps_back_inside_multibyte_char() {
        // Given a span ending just after a 3-byte char ("ab…" ends at 5).
        // When taking the end boundary.
        // Then it floors to 2, never 4 (inside the 3-byte ellipsis).
        assert_eq!(end_boundary("ab…", 5), 2);
    }

    #[test]
    fn end_boundary_at_zero_stays_zero() {
        // Given an empty (doc-level) span.
        // When taking the end boundary.
        // Then it is 0.
        assert_eq!(end_boundary("héllo", 0), 0);
    }

    #[test]
    fn slice_floor_returns_exact_text_on_aligned_range() {
        // Given a boundary-aligned range.
        // When slicing.
        // Then the exact substring comes back.
        assert_eq!(slice_floor("héllo world", 0, 6), Some("héllo"));
    }

    #[test]
    fn slice_floor_floors_both_ends_onto_boundaries() {
        // Given a range whose start lands mid-character ("ab…cd": bytes
        // 3 and 4 are inside the 3-byte ellipsis at 2..5).
        // When slicing.
        // Then both ends snap to enclosing boundaries first.
        assert_eq!(slice_floor("ab…cd", 3, 7), Some("…cd"));
    }

    #[test]
    fn slice_floor_is_none_when_range_collapses_empty() {
        // Given a range with both ends inside one char (floor(3) and
        // floor(4) both collapse onto the ellipsis's start boundary).
        // When slicing.
        // Then there is no slice to take.
        assert_eq!(slice_floor("ab…cd", 3, 4), None);
    }

    #[test]
    fn slice_floor_is_none_when_start_exceeds_end() {
        // Given an inverted range.
        // When slicing.
        // Then no slice is produced rather than a panic.
        assert_eq!(slice_floor("héllo", 4, 2), None);
    }
}
