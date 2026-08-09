use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tracing::{info, warn};

use crate::db::TestCache;
use crate::http_client::HttpClient;
use crate::rule_dsl;
use crate::search_url;
use crate::search_url::RequestSpec;
use crate::types::*;

/// A cheap, deterministic jitter source (counter mixed with wall-clock nanos)
/// so concurrent retries spread out instead of firing in lockstep.
fn jittered_delay(base_ms: u64, jitter_range_ms: u64) -> Duration {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos() as u64;
    let mut x = c.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ nanos;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    Duration::from_millis(base_ms + x % jitter_range_ms)
}

/// Increments an AtomicUsize on drop — ensures progress is counted
/// even when a task panics.
struct IncOnDrop(Arc<std::sync::atomic::AtomicUsize>);
impl Drop for IncOnDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[derive(Clone)]
pub struct TestConfig {
    pub concurrency: usize,
    pub timeout_secs: u64,
    pub test_books: Vec<TestBook>,
    pub generic_keywords: Vec<String>,
    pub force: bool,
    /// When true, a source must also extract readable content from its first
    /// search result's book page (ruleContent) before being marked passed.
    pub verify_content: bool,
}

#[derive(Clone, Debug)]
pub struct TestBook {
    pub name: String,
    pub author: String,
    pub domain_hint: Option<String>,
}

pub struct TestSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub cached: usize,
    pub js_api: Vec<BookSource>,
}

