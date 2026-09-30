use regex::Regex;

use super::parser::RuleToken;

/// Apply a chain of rule tokens against extracted strings.
///
/// Handles `||` fallback, `##` regex replacement, and `!0` index exclusion.
pub fn apply_combinators(values: Vec<String>, tokens: &[RuleToken]) -> Vec<String> {
    let mut results = values;
    let mut remaining = tokens;

    while !remaining.is_empty() {
        let token = &remaining[0];
        remaining = &remaining[1..];

        match token {
            RuleToken::Fallback => {
                // If we already have results, stop — || means "use first non-empty"
                if !results.is_empty() && !results.iter().all(|s| s.trim().is_empty()) {
                    return results;
                }
                // Otherwise clear and continue with next segment
                results.clear();
            }
            RuleToken::RegexReplace { pattern, replacement } => {
                if let Ok(re) = Regex::new(pattern) {
                    results = results
                        .into_iter()
                        .map(|s| re.replace_all(&s, replacement).to_string())
                        .collect();
                }
            }
            // Index / ExcludeIndex are fully consumed by the HTML/JSON
            // extractors (`.n` takes, `!n` excludes). Applying them again here
            // would double-apply and delete already-extracted values, so they
            // are intentionally ignored.
            _ => {}
        }
    }

    results
}

/// Merge multiple rule result sets using `||` semantics:
/// Return the first non-empty set, or empty if all empty.
pub fn merge_fallback(results: &[Vec<String>]) -> Vec<String> {
    for r in results {
        if !r.is_empty() && !r.iter().all(|s| s.trim().is_empty()) {
            return r.clone();
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule_dsl::parser::tokenize;

    #[test]
    fn test_regex_replace() {
        let tokens = tokenize("$.name##(\\d+)##NUM:$1");
        // tokens: [JsonPath, RegexReplace]
        assert_eq!(tokens.len(), 2);
    }

    #[test]
    fn test_fallback_semantics() {
        let first = vec!["".to_string()];
        let second = vec!["result".to_string()];
        let merged = merge_fallback(&[first, second]);
        assert_eq!(merged, vec!["result"]);
    }

    #[test]
    fn test_apply_combinators_noop() {
        // No combinator tokens → unchanged
        let tokens = tokenize("class.name@text");
        let values = vec!["hello".to_string()];
        let result = apply_combinators(values.clone(), &tokens);
        assert_eq!(result, vec!["hello"]);
    }

    #[test]
    fn test_apply_combinators_regex_replace() {
        let tokens = tokenize("$.name##(\\d+)##NUM:$1");
        let values = vec!["page123".to_string()];
        let result = apply_combinators(values, &tokens);
        // re.replace_all replaces the matched portion, preserving prefix text
        assert_eq!(result[0], "pageNUM:123");
    }

    #[test]
    fn test_apply_combinators_ignores_index_tokens() {
        // Index/ExcludeIndex are consumed by the extractors; the combinator
        // must NOT re-apply them (that used to delete extracted values).
        let tokens = vec![RuleToken::Index(0)];
        let values = vec!["first".to_string(), "second".to_string()];
        let result = apply_combinators(values, &tokens);
        assert_eq!(result, vec!["first", "second"]);
    }

    #[test]
    fn test_apply_combinators_fallback_uses_first() {
        // With fallback semantics, first non-empty result wins
        let tokens = tokenize("class.a@text||class.b@text");
        let values = vec!["first".to_string()];
        let result = apply_combinators(values, &tokens);
        // Fallback token stops at first non-empty — "first" is non-empty
        assert_eq!(result, vec!["first"]);
    }
}
