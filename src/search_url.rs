use regex::Regex;

use crate::types::BookSource;

/// Specification for an HTTP request.
#[derive(Debug, Clone)]
pub struct RequestSpec {
    pub url: String,
    pub method: String,
    pub body: Option<String>,
    pub headers: Vec<(String, String)>,
    pub charset: Option<String>,
    /// Don't follow redirects (needed for java.post(...).header("Location")
    /// rules, which read the redirect target from the response header).
    pub no_redirect: bool,
}

/// Build an HTTP request spec from a book source and keyword.
///
/// Returns `None` when the search URL is a JS pattern (`@js:` beyond L1, or
/// `<js>` blocks) that cannot be resolved on the Rust side — the caller is
/// then expected to fall back to the JS polyfill.
pub fn build_request(source: &BookSource, keyword: &str) -> Option<RequestSpec> {
    let search_url = source.searchUrl.as_deref()?;
    let base_url = &source.bookSourceUrl;

    // Determine the effective search URL. `@js:` L1 patterns are resolved in
    // Rust; anything else JS-related must go through the JS polyfill.
    let effective_url = if search_url.starts_with("@js:") {
        resolve_js_l1(search_url, base_url, keyword)?
    } else if search_url.contains("<js>") {
        return None;
    } else {
        resolve_template(search_url, keyword)
    };

    // Check for POST format: path,{'method':'POST','body':'...'}
    if let Some(spec) = try_parse_post(&effective_url, base_url) {
        return Some(spec);
    }

    // Default: GET request
    let full_url = resolve_url(&effective_url, base_url);
    Some(RequestSpec {
        url: full_url,
        method: "GET".to_string(),
        body: None,
        headers: Vec::new(),
        charset: None,
        no_redirect: false,
    })
}

/// Resolve `@js:` L1 patterns with Rust-side string processing.
fn resolve_js_l1(search_url: &str, base_url: &str, keyword: &str) -> Option<String> {
    if !search_url.starts_with("@js:") {
        return None;
    }

    let js_body = &search_url[4..];

    // Match: url = baseUrl + "/path/" + key  (possibly with encodeURI wrapping)
    let key_enc = urlencoding(keyword);
    let re = Regex::new(
        r#"(?s)url\s*=\s*baseUrl\s*\+\s*"([^"]*)"\s*\+\s*(?:encodeURI\s*\(\s*)?key(?:\))?"#,
    )
    .ok()?;
    if let Some(caps) = re.captures(js_body) {
        let path = caps.get(1).map(|m| m.as_str()).unwrap_or("");
        let path_resolved = resolve_template(path, &key_enc);
        return Some(format!(
            "{}{}",
            base_url.trim_end_matches('/'),
            path_resolved
        ));
    }

    None
}

/// Replace `{{key}}`, `{{page}}` templates in a string.
fn resolve_template(s: &str, keyword: &str) -> String {
    s.replace("{{key}}", keyword)
        .replace("{{page}}", "1")
        .replace("{{KEY}}", keyword)
}

/// Resolve a possibly-relative URL against a base URL.
///
/// Handles:
/// - absolute URLs (`http://`, `https://`)
/// - protocol-relative URLs (`//host/path`)
/// - absolute paths (`/path`) — resolved against base scheme+host
/// - relative paths (`../path`, `./path`, `path`) — resolved against the
///   base URL's directory
/// - query-only URLs (`?a=1`) — appended to the base path
pub fn resolve_url(url: &str, base: &str) -> String {
    let url = url.trim();
    if url.is_empty() {
        return base.to_string();
    }
    if url.starts_with("http://") || url.starts_with("https://") {
        return url.to_string();
    }
    // Protocol-relative: //host/path
    if let Some(rest) = url.strip_prefix("//") {
        if let Some(scheme) = base.split(':').next() {
            return format!("{}://{}", scheme, rest);
        }
        return format!("https://{}", rest);
    }
    // Query-only: ?a=1 → keep base (drop fragment), append query
    if let Some(q) = url.strip_prefix('?') {
        let without_frag = base.split('#').next().unwrap_or(base);
        return format!("{}?{}", without_frag, q);
    }
    // Fragment-only
    if let Some(f) = url.strip_prefix('#') {
        let without_frag = base.split('#').next().unwrap_or(base);
        return format!("{}#{}", without_frag, f);
    }

    // If base is missing a scheme, try to fix it first
    let base = if base.starts_with("http://") || base.starts_with("https://") {
        base
    } else {
        return url.to_string();
    };

    // Split base into scheme://authority/path
    let (head, base_path) = match base.find("://") {
        Some(pos) => {
            let rest = &base[pos + 3..];
            match rest.find('/') {
                Some(slash) => (&base[..pos + 3 + slash], &rest[slash..]),
                None => (base, "/"),
            }
        }
        None => ("", base),
    };

    if url.starts_with('/') {
        // Absolute path: scheme://authority + path
        return format!("{}{}", head, url);
    }

    // Relative path: resolve against base_path's directory
    let base_dir = match base_path.rfind('/') {
        Some(idx) => &base_path[..idx + 1],
        None => "/",
    };
    let joined = format!("{}{}", base_dir, url);
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    format!("{}/{}", head, parts.join("/"))
}

