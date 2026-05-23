//! Helpers for the harness-owned `skill` tool.

/// Parse a raw skill query into lowercased search terms.
pub(super) fn normalized_skill_query_terms(raw_query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    for ch in raw_query.chars().flat_map(char::to_lowercase) {
        if ch.is_alphanumeric() || ch == '-' {
            current.push(ch);
        } else {
            push_normalized_skill_term(&mut terms, &mut current);
        }
    }
    push_normalized_skill_term(&mut terms, &mut current);
    terms
}

fn push_normalized_skill_term(terms: &mut Vec<String>, current: &mut String) {
    let term = current.trim_matches('-');
    if !term.is_empty() && !terms.iter().any(|existing| existing == term) {
        terms.push(term.to_owned());
    }
    current.clear();
}
