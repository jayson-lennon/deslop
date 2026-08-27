//! wsc converter: extracts arrays out of words.ts with a tolerant scanner.
//!
//! TS is not parsed — we locate `export const NAME...= [` and read
//! brace-delimited object literals until the matching closing bracket,
//! resolving the handful of escapes wsc uses ('', \', \b, \/).

/// One extracted pattern row.
#[derive(Debug, Clone)]
pub struct TsPattern {
    pub name: String,
    pub pattern: String,
    pub reason: String,
}

/// Collapse JS string escapes to their real characters. Only `\\` pairs
/// become a single backslash, plus quote identity escapes; regex escapes
/// like `\b` arrive as TWO source chars (`\\` + `b`) and survive as `\b`.
fn resolve_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some(&n) if n == '\'' || n == '"' || n == '/' || n == '\\' => {
                    out.push(n);
                    chars.next();
                }
                Some(_) => {
                    // Unknown escape: keep backslash + char verbatim so the
                    // regex engine sees the original sequence.
                    out.push('\\');
                    out.push(chars.next().unwrap_or('\\'));
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Extract one string field from an object-literal fragment.
fn field(fragment: &str, name: &str) -> Option<String> {
    let idx = fragment.find(&format!("{name}:"))?;
    let rest = &fragment[idx + name.len() + 1..];
    let (qchar, body) = {
        let mut chars = rest.trim_start().chars();
        let q = chars.next()?;
        (q, chars.collect::<String>())
    };
    if qchar != '\'' && qchar != '"' {
        return None;
    }
    let mut acc = String::new();
    let mut escaped = false;
    for c in body.chars() {
        if escaped {
            acc.push(c);
            escaped = false;
            continue;
        }
        match c {
            '\\' => {
                acc.push(c);
                escaped = true;
            }
            _ if c == qchar => break,
            _ => acc.push(c),
        }
    }
    Some(resolve_escapes(&acc))
}

/// All `export const X ... = [ ... ]` object rows for a given const name.
///
/// String rules (verified against the snapshot): single-quoted strings honor
/// `\\` escapes AND `''` doubling; double-quoted strings honor `\\` escapes.
/// Brackets/braces inside strings never affect structure tracking.
pub fn extract(source: &str, const_name: &str) -> Vec<String> {
    let marker = format!("export const {const_name}");
    let start = match source.find(&marker) {
        Some(s) => s,
        None => return Vec::new(),
    };
    // Anchor on `= [` — the const's type annotation may contain [ ] (e.g.
    // `variants?: string[]`), so a bare bracket search false-positives.
    let open_bracket = match source[start..].find("= [") {
        Some(r) => start + r + 1,
        None => return Vec::new(),
    };

    // Find the array's matching close with string-aware bracket tracking;
    // returns byte offset of the close bracket.
    let close = match scan_close(source, open_bracket) {
        Some(c) => c,
        None => return Vec::new(),
    };
    let body = &source[open_bracket + 1..close];
    split_rows(body)
}

/// Byte offset of the `]` matching the `[` at `open`, or None.
fn scan_close(source: &str, open: usize) -> Option<usize> {
    let chars: Vec<char> = source.chars().collect();
    let mut in_str: Option<char> = None;
    let mut esc = false;
    let mut depth = 0i32;
    // `open` is a BYTE offset into the same string; chars[open] would be
    // wrong when multibyte chars precede it — convert properly:
    let mut i = source[..open].chars().count();
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if q == '\'' && c == '\'' && chars.get(i + 1) == Some(&'\'') {
                i += 1; // doubled-quote pair consumed as content
            } else if c == q {
                in_str = None;
            }
            i += 1;
            continue;
        }
        match c {
            '\'' | '"' => in_str = Some(c),
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    // Convert the char index back to a byte offset.
                    let byte = source
                        .char_indices()
                        .nth(i)
                        .map(|(b, _)| b)
                        .unwrap_or(source.len());
                    return Some(byte);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn split_rows(body: &str) -> Vec<String> {
    let mut rows: Vec<String> = Vec::new();
    let mut obj_depth = 0i32;
    let mut in_str: Option<char> = None;
    let mut esc = false;
    let mut current = String::new();
    let mut chars = body.char_indices().peekable();
    while let Some((_, c)) = chars.next() {
        if let Some(q) = in_str {
            current.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == q {
                in_str = None;
            } else if q == '\'' && c == '\'' && peek_is(&mut chars, '\'') {
                chars.next(); // consume the second of the '' pair
            }
            continue;
        }
        match c {
            '\'' | '"' => {
                in_str = Some(c);
                current.push(c);
            }
            '{' => {
                obj_depth += 1;
                current.clear();
            }
            '}' => {
                obj_depth -= 1;
                if obj_depth == 0 && !current.trim().is_empty() {
                    rows.push(std::mem::take(&mut current));
                }
            }
            _ if obj_depth > 0 => current.push(c),
            _ => {}
        }
    }
    rows
}

fn peek_is<I: Iterator<Item = (usize, char)>>(chars: &mut std::iter::Peekable<I>, c: char) -> bool {
    chars.peek().is_some_and(|&(_, ch)| ch == c)
}

/// aiTellsPatterns as typed rows; fragments lacking name/pattern are
/// reported to stderr so coverage regressions are visible at convert time.
pub fn patterns(source: &str) -> Vec<TsPattern> {
    extract(source, "aiTellsPatterns")
        .into_iter()
        .filter_map(|frag| {
            let name = match field(&frag, "name") {
                Some(n) => n,
                None => {
                    eprintln!("wsc: skipped pattern fragment (no name): {:.80}", frag);
                    return None;
                }
            };
            let pat = match field(&frag, "pattern") {
                Some(p) => p,
                None => {
                    eprintln!("wsc: skipped pattern `{name}` (no pattern field)");
                    return None;
                }
            };
            Some(TsPattern {
                name,
                pattern: pat,
                reason: field(&frag, "reason").unwrap_or_default(),
            })
        })
        .collect()
}

/// aiTellsVocabulary words (+variants) with reasons.
pub fn vocabulary(source: &str) -> Vec<crate::slop_json::RawTerm> {
    extract(source, "aiTellsVocabulary")
        .into_iter()
        .filter_map(|frag| {
            let word = field(&frag, "word")?;
            let reason = field(&frag, "reason").unwrap_or_default();
            let mut terms = vec![crate::slop_json::RawTerm {
                term: word,
                replacement: None,
                evidence: format!("wsc: {reason}"),
                source: "wsc".into(),
                severity: None,
            }];
            if let Some(variants_raw) = frag.find("variants:") {
                let variants = field(&frag[variants_raw..], "");
                for v in variants.unwrap_or_default().split(',') {
                    let t = v.trim().trim_matches(['\'', '"']);
                    if !t.is_empty() {
                        terms.push(crate::slop_json::RawTerm {
                            term: t.to_string(),
                            replacement: None,
                            evidence: format!("wsc variant: {reason}"),
                            source: "wsc".into(),
                            severity: None,
                        });
                    }
                }
            }
            Some(terms)
        })
        .flatten()
        .collect()
}

/// aiTellsPhrases with reasons.
pub fn phrases(source: &str) -> Vec<crate::slop_json::RawTerm> {
    extract(source, "aiTellsPhrases")
        .into_iter()
        .filter_map(|frag| {
            Some(crate::slop_json::RawTerm {
                term: field(&frag, "phrase")?,
                replacement: None,
                evidence: format!("wsc phrase: {}", field(&frag, "reason").unwrap_or_default()),
                source: "wsc".into(),
                severity: None,
            })
        })
        .collect()
}

#[doc(hidden)]
pub fn wsc_extract_test(source: &str) -> Vec<String> {
    extract(source, "aiTellsPatterns")
        .into_iter()
        .enumerate()
        .map(|(i, f)| format!("{i}: {:.60}", f))
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn extracts_vocabulary_word_count() {
        let src = std::fs::read_to_string("../../third-party/wsc/src/core/words.ts").unwrap();
        let rows = super::extract(&src, "aiTellsVocabulary");
        eprintln!("vocab object rows: {}", rows.len());
        assert!(rows.len() > 90, "got {}", rows.len());
    }

    #[test]
    fn extracts_all_12_pattern_rows() {
        let src = std::fs::read_to_string("../../third-party/wsc/src/core/words.ts")
            .expect("wsc snapshot present");
        let rows = super::extract(&src, "aiTellsPatterns");
        assert_eq!(rows.len(), 12, "rows: {rows:#?}");
    }

    #[test]
    fn extracts_all_vocabulary_rows() {
        let src = std::fs::read_to_string("../../third-party/wsc/src/core/words.ts")
            .expect("wsc snapshot present");
        let words = super::vocabulary(&src);
        assert!(!words.is_empty(), "no vocabulary rows extracted");
    }
}
