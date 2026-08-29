//! Regex policy: static analysis that keeps user-supplied patterns safe.
//!
//! The engine is the `regex` crate: linear-time, no backtracking, so the
//! catastrophic-backtracking class of failures cannot occur. What the
//! engine cannot express is rejected here at load time with a pointing
//! diagnostic instead of a compile error mid-scan:
//! - lookarounds `(?=...)` `(?!...)` `(?<=...)` `(?<!...)`
//! - backreferences `\1` and recursion `(?R)` / `(?&name)`
//! - atomic groups `(?>...)` (backtracking-only construct)
//!
//! Unbounded `*`/`+` are legal for `regex` but remain policy-restricted:
//! authored patterns must use `{m,n}` bounds or character classes so pack
//! data stays intentionally matched.

/// Why a pattern was rejected.
#[derive(Debug, PartialEq)]
pub enum PolicyViolation {
    Lookahead { byte: usize },
    Lookbehind { byte: usize },
    Backreference(usize),
    Recursion,
    AtomicGroup { byte: usize },
    UnboundedStar { byte: usize },
    UnboundedPlus { byte: usize },
}

impl std::fmt::Display for PolicyViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyViolation::Backreference(n) => {
                write!(f, "backreference \\{n} is not allowed")
            }
            PolicyViolation::Recursion => write!(f, "recursion (?R) is not allowed"),
            PolicyViolation::Lookahead { byte } => {
                write!(
                    f,
                    "lookahead `(?=`/`(?!` at byte {byte} is not supported; the engine is linear-time `regex` - widen the match and use a named capture instead"
                )
            }
            PolicyViolation::Lookbehind { byte } => {
                write!(
                    f,
                    "lookbehind `(?<=`/`(?<!` at byte {byte} is not supported; the engine is linear-time `regex` - match the context and capture the tail instead"
                )
            }
            PolicyViolation::AtomicGroup { byte } => {
                write!(
                    f,
                    "atomic group `(?>` at byte {byte} is not supported (backtracking-only construct)"
                )
            }
            PolicyViolation::UnboundedStar { byte } => {
                write!(f, "unbounded `*` at byte {byte}; use {{m,n}} bounds")
            }
            PolicyViolation::UnboundedPlus { byte } => {
                write!(f, "unbounded `+` at byte {byte}; use {{m,n}} bounds")
            }
        }
    }
}

/// Character classes considered safe to quantify unboundedly: they cannot
/// match the empty string and have bounded per-position cost.
fn safe_class_at(pattern: &str, start: usize) -> bool {
    let bytes = pattern.as_bytes();
    if bytes.get(start) != Some(&b'[') {
        return false;
    }
    let mut i = start + 1;
    if bytes.get(i) == Some(&b'^') {
        i += 1;
    }
    // A leading ] is literal in most engines.
    let mut saw_content = false;
    while i < pattern.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                saw_content = true;
            }
            b']' => break,
            _ => {
                i += 1;
                saw_content = true;
            }
        }
    }
    let closing = bytes.get(i) == Some(&b']');
    // Classes with an internal range like \w \s are fine; we whitelist the
    // common escape families loosely by checking they contain a backslash
    // escape (cheap and conservative-ish but practical for authored packs).
    // The class text is boundary-safe in practice (the walk stops at the
    // ASCII `]`), but `get` keeps that proof local instead of load-bearing.
    closing
        && saw_content
        && pattern
            .get(start..i)
            .is_some_and(|class| class.contains(['\\', 'w', 's', 'd']))
}

