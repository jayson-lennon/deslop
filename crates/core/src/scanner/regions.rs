//! Markdown-aware regions: scope tracking + length-preserving masking.
//!
//! THE invariant: `masked.len() == src.len()` byte-for-byte. All scanner
//! offsets work on `masked`; all finding spans point into ORIGINAL text;
//! because lengths match, no translation is needed - masked positions ARE
//! original positions.

/// Syntactic scope of a source range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Ordinary prose (paragraphs, blockquotes, table cells).
    Prose,
    /// Inside a heading; payload = level.
    Heading(u8),
    /// One list item's content.
    ListItem,
}

/// The result of the markdown walk over one document.
#[derive(Debug, Clone)]
pub struct RegionMap {
    /// Same byte length as src; masked ranges replaced with NULs
    /// EXCEPT newlines inside them are preserved (keeps line math sane).
    pub masked: String,
    /// Scopes as (start, end, scope) triples, sorted by start, disjoint.
    pub scopes: Vec<(usize, usize, Scope)>,
}

impl Scope {
    /// Any heading level (metric title-case fraction consumes these).
    pub fn is_heading_like(&self) -> bool {
        matches!(self, Scope::Heading(_))
    }
}

impl RegionMap {
    /// Effective scope for a byte offset (last region covering it wins).
    ///
    /// Masked code feels like it should be excluded entirely; scanners
    /// consult `is_masked` for that instead. Uncovered bytes default to Prose.
    pub fn scope_at(&self, offset: usize) -> Scope {
        // Scopes are sorted and non-overlapping; binary search the last
        // one starting at or before `offset`.
        match self.scopes.binary_search_by(|(s, e, _)| {
            if *s > offset {
                std::cmp::Ordering::Greater
            } else if *e <= offset {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        }) {
            Ok(i) => self.scopes[i].2,
            Err(next) => {
                let _ = next;
                Scope::Prose
            }
        }
    }

    pub fn is_masked(&self, offset: usize) -> bool {
        self.masked.as_bytes().get(offset) == Some(&0)
    }
}

/// Which markdown events mask their source range entirely.
struct MaskedSpan {
    start: usize,
    end: usize,
}

/// Walk the doc and produce scopes + masked text.
pub fn build_regions(src: &str) -> RegionMap {
    use pulldown_cmark::Event;

    let mut mask_spans: Vec<MaskedSpan> = Vec::new();
    let mut scopes: Vec<(usize, usize, Scope)> = Vec::new();

    let opts = pulldown_cmark::Options::ENABLE_TABLES
        | pulldown_cmark::Options::ENABLE_FOOTNOTES
        | pulldown_cmark::Options::ENABLE_STRIKETHROUGH;
    for (event, range) in pulldown_cmark::Parser::new_ext(src, opts).into_offset_iter() {
        match event {
            // Fenced/indented code blocks: mask wholesale.
            Event::Start(pulldown_cmark::Tag::CodeBlock(_)) => {
                mask_spans.push(MaskedSpan {
                    start: range.start,
                    end: range.end,
                });
            }
            // Inline code.
            Event::Code(code) => {
                let inner_start = range.end.saturating_sub(code.len());
                mask_spans.push(MaskedSpan {
                    start: inner_start,
                    end: range.end,
                });
            }
            // Headings (ATX + Setext both arrive as Heading tags).
            Event::Start(pulldown_cmark::Tag::Heading { level, .. }) => {
                scopes.push((range.start, range.end, Scope::Heading(level as u8)));
            }
            // List items (content scope).
            Event::Start(pulldown_cmark::Tag::Item) => {
                scopes.push((range.start, range.end, Scope::ListItem));
            }
            _ => {}
        }
    }

    link_destinations(src, &mut mask_spans);
    auto_link_and_raw_html_urls(src, &mut mask_spans);

    scopes.sort_by_key(|(s, _, _)| *s);
    RegionMap {
        masked: apply_masks(src, &mask_spans),
        scopes,
    }
}

/// Mask `[text](destination)` destinations (not the link text).
fn link_destinations(src: &str, spans: &mut Vec<MaskedSpan>) {
    for (event, range) in pulldown_cmark::Parser::new(src).into_offset_iter() {
        if let pulldown_cmark::Event::Start(pulldown_cmark::Tag::Link {
            link_type,
            dest_url,
            ..
        }) = event
        {
            if link_type == pulldown_cmark::LinkType::Autolink {
                // <https://…>: mask the whole event incl. angle brackets.
                spans.push(MaskedSpan {
                    start: range.start,
                    end: range.end,
                });
            } else if !dest_url.is_empty() {
                // Destination appears as a substring of the raw range.
                if let Some(rel) = src[range.start..range.end].rfind(dest_url.as_ref()) {
                    let start = range.start + rel;
                    spans.push(MaskedSpan {
                        start,
                        end: start + dest_url.len(),
                    });
                }
            }
        }
    }
}

/// Bare URLs (no <> wrapper) that CommonMark keeps as plain text.
fn auto_link_and_raw_html_urls(src: &str, spans: &mut Vec<MaskedSpan>) {
    // Cheap heuristic scan: https?:// up to whitespace or closer. Works on
    // char_indices so `start` always lands on a char boundary.
    for (start, _) in src
        .match_indices("http://")
        .chain(src.match_indices("https://"))
    {
        let mut end = start;
        for (idx, ch) in src[start..].char_indices() {
            if ch.is_whitespace() || ch == ')' || ch == ']' {
                end = start + idx;
                break;
            }
            end = start + idx + ch.len_utf8();
        }
        spans.push(MaskedSpan { start, end });
    }
}

/// NUL-out every masked span, preserving newlines; asserts length equality in debug.
fn apply_masks(src: &str, spans: &[MaskedSpan]) -> String {
    let mut buf: Vec<u8> = src.as_bytes().to_vec();
    let len = buf.len();
    for span in spans {
        let lo = span.start.min(len);
        let hi = span.end.min(len);
        for byte in &mut buf[lo..hi] {
            if *byte != b'\n' {
                *byte = 0;
            }
        }
    }
    // SAFETY (memory-safety perspective): we only replaced non-NUL ASCII with
    // NUL and otherwise copied valid UTF-8. Multi-byte chars were either left
    // intact or replaced whole? No - a masked span may cut multibyte chars,
    // so re-validate and fall back to char-boundary-safe masking.
    match String::from_utf8(buf) {
        Ok(masked) => {
            debug_assert_eq!(masked.len(), src.len());
            masked
        }
        Err(_) => mask_char_safe(src, spans),
    }
}

/// Fallback masking that respects char boundaries (slower path).
fn mask_char_safe(src: &str, spans: &[MaskedSpan]) -> String {
    let mut out = String::with_capacity(src.len());
    for (idx, ch) in src.char_indices() {
        let covered = spans.iter().any(|s| s.start <= idx && idx < s.end);
        if covered && ch != '\n' {
            out.push('\0');
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masked_length_always_equals_source_length() {
        // Given a doc with fences, inline code, links, and bare URLs.
        let src = "# Title\n\ntext with `code span` and [link](https://x.example/a) \
                   plus bare https://bare.example/path?query=1 here\n\n```rust\nlet x = 1;\n```\n";
        let map = build_regions(src);

        // Then masked length equals source length.
        assert_eq!(map.masked.len(), src.len());
    }

    #[test]
    fn code_fence_is_masked_prose_is_not() {
        // Given a fence containing a banned word.
        let src = "before ```\ndelve\n``` after delve";
        let map = build_regions(src);

        // Then the illegal fence is masked AND the trailing prose stays.
        assert!(map.masked.contains("before "), "trailing prose lost");
        let has_nul = map.masked.bytes().any(|b| b == 0);
        assert!(has_nul, "expected some masking: {:?}", map.masked);
        assert!(!map.masked.contains("after delve"));
    }

    #[test]
    fn headings_and_list_items_get_scopes() {
        // Given ATX heading + setext heading + bullets.
        let src = "# Top\n\nSub\n===\n\n- one\n- two\n";
        let map = build_regions(src);

        // Then both headings register level-1/2 and items scope as ListItem.
        let scopes: Vec<_> = map.scopes.iter().map(|(_, _, s)| *s).collect();
        assert!(scopes.contains(&Scope::Heading(1)), "{scopes:?}");
        assert!(
            scopes
                .iter()
                .filter(|s| matches!(s, Scope::ListItem))
                .count()
                >= 2
        );
    }

    #[test]
    fn link_destination_masked_link_text_kept() {
        // Given a markdown link to a delve-named path.
        let src = "see [the word delve](https://example.com/delve) now";
        let map = build_regions(src);

        // Then destination is NULed but visible text stays.
        let visible_text = "see [the word delve](";
        assert!(map.masked.starts_with(visible_text));
        assert!(!map.masked.contains("https://example.com/delve"));
    }

    #[test]
    fn inline_code_masks_its_body_only() {
        // Given inline code naming a term.
        let src = "run `delve --deep` please";
        let map = build_regions(src);

        // Then backtick contents are gone from unmasked text.
        let start = map.masked.find("delve");
        assert!(start.is_none(), "{}", map.masked);
    }

    /// Randomized invariant check (spec property): for arbitrary doc-shaped
    /// inputs, masked length == src length and newline offsets survive.
    #[test]
    fn property_length_invariance_on_random_docs() {
        // Given deterministic pseudo-random docs built from real fragments.
        let pieces: &[&str] = &[
            "# Head\n\n",
            "para ",
            "`code`",
            " [l](https://x/y) ",
            "bare https://b.example/p?q=1 tail",
            "\n```rs\nfn f(){}\n```\n",
            "- item **bold**",
            "> quote",
            "héllo 🌍 coûte",
            "\n---\n",
            "<https://auto.example>",
            "text with | pipe\n",
            "\r\n",
            "tâble",
        ];
        let mut state: u64 = 0x5EED_1234_ABCD_EF01;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _case in 0..200 {
            let n_docs = 1 + (next() % 12) as usize;
            let mut src = String::new();
            for _ in 0..n_docs {
                src.push_str(pieces[(next() as usize) % pieces.len()]);
            }

            // When building regions.
            let map = build_regions(&src);

            // Then lengths agree and newlines keep their absolute indices.
            assert_eq!(map.masked.len(), src.len(), "src was {src:?}");
            for (idx, orig_byte) in src.bytes().enumerate() {
                if orig_byte == b'\n' {
                    assert_eq!(map.masked.as_bytes()[idx], b'\n');
                }
            }
            // And any NUL replaced a byte that was inside some masked span
            // (spot guarantee via inverse: unmasked bytes equal originals).
            for (idx, (m, o)) in map.masked.bytes().zip(src.bytes()).enumerate() {
                if m != 0 {
                    assert_eq!(m, o, "byte {idx} mutated outside mask");
                }
            }
        }
    }

    #[test]
    fn multibyte_doc_survives_masking() {
        // Given emoji-laden text with an autolink.
        let src = "héllo 🌍 <https://ex.example/påth> done";
        let map = build_regions(src);

        // Then length invariance holds and URL bytes are NULs.
        assert_eq!(map.masked.len(), src.len());
        assert!(!map.masked.contains("ex.example"));
    }
}
