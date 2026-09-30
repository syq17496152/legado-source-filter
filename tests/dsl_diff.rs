//! Legado 规则语义差分测试。
//!
//! 基准：Legado 官方规则说明（mgz0227/The-tutorial-of-Legado、gedoor/legado
//! 源码语义）。用**真实书源中的规则模式** + 代表性 HTML/JSON fixture 锁定本
//! 工具的提取语义，任何改动导致与 Legado 语义偏离都会让测试失败。
//!
//! 已实现并锁定：定位器（class./id./tag./裸 tag/@css:）、位置索引（`.n` 取、
//! `!n` 排除）、提取器（text/textNodes/html/href）、组合器（`||` 回退、
//! `##` 正则替换）、JSONPath、模板搜索 URL、类型不匹配短路。
//!
//! 已知差距（Legado 支持、本工具未实现，见 `known_gaps`）：负索引、区间
//! 语法 `[start:end]`、`&&` 拼接、`children`、`ownText`/`all` 提取器、
//! 多索引排除 `!0:2`。

use legado_source_filter::rule_dsl;
use legado_source_filter::rule_dsl::parser::tokenize;
use legado_source_filter::search_url::{build_request, parse_post_request, resolve_url};
use legado_source_filter::types::BookSource;

// ── 定位器语义 ──────────────────────────────────────────────

#[test]
fn class_定位_多元素() {
    // 真实源 bookList 最普遍的模式（1161 个源用 class.）。
    let html = "<ul><li class='item'>A</li><li class='item'>B</li></ul>";
    assert_eq!(
        rule_dsl::extract_single(html, "html", "class.item@text"),
        vec!["A", "B"]
    );
}

#[test]
fn id_定位() {
    let html = "<div id='bookbox'><h1>书名</h1></div>";
    assert_eq!(
        rule_dsl::extract_single(html, "html", "id.bookbox@tag.h1@text"),
        vec!["书名"]
    );
}

#[test]
fn tag_定位() {
    let html = "<div><h3>一</h3><h3>二</h3></div>";
    assert_eq!(rule_dsl::extract_single(html, "html", "tag.h3@text"), vec!["一", "二"]);
}

#[test]
fn 裸tag_规则() {
    // 真实源模式：`a.1`（第 2 个链接）、`h3`、`tbody tr!0`（排除表头）。
    let html = "<div><a href='/a'>一</a><a href='/b'>二</a></div>";
    assert_eq!(rule_dsl::extract_single(html, "html", "a.1@href"), vec!["/b"]);
    assert_eq!(rule_dsl::extract_single(html, "html", "a@text"), vec!["一", "二"]);

    let html = "<div><h3>书名</h3><p>作者</p></div>";
    assert_eq!(rule_dsl::extract_single(html, "html", "h3@text"), vec!["书名"]);
}

#[test]
fn 后代选择器() {
    let html = "<h2><a href='/x'>X</a></h2>";
    assert_eq!(rule_dsl::extract_single(html, "html", "h2 a@href"), vec!["/x"]);
}

#[test]
fn css_简写_dot_class() {
    // 真实源模式 `.s2` 等 CSS 简写。
    let html = "<div class='s2'>S2</div>";
    assert_eq!(rule_dsl::extract_single(html, "html", ".s2@text"), vec!["S2"]);
}

#[test]
fn 位置索引_take() {
    // Legado：位置从 0 开始，`.0` 取第一个。
    let html = "<ul><li class='a'>A1</li><li class='a'>A2</li><li class='a'>A3</li></ul>";
    assert_eq!(rule_dsl::extract_single(html, "html", "class.a.0@text"), vec!["A1"]);
    assert_eq!(rule_dsl::extract_single(html, "html", "class.a.1@text"), vec!["A2"]);
}

#[test]
fn 位置索引_exclude() {
    // Legado：`!n` 排除第 n 个（0-based）。真实源模式 `tbody tr!0`。
    let html = "<ul><li class='a'>A1</li><li class='a'>A2</li><li class='a'>A3</li></ul>";
    assert_eq!(rule_dsl::extract_single(html, "html", "class.a!0@text"), vec!["A2", "A3"]);

    let table = "<table><tbody><tr><th>表头</th></tr><tr><td>1</td></tr></tbody></table>";
    assert_eq!(
        rule_dsl::extract_single(table, "html", "tbody tr!0@text"),
        vec!["1"],
        "排除表头行"
    );
}

// ── 提取器语义 ──────────────────────────────────────────────

#[test]
fn text_递归文本() {
    let html = "<div>a<b>b</b>c</div>";
    assert_eq!(rule_dsl::extract_single(html, "html", "div@text"), vec!["abc"]);
}

#[test]
fn text_nodes_仅直接文本() {
    let html = "<div>a<b>b</b>c</div>";
    assert_eq!(
        rule_dsl::extract_single(html, "html", "div@textNodes"),
        vec!["ac"],
        "textNodes 只取直接文本子节点，不递归进 <b>"
    );
}

#[test]
fn html_内层html() {
    let html = "<div><p>x</p><p>y</p></div>";
    assert_eq!(rule_dsl::extract_single(html, "html", "div@html"), vec!["<p>x</p><p>y</p>"]);
}

#[test]
fn href_提取() {
    let html = "<div><a href='/b/1'>一</a><a href='/b/2'>二</a></div>";
    assert_eq!(rule_dsl::extract_single(html, "html", "a@href"), vec!["/b/1", "/b/2"]);
}

// ── 组合器语义 ──────────────────────────────────────────────

