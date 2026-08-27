//! Input document: original source plus normalization bookkeeping.

/// A document under lint.
///
/// `src` is the exact bytes as read (used for spans, excerpts and fixes);
/// downstream stages derive a normalized copy with an offset remap so CRLF
/// documents still fix correctly against the original.
#[derive(Debug, Clone)]
pub struct Doc {
    pub path: camino::Utf8PathBuf,
    pub src: String,
}

impl Doc {
    pub fn from_source(path: camino::Utf8PathBuf, src: impl Into<String>) -> Doc {
        Doc {
            path,
            src: src.into(),
        }
    }

    /// The excerpt for a span, or `None` if the span is not char-safe
    /// within `src` (internal invariant violation; surfaced, never panic).
    pub fn slice(&self, span: crate::finding::Span) -> Option<&str> {
        span.slice(&self.src)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::finding::Span;

    #[test]
    fn slice_returns_exact_excerpt() {
        // Given a document.
        let doc = Doc::from_source("t.md".into(), "héllo world");

        // When slicing a multibyte-spanning range.
        let got = doc.slice(Span::new(0, 6));

        // Then the full "héllo" comes back intact.
        assert_eq!(got, Some("héllo"));
    }

    #[test]
    fn slice_rejects_mid_char_boundaries() {
        // Given a doc whose first char is multibyte.
        let doc = Doc::from_source("t.md".into(), "éoù");

        // When slicing at byte 1 (inside 'é').
        let got = doc.slice(Span::new(1, 2));

        // Then no panic occurs; None signals the invalid span.
        assert_eq!(got, None);
    }
}
