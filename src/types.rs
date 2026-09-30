use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

/// Preserve any fields not explicitly modeled in a struct, so a round-trip
/// (parse → serialize) never drops Legado rules we don't know about yet.
type Extra = serde_json::Map<String, Value>;

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[allow(non_snake_case)]
pub struct BookSource {
    pub bookSourceName: String,
    pub bookSourceUrl: String,
    pub bookSourceType: i64,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bookSourceGroup: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bookSourceComment: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bookUrlPattern: Option<String>,

    #[serde(default)]
    pub customOrder: i64,

    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default)]
    pub enabledCookieJar: bool,

    #[serde(default)]
    pub enabledExplore: bool,

    #[serde(default)]
    pub enabledReview: bool,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exploreUrl: Option<Value>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loginUrl: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loginUi: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loginCheckJs: Option<String>,

    #[serde(default)]
    pub lastUpdateTime: i64,

    #[serde(default)]
    pub respondTime: i64,

    #[serde(default)]
    pub weight: i64,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub concurrentRate: Option<Value>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverDecodeJs: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exploreScreen: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jsLib: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variableComment: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub searchUrl: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ruleSearch: Option<SearchRule>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ruleBookInfo: Option<BookInfoRule>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ruleContent: Option<ContentRule>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ruleToc: Option<TocRule>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ruleExplore: Option<ExploreRule>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ruleReview: Option<Value>,

    #[serde(flatten)]
    pub extra: Extra,
}

fn default_true() -> bool {
    true
}

// ── Rule structs ──────────────────────────────────────────────

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[allow(non_snake_case)]
pub struct SearchRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bookList: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bookUrl: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverUrl: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intro: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lastChapter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wordCount: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkKeyWord: Option<String>,

    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[allow(non_snake_case)]
pub struct BookInfoRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wordCount: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lastChapter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intro: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverUrl: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tocUrl: Option<String>,

    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[allow(non_snake_case)]
pub struct ContentRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    /// Rule extracting the URL of the next page of a paginated chapter.
    /// When a chapter spans multiple pages, `content` yields only the current
    /// page; walking `nextContentUrl` lets us assemble the full chapter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nextContentUrl: Option<String>,

    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[allow(non_snake_case)]
pub struct TocRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapterList: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapterName: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapterUrl: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isVip: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updateTime: Option<String>,

    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[allow(non_snake_case)]
pub struct ExploreRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bookList: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bookUrl: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverUrl: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intro: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lastChapter: Option<String>,

    #[serde(flatten)]
    pub extra: Extra,
}

// ── Skip/Reason enums ────────────────────────────────────────

