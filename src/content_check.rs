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
    let Some((mut chapter_body, mut chapter_ct)) =
        fetch_body(client, &chapter_url, &chapter_headers).await
    else {
        return ChainResult::fail("chapter_fetch", &chapter_url);
    };

    // ── 6. ruleContent.content → text, walking nextContentUrl pages ──
    let Some(content_rule) = source
        .ruleContent
        .as_ref()
        .and_then(|c| c.content.as_deref())
        .map(str::trim)
        .filter(|r| !r.is_empty())
    else {
        return ChainResult::fail("no_content_rule", source.bookSourceName.as_str());
    };

    let next_rule = source
        .ruleContent
        .as_ref()
        .and_then(|c| c.nextContentUrl.as_deref())
        .map(str::trim)
        .filter(|r| !r.is_empty() && *r != "-" && *r != "#");

    // Walk pages: extract the current page's text, then follow nextContentUrl
    // until it yields nothing (single-page chapter → fall back to what we have)
    // or we hit a safety cap (broken rules / infinite loops).
    const MAX_PAGES: usize = 30;
    let mut accumulated = String::new();
    let mut pages = 0;
    let mut current_url = chapter_url.to_string();
    let current_headers = &chapter_headers;

    loop {
        pages += 1;
        let raw = eval_rule(
            client,
            &chapter_body,
            &chapter_ct,
            content_rule,
            &current_url,
            current_headers,
        )
        .await
        .unwrap_or_default();
        if !raw.is_empty() {
            accumulated.push_str(&clean_text(&raw));
        }

        // Stop when there is no pagination rule or no next page to follow.
        let Some(nr) = next_rule else { break };
        if pages >= MAX_PAGES {
            break;
        }
        let Some(next) = eval_rule(
            client,
            &chapter_body,
            &chapter_ct,
            nr,
            &current_url,
            current_headers,
        )
        .await
        else {
            break;
        };
        let next = resolve_url(&next, &current_url);
        if next == current_url {
            break; // same page again — guard against self-referencing rules
        }
        let Some((body, ct)) = fetch_body(client, &next, current_headers).await else {
            break;
        };
        chapter_body = body;
        chapter_ct = ct;
        current_url = next;
    }

    let len = accumulated.chars().count();
    if len < 50 {
        return ChainResult::fail("content_short", format!("{} chars", len));
    }
    ChainResult::pass(len, accumulated.chars().take(40).collect::<String>())
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
pub fn extract_json_placeholders(s: &str) -> Vec<String> {
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

    // ── nextContentUrl pagination ──

    /// Spin up a mock HTTP server that serves a paginated chapter:
    ///   /book           → book detail page
    ///   /toc            → TOC page whose first chapter is /c/1
    ///   /c/1, /c/2, ... → chapter pages; page i carries a next link to /c/{i+1}
    /// `pages` pages total, each with `text_per_page` chars of body.
    async fn mock_paginated_server(pages: usize) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 8192];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("/")
                        .split('?')
                        .next()
                        .unwrap_or("/");
                    let body = match path {
                        "/book" => "<html><body>书名</body></html>".to_string(),
                        "/toc" => "<html><a href='/c/1'>第一章</a><a href='/c/2'>第二章</a></html>"
                            .to_string(),
                        p if p.starts_with("/c/") => {
                            let page: usize = p[3..].parse().unwrap_or(1);
                            if page < pages {
                                format!(
                                    "<html><div id='next' href='/c/{}'>下一页</div><p>{}</p></html>",
                                    page + 1,
                                    "正".repeat(200)
                                )
                            } else {
                                format!(
                                    "<html><div id='next'></div><p>{}</p></html>",
                                    "正".repeat(200)
                                )
                            }
                        }
                        _ => "not found".to_string(),
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                });
            }
        });
        addr
    }

    /// Build a source whose ruleContent has the given nextContentUrl rule.
    fn paginated_source(base: &str, next_content_rule: Option<&str>) -> (BookSource, String) {
        let source = BookSource {
            bookSourceName: "翻页测试".into(),
            bookSourceUrl: base.to_string(),
            ruleBookInfo: None,
            ruleToc: Some(TocRule {
                chapterList: Some("a".into()),
                chapterUrl: Some("@href".into()),
                chapterName: Some("@text".into()),
                ..Default::default()
            }),
            ruleContent: Some(ContentRule {
                content: Some("div#next,p@text".into()),
                nextContentUrl: next_content_rule.map(|r| r.to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let toc_url = format!("{}/toc", base);
        (source, toc_url)
    }

    #[tokio::test]
    async fn test_pagination_walks_all_pages() {
        let addr = mock_paginated_server(3).await;
        let base = format!("http://{}", addr);
        let (source, toc_url) = paginated_source(&base, Some("div#next@href"));
        let client = HttpClient::new(10).unwrap();
        // Verify chain from the TOC page: chain uses ruleBookInfo.tocUrl default
        // to book_url, so pass the TOC URL as book_url.
        let res = verify_chain(&client, &source, &toc_url, &base).await;
        assert!(res.ok, "expected pass, got {:?}", res);
        assert!(
            res.content_len >= 3 * 200,
            "expected ~600 chars, got {}",
            res.content_len
        );
    }

    #[tokio::test]
    async fn test_pagination_single_page_falls_back() {
        // Chapter has only one page (server `pages=1`), so the nextContentUrl
        // rule yields nothing. Must still pass using the single page's content.
        let addr = mock_paginated_server(1).await;
        let base = format!("http://{}", addr);
        let (source, toc_url) = paginated_source(&base, Some("div#next@href"));
        let client = HttpClient::new(10).unwrap();
        let res = verify_chain(&client, &source, &toc_url, &base).await;
        assert!(res.ok, "single-page chapter should pass, got {:?}", res);
        assert!(res.content_len >= 200, "got {}", res.content_len);
    }

    #[tokio::test]
    async fn test_pagination_missing_rule_ok() {
        // No nextContentUrl at all — plain single fetch, still passes.
        let addr = mock_paginated_server(1).await;
        let base = format!("http://{}", addr);
        let (source, toc_url) = paginated_source(&base, None);
        let client = HttpClient::new(10).unwrap();
        let res = verify_chain(&client, &source, &toc_url, &base).await;
        assert!(res.ok, "no nextContentUrl should not fail, got {:?}", res);
    }

    #[tokio::test]
    async fn test_pagination_self_referencing_guarded() {
        // nextContentUrl returns the same page → must stop, not infinite loop.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 8192];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let path = req
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("/")
                        .split('?')
                        .next()
                        .unwrap_or("/");
                    let body = match path {
                        "/book" => "<html><body>书名</body></html>".to_string(),
                        "/toc" => "<html><a href='/c/1'>第一章</a></html>".to_string(),
                        "/c/1" => {
                            // next link points at the SAME page
                            "<html><div id='next' href='/c/1'>下一页</div><p>正文</p></html>"
                                .to_string()
                        }
                        _ => "nf".to_string(),
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                });
            }
        });
        let base = format!("http://{}", addr);
        let (source, toc_url) = paginated_source(&base, Some("div#next@href"));
        let client = HttpClient::new(10).unwrap();
        let res = verify_chain(&client, &source, &toc_url, &base).await;
        // 2 chars of "正文" is below threshold, but crucially it must terminate.
        assert!(
            !res.ok,
            "self-referencing next should be content_short, got {:?}",
            res
        );
    }
}