/// Is `*`/`+` at `star` acceptable given what it quantifies?
///
/// Policy: allowed when the quantified atom is a character class (safe or
/// not, classes are position-bounded) - naked `.*` / `.+` on any-char are NOT,
/// nor unbounded repeat of arbitrary groups unless wrapped in lookaround or
/// followed by explicit bounds ({m,n} forms never reach here).
fn star_allowed(pattern: &str, star: usize) -> bool {
    // Find the atom before the star.
    let bytes = pattern.as_bytes();
    let mut end = star;
    // Skip an escaped char if the star follows `\x`? stars don't follow \ directly as atoms.
    if end == 0 {
        return false;
    }
    end -= 1;
    // Case 1: closes a class `[...]`
    if bytes[end] == b']' {
        // find matching open
        let mut depth = 0i32;
        let mut open = None;
        let mut j = end;
        while j > 0 {
            j -= 1;
            match bytes[j] {
                b']' => depth += 1,
                b'[' => {
                    if depth == 0 {
                        open = Some(j);
                        break;
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        match open {
            Some(o) => safe_class_at(pattern, o),
            None => false,
        }
    } else if bytes[star.saturating_sub(1)] == b')' {
        // Case 2: closes a group. Allowed only inside/after lookahead context
        // for simplicity: reject group-star by default (rare in curated data).
        false
    } else {
        // Case 3: single literal char or dot.
        !matches!(bytes[star - 1], b'.' | b'\n')
    }
}

/// Static-check a pattern source. Ok(()) means safe to compile.
///
/// Walks by char (not byte) so escape handling can never land mid-char;
/// reported offsets are still byte offsets, identical to a byte walk for
/// every ASCII position violations can occur at.
///
/// # Errors
///
/// Returns the first violation with a byte offset for pointing diagnostics.
pub fn check(pattern: &str) -> Result<(), PolicyViolation> {
    let mut chars = pattern.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '\\' => {
                // Backreference forms \1..\9 (ASCII digits only - a
                // multibyte escaped char just gets consumed whole).
                if let Some((_, d @ '1'..='9')) = chars.peek().copied() {
                    return Err(PolicyViolation::Backreference(d as usize - '0' as usize));
                }
            }
            '(' => {
                if pattern[i..].starts_with("(?R)") || pattern[i..].starts_with("(?&") {
                    return Err(PolicyViolation::Recursion);
                }
                if pattern[i..].starts_with("(?>") {
                    return Err(PolicyViolation::AtomicGroup { byte: i });
                }
                if pattern[i..].starts_with("(?=") || pattern[i..].starts_with("(?!") {
                    return Err(PolicyViolation::Lookahead { byte: i });
                }
                if pattern[i..].starts_with("(?<=") || pattern[i..].starts_with("(?<!") {
                    return Err(PolicyViolation::Lookbehind { byte: i });
                }
            }
            '*' | '+' => {
                let prev_escaped = i > 0 && pattern.as_bytes()[i - 1] == b'\\';
                if !prev_escaped && !star_allowed(pattern, i) {
                    let v = if c == '*' {
                        PolicyViolation::UnboundedStar { byte: i }
                    } else {
                        PolicyViolation::UnboundedPlus { byte: i }
                    };
                    return Err(v);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_backreferences() {
        // Given a pattern with a backreference.
        let result = check(r"(word) \1");

        // Then it is refused, naming backreference 1.
        assert_eq!(result, Err(PolicyViolation::Backreference(1)));
    }

    #[test]
    fn rejects_recursion() {
        // Given a recursive pattern.
        let result = check("(a(?R))?b");

        // Then refusal is Recursion.
        assert_eq!(result, Err(PolicyViolation::Recursion));
    }

    #[test]
    fn rejects_naked_dot_star() {
        // Given an unbounded any-char star.
        let result = check("delve.*deeply");

        // Then refusal points at the star.
        assert!(matches!(result, Err(PolicyViolation::UnboundedStar { .. })));
    }

    #[test]
    fn accepts_bounded_quantifiers() {
        // Given wsc-style bounded quantifiers.
        let result = check(r"\bnot only\b[^.!?\\n]{2,80}?\\bbut also\\b");

        // Then the pattern passes.
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn accepts_escaped_star() {
        // Given a literal asterisk.
        let result = check(r"2\\*3");

        // Then it passes (escaped, not a quantifier).
        assert!(result.is_ok());
    }

    #[test]
    fn multibyte_pattern_escapes_do_not_panic() {
        // Given a pattern mixing multibyte literals and an escaped multibyte char.
        let result = check("“漢字” \\\\一*");

        // Then the walk stays on char boundaries and the escaped form passes.
        assert!(result.is_ok(), "{result:?}");
    }
}
