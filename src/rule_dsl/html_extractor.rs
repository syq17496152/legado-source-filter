use scraper::{Html, Selector};

use super::parser::RuleToken;

/// Extract text or attribute values from an HTML string using rule tokens.
///
/// Each selector step re-parses the matched elements' HTML to avoid
/// complex lifetime chains with ElementRef.
///
/// `||` fallback segments are evaluated independently from the ORIGINAL
/// content and the first segment with any non-empty result wins (Legado
/// semantics). `##` regex replacement is left to the combinator pass.
pub fn extract(html: &str, tokens: &[RuleToken]) -> Vec<String> {
    // Split at `||` boundaries; each segment re-starts from the full content,
    // its `##` regex replacements are applied to its own results, and the
    // first segment with any non-empty result wins (Legado semantics).
    for segment in super::parser::split_segments(tokens) {
        let mut results = extract_segment(html, segment);
        results = super::combinator::apply_combinators(results, segment);
        if !results.is_empty() && !results.iter().all(|s| s.trim().is_empty()) {
            return results;
        }
    }
    Vec::new()
}

/// Evaluate one `||`-segment (no Fallback tokens inside).
fn extract_segment(html: &str, tokens: &[RuleToken]) -> Vec<String> {
    let mut current_html = vec![html.to_string()];
    let mut current_tokens = tokens;
    let mut results = Vec::new();

    while !current_tokens.is_empty() {
        let token = &current_tokens[0];
        current_tokens = &current_tokens[1..];

        match token {
            // ── Locators: narrow by selector ──
            RuleToken::CssClass(class) => {
                let sel_str = format!(".{}", class);
                current_html = apply_selector_all(&current_html, &sel_str);
            }
            RuleToken::CssId(id) => {
                let sel_str = format!("#{}", id);
                current_html = apply_selector_all(&current_html, &sel_str);
            }
            RuleToken::TagName(tag) => {
                current_html = apply_selector_all(&current_html, tag);
            }
            RuleToken::CssOverride(css) => {
                current_html = apply_selector_all(&current_html, css);
            }
            RuleToken::TagShort(tag) => {
                current_html = apply_selector_all(&current_html, tag);
            }

            // ── Index ──
            RuleToken::Index(n) => {
                current_html = current_html.into_iter().skip(*n).take(1).collect();
            }
            RuleToken::ExcludeIndex(n) => {
                // `!n` — drop the n-th element (0-based), keep the rest.
                current_html = current_html
                    .into_iter()
                    .enumerate()
                    .filter(|(i, _)| *i != *n)
                    .map(|(_, v)| v)
                    .collect();
            }

            // ── Extractors: final step ──
            RuleToken::Text => {
                for html_snippet in &current_html {
                    let doc = Html::parse_fragment(html_snippet);
                    results.push(doc.root_element().text().collect::<String>());
                }
                return results;
            }
            RuleToken::TextNodes => {
                for html_snippet in &current_html {
                    let doc = Html::parse_fragment(html_snippet);
                    let root = doc.root_element();
                    // Direct TEXT children only (no recursion). The fragment
                    // root may be an html wrapper whose direct children are
                    // elements — descend into the first element then.
                    let mut text = String::new();
                    let owner = if root.value().name() == "html" {
                        root.child_elements().next()
                    } else {
                        Some(root)
                    };
                    if let Some(owner) = owner {
                        for child in owner.children() {
                            if let scraper::node::Node::Text(t) = child.value() {
                                text.push_str(&t.text);
                            }
                        }
                    }
                    results.push(text);
                }
                return results;
            }
            RuleToken::Href => {
                for html_snippet in &current_html {
                    let doc = Html::parse_fragment(html_snippet);
                    if let Some(elem) = doc
                        .root_element()
                        .select(&Selector::parse("*").unwrap())
                        .next()
                        && let Some(href) = elem.value().attr("href")
                    {
                        results.push(href.to_string());
                    }
                }
                return results;
            }
            RuleToken::Html => {
                for html_snippet in &current_html {
                    let doc = Html::parse_fragment(html_snippet);
                    let root = doc.root_element();
                    let inner = if root.value().name() == "html" {
                        // Fragment wrapper: inner HTML of its first element.
                        root.child_elements()
                            .next()
                            .map(|e| e.inner_html())
                            .unwrap_or_default()
                    } else {
                        root.inner_html()
                    };
                    results.push(inner);
                }
                return results;
            }
            RuleToken::Src => {
                for html_snippet in &current_html {
                    let doc = Html::parse_fragment(html_snippet);
                    if let Some(elem) = doc
                        .root_element()
                        .select(&Selector::parse("*").unwrap())
                        .next()
                        && let Some(src) = elem.value().attr("src")
                    {
                        results.push(src.to_string());
                    }
                }
                return results;
            }

            // ── Type mismatch ──
            // A JSONPath / data-path / JS rule running against HTML content is
            // a rule/content mismatch. Previously the unknown tokens were
            // ignored and the WHOLE PAGE was returned as the "value", which
            // silently produced garbage (e.g. an entire HTML page used as a
            // book URL or name). Return empty so the caller treats the
            // extraction as failed instead of believing the garbage.
            RuleToken::JsonPath(_)
            | RuleToken::DataPath(_)
            | RuleToken::JsBlock(_)
            | RuleToken::JsTransform(_) => {
                return Vec::new();
            }

            // ── Unsupported ──
            // Fallback/RegexReplace are handled by the combinator pass;
            // Unknown keeps the historical whole-page fallback (e.g. "." rules).
            _ => {}
        }
    }

    // No extractor encountered: return the outer HTML of remaining elements
    // (preserving HTML structure for downstream selector steps).
    results = current_html;
    results
}