/// Whether a search rule contains JS that must be evaluated by the polyfill
/// before DSL extraction can proceed.
pub fn rule_has_js(rule: &SearchRule) -> bool {
    let fields = [
        rule.bookList.as_deref(),
        rule.name.as_deref(),
        rule.author.as_deref(),
        rule.bookUrl.as_deref(),
        rule.coverUrl.as_deref(),
        rule.intro.as_deref(),
    ];
    fields
        .iter()
        .flatten()
        .any(|v| v.contains("<js>") || v.starts_with("@js:") || v.contains("java."))
}

/// Extract search results when the ruleSearch contains JS. The JS `bookList`
/// (typically a `<js>...</js>` block) transforms the raw response body —
/// often decrypting it — and the transformed text is then fed to the remaining
/// DSL rule to yield per-item rows. Field rules that are themselves JS
/// (`<js>`/`@js:` in bookUrl/name/coverUrl) are evaluated per-item with the
/// item's content as `result`.
pub async fn extract_results_js(
    client: &HttpClient,
    source: &BookSource,
    result: &crate::http_client::FetchResult,
    base_url: &str,
) -> Vec<Vec<String>> {
    let Some(rule) = source.ruleSearch.as_ref() else {
        return Vec::new();
    };
    let Some(bl) = rule.bookList.as_deref() else {
        return Vec::new();
    };

    // Split `<js>...</js>` from the trailing DSL portion.
    let (js_code, dsl_rest) = split_js_rule(bl);
    if js_code.is_empty() {
        return Vec::new();
    }

    let src_headers = source_headers(source);
    let job = JsRuleJob {
        id: 0,
        base_url: base_url.to_string(),
        result: result.body_text.clone(),
        js_code: js_code.to_string(),
    };
    let out = execute_rule_batch_async(&[job], client, &src_headers)
        .await
        .into_iter()
        .next()
        .unwrap_or_default();

    // The transformed body is whatever the JS produced (content or list).
    let mut transformed = out.content.clone().unwrap_or_default();
    if transformed.trim().is_empty()
        && let Some(u) = out.url.clone()
    {
        transformed = u;
    }

    // If the JS already yielded a (name, url) list, use it directly.
    if !out.list.is_empty() {
        return out
            .list
            .into_iter()
            .map(|(name, url)| vec![name, String::new(), url, String::new()])
            .collect();
    }

    if transformed.trim().is_empty() {
        return Vec::new();
    }

    let ct = if transformed.trim_start().starts_with('{') || transformed.trim_start().starts_with('[')
    {
        "json"
    } else {
        "html"
    };

    // Extract per-item JSON snippets so field-level JS rules can run against
    // each item's content.
    let items = extract_items(&transformed, ct, dsl_rest.as_str());
    if items.is_empty() {
        // Fall back to the plain DSL path (e.g. HTML results).
        let mut work_rule = rule.clone();
        if !dsl_rest.trim().is_empty() {
            work_rule.bookList = Some(dsl_rest.to_string());
        } else {
            work_rule.bookList = Some(".".to_string());
        }
        return rule_dsl::extract_results(&transformed, ct, &work_rule);
    }

    // Per-item field extraction. JS field rules (bookUrl/name/coverUrl) are
    // collected into one batch and evaluated together after all items.
    type FieldRows = Vec<(usize, Option<String>, String, Option<String>, String)>;
    let mut rows: FieldRows = Vec::new();
    let mut field_jobs: Vec<JsRuleJob> = Vec::new();
    let mut job_map: Vec<(usize, usize, &'static str)> = Vec::new(); // (item, job_id, field)

    for (i, item) in items.iter().enumerate() {
        let name = match rule.name.as_deref() {
            Some(r) if field_rule_has_js(r) => {
                job_map.push((i, field_jobs.len(), "name"));
                field_jobs.push(JsRuleJob {
                    id: field_jobs.len(),
                    base_url: base_url.to_string(),
                    result: item.clone(),
                    js_code: js_rule_code(r),
                });
                None
            }
            Some(r) => apply_field_extract(item, ct, Some(r)),
            None => None,
        };
        let author = apply_field_extract(item, ct, rule.author.as_deref()).unwrap_or_default();
        let book_url = match rule.bookUrl.as_deref() {
            Some(r) if field_rule_has_js(r) => {
                job_map.push((i, field_jobs.len(), "bookUrl"));
                field_jobs.push(JsRuleJob {
                    id: field_jobs.len(),
                    base_url: base_url.to_string(),
                    result: item.clone(),
                    js_code: js_rule_code(r),
                });
                None
            }
            Some(r) => apply_field_extract(item, ct, Some(r)),
            None => None,
        };
        let cover_url = match rule.coverUrl.as_deref() {
            Some(r) if field_rule_has_js(r) => {
                job_map.push((i, field_jobs.len(), "coverUrl"));
                field_jobs.push(JsRuleJob {
                    id: field_jobs.len(),
                    base_url: base_url.to_string(),
                    result: item.clone(),
                    js_code: js_rule_code(r),
                });
                None
            }
            Some(r) => apply_field_extract(item, ct, Some(r)),
            None => None,
        };
        rows.push((i, name, author, book_url, cover_url.unwrap_or_default()));
    }

    // Evaluate all field-level JS jobs in one batch.
    let field_outs = execute_rule_batch_async(&field_jobs, client, &src_headers).await;
    let mut results: Vec<Vec<String>> = Vec::with_capacity(rows.len());
    for (i, name, author, book_url, cover) in rows {
        let mut name = name;
        let mut book_url = book_url;
        let mut cover = cover;
        for &(item_idx, job_id, field) in &job_map {
            if item_idx != i {
                continue;
            }
            let out = field_outs.get(job_id).cloned().unwrap_or_default();
            let val = out
                .content
                .or(out.url)
                .unwrap_or_default()
                .trim()
                .to_string();
            match field {
                "name" => name = Some(val),
                "bookUrl" => book_url = Some(val),
                "coverUrl" => cover = val,
                _ => {}
            }
        }
        results.push(vec![
            name.unwrap_or_default(),
            author,
            book_url.unwrap_or_default(),
            cover,
        ]);
    }
    results
}

/// Extract raw per-item strings from the transformed text using the DSL
/// `bookList` rule. Each item keeps its full JSON/HTML snippet so field-level
/// JS can re-parse it.
pub fn extract_items(content: &str, content_type: &str, book_list_rule: &str) -> Vec<String> {
    let rule = if book_list_rule.trim().is_empty() {
        "."
    } else {
        book_list_rule
    };
    let tokens = crate::rule_dsl::parser::tokenize(rule);
    let vals = if content_type.contains("json") {
        if let Ok(json) = serde_json::from_str(content) {
            crate::rule_dsl::json_extractor::extract(&json, &tokens)
        } else {
            Vec::new()
        }
    } else {
        crate::rule_dsl::html_extractor::extract(content, &tokens)
    };
    let mut items = Vec::new();
    for v in vals {
        let v = v.trim();
        if v.is_empty() {
            continue;
        }
        // A bookList like `$.data.list` returns the whole array as one string;
        // expand it into its elements so field rules run per item.
        if let serde_json::Value::Array(elems) =
            serde_json::from_str::<serde_json::Value>(v).unwrap_or(serde_json::Value::Null)
        {
            for e in elems {
                items.push(e.to_string());
            }
            continue;
        }
        items.push(v.to_string());
    }
    items
}

/// Whether a single field rule needs JS evaluation.
fn field_rule_has_js(rule: &str) -> bool {
    rule.contains("<js>") || rule.starts_with("@js:") || rule.contains("java.")
}

/// Extract the JS body from a field rule (strip `<js>...`/`</js>` or `@js:`).
fn js_rule_code(rule: &str) -> String {
    let (code, _) = split_js_rule(rule);
    code
}

/// Apply a non-JS field rule against a single item's content.
fn apply_field_extract(item: &str, content_type: &str, rule: Option<&str>) -> Option<String> {
    let rule = rule?;
    if rule.is_empty() {
        return None;
    }
    let tokens = crate::rule_dsl::parser::tokenize(rule);
    let vals = if content_type.contains("json") {
        if let Ok(json) = serde_json::from_str(item) {
            crate::rule_dsl::json_extractor::extract(&json, &tokens)
        } else {
            Vec::new()
        }
    } else {
        crate::rule_dsl::html_extractor::extract(item, &tokens)
    };
    let out = crate::rule_dsl::combinator::apply_combinators(vals, &tokens);
    if let Some(first) = out.first().cloned()
        && !first.is_empty()
    {
        return Some(first);
    }
    // Template rules like `https://api/x?id={{$.novel_id||$.id}}`: expand the
    // `{{...}}` placeholders against the item JSON.
    if rule.contains("{{")
        && content_type.contains("json")
        && let Ok(json) = serde_json::from_str::<serde_json::Value>(item)
        && let Some(expanded) = expand_template(rule, &json)
    {
        return Some(expanded);
    }
    None
}

/// Expand every `{{...}}` placeholder in a template against a JSON value.
/// Supports fallbacks like `{{$.a||$.b}}`: each `||`-separated segment is a
/// JSONPath; the first that resolves to a non-empty string wins. Returns None
/// if any placeholder fails to resolve.
pub fn expand_template(tpl: &str, json: &serde_json::Value) -> Option<String> {
    let re = regex::Regex::new(r"\{\{(.*?)}}").ok()?;
    let mut out = tpl.to_string();
    let mut ok = true;
    for cap in re.captures_iter(tpl) {
        let inner = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let mut resolved = None;
        for seg in inner.split("||") {
            let path = seg.trim().trim_start_matches('$').trim_start_matches('.');
            if let Some(v) = crate::rule_dsl::json_extractor::json_path_find(json, path)
                && !v.is_empty()
            {
                resolved = Some(v);
                break;
            }
        }
        match resolved {
            Some(v) => out = out.replacen(cap.get(0).map(|m| m.as_str()).unwrap_or(""), &v, 1),
            None => {
                ok = false;
                break;
            }
        }
    }
    if ok && !out.contains("{{") {
        Some(out)
    } else {
        None
    }
}

/// Split a rule string into its `<js>...</js>` body and the trailing DSL part.
/// Returns (js_code, dsl_rest).
pub fn split_js_rule(rule: &str) -> (String, String) {
    if let Some(start) = rule.find("<js>") {
        let s = start + 4;
        if let Some(end_rel) = rule[s..].find("</js>") {
            let end = s + end_rel;
            let code = rule[s..end].to_string();
            let rest = rule[end + 5..].to_string();
            return (code, rest);
        }
        let code = rule[s..].to_string();
        return (code, String::new());
    }
    let code = rule.strip_prefix("@js:").unwrap_or(rule).to_string();
    (code, String::new())
}

#[cfg(test)]
mod js_search_tests {
    use super::*;
    use crate::http_client::FetchResult;

    /// A source that mirrors 笔书阁/文趣阁: `bookList` decrypts via JS into a
    /// JSON array, and `bookUrl` is a `<js>` field that reads `book_id` from
    /// each item via `java.getString('$.book_id')`.
    fn mock_source() -> BookSource {
        BookSource {
            bookSourceName: "mock_js".into(),
            bookSourceUrl: "https://mock.example".into(),
            ruleSearch: Some(SearchRule {
                bookList: Some(
                    "<js>\nresult = JSON.stringify(JSON.parse(result).data.list);\n</js>\n$.[*]"
                        .into(),
                ),
                name: Some("$.book_name".into()),
                author: Some("$.author_name".into()),
                bookUrl: Some(
                    "<js>\nlet bid=parseInt(java.getString('$.book_id'));\nresult='https://mock.example/book/'+bid;\n</js>"
                        .into(),
                ),
                coverUrl: Some("$.book_cover".into()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn mock_result(body: &str) -> FetchResult {
        FetchResult {
            body_text: body.to_string(),
            body_bytes: body.as_bytes().to_vec(),
            content_type: "application/json".into(),
            status_code: 200,
            elapsed_ms: 10,
            charset_detected: None,
            charset_uncertain: false,
            headers: Vec::new(),
        }
    }

    #[tokio::test]
    async fn test_extract_results_js_field_js_rules() {
        let body = r#"{"data":{"list":[{"book_id":1001,"book_name":"书名一","author_name":"作者A","book_cover":"/c1.jpg"},{"book_id":2002,"book_name":"书名二","author_name":"作者B","book_cover":"/c2.jpg"}]}}"#;
        let source = mock_source();
        let client = HttpClient::new(10).unwrap();
        let rows = extract_results_js(&client, &source, &mock_result(body), "https://mock.example")
            .await;
        assert_eq!(rows.len(), 2, "expected 2 rows, got {rows:?}");
        assert_eq!(rows[0][0], "书名一");
        assert_eq!(rows[0][1], "作者A");
        assert_eq!(rows[0][2], "https://mock.example/book/1001");
        assert_eq!(rows[0][3], "/c1.jpg");
        assert_eq!(rows[1][2], "https://mock.example/book/2002");
    }

    #[tokio::test]
    async fn test_extract_results_js_no_field_js_falls_back_to_dsl() {
        // bookUrl is a plain JSONPath, no JS — should resolve via DSL on item.
        let mut source = mock_source();
        source.ruleSearch.as_mut().unwrap().bookUrl =
            Some("https://mock.example/book/{{$.book_id}}".into());
        let body = r#"{"data":{"list":[{"book_id":1001,"book_name":"书名一","author_name":"作者A"}]}}"#;
        let client = HttpClient::new(10).unwrap();
        let rows = extract_results_js(&client, &source, &mock_result(body), "https://mock.example")
            .await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], "书名一");
        assert_eq!(rows[0][2], "https://mock.example/book/1001");
    }

    #[tokio::test]
    async fn test_extract_results_js_html_falls_back_to_dsl() {
        // Non-JSON body (HTML results) → plain DSL path.
        let mut source = mock_source();
        source.ruleSearch.as_mut().unwrap().bookList = Some("<js>result=result;</js>".into());
        source.ruleSearch.as_mut().unwrap().name = Some("h2@text".into());
        source.ruleSearch.as_mut().unwrap().bookUrl = Some("h2 a@href".into());
        let body = "<div class='item'><h2><a href='/b/1'>书名一</a></h2></div>";
        let client = HttpClient::new(10).unwrap();
        let mut res = mock_result(body);
        res.content_type = "text/html".into();
        let rows = extract_results_js(&client, &source, &res, "https://mock.example").await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], "书名一");
    }
}
