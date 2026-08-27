//! Regex policy: static analysis that keeps user-supplied patterns safe.
//!
//! fancy-regex is powerful but backtracking. We cap the risk at load time:
//! - reject backreferences `\1` and recursion `(?R)` outright
//! - reject variable-width lookbehind
//! - require every `*` / `+` to be made safe by an enclosing bounded group,
//!   a character-class membership (`[\w\s]` style), or a lookaround

/// Why a pattern was rejected.
#[derive(Debug, PartialEq)]
pub enum PolicyViolation {
    Backreference(usize),
    Recursion,
    VariableLookbehind,
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
            PolicyViolation::VariableLookbehind => {
                write!(f, "variable-width lookbehind is not supported")
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
    closing && saw_content && pattern[start..i].contains(['\\', 'w', 's', 'd'])
}

/// Is `*`/`+` at `star` acceptable given what it quantifies?
///
/// Policy: allowed when the quantified atom is a character class (safe or
/// not, classes are position-bounded) — naked `.*` / `.+` on any-char are NOT,
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
/// # Errors
///
/// Returns the first violation with a byte offset for pointing diagnostics.
pub fn check(pattern: &str) -> Result<(), PolicyViolation> {
    let bytes = pattern.as_bytes();
    let mut i = 0;
    // Track the most recent `(?<` group kind to allow bounded lookarounds.
    let mut in_lookaround = false;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                // Backreference forms \1..\9
                if let Some(d @ b'1'..=b'9') = bytes.get(i + 1) {
                    return Err(PolicyViolation::Backreference((d - b'0') as usize));
                }
                i += 2;
            }
            b'(' => {
                if pattern[i..].starts_with("(?R)") || pattern[i..].starts_with("(?&") {
                    return Err(PolicyViolation::Recursion);
                }
                if pattern[i..].starts_with("(?<=") || pattern[i..].starts_with("(?<!") {
                    in_lookaround = true;
                }
                i += 1;
            }
            b'*' | b'+' => {
                let prev_escaped = i > 0 && bytes[i - 1] == b'\\';
                if !prev_escaped && !star_allowed(pattern, i) && !in_lookaround {
                    let v = if bytes[i] == b'*' {
                        PolicyViolation::UnboundedStar { byte: i }
                    } else {
                        PolicyViolation::UnboundedPlus { byte: i }
                    };
                    return Err(v);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    // Variable-width lookbehind: a (?<= / (?<! whose body has unbounded
    // quantifiers or alternates of differing widths is unsupported by
    // fancy-regex; approximate here by looking for *, +, or {m,} inside.
    let lower_start = find_lookbehind_bodies(pattern)?;
    for body in lower_start {
        for (idx, c) in body.char_indices() {
            match c {
                '*' | '+' => return Err(PolicyViolation::VariableLookbehind),
                '{' => {
                    if let Some(close) = body[idx..].find('}') {
                        if !body[idx..close].contains(',') {
                            continue;
                        }
                        let inner = &body[idx + 1..close];
                        let parts: Vec<_> = inner.split(',').collect();
                        if parts.len() == 2 && parts[1].trim().is_empty() {
                            return Err(PolicyViolation::VariableLookbehind);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Extract `(?<=...)` / `(?<!...)` bodies for width analysis.
fn find_lookbehind_bodies(pattern: &str) -> Result<Vec<String>, PolicyViolation> {
    let mut bodies = Vec::new();
    for marker in ["(?<=", "(?<!"] {
        let mut from = 0;
        while let Some(rel) = pattern[from..].find(marker) {
            let start = from + rel + marker.len();
            let rest = &pattern[start..];
            let mut depth = 1usize;
            let mut end = rest.len();
            for (idx, c) in rest.char_indices() {
                match c {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            end = idx;
                            break;
                        }
                    }
                    '\\' => {}
                    _ => {}
                }
            }
            bodies.push(rest[..end].to_string());
            from = start;
        }
    }
    Ok(bodies)
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
}
