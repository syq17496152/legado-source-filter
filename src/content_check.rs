//! Reading-chain verification: given a source whose search already returned
//! results, walk the full Legado flow — book detail page (ruleBookInfo) →
//! TOC page (ruleToc) → first chapter page (ruleContent) — and confirm that
//! actual readable chapter text comes out.
//!
//! This is the difference between "search works" (the old `passed`) and
//! "the book can actually be read" (what matters to a user).

use crate::http_client::HttpClient;
use crate::js_polyfill::runner::{JsRuleJob, execute_rule_batch_async};
use crate::rule_dsl;
use crate::search_url::{page_headers, resolve_url, source_headers};
use crate::types::*;

/// Result of the chain walk.
#[derive(Debug, Clone)]
pub struct ChainResult {
    pub ok: bool,
    /// Short failure stage/reason, e.g. "bookinfo_fetch", "toc_parse", "content_empty".
    pub stage: &'static str,
    /// Extra detail (chapter name / length).
    pub detail: String,
    /// Extracted content length (chars) on success.
    pub content_len: usize,
}

impl ChainResult {
    fn fail(stage: &'static str, detail: impl Into<String>) -> Self {
        ChainResult {
            ok: false,
            stage,
            detail: detail.into(),
            content_len: 0,
        }
    }
    fn pass(len: usize, detail: impl Into<String>) -> Self {
        ChainResult {
            ok: true,
            stage: "content",
            detail: detail.into(),
            content_len: len,
        }
    }
}

/// Whether a value is JS-based (needs the polyfill) vs a plain DSL rule.
fn is_js_rule(v: &str) -> bool {
    let v = v.trim();
    v.starts_with("@js:") || v.starts_with("<js>") || v.contains("java.") || v.contains("<js>")
}

fn looks_like_js(v: &str) -> bool {
    is_js_rule(v)
}

/// Fetch a URL and return (body_text, content_type_like). None on HTTP failure.
async fn fetch_body(
    client: &HttpClient,
    url: &str,
    headers: &[(String, String)],
) -> Option<(String, String)> {
    let spec = crate::search_url::RequestSpec {
        url: url.to_string(),
        method: "GET".to_string(),
        body: None,
        headers: headers.to_vec(),
        charset: None,
        no_redirect: false,
    };
    let result = client.fetch(&spec).await.ok()?;
    if result.body_text.trim().is_empty() {
        return None;
    }
    let ct = if result.content_type.contains("json") {
        "json".to_string()
    } else {
        "html".to_string()
    };
    Some((result.body_text, ct))
}

