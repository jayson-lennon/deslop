//! Mechanical inflection expansion for vocab terms.
//!
//! NOT a lemmatizer: a deterministic suffixer with a min-length guard. Junk
//! forms are harmless dead weight (lookups go through the word index), but
//! missing forms would be silent misses - so we over-generate and dedupe.
//!
//! All suffix surgery goes through `strip_suffix`, never byte slicing: the
//! non-ASCII guard makes the ASCII-only fast path provably safe, but the
//! stem operations below are boundary-safe by construction regardless.

/// Expand `term` into its plausible surface forms (always includes term).
pub fn expand(term: &str) -> Vec<String> {
    let base = term.trim().to_lowercase();
    if base.chars().count() < 3 || !base.chars().all(|c| c.is_ascii_alphabetic()) {
        return vec![base];
    }

    let mut forms = vec![base.clone()];

    // --- plural / 3rd person singular: -s, -es, y->ies ---
    if base.ends_with(['s', 'x', 'z']) || base.ends_with("ch") || base.ends_with("sh") {
        forms.push(format!("{base}es"));
    } else if let Some(stem) = strip_consonant_y(&base) {
        forms.push(format!("{stem}ies"));
        forms.push(format!("{stem}ys")); // plastics vs. plys
    } else {
        forms.push(format!("{base}s"));
    }

    // --- past / progressive with e-drop and CVC-doubling variants ---
    let ed = if let Some(stem) = base.strip_suffix('e') {
        vec![format!("{base}d"), format!("{stem}ed")]
    } else if let Some(stem) = strip_consonant_y(&base) {
        vec![format!("{stem}ied"), format!("{stem}yed")]
    } else {
        let mut v = vec![format!("{base}ed")];
        if let Some(doubled) = double_final_consonant(&base) {
            v.push(format!("{doubled}ed"));
        }
        v
    };
    forms.extend(ed);

    let ing = if let Some(stem) = base.strip_suffix('e') {
        vec![format!("{stem}ing")]
    } else if let Some(stem) = strip_consonant_y(&base) {
        vec![format!("{stem}ying")]
    } else {
        let mut v = vec![format!("{base}ing")];
        if let Some(doubled) = double_final_consonant(&base) {
            v.push(format!("{doubled}ing"));
        }
        v
    };
    forms.extend(ing);

    forms.sort();
    forms.dedup();
    forms
}

/// `word` minus a consonant-y ending (`study` -> `stud`), or `None` when the
/// word does not end in consonant-y.
fn strip_consonant_y(word: &str) -> Option<&str> {
    let stem = word.strip_suffix('y')?;
    let prev_vowel = stem
        .bytes()
        .next_back()
        .is_some_and(|b| matches!(b, b'a' | b'e' | b'i' | b'o' | b'u'));
    (!prev_vowel).then_some(stem)
}

/// CVC ending doubling (consonant-vowel-CONSONANT: stop/stopped).
fn double_final_consonant(word: &str) -> Option<String> {
    const DOUBLABLE: [u8; 7] = [b'b', b'd', b'g', b'm', b'n', b'p', b't'];
    let bytes = word.as_bytes();
    let &last = bytes.last()?;
    if !DOUBLABLE.contains(&last) {
        return None;
    }
    // Need vowel-consonant ending (>= 3 chars).
    if bytes.len() < 3 {
        return None;
    }
    let mid = bytes[bytes.len() - 2];
    let pre = bytes[bytes.len() - 3];
    let is_vowel = |c: u8| matches!(c, b'a' | b'e' | b'i' | b'o' | b'u');
    // Require a vowel immediately before the final consonant
    // (stop->stopped) and not two vowels (need->needing, no doubling).
    if !is_vowel(mid) || is_vowel(pre) {
        return None;
    }
    Some(format!("{word}{}", last as char))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rstest::rstest]
    #[case("delve", &["delve", "delved", "delves", "delving"])]
    #[case("stop", &["stop", "stopped", "stopping", "stops"])]
    #[case("make", &["make", "maked", "making", "makes"])]
    #[case("study", &["studied", "study", "studying", "studies"])]
    #[case("showcase", &["showcase", "showcased", "showcases", "showcasing"])]
    fn expand_generates_expected_core_forms(#[case] term: &str, #[case] expected: &[&str]) {
        // Given a base term.
        let forms = expand(term);

        // Then each expected surface form is present.
        for want in expected {
            assert!(forms.iter().any(|f| f == want), "{term} -> {forms:?}");
        }
    }

    #[test]
    fn short_terms_are_not_expanded() {
        // Given a sub-minimal term.
        let forms = expand("ai");

        // Then only the base returns (guard against junk).
        assert_eq!(forms, vec!["ai"]);
    }

    #[test]
    fn non_alpha_terms_pass_through_untouched() {
        // Given a hyphenated phrase.
        let forms = expand("shine-like");

        // Then no suffix machinery applies.
        assert_eq!(forms, vec!["shine-like"]);
    }

    #[test]
    fn expansion_is_idempotent_and_sorted() {
        // Given the output of one expansion.
        let forms = expand("underscore");

        // Then re-expanding any member yields a sorted, deduped list.
        for form in &forms {
            let again = expand(form);
            let mut sorted = again.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(again, sorted);
        }
    }

    #[test]
    fn multibyte_term_returns_just_the_base_without_panicking() {
        // Given a term ending in a multibyte character.
        let forms = expand("café");

        // Then only the base returns: the guard rejects it before any
        // suffix machinery runs.
        assert_eq!(forms, vec!["café"]);
    }

    #[rstest::rstest]
    #[case("ply", &["plied", "plies", "ply", "plyed", "plying", "plys"])]
    #[case(
        "study",
        &["studied", "studies", "study", "studying", "studyed", "studys"]
    )]
    #[case("make", &["make", "maked", "making", "makes"])]
    fn expand_full_form_lists_are_pinned_byte_for_byte(
        #[case] term: &str,
        #[case] expected: &[&str],
    ) {
        // Given terms exercising every suffix branch.
        let mut forms = expand(term);

        // When normalizing the expectation the same way.
        let mut want: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        want.sort();
        want.dedup();
        forms.sort();

        // Then the full generated set matches exactly — new or missing
        // forms would silently change what the word index can hit.
        assert_eq!(forms, want, "{term}");
    }
}