/// Apply a CSS selector to each HTML snippet, returning outer HTML of matched
/// elements. This preserves the element context (including all attributes,
/// which `element.html()` serializes) for subsequent locator and extractor
/// steps.
fn apply_selector_all(snippets: &[String], selector_str: &str) -> Vec<String> {
    let mut results = Vec::new();
    let selector = match Selector::parse(selector_str) {
        Ok(s) => s,
        Err(_) => return results,
    };

    for snippet in snippets {
        let doc = Html::parse_fragment(snippet);
        for element in doc.root_element().select(&selector) {
            // `ElementRef::html()` already serializes the element itself with
            // all its attributes — do NOT wrap it again (that produced
            // `<div><div>…</div></div>` and broke direct-text extraction).
            results.push(element.html());
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rule_dsl::parser::tokenize;

    #[test]
    fn test_simple_text() {
        let html = r#"<div class="item"><a href="/book/1">Book 1</a></div>"#;
        let tokens = tokenize("class.item@tag.a@text");
        let results = extract(html, &tokens);
        assert_eq!(results, vec!["Book 1"]);
    }

    #[test]
    fn test_href() {
        let html = r#"<div class="item"><a href="/book/1">Title</a></div>"#;
        let tokens = tokenize("class.item@tag.a@href");
        let results = extract(html, &tokens);
        assert_eq!(results, vec!["/book/1"]);
    }

    #[test]
    fn test_index_modifier() {
        let html = r#"<ul><li>First</li><li>Second</li></ul>"#;
        let tokens = tokenize("tag.li.0@text");
        let results = extract(html, &tokens);
        assert_eq!(results, vec!["First"]);
    }

    #[test]
    fn test_preserves_custom_attributes() {
        // The fix ensures ALL attributes are preserved when rebuilding outer HTML,
        // not just id/class/href/src.
        let html = r#"<div class="item" data-id="42" style="color:red" title="tip" rel="nofollow">
            <a href="/book/1">Book 1</a>
        </div>"#;
        // Apply class selector, then extract href from the child link
        let tokens = tokenize("class.item@tag.a@href");
        let results = extract(html, &tokens);
        assert_eq!(results, vec!["/book/1"], "href from child link");

        // Verify the outer HTML reconstruction includes nested elements
        // by extracting inner HTML after the class selector.
        let with_html = tokenize("class.item@html");
        let results = extract(html, &with_html);
        assert_eq!(results.len(), 1, "should match one .item element");
        assert!(
            results[0].contains("href=\"/book/1\""),
            "href attribute should be preserved in inner HTML"
        );
    }

    #[test]
    fn test_bare_tag_rule_extracts() {
        // Bare `a` used to parse as Unknown → whole page fallback → href of
        // the first element. It must now select <a> elements properly.
        let html = "<div><a href='/b/1'>一</a><a href='/b/2'>二</a></div>";
        let results = extract(html, &tokenize("a@href"));
        assert_eq!(results, vec!["/b/1", "/b/2"]);
        let results = extract(html, &tokenize("a@text"));
        assert_eq!(results, vec!["一", "二"]);
    }

    #[test]
    fn test_json_rule_on_html_returns_empty() {
        // A JSONPath rule against HTML content is a type mismatch — must return
        // empty, NOT the whole page (which was silently used as a book URL /
        // name before).
        let html = "<html><body><div class='item'>内容</div></body></html>";
        assert_eq!(extract(html, &tokenize("$.result.book_id")), Vec::<String>::new());
        assert_eq!(extract(html, &tokenize("data.books")), Vec::<String>::new());
    }
}