pub async fn run(
    eligible: Vec<BookSource>,
    config: TestConfig,
    cache: Arc<TestCache>,
    quiet: bool,
) -> anyhow::Result<TestSummary> {
    let client = Arc::new(HttpClient::new(config.timeout_secs)?);
    let config = Arc::new(config);
    let semaphore = Arc::new(Semaphore::new(config.concurrency));
    let mut handles = Vec::new();
    let passed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let failed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let cached = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let js_api = Arc::new(std::sync::Mutex::new(Vec::new()));
    let completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let total = eligible.len();
    let completed_progress = completed.clone();
    let progress_handle = tokio::spawn(async move {
        let mut last = 0;
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let current = completed_progress.load(std::sync::atomic::Ordering::Relaxed);
            if current >= total {
                break;
            }
            if current > last {
                if !quiet {
                    println!(
                        "Progress: {}/{} ({:.0}%)",
                        current,
                        total,
                        current as f64 / total as f64 * 100.0
                    );
                }
                last = current;
            }
        }
    });

    for source in eligible {
        let client = client.clone();
        let cache = Arc::clone(&cache);
        let semaphore = semaphore.clone();
        let passed = passed.clone();
        let failed = failed.clone();
        let cached = cached.clone();
        let js_api = js_api.clone();
        let completed = completed.clone();
        let config = Arc::clone(&config);

        handles.push(tokio::spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();
            let _inc_on_drop = IncOnDrop(completed.clone());

            // Check cache first (skipped entirely under --force)
            if !config.force
                && let Ok(Some((status, _))) =
                    cache.check(&source.bookSourceUrl, &source.bookSourceName)
            {
                match status.as_str() {
                    "passed" => {
                        passed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        return;
                    }
                    "failed" => {
                        // Re-test failed sources
                    }
                    "dead_domain" => {
                        cached.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        return;
                    }
                    "js_api" => {
                        cached.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        return;
                    }
                    _ => {}
                }
            }

            // Check startBrowserAwait — unrecoverable
            let su = source.searchUrl.as_deref().unwrap_or("");
            let is_startbrowser = su.contains("java.startBrowserAwait");
            let needs_js = (su.starts_with("@js:") || su.contains("<js>")) && !is_startbrowser;

            if is_startbrowser {
                if let Err(e) = cache.save(
                    &source.bookSourceUrl,
                    &source.bookSourceName,
                    "js_api",
                    None,
                    0,
                ) {
                    warn!("cache save failed for {}: {}", source.bookSourceName, e);
                }
                // Use into_inner() to recover from a poisoned mutex
                let mut guard = match std::sync::Mutex::lock(&*js_api) {
                    Ok(g) => g,
                    Err(e) => e.into_inner(),
                };
                guard.push(source);
                return;
            }

            // Select keywords
            let keywords = select_keywords(&source, &config);

            let base_url = source.bookSourceUrl.clone();
            let js_code = su.to_string();
            let src_headers = crate::search_url::source_headers(&source);

            let mut all_spec_failed = true;
            let mut any_network_error = false;

            // ── Phase 1: build request specs, collecting JS jobs for batching ──
            let mut specs: Vec<(String, RequestSpec)> = Vec::new();
            let mut js_jobs: Vec<crate::js_polyfill::runner::JsJob> = Vec::new();

            for keyword in &keywords {
                // Rust-side request builder handles templates and @js: L1
                if let Some(mut spec) = search_url::build_request(&source, keyword) {
                    all_spec_failed = false;
                    spec.headers = src_headers.clone();
                    specs.push((keyword.clone(), spec));
                } else if needs_js {
                    // L2/L3 @js: and <js> sources go through the JS polyfill
                    js_jobs.push(crate::js_polyfill::runner::JsJob {
                        id: js_jobs.len(),
                        base_url: base_url.clone(),
                        keyword: keyword.clone(),
                        js_code: js_code.clone(),
                    });
                }
            }

            // Evaluate all JS jobs in ONE node process (with java.ajax bridge
            // so `@js` search rules that issue their own HTTP get real answers)
            if !js_jobs.is_empty() {
                let src_headers_clone = src_headers.clone();
                let (js_results, js_jobs) = {
                    let client_ref = client.clone();
                    let out = crate::js_polyfill::runner::execute_js_batch_async(
                        &js_jobs,
                        &client_ref,
                        &src_headers_clone,
                    )
                    .await;
                    (out, js_jobs)
                };
                for (job, url) in js_jobs.iter().zip(js_results.iter()) {
                    if let Some(url) = url.as_deref().filter(|u| !u.trim().is_empty()) {
                        all_spec_failed = false;
                        // JS rules may emit Legado POST convention (`url,{...}`) —
                        // parse it just like the Rust-side builder does.
                        let mut spec = if let Some(s) =
                            crate::search_url::parse_post_request(url, &base_url)
                        {
                            s
                        } else {
                            crate::search_url::RequestSpec {
                                url: url.to_string(),
                                method: "GET".to_string(),
                                body: None,
                                headers: Vec::new(),
                                charset: None,
                                no_redirect: false,
                            }
                        };
                        spec.headers = src_headers.clone();
                        specs.push((job.keyword.clone(), spec));
                    }
                }
            }

            // ── Phase 2: fetch each spec and evaluate search results ──
            let mut content_failed: Option<String> = None;
            let mut http_reason: Option<&'static str> = None;
            for (keyword, spec) in &specs {
                // First attempt
                let result = client.fetch(spec).await;

                let result = match result {
                    Ok(r) => r,
                    Err(_) => {
                        // Retry once with jittered backoff to avoid synchronized
                        // retry storms across concurrent tasks
                        tokio::time::sleep(jittered_delay(1500, 1500)).await;
                        match client.fetch(spec).await {
                            Ok(r) => r,
                            Err(e2) => {
                                warn!(
                                    "{}: request failed after retry: {}",
                                    source.bookSourceName, e2
                                );
                                any_network_error = true;
                                continue;
                            }
                        }
                    }
                };

                // Non-2xx statuses often mean "the site blocks us" rather than
                // "no results" — record a distinct reason so the report is honest.
                if result.status_code >= 400 {
                    let label = crate::http_client::classify_status(result.status_code);
                    http_reason = Some(label);
                }

                // Determine content type for parsing.
                // Prefer the response Content-Type header; only fall back to URL
                // heuristics if the header is absent or generic (e.g., text/html
                // for JSON APIs that misreport).
                let content_type = if result.content_type.contains("json") {
                    "json".to_string()
                } else if spec.url.contains("json") && !result.content_type.contains("html") {
                    // URL contains "json" and the response isn't explicitly HTML
                    "json".to_string()
                } else {
                    "html".to_string()
                };

                // If response is empty, skip
                if result.body_text.trim().is_empty() {
                    continue;
                }

                // Apply ruleSearch to extract results. When the rule contains
                // JS (`<js>`/`@js:`/`java.`), transform the response body via the
                // JS polyfill first (many sources decrypt the payload), then
                // apply the remaining DSL rules on the transformed text.
                let mut results: Vec<Vec<String>> = Vec::new();
                if let Some(ref rule) = source.ruleSearch {
                    if crate::content_check::rule_has_js(rule) {
                        results =
                            crate::content_check::extract_results_js(&client, &source, &result, &base_url)
                                .await;
                    } else {
                        results = rule_dsl::extract_results(&result.body_text, &content_type, rule);
                    }
                }

                if rule_dsl::has_valid_results(&results) {
                    // Optional secondary check: walk the full reading chain
                    // (book info → toc → first chapter content) and confirm
                    // ruleContent yields real readable text.
                    if config.verify_content
                        && let Some(fail) =
                            verify_reading_chain(&client, &source, &results, &base_url).await
                    {
                        info!(
                            "{}: search ok but content unverified: {}",
                            source.bookSourceName, fail
                        );
                        content_failed = Some(fail);
                        continue;
                    }

                    info!(
                        "{}: PASSED (keyword: {}, {} items, {}ms)",
                        source.bookSourceName,
                        keyword,
                        results.len(),
                        result.elapsed_ms
                    );
                    if let Err(e) = cache.save(
                        &source.bookSourceUrl,
                        &source.bookSourceName,
                        "passed",
                        None,
                        1,
                    ) {
                        warn!("cache save failed for {}: {}", source.bookSourceName, e);
                    }
                    passed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return;
                }
            }

            // All keywords failed — classify the reason
            let (fail_reason, retry_count) = if all_spec_failed {
                ("builder_error".to_string(), 0u32)
            } else if let Some(stage) = content_failed {
                (stage, 1)
            } else if any_network_error {
                ("network_error".to_string(), 1)
            } else if let Some(reason) = http_reason {
                (reason.to_string(), 1)
            } else {
                ("no_results".to_string(), 1)
            };
            info!("{}: FAILED (reason={})", source.bookSourceName, fail_reason);
            if let Err(e) = cache.save(
                &source.bookSourceUrl,
                &source.bookSourceName,
                "failed",
                Some(&fail_reason),
                retry_count,
            ) {
                warn!("cache save failed for {}: {}", source.bookSourceName, e);
            }
            failed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }));
    }

    // Wait for all tasks. If a task panicked, count it as failed
    // (the source is skipped, and IncOnDrop already incremented completed).
    for handle in handles {
        if handle.await.is_err() {
            failed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
    progress_handle.await.unwrap_or(());

    let passed_count = passed.load(std::sync::atomic::Ordering::Relaxed);
    let failed_count = failed.load(std::sync::atomic::Ordering::Relaxed);
    let cached_count = cached.load(std::sync::atomic::Ordering::Relaxed);
    let js_api_sources = std::sync::Mutex::lock(&*js_api)
        .map(|g| g.clone())
        .unwrap_or_else(|e| e.into_inner().clone());

    Ok(TestSummary {
        total: passed_count + failed_count + cached_count + js_api_sources.len(),
        passed: passed_count,
        failed: failed_count,
        cached: cached_count,
        js_api: js_api_sources,
    })
}

fn select_keywords(source: &BookSource, config: &TestConfig) -> Vec<String> {
    let mut keywords = Vec::new();
    let mut tried = std::collections::HashSet::new();

    // Priority 1: checkKeyWord from ruleSearch
    if let Some(ref rs) = source.ruleSearch
        && let Some(ref kw) = rs.checkKeyWord
    {
        let trimmed = kw.trim();
        if !trimmed.is_empty() && tried.insert(trimmed.to_string()) {
            keywords.push(trimmed.to_string());
            if keywords.len() >= 3 {
                return keywords;
            }
        }
    }

    // Priority 2: domain-matched test books
    let url = source.bookSourceUrl.to_lowercase();
    for book in &config.test_books {
        if let Some(ref hint) = book.domain_hint
            && url.contains(&hint.to_lowercase())
            && tried.insert(book.name.clone())
        {
            keywords.push(book.name.clone());
        }
    }

    // Fill up to 3 with generic keywords and test books
    if keywords.len() < 3 {
        for kw in &config.generic_keywords {
            if tried.insert(kw.clone()) {
                keywords.push(kw.clone());
                if keywords.len() >= 3 {
                    return keywords;
                }
            }
        }
    }

    if keywords.len() < 3 {
        for book in &config.test_books {
            if tried.insert(book.name.clone()) {
                keywords.push(book.name.clone());
                if keywords.len() >= 3 {
                    break;
                }
            }
        }
    }

    keywords
}

/// Secondary verification for `--verify-content`: walk the full reading chain
/// (book detail → TOC → first chapter) and confirm ruleContent extracts
/// readable text. Returns `Some(stage)` on failure, `None` on pass or when
/// there is no book URL to follow (cannot verify → treat as OK).
async fn verify_reading_chain(
    client: &HttpClient,
    source: &BookSource,
    results: &[Vec<String>],
    base_url: &str,
) -> Option<String> {
    let book_url = results
        .first()
        .and_then(|row| row.get(2))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let Some(book_url) = book_url else {
        return None; // no book URL to follow → cannot verify → treat as OK
    };
    let book_url = crate::search_url::resolve_url(&book_url, base_url);

    let verdict = crate::content_check::verify_chain(client, source, &book_url, base_url).await;
    if verdict.ok {
        info!(
            "{}: chain OK ({} chars)",
            source.bookSourceName, verdict.content_len
        );
        None
    } else {
        info!(
            "{}: chain failed at {} ({})",
            source.bookSourceName, verdict.stage, verdict.detail
        );
        Some(verdict.stage.to_string())
    }
}
