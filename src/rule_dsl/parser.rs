/// Legado rule DSL lexer & parser.
///
/// Tokenizes a rule string like `class.item@tag.li.0@text` into a chain
/// of locator, index, and extractor tokens.

#[derive(Debug, Clone, PartialEq)]
pub enum RuleToken {
    // ── Locators ──
    /// `class.name`
    CssClass(String),
    /// `id.name`
    CssId(String),
    /// `tag.name`
    TagName(String),
    /// `@css:selector`
    CssOverride(String),
    /// `$.path` or `@JSon:path`
    JsonPath(String),
    /// `data.path`
    DataPath(Vec<String>),
    /// `<js>code</js>`
    JsBlock(String),

    // ── Modifiers ──
    /// `.n` → take index n (0-based)
    Index(usize),
    /// `!n` → EXCLUDE index n (Legado exclusion modifier). `.n` and `!n` are
    /// distinct semantics and must not be conflated.
    ExcludeIndex(usize),

    // ── Extractors ──
    /// `@text`
    Text,
    /// `@textNodes` — direct text nodes only
    TextNodes,
    /// `@href`
    Href,
    /// `@html` — inner HTML
    Html,
    /// `@src`
    Src,

    // ── Combinators ──
    /// `||` — fallback separator
    Fallback,
    /// `##pattern##replacement`
    RegexReplace {
        pattern: String,
        replacement: String,
    },

    // ── Special ──
    /// Bare element name like `@a`, `@li` — shorthand tag selector
    TagShort(String),
    /// `@js:code`
    JsTransform(String),
    /// Unknown — forward compat
    Unknown(String),
}

/// Split a rule string at `||` boundaries, then tokenize each segment.
pub fn tokenize(rule: &str) -> Vec<RuleToken> {
    let mut tokens = Vec::new();
    if rule.is_empty() {
        return tokens;
    }

    // First, handle <js> blocks at the very start
    let remaining = if rule.trim_start().starts_with("<js>") {
        let s = rule.trim_start();
        if let Some(end) = s.find("</js>") {
            let code = &s[4..end];
            tokens.push(RuleToken::JsBlock(code.to_string()));
            let after = s[end + 5..].trim();
            if after.is_empty() {
                return tokens;
            }
            after
        } else {
            tokens.push(RuleToken::JsBlock(s[4..].to_string()));
            return tokens;
        }
    } else {
        rule
    };

    // Now handle || and ## and @-based tokens
    let segments = split_fallback(remaining);
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 {
            tokens.push(RuleToken::Fallback);
        }
        if let Some(re) = parse_regex_replace(seg) {
            tokens.extend(re);
        } else {
            tokens.extend(tokenize_at_chain(seg));
        }
    }

    tokens
}

fn split_fallback(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len().saturating_sub(1) {
        if bytes[i] == b'|' && bytes[i + 1] == b'|' {
            if start <= i {
                let part = s[start..i].trim();
                if !part.is_empty() {
                    parts.push(part);
                }
            }
            start = i + 2;
            i += 2; // skip past || so ||| doesn't cause start > i
        } else {
            i += 1;
        }
    }
    if start < s.len() {
        let part = s[start..].trim();
        if !part.is_empty() {
            parts.push(part);
        }
    }
    parts
}

/// Try to parse a segment as `rule##pattern##replacement`.
fn parse_regex_replace(s: &str) -> Option<Vec<RuleToken>> {
    let _bytes = s.as_bytes();
    // Find first ## that separates rule from regex
    let first = s.find("##")?;
    // After first ##, find second ## that separates regex from replacement
    let after_first = &s[first + 2..];
    let second = after_first.find("##")?;
    let rule_part = &s[..first];
    let pattern = &after_first[..second];
    let replacement = &after_first[second + 2..];

    let mut tokens = tokenize_at_chain(rule_part);
    tokens.push(RuleToken::RegexReplace {
        pattern: pattern.to_string(),
        replacement: replacement.to_string(),
    });
    Some(tokens)
}

/// Tokenize a segment by `@` separators into a chain.
fn tokenize_at_chain(s: &str) -> Vec<RuleToken> {
    let mut tokens = Vec::new();
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return tokens;
    }

    let parts: Vec<&str> = trimmed.split('@').collect();
    for (i, part) in parts.iter().enumerate() {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        if i == 0 {
            // First part: locator (may include index)
            let (loc, idx) = split_index(p);
            tokens.push(parse_locator(loc));
            if let Some(spec) = idx {
                tokens.push(index_token(spec));
            }
        } else {
            // @-prefixed part
            let (annot, idx) = split_index(p);
            tokens.push(parse_annotation(annot));
            if let Some(spec) = idx {
                tokens.push(index_token(spec));
            }
        }
    }

    tokens
}

