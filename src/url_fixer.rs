/// Auto-fix malformed `bookSourceUrl` values.
/// Returns `Some(fixed_url)` if fixable, `None` if irreparably broken.
pub fn fix_url(raw: &str) -> Option<String> {
    let trimmed = raw.trim();

    // Already valid
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        return Some(trimmed.to_string());
    }

    // [图片]http://... → strip prefix.
    // Only match at protocol start to avoid matching "http" in URL paths.
    if let Some(pos) = trimmed.find("http://").or_else(|| trimmed.find("https://"))
        && pos > 0
        && trimmed[..pos].contains("图片")
    {
        return Some(trimmed[pos..].to_string());
    }

    // http:www.xxx.com → add slash
    if trimmed.starts_with("http:") && !trimmed.starts_with("http://") {
        return Some(trimmed.replacen("http:", "http://", 1));
    }

    // www.xxx.com or m.xxx.com → prepend https://
    if trimmed.starts_with("www.") || trimmed.starts_with("m.") {
        return Some(format!("https://{}", trimmed));
    }

    // something like "novel.html5.qq.com" — domain-like, no protocol
    if let Some(dot_pos) = trimmed.find('.') {
        let before_dot = &trimmed[..dot_pos];
        // must start with an alphanumeric char (not whitespace, not emoji)
        if before_dot
            .chars()
            .next()
            .is_some_and(|c| c.is_alphanumeric())
            && !trimmed.contains(char::is_whitespace)
            && !trimmed.contains("：")
        {
            return Some(format!("https://{}", trimmed));
        }
    }

    None
}

/// Normalize a URL's host to a canonical domain key for cross-source
/// aggregation: lowercase, strip a leading `www.` and any trailing dot,
/// drop the port. `None` when the URL has no usable host.
///
/// Used by dead-domain detection so `m.example.com`, `www.example.com` and
/// `example.com:8080` all count as the same site.
pub fn normalized_domain(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let parsed = url::Url::parse(trimmed)
        .or_else(|_| url::Url::parse(&format!("https://{trimmed}")))
        .ok()?;
    let host = parsed.host_str()?.trim_end_matches('.').to_lowercase();
    Some(host.strip_prefix("www.").unwrap_or(&host).to_string())
}

/// Whether a normalized domain belongs to a site hint like `qidian.com`
/// (`m.qidian.com` and `www.qidian.com` both match; `notqidian.com` does not).
pub fn domain_matches(domain: &str, hint: &str) -> bool {
    let hint = hint.trim().trim_end_matches('.').to_lowercase();
    if hint.is_empty() {
        return false;
    }
    domain == hint || domain.ends_with(&format!(".{}", hint))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_already_valid() {
        assert_eq!(
            fix_url("https://example.com"),
            Some("https://example.com".into())
        );
        assert_eq!(
            fix_url("http://example.com"),
            Some("http://example.com".into())
        );
    }

    #[test]
    fn test_strip_image_prefix() {
        assert_eq!(
            fix_url("[图片]http://www.sfacg.com"),
            Some("http://www.sfacg.com".into())
        );
    }

    #[test]
    fn test_add_https() {
        assert_eq!(
            fix_url("www.example.com"),
            Some("https://www.example.com".into())
        );
        assert_eq!(
            fix_url("m.example.com"),
            Some("https://m.example.com".into())
        );
        assert_eq!(
            fix_url("novel.html5.qq.com"),
            Some("https://novel.html5.qq.com".into())
        );
    }

    #[test]
    fn test_fix_http_colon() {
        assert_eq!(
            fix_url("http:www.example.com"),
            Some("http://www.example.com".into())
        );
    }

    #[test]
    fn test_trim_whitespace() {
        assert_eq!(
            fix_url(" https://example.com"),
            Some("https://example.com".into())
        );
    }

    #[test]
    fn test_unfixable() {
        assert_eq!(fix_url("小说合集"), None);
        assert_eq!(fix_url("DQuestQBall"), None);
        assert_eq!(fix_url(""), None);
        assert_eq!(fix_url("一个书源"), None);
    }

    #[test]
    fn test_normalized_domain() {
        assert_eq!(
            normalized_domain("https://www.example.com"),
            Some("example.com".into())
        );
        assert_eq!(
            normalized_domain("https://m.example.com/path?q=1"),
            Some("m.example.com".into())
        );
        assert_eq!(
            normalized_domain("HTTP://EXAMPLE.COM.:8080/"),
            Some("example.com".into())
        );
        assert_eq!(normalized_domain("example.com"), Some("example.com".into()));
        assert_eq!(normalized_domain("not a url"), None);
        assert_eq!(normalized_domain(""), None);
    }

    #[test]
    fn test_domain_matches() {
        assert!(domain_matches("qidian.com", "qidian.com"));
        assert!(domain_matches("m.qidian.com", "qidian.com"));
        assert!(domain_matches("www.qidian.com", "qidian.com"));
        assert!(!domain_matches("notqidian.com", "qidian.com"));
        assert!(!domain_matches("qidian.com", ""));
    }
}
