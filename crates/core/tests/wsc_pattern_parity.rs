//! Policy parity: every wsc structural pattern passes the policy checker
//! VERBATIM. This pins the spec promise that upstream patterns compile
//! unchanged; the file path encodes the source snapshot.

use std::path::Path;

/// Extract the `pattern` strings from wsc's words.ts aiTellsPatterns block.
fn extract_patterns(ts: &str) -> Vec<String> {
    let mut out = Vec::new();
    let start = ts.find("aiTellsPatterns").expect("patterns block");
    let body = &ts[start..];
    let mut rest = body;
    while let Some(key) = rest.find("pattern: ") {
        rest = &rest[key + "pattern: ".len()..];
        // TS strings here use double or single quotes with escapes.
        let quote = rest.chars().next().expect("quote char");
        if quote != '"' && quote != '\'' {
            continue;
        }
        rest = &rest[1..];
        let mut value = String::new();
        let mut chars = rest.char_indices();
        while let Some((idx, c)) = chars.next() {
            match c {
                '\\' => {
                    // TS escape resolution: \\ is one literal backslash;
                    // \X stays verbatim so the regex engine sees \X.
                    let next = rest[idx..].chars().nth(1).expect("escaped char");
                    if next == '\\' {
                        value.push('\\');
                    } else {
                        value.push('\\');
                        value.push(next);
                    }
                    chars.next();
                }
                c if c == quote => {
                    rest = &rest[idx + 1..];
                    break;
                }
                c => value.push(c),
            }
        }
        out.push(value);
    }
    out
}

#[test]
fn all_wsc_structural_patterns_pass_policy_and_compile() {
    // Given the vendored wsc words.ts at the recorded snapshot.
    let ts_path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third-party/wsc/src/core/words.ts");
    let ts = std::fs::read_to_string(&ts_path).expect("read wsc words.ts");

    let patterns = extract_patterns(&ts);
    assert_eq!(patterns.len(), 12, "expected 12 wsc patterns");

    // When each is policy-checked AND compiled by fancy-regex.
    for (idx, pattern) in patterns.iter().enumerate() {
        let verdict = deslop_core::rule::policy::check(pattern);
        assert!(verdict.is_ok(), "pattern[{idx}] {pattern:?}: {verdict:?}");
        let compiled = regex::Regex::new(pattern);
        assert!(
            compiled.is_ok(),
            "pattern[{idx}] failed to compile: {compiled:?}"
        );
    }
}
