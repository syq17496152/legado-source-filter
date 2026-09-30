use std::path::Path;
use std::sync::Arc;

use crate::status::{FailReason, TestStatus};
use crate::{db, tester, types, url_fixer};

/// Run the full test campaign for an eligible set: N rounds of testing,
/// dead-domain marking between rounds, final cumulative summary, and
/// output file generation (filtered.json).
pub fn run_test_campaign(
    eligible: Vec<types::BookSource>,
    config: tester::TestConfig,
    output_path: &Path,
    json_output: bool,
    rounds: u32,
) -> anyhow::Result<()> {
    if eligible.is_empty() {
        if json_output {
            println!("{}", serde_json::to_string(&serde_json::json!({
                "event": "round_summary",
                "total_rounds": 0, "total": 0, "passed": 0, "failed": 0, "cached": 0, "unverified": 0, "js_api": 0,
                "note": "no eligible sources",
            })).unwrap());
        }
        return Ok(());
    }
    if config.concurrency < 1 {
        anyhow::bail!("concurrency must be at least 1");
    }
    let rt = tokio::runtime::Runtime::new()?;
    let rounds = rounds.max(1);

    // Open the cache once and share it across rounds, retry filtering,
    // dead-domain marking, and summary generation.
    let cache = Arc::new(db::TestCache::new(&output_path.join("test_cache.db"))?);

    for round in 1..=rounds {
        let round_config = config.clone();

        let sources_to_test = if round > 1 {
            filter_retryable(&eligible, &cache)
        } else {
            eligible.clone()
        };

        if sources_to_test.is_empty() {
            let event = serde_json::json!({
                "event": "round_summary",
                "round": round,
                "total_rounds": rounds,
                "total": 0, "passed": 0, "failed": 0, "js_api": 0,
                "note": "nothing to retry",
            });
            if json_output {
                println!("{}", serde_json::to_string(&event).unwrap());
            } else {
                println!("\n=== Round {}/{} === nothing to retry", round, rounds);
            }
            break;
        }

        if json_output {
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "event": "round_start",
                    "round": round,
                    "total_rounds": rounds,
                    "sources": sources_to_test.len(),
                }))
                .unwrap()
            );
        } else {
            tracing::info!(
                "Starting round {}/{}: sources={}, concurrency={}, timeout={}s",
                round,
                rounds,
                sources_to_test.len(),
                round_config.concurrency,
                round_config.timeout_secs
            );
        }

        let summary = rt.block_on(tester::run(
            sources_to_test,
            round_config,
            cache.clone(),
            json_output,
        ))?;

        let event = serde_json::json!({
            "event": "round_summary",
            "round": round,
            "total_rounds": rounds,
            "total": summary.total,
            "passed": summary.passed,
            "failed": summary.failed,
            "cached": summary.cached,
            "unverified": summary.unverified,
            "js_api": summary.js_api.len(),
        });
        if json_output {
            println!("{}", serde_json::to_string(&event).unwrap());
        } else {
            println!("\n=== Round {}/{} Summary ===", round, rounds);
            println!("Total:      {}", summary.total);
            println!("Passed:     {}", summary.passed);
            println!("Failed:     {}", summary.failed);
            println!("Cached:     {}", summary.cached);
            println!("Unverified: {}", summary.unverified);
            println!("JS API:     {}", summary.js_api.len());
        }

        if round < rounds {
            mark_dead_domains(&eligible, &cache);
        }
    }

    // ── Final cumulative summary across all rounds ──
    {
        // The source dataset contains many duplicate entries (same URL+name
        // collected multiple times), so counts and outputs are deduplicated by
        // (bookSourceUrl, bookSourceName).
        let count_status = |want: TestStatus| {
            let mut seen = std::collections::HashSet::new();
            eligible
                .iter()
                .filter(|s| {
                    seen.insert((s.bookSourceUrl.clone(), s.bookSourceName.clone()))
                        && cache
                            .check(&s.bookSourceUrl, &s.bookSourceName)
                            .ok()
                            .flatten()
                            .is_some_and(|(st, _)| TestStatus::from_str(&st) == Some(want))
                })
                .count()
        };
        let final_passed = count_status(TestStatus::Passed);
        let final_failed = count_status(TestStatus::Failed);
        let final_js_api = count_status(TestStatus::JsApi);
        let final_dead = count_status(TestStatus::DeadDomain);
        let final_skipped = count_status(TestStatus::Skipped);
        let final_unverified = count_status(TestStatus::Unverified);
        // Sources whose chain verified but whose search looks hardcoded
        // (static probe hit) — still usable, but flagged for review.
        // Counted on unique sources only (the dataset repeats entries).
        let final_static = {
            let mut seen = std::collections::HashSet::new();
            eligible
                .iter()
                .filter(|s| {
                    seen.insert((s.bookSourceUrl.clone(), s.bookSourceName.clone()))
                        && is_static_flag(&cache, &s.bookSourceUrl, &s.bookSourceName)
                })
                .count()
        };
        // Unique eligible sources (deduplicated — the dataset repeats entries).
        let unique_total = {
            let mut seen = std::collections::HashSet::new();
            eligible
                .iter()
                .filter(|s| seen.insert((s.bookSourceUrl.clone(), s.bookSourceName.clone())))
                .count()
        };
        let untested = unique_total
            - final_passed
            - final_dead
            - final_failed
            - final_js_api
            - final_skipped
            - final_unverified;

        let final_event = serde_json::json!({
            "event": "final_summary",
            "rounds": rounds,
            "total": unique_total,
            "total_entries": eligible.len(),
            "passed": final_passed,
            "dead_domain": final_dead,
            "failed": final_failed,
            "js_api": final_js_api,
            "skipped": final_skipped,
            "unverified": final_unverified,
            "static_list": final_static,
            "untested": untested,
        });
        if json_output {
            println!("{}", serde_json::to_string(&final_event).unwrap());
        }

        // Merge test summary into report in DB
        let mut report: serde_json::Value = cache
            .load_meta("report")
            .ok()
            .flatten()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or(serde_json::json!({}));
        report["test_summary"] = serde_json::json!({
            "rounds": rounds,
            "total": unique_total,
            "total_entries": eligible.len(),
            "passed": final_passed,
            "dead_domain": final_dead,
            "failed": final_failed,
            "js_api": final_js_api,
            "skipped": final_skipped,
            "unverified": final_unverified,
            "static_list": final_static,
            "untested": untested,
        });
        let _ = cache.save_meta(
            "report",
            &serde_json::to_string(&report).ok().unwrap_or_default(),
        );

        if !json_output {
            println!("\n=== Final Summary ({} rounds) ===", rounds);
            println!(
                "Passed:      {} ({:.1}%)",
                final_passed,
                final_passed as f64 / unique_total as f64 * 100.0
            );
            println!(
                "Dead domain: {} ({:.1}%)",
                final_dead,
                final_dead as f64 / unique_total as f64 * 100.0
            );
            println!(
                "Failed:      {} ({:.1}%)",
                final_failed,
                final_failed as f64 / unique_total as f64 * 100.0
            );
            println!("JS API:      {}", final_js_api);
            println!("Unverified:  {}", final_unverified);
            println!("Static list: {}", final_static);
            println!("Skipped:     {}", final_skipped);
            println!("Untested:    {}", untested);
        }
    }

    // ── Generate final output files from cache ──
    {
        // Deduplicate by (url, name): the dataset repeats many sources.
        let collect = |want: TestStatus| -> Vec<types::BookSource> {
            let mut seen = std::collections::HashSet::new();
            eligible
                .iter()
                .filter(|s| {
                    seen.insert((s.bookSourceUrl.clone(), s.bookSourceName.clone()))
                        && cache
                            .check(&s.bookSourceUrl, &s.bookSourceName)
                            .ok()
                            .flatten()
                            .is_some_and(|(st, _)| TestStatus::from_str(&st) == Some(want))
                })
                .cloned()
                .collect()
        };
        let passed_sources = collect(TestStatus::Passed);
        if !passed_sources.is_empty() {
            let filtered_path = output_path.join("filtered.json");
            std::fs::write(
                &filtered_path,
                serde_json::to_string_pretty(&passed_sources)?,
            )?;
            tracing::info!(
                "Wrote {} passed sources to filtered.json",
                passed_sources.len()
            );
        }
        // Sources whose search works but never yields a book URL: not usable
        // in filtered.json, but worth keeping for manual inspection.
        let unverified_sources = collect(TestStatus::Unverified);
        if !unverified_sources.is_empty() {
            let path = output_path.join("unverified.json");
            std::fs::write(&path, serde_json::to_string_pretty(&unverified_sources)?)?;
            tracing::info!(
                "Wrote {} unverified sources to unverified.json",
                unverified_sources.len()
            );
        }
        // Passed sources whose static probe hit: search is likely hardcoded.
        // They stay in filtered.json (the chain verifies), but this list lets
        // the user review them separately.
        let static_sources: Vec<_> = passed_sources
            .iter()
            .filter(|s| is_static_flag(&cache, &s.bookSourceUrl, &s.bookSourceName))
            .cloned()
            .collect();
        if !static_sources.is_empty() {
            let path = output_path.join("static.json");
            std::fs::write(&path, serde_json::to_string_pretty(&static_sources)?)?;
            tracing::info!(
                "Wrote {} static-list sources to static.json",
                static_sources.len()
            );
        }
    }

    Ok(())
}

