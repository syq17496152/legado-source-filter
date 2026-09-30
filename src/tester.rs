use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tracing::{info, warn};

use crate::db::TestCache;
use crate::http_client::HttpClient;
use crate::rule_dsl;
use crate::search_url;
use crate::search_url::RequestSpec;
use crate::status::{FailReason, TestStatus};
use crate::types::*;
use crate::url_fixer;

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
    /// Rare keywords used for the static-list probe: after a chain verifies,
    /// one more search with a different keyword runs; if the results overlap
    /// the primary keyword's almost completely, the source's search is likely
    /// hardcoded (always returns the same page). The source stays `passed` but
    /// is flagged in evidence and written to static.json for review.
    pub static_probe_keywords: Vec<String>,
    /// Re-test `passed` sources whose last test is older than this many days
    /// (0 = never re-test, i.e. cache is trusted forever). Failed sources are
    /// always re-tested regardless.
    pub cache_ttl_days: u64,
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
    pub unverified: usize,
    pub js_api: Vec<BookSource>,
}

pub async fn run(
    eligible: Vec<BookSource>,
    config: TestConfig,
    cache: Arc<TestCache>,
    quiet: bool,
) -> anyhow::Result<TestSummary> {
    // Honor the per-source `concurrentRate` declarations: build a per-domain
    // min-interval table (the most conservative declared rate wins per domain)
    // and attach it to the shared client so every request to that site paces.
    let rate_limits = build_rate_limits(&eligible);
    let client = Arc::new(HttpClient::new(config.timeout_secs)?.with_rate_limits(rate_limits));
    let config = Arc::new(config);
    let semaphore = Arc::new(Semaphore::new(config.concurrency));
    let mut handles = Vec::new();
    let passed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let failed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let cached = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let unverified = Arc::new(std::sync::atomic::AtomicUsize::new(0));
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
        let unverified = unverified.clone();
        let js_api = js_api.clone();
        let completed = completed.clone();
        let config = Arc::clone(&config);

        handles.push(tokio::spawn(async move {
            let _permit = semaphore.acquire().await.unwrap();
            let _inc_on_drop = IncOnDrop(completed.clone());

            // Check cache first (skipped entirely under --force)
            let cache_hit = if config.force {
                None
            } else {
                cache
                    .check_full(&source.bookSourceUrl, &source.bookSourceName)
                    .ok()
                    .flatten()
            };
            if let Some((status, _, tested_at, _, _)) = cache_hit
                && let Some(st) = TestStatus::from_str(&status)
            {
                // A `passed` verdict older than the TTL is re-tested — the
                // source may have died since.
                let stale = config.cache_ttl_days > 0
                    && st == TestStatus::Passed
                    && tested_at > 0
                    && {
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap()
                            .as_secs() as i64;
                        now.saturating_sub(tested_at) > (config.cache_ttl_days * 86400) as i64
                    };
                if !stale {
                    match st {
                        TestStatus::Passed => {
                            passed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            return;
                        }
                        // Terminal statuses: the site is dead, needs a browser,
                        // or produced no verifiable book URL — re-testing won't
                        // help.
                        TestStatus::DeadDomain
                        | TestStatus::JsApi
                        | TestStatus::Unverified => {
                            cached.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            return;
                        }
                        TestStatus::Failed | TestStatus::Skipped => {
                            // Re-test failed sources; skipped is not expected.
                        }
                    }
                }
            }

            // Check startBrowserAwait — unrecoverable
            let su = source.searchUrl.as_deref().unwrap_or("");
            let is_startbrowser = su.contains("java.startBrowserAwait");
            let needs_js = (su.starts_with("@js:") || su.contains("<js>")) && !is_startbrowser;

            if is_startbrowser {
                if let Err(e) = cache.save_with_evidence(
                    &source.bookSourceUrl,
                    &source.bookSourceName,
                    TestStatus::JsApi.as_str(),
                    None,
                    0,
                    None,
                ) {
                    warn!("cache save failed for {}: {}", source.bookSourceName, e);
                }
                let _ = cache.history_append(
                    &source.bookSourceUrl,
                    &source.bookSourceName,
                    TestStatus::JsApi.as_str(),
                    None,
                    None,
                );
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
            let mut content_failed: Option<FailReason> = None;
            let mut http_reason: Option<&'static str> = None;
            let mut last_fetch_err: Option<crate::http_client::FetchError> = None;
            let mut saw_unverifiable = false;

            // Evidence about this attempt, recorded into the cache on every
            // terminal verdict (foundation for stability/regression analysis).
            let mut keywords_tried = 0usize;
            let mut chains_ok = 0usize;
            let mut verified_keyword: Option<String> = None;
            let mut verified_content_len = 0usize;
            let mut verified_book_name: Option<String> = None;
            let mut static_probe: Option<bool> = None;

            for (keyword, spec) in &specs {
                // First attempt
                let result = client.fetch(spec).await;

                let result = match result {
                    Ok(r) => r,
                    Err(_e) => {
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
                                // Keep the final error so the reason column can
                                // distinguish a 5xx (server_error) from a
                                // transport failure (network_error).
                                last_fetch_err = Some(e2);
                                continue;
                            }
                        }
                    }
                };
                keywords_tried += 1;

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
                    if config.verify_content {
                        match verify_reading_chain(&client, &source, &results, &base_url).await {
                            ChainOutcome::Verified { content_len } => {
                                verified_content_len = content_len;
                            }
                            ChainOutcome::Unverifiable => {
                                info!(
                                    "{}: search ok but no bookUrl — chain unverifiable",
                                    source.bookSourceName
                                );
                                saw_unverifiable = true;
                                continue;
                            }
                            ChainOutcome::Failed(reason) => {
                                info!(
                                    "{}: search ok but content unverified: {}",
                                    source.bookSourceName,
                                    reason.as_str()
                                );
                                content_failed = Some(reason);
                                continue;
                            }
                        }
                    }

                    chains_ok += 1;
                    verified_keyword = Some(keyword.clone());
                    verified_book_name = results
                        .first()
                        .and_then(|row| row.first())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());

                    // Static-list probe: a second search with a deliberately
                    // rare keyword. If the site returns nearly the same results
                    // for an unrelated query, its search is likely hardcoded.
                    if config.verify_content && static_probe.is_none() {
                        let probe = config
                            .static_probe_keywords
                            .iter()
                            .find(|k| **k != *keyword).cloned();
                        if let Some(probe_kw) = probe
                            && !needs_js
                            && let Some(probe_results) = run_static_probe(
                                &client, &source, &probe_kw, &src_headers,
                            )
                            .await
                        {
                            let overlap = result_overlap(&results, &probe_results);
                            let is_static = overlap >= 0.9;
                            static_probe = Some(is_static);
                            if is_static {
                                info!(
                                    "{}: static-list probe hit (overlap {:.0}% with keyword '{}')",
                                    source.bookSourceName,
                                    overlap * 100.0,
                                    probe_kw
                                );
                            }
                        }
                    }

                    info!(
                        "{}: PASSED (keyword: {}, {} items, {}ms)",
                        source.bookSourceName,
                        keyword,
                        results.len(),
                        result.elapsed_ms
                    );
                    let evidence = serde_json::json!({
                        "keywords_tried": keywords_tried,
                        "chains_ok": chains_ok,
                        "verified_keyword": verified_keyword,
                        "result_count": results.len(),
                        "book_name": verified_book_name,
                        "content_len": verified_content_len,
                        "static_probe": static_probe,
                    });
                    if let Err(e) = cache.save_with_evidence(
                        &source.bookSourceUrl,
                        &source.bookSourceName,
                        TestStatus::Passed.as_str(),
                        None,
                        1,
                        Some(&evidence.to_string()),
                    ) {
                        warn!("cache save failed for {}: {}", source.bookSourceName, e);
                    }
                    let _ = cache.history_append(
                        &source.bookSourceUrl,
                        &source.bookSourceName,
                        TestStatus::Passed.as_str(),
                        None,
                        Some(&evidence.to_string()),
                    );
                    passed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return;
                }
            }

            // All keywords failed — classify the verdict. Priority: structural
            // request-build failures first, then chain failures, then "we could
            // search but never got a book URL" (unverifiable), then transport
            // errors, then HTTP-status blocks, then plain no-results.
            let (status, fail_reason, retry_count) = if all_spec_failed {
                (TestStatus::Failed, Some(FailReason::BuilderError), 0u32)
            } else if let Some(reason) = content_failed {
                (TestStatus::Failed, Some(reason), 1)
            } else if saw_unverifiable {
                (TestStatus::Unverified, Some(FailReason::NoBookUrl), 0)
            } else if let Some(err) = last_fetch_err {
                (TestStatus::Failed, Some(err.fail_reason()), 1)
            } else if let Some(reason) = http_reason {
                (TestStatus::Failed, FailReason::from_str(reason), 1)
            } else {
                (TestStatus::Failed, Some(FailReason::NoResults), 1)
            };
            let reason_str = fail_reason.map(|r| r.as_str());
            info!(
                "{}: {} (reason={})",
                source.bookSourceName,
                status.as_str(),
                reason_str.unwrap_or("none")
            );
            let evidence = serde_json::json!({
                "keywords_tried": keywords_tried,
                "chains_ok": chains_ok,
                "verified_keyword": verified_keyword,
                "book_name": verified_book_name,
                "content_len": verified_content_len,
                "static_probe": static_probe,
            });
            if let Err(e) = cache.save_with_evidence(
                &source.bookSourceUrl,
                &source.bookSourceName,
                status.as_str(),
                reason_str,
                retry_count,
                Some(&evidence.to_string()),
            ) {
                warn!("cache save failed for {}: {}", source.bookSourceName, e);
            }
            let _ = cache.history_append(
                &source.bookSourceUrl,
                &source.bookSourceName,
                status.as_str(),
                reason_str,
                Some(&evidence.to_string()),
            );
            match status {
                TestStatus::Unverified => {
                    unverified.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                _ => {
                    failed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
            }
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
    let unverified_count = unverified.load(std::sync::atomic::Ordering::Relaxed);
    let js_api_sources = std::sync::Mutex::lock(&*js_api)
        .map(|g| g.clone())
        .unwrap_or_else(|e| e.into_inner().clone());

    Ok(TestSummary {
        total: passed_count
            + failed_count
            + cached_count
            + unverified_count
            + js_api_sources.len(),
        passed: passed_count,
        failed: failed_count,
        cached: cached_count,
        unverified: unverified_count,
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

    // Priority 2: domain-matched test books. The hint matches the normalized
    // domain, so `qidian.com` covers `www.qidian.com` and `m.qidian.com`.
    let domain = url_fixer::normalized_domain(&source.bookSourceUrl);
    for book in &config.test_books {
        if let Some(ref hint) = book.domain_hint
            && domain
                .as_deref()
                .is_some_and(|d| url_fixer::domain_matches(d, hint))
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

/// Outcome of the optional reading-chain verification after a successful search.
enum ChainOutcome {
    /// Chain verified (or verification disabled) — source may be marked passed.
    /// Carries the extracted content length as evidence.
    Verified { content_len: usize },
    /// Search produced results but no book URL, so the chain cannot be walked.
    Unverifiable,
    /// Chain was walked but failed at some stage.
    Failed(FailReason),
}

/// Collect the per-domain minimum inter-request interval declared by the
/// sources' `concurrentRate` fields. Same domain from multiple sources → the
/// most conservative (largest interval) wins.
fn build_rate_limits(sources: &[BookSource]) -> std::collections::HashMap<String, std::time::Duration> {
    let mut limits: std::collections::HashMap<String, std::time::Duration> =
        std::collections::HashMap::new();
    for s in sources {
        let Some(d) = crate::types::parse_concurrent_rate(s.concurrentRate.as_ref()) else {
            continue;
        };
        let Some(domain) = url_fixer::normalized_domain(&s.bookSourceUrl) else {
            continue;
        };
        limits
            .entry(domain)
            .and_modify(|e| {
                if d > *e {
                    *e = d;
                }
            })
            .or_insert(d);
    }
    limits
}

/// Secondary verification for `--verify-content`: walk the full reading chain
/// (book detail → TOC → first chapter) and confirm ruleContent extracts
/// readable text.
async fn verify_reading_chain(
    client: &HttpClient,
    source: &BookSource,
    results: &[Vec<String>],
    base_url: &str,
) -> ChainOutcome {
    let book_url = results
        .first()
        .and_then(|row| row.get(2))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let expected_name = results
        .first()
        .and_then(|row| row.first())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let Some(book_url) = book_url else {
        // No book URL to follow → the chain cannot be verified at all. This is
        // NOT a pass: a source whose search never yields a book URL is useless
        // to a reader, so we report it as unverifiable instead of silently
        // granting `passed`.
        return ChainOutcome::Unverifiable;
    };
    let book_url = crate::search_url::resolve_url(&book_url, base_url);

    let verdict = crate::content_check::verify_chain(
        client,
        source,
        &book_url,
        base_url,
        expected_name.as_deref(),
    )
    .await;
    if verdict.ok {
        info!(
            "{}: chain OK ({} chars)",
            source.bookSourceName, verdict.content_len
        );
        ChainOutcome::Verified {
            content_len: verdict.content_len,
        }
    } else {
        info!(
            "{}: chain failed at {} ({})",
            source.bookSourceName, verdict.stage, verdict.detail
        );
        // Chain-stage strings were chosen to match the cache reason vocabulary;
        // anything unknown falls back to a generic content failure.
        let reason = FailReason::from_str(verdict.stage).unwrap_or(FailReason::ContentEmpty);
        ChainOutcome::Failed(reason)
    }
}

/// Jaccard overlap of the book-URL sets of two search result sets, in [0, 1].
/// 1.0 means both searches returned exactly the same books (static-list smell).
fn result_overlap(a: &[Vec<String>], b: &[Vec<String>]) -> f64 {
    fn url_set(rows: &[Vec<String>]) -> std::collections::HashSet<&str> {
        rows.iter()
            .filter_map(|r| r.get(2))
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect()
    }
    let sa = url_set(a);
    let sb = url_set(b);
    if sa.is_empty() || sb.is_empty() {
        return 0.0;
    }
    let inter = sa.intersection(&sb).count();
    let union = sa.len() + sb.len() - inter;
    if union == 0 {
        0.0
    } else {
        inter as f64 / union as f64
    }
}

/// Run one extra search with a probe keyword and extract its results.
/// Returns `None` when the request fails, the source can't build a Rust-side
/// request (JS sources are skipped — the probe is cheap and structural), or
/// the response is unusable.
async fn run_static_probe(
    client: &HttpClient,
    source: &BookSource,
    probe_kw: &str,
    src_headers: &[(String, String)],
) -> Option<Vec<Vec<String>>> {
    let mut spec = search_url::build_request(source, probe_kw)?;
    spec.headers = src_headers.to_vec();
    let result = client.fetch(&spec).await.ok()?;
    if result.status_code >= 400 || result.body_text.trim().is_empty() {
        return None;
    }
    let content_type = if result.content_type.contains("json") {
        "json".to_string()
    } else {
        "html".to_string()
    };
    let rule = source.ruleSearch.as_ref()?;
    Some(rule_dsl::extract_results(
        &result.body_text,
        &content_type,
        rule,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_result_overlap() {
        let rows = |urls: &[&str]| -> Vec<Vec<String>> {
            urls.iter()
                .map(|u| vec!["名".to_string(), "作者".to_string(), u.to_string()])
                .collect()
        };
        let a = rows(&["/b/1", "/b/2", "/b/3"]);
        let b = rows(&["/b/1", "/b/2", "/b/3"]);
        assert!((result_overlap(&a, &b) - 1.0).abs() < 1e-9, "identical → 1.0");

        let c = rows(&["/b/1", "/b/4"]);
        let o = result_overlap(&a, &c);
        assert!(o < 1.0 && o > 0.0, "partial overlap, got {o}");

        let d = rows(&["/x/9"]);
        assert_eq!(result_overlap(&a, &d), 0.0, "disjoint → 0.0");

        let empty: Vec<Vec<String>> = Vec::new();
        assert_eq!(result_overlap(&a, &empty), 0.0, "empty probe → 0.0");

        // Rows without a book URL never count toward overlap.
        let no_url = vec![vec!["名".to_string(), "作者".to_string(), String::new()]];
        assert_eq!(result_overlap(&a, &no_url), 0.0);
    }

    /// Mock search server: `/s?kw=<keyword>` — `static` mode returns the same
    /// book list for every keyword (hardcoded search); `dynamic` mode returns
    /// a different list for the rare probe keyword.
    async fn mock_search_server(static_mode: bool) -> std::net::SocketAddr {
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
                    let kw = req
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("/")
                        .split("kw=")
                        .nth(1)
                        .unwrap_or("")
                        .to_string();
                    let body = if static_mode {
                        // Hardcoded search: identical list for every keyword.
                        "<div class='item'><a href='/b/1'>书一</a></div>\
                         <div class='item'><a href='/b/2'>书二</a></div>"
                            .to_string()
                    } else if kw.starts_with("zzzzz") {
                        // Dynamic site: a rare keyword returns nothing related.
                        "<div class='item'><a href='/b/9'>别的书</a></div>".to_string()
                    } else {
                        "<div class='item'><a href='/b/1'>书一</a></div>\
                         <div class='item'><a href='/b/2'>书二</a></div>"
                            .to_string()
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

    fn probe_source(base: &str) -> BookSource {
        BookSource {
            bookSourceName: "probe".into(),
            bookSourceUrl: base.to_string(),
            searchUrl: Some("/s?kw={{key}}".into()),
            ruleSearch: Some(SearchRule {
                bookList: Some("class.item".into()),
                name: Some("tag.a@text".into()),
                bookUrl: Some("tag.a@href".into()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_static_probe_hardcoded_search() {
        let addr = mock_search_server(true).await; // static: same list always
        let base = format!("http://{}", addr);
        let source = probe_source(&base);
        let client = HttpClient::new(10).unwrap();
        let headers = crate::search_url::source_headers(&source);

        // Primary keyword search.
        let spec = search_url::build_request(&source, "重生").unwrap();
        let primary = {
            let r = client.fetch(&spec).await.unwrap();
            rule_dsl::extract_results(&r.body_text, "html", source.ruleSearch.as_ref().unwrap())
        };
        assert_eq!(primary.len(), 2);

        // Probe with a rare keyword returns the same list → overlap 1.0.
        let probe = run_static_probe(&client, &source, "zzzzz", &headers)
            .await
            .unwrap();
        assert_eq!(probe.len(), 2, "static site ignores the keyword");
        assert!(
            (result_overlap(&primary, &probe) - 1.0).abs() < 1e-9,
            "hardcoded search → identical results, overlap must be 1.0"
        );
    }

    #[tokio::test]
    async fn test_static_probe_dynamic_search_low_overlap() {
        let addr = mock_search_server(false).await; // dynamic: probe differs
        let base = format!("http://{}", addr);
        let source = probe_source(&base);
        let client = HttpClient::new(10).unwrap();
        let headers = crate::search_url::source_headers(&source);

        let spec = search_url::build_request(&source, "重生").unwrap();
        let primary = {
            let r = client.fetch(&spec).await.unwrap();
            rule_dsl::extract_results(&r.body_text, "html", source.ruleSearch.as_ref().unwrap())
        };
        let probe = run_static_probe(&client, &source, "zzzzz", &headers)
            .await
            .unwrap();
        assert!(
            result_overlap(&primary, &probe) < 0.5,
            "dynamic site returns different results for a rare keyword"
        );
    }
}