/// Try to parse a search URL as a POST request in Legado format: `path,{dict}`.
fn try_parse_post(search_url: &str, base_url: &str) -> Option<RequestSpec> {
    if !contains_post_marker(search_url) {
        return None;
    }
    parse_post_request(search_url, base_url)
}

/// Whether a URL string declares the Legado POST convention (`'method':'POST'`).
fn contains_post_marker(s: &str) -> bool {
    s.contains("'method':'POST'")
        || s.contains("\"method\":\"POST\"")
        || s.contains("'method':'post'")
        || s.contains("\"method\":\"post\"")
}

/// Parse a `url,{method,body,charset,...}` Legado POST spec into a RequestSpec.
///
/// Public so the tester can apply the same parsing to URLs produced by the
/// JS polyfill (which may emit POST conventions too).
pub fn parse_post_request(search_url: &str, base_url: &str) -> Option<RequestSpec> {
    // Extract body content
    let body = extract_post_body(search_url)?;
    let charset = extract_charset(search_url);

    // The URL is whatever comes before the first `,{`
    let url_part = if let Some(pos) = search_url.find(",{") {
        &search_url[..pos]
    } else if let Some(pos) = search_url.find(", {\"method\"") {
        &search_url[..pos]
    } else if let Some(pos) = search_url.find(", {'method'") {
        &search_url[..pos]
    } else {
        search_url
    };

    let full_url = resolve_url(url_part.trim(), base_url);

    Some(RequestSpec {
        url: full_url,
        method: "POST".to_string(),
        body: Some(body),
        headers: Vec::new(),
        charset,
        no_redirect: false,
    })
}