/// Parse a source's `concurrentRate` into a minimum inter-request interval.
///
/// Legado allows either a plain millisecond number (`"1000"`) or a JSON
/// object (`{"rate": N, "interval": N, "timeUnit": "ms"}` / `"s"`).
/// Returns `None` when absent or unparseable (no rate limiting).
pub fn parse_concurrent_rate(v: Option<&Value>) -> Option<Duration> {
    let v = v?;
    match v {
        Value::String(s) => {
            let s = s.trim();
            if s.is_empty() {
                return None;
            }
            // Plain milliseconds ("1000").
            if let Ok(ms) = s.parse::<u64>() {
                return Some(Duration::from_millis(ms));
            }
            // JSON object serialized as a string.
            if let Ok(obj) = serde_json::from_str::<Value>(s) {
                return parse_concurrent_rate(Some(&obj));
            }
            None
        }
        Value::Object(obj) => {
            // Semantics: every `interval` × `timeUnit`, at most `rate` requests
            // are allowed → per-request pause = interval × unit / rate.
            let interval_units = obj
                .get("interval")
                .and_then(|v| v.as_u64())
                .unwrap_or(1)
                .max(1);
            let rate = obj
                .get("rate")
                .and_then(|v| v.as_u64())
                .filter(|r| *r > 0)
                .unwrap_or(1);
            let time_unit = obj
                .get("timeUnit")
                .and_then(|v| v.as_str())
                .unwrap_or("ms")
                .to_lowercase();
            let unit_ms = match time_unit.as_str() {
                "s" | "second" | "seconds" => 1000u64,
                "m" | "minute" | "minutes" => 60_000u64,
                _ => 1u64, // ms / anything unknown
            };
            let interval_ms = interval_units.saturating_mul(unit_ms).checked_div(rate);
            interval_ms.map(|ms| Duration::from_millis(ms.max(1)))
        }
        _ => None,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[allow(non_snake_case)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// bookSourceUrl is invalid and can't be auto-fixed
    BadUrl,
    /// No searchUrl and no exploreUrl
    NoCapability,
    /// Has searchUrl but missing ruleSearch
    NoSearchRule,
    /// Missing ruleContent (can search but can't read)
    NoContentRule,
}

// ── Output containers ────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PreflightOutput {
    pub total_input: usize,
    pub excluded: usize,
    pub text_enabled: usize,
    pub eligible: Vec<BookSource>,
    pub skipped: Vec<(BookSource, SkipReason)>,
    pub explore_only: Vec<BookSource>,
    /// Breakdown of eligible sources by searchUrl type
    pub breakdown: PreflightBreakdown,
}

#[derive(Debug, Serialize)]
pub struct PreflightBreakdown {
    pub template: usize,
    pub js_prefix: usize,
    pub js_block: usize,
    pub pure_url: usize,
    pub placeholder: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Round-trip must preserve unknown top-level and nested-rule fields,
    /// otherwise filtered.json would silently drop Legado rules.
    #[test]
    fn test_roundtrip_preserves_unknown_fields() {
        let json = r###"{
            "bookSourceName": "示例",
            "bookSourceUrl": "https://example.com",
            "bookSourceType": 0,
            "enabled": true,
            "searchUrl": "/s?q={{key}}",
            "futureTopLevel": {"a": 1},
            "ruleSearch": {"bookList": "ul", "name": "a@text", "futureSearch": "x"},
            "ruleContent": {"content": "div@text", "nextContentUrl": "@css:body@text", "replaceRegex": "##x||y"},
            "ruleBookInfo": {"name": "h1@text", "init": "@js:var a=1"},
            "ruleToc": {"chapterList": "li", "nextTocUrl": "url", "isVolume": "true"}
        }"###;

        let source: super::BookSource = serde_json::from_str(json).unwrap();
        let back = serde_json::to_value(&source).unwrap();

        assert_eq!(back["futureTopLevel"], serde_json::json!({"a": 1}));
        let rc = &back["ruleContent"];
        assert_eq!(rc["nextContentUrl"], serde_json::json!("@css:body@text"));
        assert_eq!(rc["replaceRegex"], serde_json::json!("##x||y"));
        assert_eq!(back["ruleSearch"]["futureSearch"], serde_json::json!("x"));
        assert_eq!(
            back["ruleBookInfo"]["init"],
            serde_json::json!("@js:var a=1")
        );
        assert_eq!(back["ruleToc"]["isVolume"], serde_json::json!("true"));
    }

    #[test]
    fn test_parse_concurrent_rate() {
        use serde_json::json;

        // Plain millisecond strings (the format in the real dataset).
        assert_eq!(
            parse_concurrent_rate(Some(&json!("1000"))),
            Some(Duration::from_millis(1000))
        );
        assert_eq!(
            parse_concurrent_rate(Some(&json!("3000"))),
            Some(Duration::from_millis(3000))
        );
        // JSON object forms.
        assert_eq!(
            parse_concurrent_rate(Some(&json!({"interval": 500}))),
            Some(Duration::from_millis(500))
        );
        assert_eq!(
            parse_concurrent_rate(Some(&json!({"rate": 2, "timeUnit": "s"}))),
            Some(Duration::from_millis(500)),
            "2 requests per second → 500ms apart"
        );
        assert_eq!(
            parse_concurrent_rate(Some(&json!({"rate": 1, "interval": 3, "timeUnit": "s"}))),
            Some(Duration::from_millis(3000)),
            "1 request per 3 seconds"
        );
        // Object serialized as string.
        assert_eq!(
            parse_concurrent_rate(Some(&json!(r#"{"interval": 700}"#))),
            Some(Duration::from_millis(700))
        );
        // Absent / unparseable.
        assert_eq!(parse_concurrent_rate(None), None);
        assert_eq!(parse_concurrent_rate(Some(&json!("abc"))), None);
        assert_eq!(parse_concurrent_rate(Some(&json!(42))), None);
    }
}
