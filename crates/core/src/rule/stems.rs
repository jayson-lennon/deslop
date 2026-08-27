//! Mechanical inflection expansion for vocab terms.
//!
//! NOT a lemmatizer: a deterministic suffixer with a min-length guard. Junk
//! forms are harmless dead weight (lookups go through the word index), but
//! missing forms would be silent misses — so we over-generate and dedupe.

/// Expand `term` into its plausible surface forms (always includes term).
pub fn expand(term: &str) -> Vec<String> {
    let base = term.trim().to_lowercase();
    if base.chars().count() < 3 || !base.chars().all(|c| c.is_ascii_alphabetic()) {
        return vec![base];
    }

    let mut forms = vec![base.clone()];
    let last = base.chars().last().unwrap_or_default();

    // --- plural / 3rd person singular: -s, -es, y->ies ---
    if matches!(last, 's' | 'x' | 'z') || base.ends_with("ch") || base.ends_with("sh") {
        forms.push(format!("{base}es"));
    } else if ends_consonant_y(&base) {
        forms.push(format!("{}ies", &base[..base.len() - 1]));
        forms.push(format!("{}ys", &base[..base.len() - 1])); // plastics vs. plys
    } else {
        forms.push(format!("{base}s"));
    }

    // --- past / progressive with e-drop and CVC-doubling variants ---
    let ed = if ends_with_e(&base) {
        vec![format!("{base}d"), format!("{}ed", &base[..base.len() - 1])]
    } else if ends_consonant_y(&base) {
        let stem = &base[..base.len() - 1];
        vec![format!("{stem}ied"), format!("{}yed", stem)]
    } else {
        let mut v = vec![format!("{base}ed")];
        if let Some(doubled) = double_final_consonant(&base) {
            v.push(format!("{doubled}ed"));
        }
        v
    };
    forms.extend(ed);

    let ing = if ends_with_e(&base) {
        vec![format!("{}ing", &base[..base.len() - 1])]
    } else if ends_consonant_y(&base) {
        vec![format!("{}ying", &base[..base.len() - 1])]
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

fn ends_with_e(word: &str) -> bool {
    word.ends_with('e')
}

fn ends_consonant_y(word: &str) -> bool {
    let bytes = word.as_bytes();
    bytes.last() == Some(&b'y')
        && bytes
            .get(bytes.len() - 2)
            .is_some_and(|b| !matches!(b, b'a' | b'e' | b'i' | b'o' | b'u'))
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
}