/// Remove output files that older versions produced but the current pipeline
/// no longer writes (eligible/skipped/missed/report are now stored in the DB).
pub fn remove_stale_outputs(output_dir: &Path) {
    for entry in &[
        "eligible.json",
        "skipped.json",
        "missed.json",
        "report.json",
        "report.txt",
    ] {
        let path = output_dir.join(entry);
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Randomly sample `n` items without replacement using a xorshift PRNG seeded
/// from wall-clock time. Returns at most `items.len()` items; with `n >= len`
/// the whole slice is returned (order preserved).
pub fn sample_n<T: Clone>(items: &[T], n: usize) -> Vec<T> {
    let n = n.min(items.len());
    if n == 0 {
        return Vec::new();
    }
    if n == items.len() {
        return items.to_vec();
    }
    let mut seed: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let mut idx: Vec<usize> = (0..items.len()).collect();
    // Partial Fisher-Yates: shuffle only the first n positions.
    for i in 0..n {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let j = (seed as usize) % (items.len() - i);
        idx.swap(i, i + j);
    }
    idx[..n].iter().map(|&i| items[i].clone()).collect()
}

/// Analyze cache after a round: if every tested source on a domain has "failed"
/// (none passed), mark its *deterministically* failed sources as "dead_domain"
/// so subsequent rounds skip them — retrying a dead site is wasted effort.
/// Transient failures (`network_error`, `*_fetch`, `rate_limited`, ...) are
/// left as `failed` so later rounds still retry them.
///
/// Domains are compared through `url_fixer::normalized_domain`, so
/// `m.example.com` / `www.example.com` / `example.com:8080` count as one site.
fn mark_dead_domains(eligible: &[types::BookSource], cache: &db::TestCache) {
    let mut domain_has_passed = std::collections::HashSet::new();

    // First pass: find domains with at least one passed source
    for src in eligible {
        let Ok(Some((status, _))) = cache.check(&src.bookSourceUrl, &src.bookSourceName) else {
            continue;
        };
        let Some(domain) = url_fixer::normalized_domain(&src.bookSourceUrl) else {
            continue;
        };
        if TestStatus::from_str(&status) == Some(TestStatus::Passed) {
            domain_has_passed.insert(domain);
        }
    }

    // Second pass: mark deterministically failed sources on dead domains
    for src in eligible {
        let Some(domain) = url_fixer::normalized_domain(&src.bookSourceUrl) else {
            continue;
        };
        if domain_has_passed.contains(&domain) {
            continue;
        }

        let Ok(Some((status, reason))) = cache.check(&src.bookSourceUrl, &src.bookSourceName)
        else {
            continue;
        };
        // Unknown reasons are treated as deterministic (conservative), matching
        // the old string-based behaviour.
        let is_deterministic = TestStatus::from_str(&status) == Some(TestStatus::Failed)
            && reason
                .as_deref()
                .map(|r| FailReason::from_str(r).map(|fr| !fr.is_transient()).unwrap_or(true))
                .unwrap_or(false);
        if is_deterministic
            && let Err(e) = cache.save(
                &src.bookSourceUrl,
                &src.bookSourceName,
                TestStatus::DeadDomain.as_str(),
                reason.as_deref(),
                0,
            )
        {
            tracing::warn!(
                "cache save (dead_domain) failed for {}: {}",
                src.bookSourceName,
                e
            );
        }
    }
}

/// For rounds > 1, only include sources worth retrying:
///   - Not yet cached (untested)
///   - Cached as "failed" with a *transient* reason: network-level failures
///     (`network_error`, `bookinfo_fetch`, `toc_fetch`, `chapter_fetch`,
///     `rate_limited`, `server_error`) that could succeed on a later attempt.
///
/// Deterministic failures (`no_results`, `toc_parse`, `blocked`, `not_found`,
/// `builder_error`, `content_short`) are skipped.
fn filter_retryable(
    eligible: &[types::BookSource],
    cache: &db::TestCache,
) -> Vec<types::BookSource> {
    eligible
        .iter()
        .filter(|src| {
            let Ok(Some((status, reason))) = cache.check(&src.bookSourceUrl, &src.bookSourceName)
            else {
                return true; // Not in cache yet → include
            };
            TestStatus::from_str(&status) == Some(TestStatus::Failed)
                && reason
                    .as_deref()
                    .and_then(FailReason::from_str)
                    .is_some_and(|r| r.is_transient())
        })
        .cloned()
        .collect()
}

/// Whether a source's evidence marks it as a static-list hit (search returns
/// the same hardcoded results for every keyword). Only `passed` sources with
/// `"static_probe": true` in their evidence qualify.
fn is_static_flag(cache: &db::TestCache, url: &str, name: &str) -> bool {
    let Ok(Some((status, _))) = cache.check(url, name) else {
        return false;
    };
    if TestStatus::from_str(&status) != Some(TestStatus::Passed) {
        return false;
    }
    cache
        .evidence_of(url, name)
        .ok()
        .flatten()
        .and_then(|e| serde_json::from_str::<serde_json::Value>(&e).ok())
        .and_then(|v| v.get("static_probe").and_then(|b| b.as_bool()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cache_with(db_name: &str, rows: &[(&str, &str, &str, &str)]) -> db::TestCache {
        let dir = std::env::temp_dir().join(format!("lsf-pipe-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(db_name);
        let _ = std::fs::remove_file(&path);
        let cache = db::TestCache::new(&path).unwrap();
        for (url, name, status, reason) in rows {
            cache.save(url, name, status, Some(reason), 1).unwrap();
        }
        cache
    }

    fn src(url: &str, name: &str) -> types::BookSource {
        types::BookSource {
            bookSourceUrl: url.to_string(),
            bookSourceName: name.to_string(),
            bookSourceType: 0,
            enabled: true,
            ..Default::default()
        }
    }

    #[test]
    fn test_filter_retryable_only_network_errors_and_untested() {
        let cache = cache_with(
            "retry.db",
            &[
                ("https://a.com", "A", "failed", "network_error"),
                ("https://b.com", "B", "failed", "no_results"),
                ("https://c.com", "C", "passed", ""),
                ("https://d.com", "D", "dead_domain", "no_results"),
                ("https://f.com", "F", "failed", "toc_fetch"),
            ],
        );
        let eligible = vec![
            src("https://a.com", "A"),
            src("https://b.com", "B"),
            src("https://c.com", "C"),
            src("https://d.com", "D"),
            src("https://e.com", "E"), // not cached
            src("https://f.com", "F"),
        ];
        let retryable = filter_retryable(&eligible, &cache);
        let names: Vec<&str> = retryable
            .iter()
            .map(|s| s.bookSourceName.as_str())
            .collect();
        assert_eq!(
            names,
            vec!["A", "E", "F"],
            "transient network failures + untested are retried"
        );
    }

    #[test]
    fn test_mark_dead_domains() {
        let cache = cache_with(
            "dead.db",
            &[
                ("https://dead.com", "D1", "failed", "no_results"),
                ("https://dead.com", "D2", "failed", "network_error"),
                ("https://alive.com", "A1", "passed", ""),
                ("https://alive.com", "A2", "failed", "no_results"),
            ],
        );
        let eligible = vec![
            src("https://dead.com", "D1"),
            src("https://dead.com", "D2"),
            src("https://alive.com", "A1"),
            src("https://alive.com", "A2"),
        ];
        mark_dead_domains(&eligible, &cache);

        let (st1, _) = cache.check("https://dead.com", "D1").unwrap().unwrap();
        let (st2, _) = cache.check("https://dead.com", "D2").unwrap().unwrap();
        let (st_a2, _) = cache.check("https://alive.com", "A2").unwrap().unwrap();
        assert_eq!(st1, "dead_domain", "deterministic failure on dead domain");
        assert_eq!(
            st2, "failed",
            "transient failure stays failed so later rounds retry it"
        );
        assert_eq!(
            st_a2, "failed",
            "domain with a passed source stays failed, not dead"
        );
    }

    #[test]
    fn test_sample_n() {
        let items: Vec<u32> = (0..100).collect();

        let empty = sample_n(&items, 0);
        assert!(empty.is_empty());

        let all = sample_n(&items, 100);
        assert_eq!(all.len(), 100);

        let over = sample_n(&items, 500);
        assert_eq!(over.len(), 100, "capped at len");

        let s = sample_n(&items, 10);
        assert_eq!(s.len(), 10);
        // No duplicates, all in range.
        let mut seen = std::collections::HashSet::new();
        for v in &s {
            assert!(*v < 100);
            assert!(seen.insert(*v), "duplicate element in sample");
        }

        // Two samples differ in all likelihood (PRNG seeded from time) — this
        // can only flake if the two draws land in the same nanosecond AND the
        // shuffle produces identical output, which is astronomically unlikely.
        let s2 = sample_n(&items, 10);
        assert_ne!(s, s2, "samples should differ across draws");
    }

    #[test]
    fn test_is_static_flag() {
        let cache = cache_with(
            "static-flag.db",
            &[
                ("https://a.com", "A", "passed", ""),
                ("https://b.com", "B", "passed", ""),
                ("https://c.com", "C", "failed", "no_results"),
                ("https://d.com", "D", "passed", ""),
            ],
        );
        // Only A gets the static_probe evidence flag.
        cache
            .save_with_evidence(
                "https://a.com",
                "A",
                "passed",
                None,
                1,
                Some(r#"{"static_probe":true,"chains_ok":1}"#),
            )
            .unwrap();
        cache
            .save_with_evidence(
                "https://b.com",
                "B",
                "passed",
                None,
                1,
                Some(r#"{"static_probe":false,"chains_ok":1}"#),
            )
            .unwrap();
        cache
            .save_with_evidence(
                "https://d.com",
                "D",
                "passed",
                None,
                1,
                Some(r#"{"chains_ok":1}"#),
            )
            .unwrap();

        assert!(is_static_flag(&cache, "https://a.com", "A"), "flag=true");
        assert!(!is_static_flag(&cache, "https://b.com", "B"), "flag=false");
        assert!(!is_static_flag(&cache, "https://c.com", "C"), "not passed");
        assert!(!is_static_flag(&cache, "https://d.com", "D"), "no flag field");
        assert!(!is_static_flag(&cache, "https://missing.com", "X"), "no row");
    }

    #[test]
    fn test_mark_dead_domains_normalizes_www() {
        // `www.dead.com` and `dead.com` must count as the same site after
        // normalization, so a pass on one protects the other and a dead site
        // is detected across both spellings.
        let cache = cache_with(
            "dead-www.db",
            &[
                ("https://www.dead.com", "D1", "failed", "no_results"),
                ("https://dead.com", "D2", "failed", "no_results"),
                ("https://www.alive.com", "A1", "passed", ""),
                ("https://alive.com", "A2", "failed", "no_results"),
            ],
        );
        let eligible = vec![
            src("https://www.dead.com", "D1"),
            src("https://dead.com", "D2"),
            src("https://www.alive.com", "A1"),
            src("https://alive.com", "A2"),
        ];
        mark_dead_domains(&eligible, &cache);

        let (st1, _) = cache.check("https://www.dead.com", "D1").unwrap().unwrap();
        let (st2, _) = cache.check("https://dead.com", "D2").unwrap().unwrap();
        assert_eq!(st1, "dead_domain", "www.dead.com marked dead");
        assert_eq!(st2, "dead_domain", "bare dead.com marked dead too");
        let (st_a2, _) = cache.check("https://alive.com", "A2").unwrap().unwrap();
        assert_eq!(st_a2, "failed", "alive.com protected by www.alive.com pass");
    }

    #[test]
    fn test_mark_dead_domains_subdomain_distinct() {
        // `m.dead.com` is a different site from `dead.com`: a pass on the
        // desktop site must NOT protect the mobile subdomain (and vice versa).
        let cache = cache_with(
            "dead-sub.db",
            &[
                ("https://m.dead.com", "M1", "failed", "no_results"),
                ("https://dead.com", "D1", "passed", ""),
            ],
        );
        let eligible = vec![
            src("https://m.dead.com", "M1"),
            src("https://dead.com", "D1"),
        ];
        mark_dead_domains(&eligible, &cache);

        let (st_m1, _) = cache.check("https://m.dead.com", "M1").unwrap().unwrap();
        assert_eq!(
            st_m1, "dead_domain",
            "m. subdomain is a separate site and may be marked dead"
        );
    }
}
