//! End-to-end smoke test of the reading-chain verification on real sources:
//! runs the actual search step, then walks book info → TOC → first chapter.
//! Run: `cargo run --example chain_check`

use legado_source_filter::content_check::verify_chain;
use legado_source_filter::http_client::HttpClient;
use legado_source_filter::rule_dsl;
use legado_source_filter::search_url;
use legado_source_filter::types::BookSource;

fn main() -> anyhow::Result<()> {
    let filter = std::env::args().nth(1);
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run(filter))
}

async fn run(filter: Option<String>) -> anyhow::Result<()> {
    let raw = std::fs::read_to_string("data/b778fe6b.json")?;
    let list: Vec<serde_json::Value> = serde_json::from_str(&raw)?;
    let all: Vec<BookSource> = list
        .iter()
        .map(|v| serde_json::from_value(v.clone()).unwrap())
        .collect();

    let client = HttpClient::new(20)?;

    let db =
        legado_source_filter::db::TestCache::new(std::path::Path::new("output/test_cache.db"))?;

    let mut tested = 0;
    for s in all.iter() {
        // Optional name filter: test matching sources regardless of cache state.
        if let Some(ref f) = filter {
            if !s.bookSourceName.contains(f.as_str()) {
                continue;
            }
        } else {
            // Default: only sources previously marked passed in the cache.
            let Ok(Some((st, _))) = db.check(&s.bookSourceUrl, &s.bookSourceName) else {
                continue;
            };
            if st != "passed" {
                continue;
            }
        }
        tested += 1;
        if tested > 10 {
            break;
        }
        // ── Search step ──
        // Template / L1 @js search URLs resolve on the Rust side; `@js:` / `<js>`
        // search rules go through the JS polyfill (with java.ajax bridge).
        let mut book_url = None;
        let su = s.searchUrl.as_deref().unwrap_or("");
        let needs_js =
            (su.starts_with("@js:") || su.contains("<js>")) && !su.contains("startBrowserAwait");
        if needs_js {
            use legado_source_filter::js_polyfill::runner::{JsJob, execute_js_batch_async};
            let src_headers = search_url::source_headers(s);
            let job = JsJob {
                id: 0,
                base_url: s.bookSourceUrl.clone(),
                keyword: "重生".into(),
                js_code: su.to_string(),
            };
            let out = execute_js_batch_async(&[job], &client, &src_headers).await;
            if let Some(u) = out
                .into_iter()
                .next()
                .flatten()
                .filter(|u| !u.trim().is_empty())
            {
                book_url = Some(u);
            }
            if std::env::var("CHAIN_DEBUG").is_ok() {
                println!("  [js-search] {}", book_url.as_deref().unwrap_or("<none>"));
            }
        } else if let Some(spec) = search_url::build_request(s, "重生")
            && let Ok(res) = client.fetch(&spec).await {
                let ct = if res.content_type.contains("json") {
                    "json"
                } else {
                    "html"
                };
                if let Some(ref rule) = s.ruleSearch {
                    let results = if legado_source_filter::content_check::rule_has_js(rule) {
                        legado_source_filter::content_check::extract_results_js(
                            &client,
                            s,
                            &res,
                            &s.bookSourceUrl,
                        )
                        .await
                    } else {
                        rule_dsl::extract_results(&res.body_text, ct, rule)
                    };
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
