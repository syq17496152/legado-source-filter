use serde::{Deserialize, Serialize};
use serde_json::Value;

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
}
