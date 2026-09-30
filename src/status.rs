//! Typed test statuses and failure reasons.
//!
//! These serialize to the same short strings the SQLite cache has always
//! stored (`passed`, `failed`, `network_error`, ...), so existing caches keep
//! working; the types exist to make the semantics explicit in code instead of
//! scattered `&str` literals that can silently drift apart.

use serde::{Deserialize, Serialize};

/// Final verdict for a book source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestStatus {
    /// Full reading chain verified: search → book info → TOC → readable content.
    Passed,
    /// Deterministic or transient failure (see `FailReason`).
    Failed,
    /// Every tested source on the (normalized) domain failed deterministically —
    /// the whole site looks dead, so later rounds skip it.
    DeadDomain,
    /// Requires interactive browser / login the test harness cannot perform.
    JsApi,
    /// Skipped by preflight (normally not stored in the test table).
    Skipped,
    /// Search worked but no book URL was produced, so the reading chain could
    /// not be verified. Not included in filtered.json.
    Unverified,
}

impl TestStatus {
    /// The string stored in `test_results.status` (backward compatible).
    pub fn as_str(&self) -> &'static str {
        match self {
            TestStatus::Passed => "passed",
            TestStatus::Failed => "failed",
            TestStatus::DeadDomain => "dead_domain",
            TestStatus::JsApi => "js_api",
            TestStatus::Skipped => "skipped",
            TestStatus::Unverified => "unverified",
        }
    }

    /// Parse a cached status string. Unknown strings map to `None` so callers
    /// treat unrecognized rows as untested rather than guessing.
    // Returns `Option`, not `Result`, so it can't be `FromStr`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<TestStatus> {
        match s {
            "passed" => Some(TestStatus::Passed),
            "failed" => Some(TestStatus::Failed),
            "dead_domain" => Some(TestStatus::DeadDomain),
            "js_api" => Some(TestStatus::JsApi),
            "skipped" => Some(TestStatus::Skipped),
            "unverified" => Some(TestStatus::Unverified),
            _ => None,
        }
    }

    /// Whether a cached row with this status should be re-tested (used by
    /// `tester`'s cache check: passed/dead/js_api/unverified are terminal).
    pub fn is_terminal(&self) -> bool {
        !matches!(self, TestStatus::Failed)
    }
}

/// Why a source failed. Serialized to the same strings the cache has always
/// used for the `reason` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailReason {
    /// Search returned no results (site may not have the book, or rule broken).
    NoResults,
    /// TOC rule extracted no chapter links (rule broken).
    TocParse,
    /// Extracted chapter text below the readability threshold.
    ContentShort,
    /// HTTP 401/403 — anti-bot / auth wall.
    Blocked,
    /// HTTP 404.
    NotFound,
    /// HTTP 408/429 — throttled.
    RateLimited,
    /// Network-level failure: timeout, DNS, connect, TLS.
    NetworkError,
    /// Book detail page fetch failed (transient).
    BookinfoFetch,
    /// TOC page fetch failed (transient).
    TocFetch,
    /// Chapter page fetch failed (transient).
    ChapterFetch,
    /// HTTP 5xx (transient).
    ServerError,
    /// Could not build a request from the search URL rule.
    BuilderError,
    /// No ruleToc on a source (chain cannot proceed).
    NoTocRule,
    /// No ruleContent.content rule.
    NoContentRule,
    /// Body was empty / content extraction yielded nothing.
    ContentEmpty,
    /// Other HTTP status (unexpected 4xx, or a 2xx that never happens).
    HttpError,
    /// Search succeeded but no book URL was produced — chain unverifiable.
    NoBookUrl,
    /// Search results look static (same results for different keywords).
    StaticList,
    /// The reading chain works, but the detail page's book name doesn't match
    /// the search result — the rule is pulling the wrong book ("张冠李戴").
    Mismatch,
}

