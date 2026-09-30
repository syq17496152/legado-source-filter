//! End-to-end check of the java.ajax bridge on a real source whose
//! ruleContent depends on a secondary HTTP request.
//! Run: `cargo run --example ajax_bridge_check`

use legado_source_filter::content_check::verify_chain;
use legado_source_filter::http_client::HttpClient;
use legado_source_filter::rule_dsl;
use legado_source_filter::search_url;
use legado_source_filter::types::BookSource;

fn main() -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run())
}

async fn run() -> anyhow::Result<()> {
    let raw = std::fs::read_to_string("data/b778fe6b.json")?;
    let list: Vec<serde_json::Value> = serde_json::from_str(&raw)?;
    let all: Vec<BookSource> = list
        .iter()
        .map(|v| serde_json::from_value(v.clone()).unwrap())
        .collect();

    let client = HttpClient::new(20)?;

    for s in all.iter().take(2000) {
        let rc = s.ruleContent.as_ref();
        let has_ajax = rc
            .and_then(|c| c.content.as_deref())
            .map(|c| c.contains("java.ajax") || c.contains("java.post"))
            .unwrap_or(false);
        if !has_ajax {
            continue;
        }
        // Run the full chain for this source
        let mut book_url = None;
        if let Some(spec) = search_url::build_request(s, "重生")
            && let Ok(res) = client.fetch(&spec).await {
                let ct = if res.content_type.contains("json") {
                    "json"
                } else {
                    "html"
                };
                if let Some(ref rule) = s.ruleSearch {
                    let results = rule_dsl::extract_results(&res.body_text, ct, rule);
                    if rule_dsl::has_valid_results(&results) {
                        book_url = results
                            .first()
                            .and_then(|r| r.get(2))
                            .cloned()
                            .map(|u| u.trim().to_string())
                            .filter(|u| !u.is_empty());
                    }
                }
            }
        let Some(bu) = book_url else {
            println!("{}: search produced no bookUrl", s.bookSourceName);
            continue;
        };
        let resolved = search_url::resolve_url(&bu, &s.bookSourceUrl);
        let verdict = verify_chain(&client, s, &resolved, &s.bookSourceUrl, None).await;
        println!(
            "{}: {} at {} ({}) len={}",
            s.bookSourceName,
            if verdict.ok { "PASS" } else { "FAIL" },
            verdict.stage,
            verdict.detail,
            verdict.content_len
        );
    }
    Ok(())
}