/// Convert a parsed index suffix into the matching token: `.n` → take,
/// `!n` → exclude.
fn index_token(spec: IndexSpec) -> RuleToken {
    if spec.exclude {
        RuleToken::ExcludeIndex(spec.n)
    } else {
        RuleToken::Index(spec.n)
    }
}

/// Index modifier parsed from a `.n` / `!n` suffix.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexSpec {
    pub n: usize,
    /// `!n` excludes, `.n` takes.
    pub exclude: bool,
}

/// Split a name like "item.0" into ("item", Some(IndexSpec { n: 0, exclude: false }))
/// or "item!0" into ("item", Some(IndexSpec { n: 0, exclude: true })).
/// Negative indices (`-1`, `!-1`) are NOT parsed (documented gap vs Legado).
pub fn split_index(s: &str) -> (&str, Option<IndexSpec>) {
    let bytes = s.as_bytes();
    let len = bytes.len();

    // Find the last '.' or '!' followed by digits at the end
    if len < 3 {
        return (s, None);
    }

    // Scan backwards from the end
    let mut digit_end = len;
    while digit_end > 0 && bytes[digit_end - 1].is_ascii_digit() {
        digit_end -= 1;
    }

    if digit_end > 0
        && digit_end < len
        && (bytes[digit_end - 1] == b'.' || bytes[digit_end - 1] == b'!')
        && let Ok(num) = s[digit_end..].parse::<usize>()
    {
        let exclude = bytes[digit_end - 1] == b'!';
        let spec = IndexSpec { n: num, exclude };
        return (&s[..digit_end - 1], Some(spec));
    }
    (s, None)
}

fn parse_locator(s: &str) -> RuleToken {
    if let Some(rest) = s.strip_prefix("class.") {
        RuleToken::CssClass(rest.to_string())
    } else if let Some(rest) = s.strip_prefix("id.") {
        RuleToken::CssId(rest.to_string())
    } else if let Some(rest) = s.strip_prefix("tag.") {
        RuleToken::TagName(rest.to_string())
    } else if s.starts_with("$.") || s.starts_with("$[") {
        RuleToken::JsonPath(s.to_string())
    } else if let Some(rest) = s.strip_prefix("data.") {
        let path: Vec<String> = rest.split('.').map(|x| x.to_string()).collect();
        RuleToken::DataPath(path)
    } else if s == "." {
        // "." is the "match everything" rule — keep the whole-content
        // fallback semantics of the HTML extractor.
        RuleToken::Unknown(s.to_string())
    } else {
        // A bare token like `a`, `li`, `div` is a CSS tag selector in Legado.
        // Previously parsed as Unknown, which the HTML extractor ignored and
        // fell back to the whole page — so `a@href` style field rules silently
        // extracted garbage. Map bare tokens to TagName so real extraction works.
        RuleToken::TagName(s.to_string())
    }
}

fn parse_annotation(s: &str) -> RuleToken {
    match s {
        "text" | "textNodes" => {
            if s == "textNodes" {
                RuleToken::TextNodes
            } else {
                RuleToken::Text
            }
        }
        "href" => RuleToken::Href,
        "html" => RuleToken::Html,
        "src" => RuleToken::Src,
        _ => {
            if let Some(rest) = s.strip_prefix("css:") {
                RuleToken::CssOverride(rest.to_string())
            } else if let Some(rest) = s.strip_prefix("JSon:") {
                RuleToken::JsonPath(rest.to_string())
            } else if let Some(rest) = s.strip_prefix("tag.") {
                RuleToken::TagName(rest.to_string())
            } else if let Some(rest) = s.strip_prefix("js:") {
                RuleToken::JsTransform(rest.to_string())
            } else {
                // Bare element/tag like @a, @li, @div
                RuleToken::TagShort(s.to_string())
            }
        }
    }
}

/// Split a token stream at `Fallback` boundaries. Each segment is one `||`
/// alternative, evaluated independently against the ORIGINAL content; the
/// first segment with a non-empty result wins (Legado fallback semantics).
pub fn split_segments(tokens: &[RuleToken]) -> Vec<&[RuleToken]> {
    let mut segments = Vec::new();
    let mut start = 0;
    for (i, t) in tokens.iter().enumerate() {
        if matches!(t, RuleToken::Fallback) {
            segments.push(&tokens[start..i]);
            start = i + 1;
        }
    }
    segments.push(&tokens[start..]);
    segments
}