impl FailReason {
    /// The string stored in `test_results.reason` (backward compatible).
    pub fn as_str(&self) -> &'static str {
        match self {
            FailReason::NoResults => "no_results",
            FailReason::TocParse => "toc_parse",
            FailReason::ContentShort => "content_short",
            FailReason::Blocked => "blocked",
            FailReason::NotFound => "not_found",
            FailReason::RateLimited => "rate_limited",
            FailReason::NetworkError => "network_error",
            FailReason::BookinfoFetch => "bookinfo_fetch",
            FailReason::TocFetch => "toc_fetch",
            FailReason::ChapterFetch => "chapter_fetch",
            FailReason::ServerError => "server_error",
            FailReason::BuilderError => "builder_error",
            FailReason::NoTocRule => "no_toc_rule",
            FailReason::NoContentRule => "no_content_rule",
            FailReason::ContentEmpty => "content_empty",
            FailReason::HttpError => "http_error",
            FailReason::NoBookUrl => "no_book_url",
            FailReason::StaticList => "static_list",
            FailReason::Mismatch => "mismatch",
        }
    }

    /// Parse a cached reason string. Unknown strings (e.g. from older tool
    /// versions) map to `None`; callers must treat those conservatively.
    // Returns `Option`, not `Result`, so it can't be `FromStr`.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<FailReason> {
        match s {
            "no_results" => Some(FailReason::NoResults),
            "toc_parse" => Some(FailReason::TocParse),
            "content_short" => Some(FailReason::ContentShort),
            "blocked" => Some(FailReason::Blocked),
            "not_found" => Some(FailReason::NotFound),
            "rate_limited" => Some(FailReason::RateLimited),
            "network_error" => Some(FailReason::NetworkError),
            "bookinfo_fetch" => Some(FailReason::BookinfoFetch),
            "toc_fetch" => Some(FailReason::TocFetch),
            "chapter_fetch" => Some(FailReason::ChapterFetch),
            "server_error" => Some(FailReason::ServerError),
            "builder_error" => Some(FailReason::BuilderError),
            "no_toc_rule" => Some(FailReason::NoTocRule),
            "no_content_rule" => Some(FailReason::NoContentRule),
            "content_empty" => Some(FailReason::ContentEmpty),
            "http_error" => Some(FailReason::HttpError),
            "no_book_url" => Some(FailReason::NoBookUrl),
            "static_list" => Some(FailReason::StaticList),
            "mismatch" => Some(FailReason::Mismatch),
            _ => None,
        }
    }

    /// Failures that may succeed on a later attempt and are worth retrying in
    /// later rounds. Deterministic failures (`no_results`, `toc_parse`,
    /// `blocked`, `not_found`, `builder_error`, `content_short`, ...) are
    /// skipped. Unknown reasons are treated as non-transient (conservative).
    pub fn is_transient(&self) -> bool {
        matches!(
            self,
            FailReason::NetworkError
                | FailReason::BookinfoFetch
                | FailReason::TocFetch
                | FailReason::ChapterFetch
                | FailReason::RateLimited
                | FailReason::ServerError
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_roundtrip() {
        for s in [
            TestStatus::Passed,
            TestStatus::Failed,
            TestStatus::DeadDomain,
            TestStatus::JsApi,
            TestStatus::Skipped,
            TestStatus::Unverified,
        ] {
            assert_eq!(TestStatus::from_str(s.as_str()), Some(s));
        }
        assert_eq!(TestStatus::from_str("bogus"), None);
    }

    #[test]
    fn test_reason_roundtrip() {
        for r in [
            FailReason::NoResults,
            FailReason::TocParse,
            FailReason::ContentShort,
            FailReason::Blocked,
            FailReason::NotFound,
            FailReason::RateLimited,
            FailReason::NetworkError,
            FailReason::BookinfoFetch,
            FailReason::TocFetch,
            FailReason::ChapterFetch,
            FailReason::ServerError,
            FailReason::BuilderError,
            FailReason::NoTocRule,
            FailReason::NoContentRule,
            FailReason::ContentEmpty,
            FailReason::HttpError,
            FailReason::NoBookUrl,
            FailReason::StaticList,
            FailReason::Mismatch,
        ] {
            assert_eq!(FailReason::from_str(r.as_str()), Some(r));
        }
        assert_eq!(FailReason::from_str("bogus"), None);
    }

    #[test]
    fn test_transient_classification() {
        for r in [
            FailReason::NetworkError,
            FailReason::BookinfoFetch,
            FailReason::TocFetch,
            FailReason::ChapterFetch,
            FailReason::RateLimited,
            FailReason::ServerError,
        ] {
            assert!(r.is_transient(), "{r:?} should be transient");
        }
        for r in [
            FailReason::NoResults,
            FailReason::TocParse,
            FailReason::ContentShort,
            FailReason::Blocked,
            FailReason::NotFound,
            FailReason::BuilderError,
            FailReason::NoTocRule,
            FailReason::NoContentRule,
            FailReason::ContentEmpty,
            FailReason::HttpError,
            FailReason::NoBookUrl,
            FailReason::StaticList,
            FailReason::Mismatch,
        ] {
            assert!(!r.is_transient(), "{r:?} should be deterministic");
        }
    }

    #[test]
    fn test_legacy_reason_strings_still_parse() {
        // Strings that old tool versions wrote must keep working.
        assert_eq!(FailReason::from_str("network_error"), Some(FailReason::NetworkError));
        assert_eq!(FailReason::from_str("content_short"), Some(FailReason::ContentShort));
    }
}