/// Extract a rule value from content, or run JS on it when the rule is JS.
/// Returns None if the rule yields nothing usable.
async fn eval_rule(
    client: &HttpClient,
    content: &str,
    content_type: &str,
    rule: &str,
    base_url: &str,
    headers: &[(String, String)],
) -> Option<String> {
    if looks_like_js(rule) {
        let job = JsRuleJob {
            id: 0,
            base_url: base_url.to_string(),
            result: content.to_string(),
            js_code: rule.to_string(),
        };
        let out = execute_rule_batch_async(&[job], client, headers)
            .await
            .into_iter()
            .next()
            .unwrap_or_default();
        for v in [out.content.clone(), out.url.clone()].into_iter().flatten() {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
        return None;
    }

    let vals = rule_dsl::extract_single(content, content_type, rule);
    vals.into_iter()
        .map(|s| s.trim().to_string())
        .find(|s| !s.is_empty())
}

/// Walk the full reading chain for a source whose first search result has
/// `book_url`. `base_url` is the source root.
///
/// Flow:
///   1. fetch book_url (book detail page)
///   2. ruleBookInfo → tocUrl (default: book_url itself)
///   3. fetch tocUrl (TOC page)
///   4. ruleToc → first chapter's URL
///   5. fetch chapterUrl (chapter page)
///   6. ruleContent.content → chapter text, check it's non-trivial
pub async fn verify_chain(
    client: &HttpClient,
    source: &BookSource,
    book_url: &str,
    _base_url: &str,
) -> ChainResult {
    let src_headers = source_headers(source);

    // ── 1. Book detail page ──
    let Some((book_body, book_ct)) = fetch_body(client, book_url, &src_headers).await else {
        return ChainResult::fail("bookinfo_fetch", book_url);
    };

    // ── 2. ruleBookInfo → tocUrl ──
    let toc_url = match source.ruleBookInfo.as_ref() {
        Some(bi) => {
            let mut toc = None;
            if let Some(ref toc_url_rule) = bi.tocUrl {
                let rule = toc_url_rule.trim();
                if !rule.is_empty() && rule != "-" && rule != "#" {
                    if looks_like_js(rule) {
                        toc = eval_rule(client, &book_body, &book_ct, rule, book_url, &src_headers)
                            .await;
                    } else if rule.contains("{{") {
                        // Template with book-info fields — substitute from the
                        // extracted book info, else keep as-is.
                        toc = substitute_book_fields(rule, source, &book_body, &book_ct);
                    } else {
                        toc = rule_dsl::extract_single(&book_body, &book_ct, rule)
                            .into_iter()
                            .map(|s| s.trim().to_string())
                            .find(|s| !s.is_empty());
                    }
                }
            }
            match toc {
                Some(t) if !t.is_empty() => resolve_url(&t, book_url),
                _ => book_url.to_string(),
            }
        }
        None => book_url.to_string(),
    };

    // ── 3. TOC page ──
    let toc_headers = page_headers(&src_headers, book_url);
    let Some((toc_body, toc_ct)) = fetch_body(client, &toc_url, &toc_headers).await else {
        return ChainResult::fail("toc_fetch", &toc_url);
    };

    // ── 4. ruleToc → first chapter URL ──
    let chapter_url = match source.ruleToc.as_ref() {
        Some(toc_rule) => {
            // chapterList may be JS (returns a list)
            let mut chapters: Vec<(String, String)> = Vec::new();
            if let Some(ref cl) = toc_rule.chapterList
                && looks_like_js(cl)
            {
                let job = JsRuleJob {
                    id: 0,
                    base_url: toc_url.clone(),
                    result: toc_body.clone(),
                    js_code: cl.clone(),
                };
                if let Some(out) = execute_rule_batch_async(&[job], client, &toc_headers)
                    .await
                    .into_iter()
                    .next()
                {
                    chapters = out.list;
                    if chapters.is_empty()
                        && let Some(v) = out.url.clone()
                    {
                        chapters = parse_pair_list(&v);
                    }
                }
            } else {
                chapters = rule_dsl::extract_toc(&toc_body, &toc_ct, toc_rule);
            }

            let first_url = chapters.iter().find_map(|(_name, url)| {
                if !url.trim().is_empty() {
                    Some(url.trim().to_string())
                } else {
                    None
                }
            });

            match first_url {
                Some(u) if !u.is_empty() => {
                    // chapterUrl rule itself may be JS — apply to the chapter item context
                    if let Some(ref cu) = toc_rule.chapterUrl
                        && looks_like_js(cu)
                    {
                        let job = JsRuleJob {
                            id: 0,
                            base_url: toc_url.clone(),
                            result: toc_body.clone(),
                            js_code: cu.clone(),
                        };
                        let out = execute_rule_batch_async(&[job], client, &toc_headers)
                            .await
                            .into_iter()
                            .next()
                            .unwrap_or_default();
                        if let Some(v) = out.url.or(out.content).map(|s| s.trim().to_string())
                            && !v.is_empty()
                        {
                            resolve_url(&v, &toc_url)
                        } else {
                            resolve_url(&u, &toc_url)
                        }
                    } else {
                        resolve_url(&u, &toc_url)
                    }
                }
                _ => return ChainResult::fail("toc_parse", &toc_url),
            }
        }
        None => return ChainResult::fail("no_toc_rule", source.bookSourceName.as_str()),
    };

    // ── 5. Chapter page ──
    let chapter_headers = page_headers(&src_headers, &toc_url);
    let Some((chapter_body, chapter_ct)) = fetch_body(client, &chapter_url, &chapter_headers).await
    else {
        return ChainResult::fail("chapter_fetch", &chapter_url);
    };

    // ── 6. ruleContent.content → text ──
    let Some(content_rule) = source
        .ruleContent
        .as_ref()
        .and_then(|c| c.content.as_deref())
        .map(str::trim)
        .filter(|r| !r.is_empty())
    else {
        return ChainResult::fail("no_content_rule", source.bookSourceName.as_str());
    };

    let raw = eval_rule(
        client,
        &chapter_body,
        &chapter_ct,
        content_rule,
        &chapter_url,
        &chapter_headers,
    )
    .await
    .unwrap_or_default();

    let text = clean_text(&raw);
    let len = text.chars().count();
    if len < 50 {
        return ChainResult::fail("content_short", format!("{} chars", len));
    }
    ChainResult::pass(len, text.chars().take(40).collect::<String>())
}

/// Strip tags & whitespace for length heuristics, collapse the whole thing.
fn clean_text(raw: &str) -> String {
    let no_tags = raw
        .replace("<br>", "\n")
        .replace("<br/>", "\n")
        .replace("<br />", "\n");
    // Remove any remaining tags
    let mut out = String::with_capacity(no_tags.len());
    let mut in_tag = false;
    for ch in no_tags.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

/// Substitute `{{$.field}}` / `{{field}}` style placeholders in a tocUrl
/// template using fields extracted from the book detail page.
fn substitute_book_fields(
    rule: &str,
    source: &BookSource,
    book_body: &str,
    book_ct: &str,
) -> Option<String> {
    let bi = source.ruleBookInfo.as_ref()?;
    let mut fields: Vec<(String, String)> = Vec::new();
    for (name, rule_opt) in [
        ("name", &bi.name),
        ("author", &bi.author),
        ("kind", &bi.kind),
        ("wordCount", &bi.wordCount),
        ("lastChapter", &bi.lastChapter),
        ("intro", &bi.intro),
        ("coverUrl", &bi.coverUrl),
    ] {
        if let Some(r) = rule_opt {
            let v = rule_dsl::extract_single(book_body, book_ct, r)
                .into_iter()
                .map(|s| s.trim().to_string())
                .find(|s| !s.is_empty());
            if let Some(v) = v {
                fields.push((name.to_string(), v));
            }
        }
    }

    let mut out = rule.to_string();
    for (k, v) in &fields {
        out = out.replace(&format!("{{{{$.{}}}}}", k), v);
        out = out.replace(&format!("{{{{{}}}}}", k), v);
    }

    // Resolve any remaining `{{$.field}}` / `{{field}}` placeholders by
    // extracting them directly from the book page body as JSONPath (this covers
    // fields like `_id`, `total_num` that aren't part of the fixed list).
    let remaining = extract_json_placeholders(&out);
    if remaining.is_empty() {
        return Some(out);
    }
    if book_ct == "json" {
        for field in &remaining {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(book_body) {
                let path = field.trim_start_matches('$').trim_start_matches('.');
                if let Some(v) = rule_dsl::json_extractor::json_path_find(&val, path)
                    && !v.is_empty()
                {
                    out = out.replace(&format!("{{{{$.{}}}}}", field), &v);
                    out = out.replace(&format!("{{{{{}}}}}", field), &v);
                }
            }
        }
        if out.contains("{{") {
            return None; // unresolvable placeholders remain
        }
        return Some(out);
    }
    None
}

/// Collect `{{...}}` placeholder names from a template.
fn extract_json_placeholders(s: &str) -> Vec<String> {
    use regex::Regex;
    let Ok(re) = Regex::new(r"\{\{\$?\.?([A-Za-z_][A-Za-z0-9_.\[\]*]*)}}") else {
        return Vec::new();
    };
    re.captures_iter(s)
        .filter_map(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .collect()
}

/// Parse a JS-produced list string like `[["第1章","/c/1"],["第2章","/c/2"]]`
/// or `[{text:'x',href:'/c/1'}]` into (name, url) pairs.
fn parse_pair_list(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(s.trim())
        && let Some(arr) = val.as_array()
    {
        for item in arr {
            if let Some(pair) = item.as_array() {
                let name = pair
                    .first()
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let url = pair
                    .get(1)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                out.push((name, url));
            } else if let Some(obj) = item.as_object() {
                let name = obj
                    .get("text")
                    .or_else(|| obj.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let url = obj
                    .get("href")
                    .or_else(|| obj.get("url"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                out.push((name, url));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_text_strips_tags() {
        let raw = "<div>第一章</div><br>正文内容<p>更多</p>";
        let out = clean_text(raw);
        assert!(!out.contains('<'));
        assert!(out.contains("第一章"));
        assert!(out.contains("正文内容"));
    }

    #[test]
    fn test_parse_pair_list_nested_arrays() {
        let s = r#"[["第1章","/c/1"],["第2章","/c/2"]]"#;
        let pairs = parse_pair_list(s);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("第1章".to_string(), "/c/1".to_string()));
    }

    #[test]
    fn test_parse_pair_list_objects() {
        let s = r#"[{"text":"第1章","href":"/c/1"},{"name":"第2章","url":"/c/2"}]"#;
        let pairs = parse_pair_list(s);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("第1章".to_string(), "/c/1".to_string()));
        assert_eq!(pairs[1], ("第2章".to_string(), "/c/2".to_string()));
    }

    #[test]
    fn test_parse_pair_list_garbage() {
        assert!(parse_pair_list("not json").is_empty());
    }

    #[test]
    fn test_substitute_book_fields() {
        let source = BookSource {
            bookSourceName: "测试".into(),
            bookSourceUrl: "https://example.com".into(),
            ruleBookInfo: Some(BookInfoRule {
                name: Some("tag.h1@text".into()),
                author: Some("class.author@text".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let body = "<h1>书名</h1><span class='author'>作者甲</span>";
        let out = substitute_book_fields("/book/{{$.name}}/{{$.author}}", &source, body, "html");
        assert_eq!(out.as_deref(), Some("/book/书名/作者甲"));
    }

    #[test]
    fn test_substitute_book_fields_unresolvable() {
        let source = BookSource {
            bookSourceName: "测试".into(),
            bookSourceUrl: "https://example.com".into(),
            ruleBookInfo: Some(BookInfoRule {
                name: Some("tag.h1@text".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let body = "<h1>书名</h1>";
        // `{{$.unknown}}` can't be substituted → None
        let out = substitute_book_fields("/book/{{$.name}}/{{$.unknown}}", &source, body, "html");
        assert!(out.is_none());
    }

    #[test]
    fn test_substitute_book_fields_json_path_fields() {
        // Fields not in the fixed list (`_id`, `total_num`) get resolved via
        // direct JSONPath against the book body.
        let source = BookSource {
            bookSourceName: "测试".into(),
            bookSourceUrl: "https://example.com".into(),
            ruleBookInfo: Some(BookInfoRule {
                tocUrl: Some(
                    "https://example.com/getCataLogs?bookid={{$._id}}&limit={{$.total_num}}".into(),
                ),
                ..Default::default()
            }),
            ..Default::default()
        };
        let body = r#"{"_id":"abc123","total_num":100,"name":"书名"}"#;
        let out = substitute_book_fields(
            "https://example.com/getCataLogs?bookid={{$._id}}&limit={{$.total_num}}",
            &source,
            body,
            "json",
        );
        assert_eq!(
            out.as_deref(),
            Some("https://example.com/getCataLogs?bookid=abc123&limit=100")
        );
    }

    #[test]
    fn test_extract_json_placeholders() {
        let out = extract_json_placeholders("/a/{{$.x}}/{{y}}/{{$.data.list[0]}}");
        assert!(out.contains(&"x".to_string()));
        assert!(out.contains(&"y".to_string()));
        assert!(out.contains(&"data.list[0]".to_string()));
    }
}