fn extract_post_body(s: &str) -> Option<String> {
    // JSON-object body: `'body':{'keyword':'{{key}}'}` or `"body":{...}`
    if let Some(start) = s.find("body") {
        let after_key = &s[start + 4..];
        if let Some(colon) = after_key.find(':') {
            let value = after_key[colon + 1..].trim_start();
            if value.starts_with('{') {
                // Find the matching closing brace (naive scan; body objects are
                // simple key:value dicts in practice).
                let mut depth = 0i32;
                let mut in_str = false;
                for (i, ch) in value.char_indices() {
                    match ch {
                        '"' | '\'' => in_str = !in_str,
                        '{' if !in_str => depth += 1,
                        '}' if !in_str => {
                            depth -= 1;
                            if depth == 0 {
                                let obj = &value[..=i];
                                return Some(obj.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    // Try single-quote: 'body':'...'
    let re_single = Regex::new(r"'body'\s*:\s*'([^']*)'").ok()?;
    if let Some(caps) = re_single.captures(s) {
        return Some(caps[1].to_string());
    }
    // Try double-quote: "body":"..."
    let re_double = Regex::new(r#""body"\s*:\s*"([^"]*)""#).ok()?;
    if let Some(caps) = re_double.captures(s) {
        return Some(caps[1].to_string());
    }
    None
}

fn extract_charset(s: &str) -> Option<String> {
    // Try 'charset':'xxx'
    let re_single = Regex::new(r"'charset'\s*:\s*'([^']*)'").ok()?;
    if let Some(caps) = re_single.captures(s) {
        return Some(caps[1].to_string().to_lowercase());
    }
    // Try "charset":"xxx"
    let re_double = Regex::new(r#""charset"\s*:\s*"([^"]*)""#).ok()?;
    if let Some(caps) = re_double.captures(s) {
        return Some(caps[1].to_string().to_lowercase());
    }
    None
}

/// Build request headers for a source: parse the `header` field (Python-dict
/// syntax) and add a `Referer` pointing at the source site (most content pages
/// require it), unless the source explicitly sets one.
pub fn source_headers(source: &BookSource) -> Vec<(String, String)> {
    let mut headers = crate::http_client::parse_headers(source.header.as_deref().unwrap_or(""));
    let has_referer = headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("Referer"));
    if !has_referer {
        let base = source
            .bookSourceUrl
            .split('#')
            .next()
            .unwrap_or(&source.bookSourceUrl)
            .trim_end_matches('/');
        if base.starts_with("http://") || base.starts_with("https://") {
            headers.push(("Referer".to_string(), base.to_string()));
        }
    }
    headers
}

/// Build headers for a resolved URL, merging source headers with a Referer
/// for a specific page being fetched.
pub fn page_headers(source_headers: &[(String, String)], page_url: &str) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = source_headers.to_vec();
    let has_referer = headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("Referer"));
    if !has_referer && !page_url.is_empty() {
        headers.push(("Referer".to_string(), page_url.to_string()));
    }
    headers
}

fn urlencoding(s: &str) -> String {
    // Simple URL encoding similar to JavaScript's encodeURI (not encodeURIComponent)
    // For Chinese characters, Python's urllib.parse.quote is closer.
    // We'll use percent-encoding for non-ASCII chars
    let mut result = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                result.push(byte as char);
            }
            0x20 => result.push('+'),
            _ => {
                result.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_template_replacement() {
        let result = resolve_template("/search?keyword={{key}}&page={{page}}", "测试");
        assert_eq!(result, "/search?keyword=测试&page=1");
    }

    #[test]
    fn test_url_resolve_relative() {
        assert_eq!(
            resolve_url("/search", "https://example.com"),
            "https://example.com/search"
        );
    }

    #[test]
    fn test_url_resolve_protocol_relative() {
        assert_eq!(
            resolve_url("//cdn.example.com/img/a.jpg", "https://example.com"),
            "https://cdn.example.com/img/a.jpg"
        );
        assert_eq!(
            resolve_url("//cdn.example.com/img/a.jpg", "http://example.com"),
            "http://cdn.example.com/img/a.jpg"
        );
    }

    #[test]
    fn test_url_resolve_relative_path_traversal() {
        // base has a path directory
        assert_eq!(
            resolve_url("chapter/1.html", "https://example.com/novel/"),
            "https://example.com/novel/chapter/1.html"
        );
        assert_eq!(
            resolve_url("../chap/2.html", "https://example.com/novel/book/"),
            "https://example.com/novel/chap/2.html"
        );
        // absolute path drops base path
        assert_eq!(
            resolve_url("/api/list", "https://example.com/novel/book/"),
            "https://example.com/api/list"
        );
    }

    #[test]
    fn test_url_resolve_query_only() {
        assert_eq!(
            resolve_url("?page=2", "https://example.com/novel/1.html"),
            "https://example.com/novel/1.html?page=2"
        );
    }

    #[test]
    fn test_url_resolve_fragment() {
        assert_eq!(
            resolve_url("#top", "https://example.com/novel/1.html"),
            "https://example.com/novel/1.html#top"
        );
    }

    #[test]
    fn test_url_resolve_empty() {
        assert_eq!(
            resolve_url("", "https://example.com"),
            "https://example.com"
        );
    }

    #[test]
    fn test_url_resolve_absolute() {
        assert_eq!(
            resolve_url("https://other.com/search", "https://example.com"),
            "https://other.com/search"
        );
    }

    #[test]
    fn test_post_body_extraction() {
        let s = "/modules/article/search.php,{'charset':'gbk','body':'searchkey={{key}}','method':'POST'}";
        let body = extract_post_body(s);
        assert_eq!(body, Some("searchkey={{key}}".into()));

        let charset = extract_charset(s);
        assert_eq!(charset, Some("gbk".into()));
    }

    #[test]
    fn test_post_body_extraction_json_object() {
        let s = "http://www.lianjianxsw.com/search,{\"method\":\"POST\",\"body\":{\"keyword\":\"重生\"}}";
        let spec = parse_post_request(s, "http://www.lianjianxsw.com/").unwrap();
        assert_eq!(spec.url, "http://www.lianjianxsw.com/search");
        assert_eq!(spec.method, "POST");
        assert_eq!(spec.body.as_deref(), Some("{\"keyword\":\"重生\"}"));
    }

    #[test]
    fn test_post_body_extraction_json_object_single_quotes() {
        // Legado JS often emits single-quoted dicts
        let s = "http://www.lianjianxsw.com/search,{'method':'POST','body':{'keyword':'重生'}}";
        let spec = parse_post_request(s, "http://www.lianjianxsw.com/").unwrap();
        assert_eq!(spec.url, "http://www.lianjianxsw.com/search");
        assert_eq!(spec.body.as_deref(), Some("{'keyword':'重生'}"));
    }

    #[test]
    fn test_build_request_js_post_through_parse() {
        // Simulate a JS-produced POST URL (what the polyfill emits).
        let s = "http://www.lianjianxsw.com/search,{'method':'POST','body':{'keyword':'重生'}}";
        let spec = parse_post_request(s, "http://www.lianjianxsw.com/").unwrap();
        assert_eq!(spec.method, "POST");
        assert!(spec.body.as_deref().unwrap().contains("重生"));
    }
}