// ── Public convenience ──

pub fn parse_rule(rule: &str) -> Vec<RuleToken> {
    tokenize(rule)
}

/// Split a base name from an index suffix, if any. Kept for callers that only
/// need the raw numbers (no take/exclude distinction).
pub fn extract_index(s: &str) -> Option<(String, usize)> {
    let (base, idx) = split_index(s);
    idx.map(|spec| (base.to_string(), spec.n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_css() {
        let t = tokenize("class.item@tag.li@text");
        assert_eq!(
            t,
            vec![
                RuleToken::CssClass("item".into()),
                RuleToken::TagName("li".into()),
                RuleToken::Text,
            ]
        );
    }

    #[test]
    fn test_index_modifier() {
        let t = tokenize("class.item.0@tag.a@href");
        assert_eq!(t[0], RuleToken::CssClass("item".into()));
        assert_eq!(t[1], RuleToken::Index(0));
        assert_eq!(t[2], RuleToken::TagName("a".into()));
        assert_eq!(t[3], RuleToken::Href);
    }

    #[test]
    fn test_jsonpath() {
        let t = tokenize("$.data[*].name");
        assert_eq!(t, vec![RuleToken::JsonPath("$.data[*].name".into())]);
    }

    #[test]
    fn test_fallback() {
        let t = tokenize("class.a@li@text||class.b@li@text");
        assert_eq!(t.len(), 7, "3 tokens per segment + 1 Fallback = 7");
        assert!(t.contains(&RuleToken::Fallback));
    }

    #[test]
    fn test_regex_replace() {
        let t = tokenize("$.name##foo##bar");
        assert_eq!(t.len(), 2);
        assert_eq!(t[0], RuleToken::JsonPath("$.name".into()));
        assert!(matches!(t[1], RuleToken::RegexReplace { .. }));
    }

    #[test]
    fn test_split_index() {
        assert_eq!(
            split_index("item.0"),
            ("item", Some(IndexSpec { n: 0, exclude: false }))
        );
        assert_eq!(
            split_index("item!0"),
            ("item", Some(IndexSpec { n: 0, exclude: true }))
        );
        assert_eq!(
            split_index("item.12"),
            ("item", Some(IndexSpec { n: 12, exclude: false }))
        );
        assert_eq!(split_index("item"), ("item", None));
    }

    #[test]
    fn test_index_vs_exclude_token() {
        let t = tokenize("class.a.0@text");
        assert!(t.contains(&RuleToken::Index(0)));
        assert!(!t.contains(&RuleToken::ExcludeIndex(0)));

        let t = tokenize("class.a!0@text");
        assert!(t.contains(&RuleToken::ExcludeIndex(0)));
        assert!(!t.contains(&RuleToken::Index(0)));

        // Bare tag + take index (real-world `a.1` field rules).
        let t = tokenize("a.1@href");
        assert_eq!(t[0], RuleToken::TagName("a".into()));
        assert_eq!(t[1], RuleToken::Index(1));
    }

    #[test]
    fn test_annotation() {
        assert_eq!(parse_annotation("text"), RuleToken::Text);
        assert_eq!(parse_annotation("textNodes"), RuleToken::TextNodes);
        assert_eq!(parse_annotation("href"), RuleToken::Href);
        assert_eq!(parse_annotation("a"), RuleToken::TagShort("a".into()));
    }

    #[test]
    fn test_bare_token_is_tag_name() {
        // `a@href` — the bare `a` must be a tag selector, not Unknown, so
        // field rules like `a@href` extract real anchors.
        let t = tokenize("a@href");
        assert_eq!(t[0], RuleToken::TagName("a".into()));
        assert_eq!(t[1], RuleToken::Href);

        let t = tokenize("li.0@text");
        assert_eq!(t[0], RuleToken::TagName("li".into()));
        assert_eq!(t[1], RuleToken::Index(0));

        // A CSS descendant selector stays a single tag token.
        let t = tokenize("h2 a@href");
        assert_eq!(t[0], RuleToken::TagName("h2 a".into()));
    }

    #[test]
    fn test_dot_is_match_everything() {
        // "." is the whole-page fallback rule — must stay Unknown so the HTML
        // extractor keeps its historical "return the whole content" behaviour.
        let t = tokenize(".");
        assert_eq!(t, vec![RuleToken::Unknown(".".into())]);
    }
}