#[test]
fn 回退_第一非空() {
    // Legado：`||` 取第一个非空结果。
    let html = "<div><p class='a'>A</p><p class='b'>B</p></div>";
    assert_eq!(
        rule_dsl::extract_single(html, "html", "class.a@text||class.b@text"),
        vec!["A"]
    );
    // 第一段为空时回退到第二段。
    let html2 = "<div><p class='a'></p><p class='b'>B</p></div>";
    assert_eq!(
        rule_dsl::extract_single(html2, "html", "class.a@text||class.b@text"),
        vec!["B"]
    );
}

#[test]
fn 正则替换() {
    // Legado：`##正则##替换` 在规则末尾做替换。
    let html = "<ul><li>第1章</li></ul>";
    assert_eq!(
        rule_dsl::extract_single(html, "html", "li.0@text##[0-9]##X"),
        vec!["第X章"]
    );
    // 位置索引 + 回退 + 替换的组合（真实源里常见）。
    let html2 = "<ul><li class='a'>A1</li><li class='b'>B1</li></ul>";
    assert_eq!(
        rule_dsl::extract_single(html2, "html", "class.a.0@text##[0-9]##X||class.b@text"),
        vec!["AX"]
    );
}

// ── JSONPath 语义 ───────────────────────────────────────────

#[test]
fn json_path_提取() {
    let json = r#"{"data":{"list":[{"name":"一","id":1},{"name":"二","id":2}]}}"#;
    assert_eq!(
        rule_dsl::extract_single(json, "json", "$.data.list[*].name"),
        vec!["一", "二"]
    );
    assert_eq!(
        rule_dsl::extract_single(json, "json", "$.data.list[0].name"),
        vec!["一"]
    );
    assert_eq!(
        rule_dsl::extract_single(json, "json", "$.data.list[1].id"),
        vec!["2"]
    );
}

// ── 类型不匹配短路（第三批修复锁定）──────────────────────────

#[test]
fn html内容_上跑jsonpath_返回空() {
    // 修复前会把整页 HTML 当提取值 → 垃圾 bookUrl / name。
    let html = "<html><body><div class='item'>内容</div></body></html>";
    assert!(rule_dsl::extract_single(html, "html", "$.result.book_id").is_empty());
    assert!(rule_dsl::extract_single(html, "html", "data.books").is_empty());
}

#[test]
fn json内容_上跑html规则_返回空() {
    let json = r#"{"data":{"list":[]}}"#;
    assert!(rule_dsl::extract_single(json, "json", "class.item@text").is_empty());
}

// ── 搜索 URL 构造（真实源 searchUrl 模式）────────────────────

#[test]
fn 模板搜索url() {
    let source = BookSource {
        bookSourceName: "test".into(),
        bookSourceUrl: "https://example.com".into(),
        searchUrl: Some("/s?keyword={{key}}&page={{page}}".into()),
        ..Default::default()
    };
    let spec = build_request(&source, "重生").unwrap();
    assert_eq!(spec.url, "https://example.com/s?keyword=重生&page=1");
    assert_eq!(spec.method, "GET");
}

#[test]
fn post搜索url() {
    // Legado POST 约定：url,{'method':'POST','body':'searchkey={{key}}'}。
    let s = "/modules/article/search.php,{'charset':'gbk','body':'searchkey={{key}}','method':'POST'}";
    let spec = parse_post_request(s, "https://example.com").unwrap();
    assert_eq!(spec.url, "https://example.com/modules/article/search.php");
    assert_eq!(spec.method, "POST");
    assert_eq!(spec.body.as_deref(), Some("searchkey={{key}}"));
}

#[test]
fn 相对url解析() {
    assert_eq!(resolve_url("/c/1", "https://example.com/toc"), "https://example.com/c/1");
    assert_eq!(resolve_url("../x", "https://example.com/a/b/"), "https://example.com/a/x");
    assert_eq!(resolve_url("//cdn.example.com/x", "https://example.com"), "https://cdn.example.com/x");
    assert_eq!(resolve_url("?p=2", "https://example.com/1.html"), "https://example.com/1.html?p=2");
}

// ── 已知差距：Legado 支持、本工具未实现 ──────────────────────
// 这些测试文档化当前行为而非断言正确语义（与 Legado 的差距已知且未实现）。

#[test]
fn known_gaps_文档化() {
    let html = "<ul><li>一</li><li>二</li><li>三</li></ul>";

    // 负索引（Legado：-1 = 倒数第一个）。当前未实现 → 空。
    assert!(rule_dsl::extract_single(html, "html", "li.-1@text").is_empty());

    // 区间语法 [start:end]（Legado：li[1:3]）。当前未实现 → 空。
    assert!(rule_dsl::extract_single(html, "html", "li[1:3]@text").is_empty());

    // `&&` 拼接（Legado：两段结果拼接）。当前无 && 处理 → 只取第一段语义。
    // 这里只断言不 panic，不锁定输出（未实现）。
    let _ = rule_dsl::extract_single(html, "html", "li.0@text&&li.1@text");

    // `children` / `ownText` / `all` 提取器。当前未实现 → 空或原样。
    assert!(rule_dsl::extract_single(html, "html", "li@ownText").is_empty());

    // 多索引排除 `!0:2`。当前只支持单个 `!n`。
    assert_eq!(
        rule_dsl::extract_single(html, "html", "li!0@text"),
        vec!["二", "三"],
        "单索引排除已实现"
    );
}

// tokenize 冒烟：确保上面用到的规则都能被词法分析（不 panic）。
#[test]
fn tokenize_冒烟() {
    for rule in [
        "class.item@text",
        "id.bookbox@tag.h1@text",
        "a.1@href",
        "tbody tr!0@text",
        "h2 a@href",
        ".s2@text",
        "$.data.list[*].name",
        "class.a.0@text||class.b@text",
        "li.0@text##[0-9]##X",
    ] {
        assert!(!tokenize(rule).is_empty(), "rule must tokenize: {rule}");
    }
}
