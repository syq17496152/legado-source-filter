use std::path::Path;
use std::sync::Arc;

use crate::{db, tester, types};

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
                "total_rounds": 0, "total": 0, "passed": 0, "failed": 0, "cached": 0, "js_api": 0,
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
            "js_api": summary.js_api.len(),
        });
        if json_output {
            println!("{}", serde_json::to_string(&event).unwrap());
        } else {
            println!("\n=== Round {}/{} Summary ===", round, rounds);
            println!("Total:  {}", summary.total);
            println!("Passed: {}", summary.passed);
            println!("Failed: {}", summary.failed);
            println!("Cached: {}", summary.cached);
            println!("JS API: {}", summary.js_api.len());
        }

        if round < rounds {
            mark_dead_domains(&eligible, &cache);
        }
    }

    // ── Final cumulative summary across all rounds ──
    {
        let final_passed = eligible
            .iter()
            .filter(|s| {
                cache
                    .check(&s.bookSourceUrl, &s.bookSourceName)
                    .ok()
                    .flatten()
                    .is_some_and(|(st, _)| st == "passed")
            })
            .count();
        let final_failed = eligible
            .iter()
            .filter(|s| {
                cache
                    .check(&s.bookSourceUrl, &s.bookSourceName)
                    .ok()
                    .flatten()
                    .is_some_and(|(st, _)| st == "failed")
            })
            .count();
        let final_js_api = eligible
            .iter()
            .filter(|s| {
                cache
                    .check(&s.bookSourceUrl, &s.bookSourceName)
                    .ok()
                    .flatten()
                    .is_some_and(|(st, _)| st == "js_api")
            })
            .count();
        let final_dead = eligible
            .iter()
            .filter(|s| {
                cache
                    .check(&s.bookSourceUrl, &s.bookSourceName)
                    .ok()
                    .flatten()
                    .is_some_and(|(st, _)| st == "dead_domain")
            })
            .count();
        let final_skipped = eligible
            .iter()
            .filter(|s| {
                cache
                    .check(&s.bookSourceUrl, &s.bookSourceName)
                    .ok()
                    .flatten()
                    .is_some_and(|(st, _)| st == "skipped")
            })
            .count();
        let untested = eligible.len()
            - final_passed
            - final_dead
            - final_failed
            - final_js_api
            - final_skipped;

        let final_event = serde_json::json!({
            "event": "final_summary",
            "rounds": rounds,
            "total": eligible.len(),
            "passed": final_passed,
            "dead_domain": final_dead,
            "failed": final_failed,
            "js_api": final_js_api,
            "skipped": final_skipped,
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
            "total": eligible.len(),
            "passed": final_passed,
            "dead_domain": final_dead,
            "failed": final_failed,
            "js_api": final_js_api,
            "skipped": final_skipped,
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
                final_passed as f64 / eligible.len() as f64 * 100.0
            );
            println!(
                "Dead domain: {} ({:.1}%)",
                final_dead,
                final_dead as f64 / eligible.len() as f64 * 100.0
            );
            println!(
                "Failed:      {} ({:.1}%)",
                final_failed,
                final_failed as f64 / eligible.len() as f64 * 100.0
            );
            println!("JS API:      {}", final_js_api);
            println!("Skipped:     {}", final_skipped);
            println!("Untested:    {}", untested);
        }
    }

    // ── Generate final output files from cache ──
    {
        let passed_sources: Vec<_> = eligible
            .iter()
            .filter(|s| {
                cache
                    .check(&s.bookSourceUrl, &s.bookSourceName)
                    .ok()
                    .flatten()
                    .is_some_and(|(st, _)| st == "passed")
            })
            .cloned()
            .collect();
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

/// Analyze cache after a round: if every tested source on a domain has "failed"
/// (none passed), mark its *deterministically* failed sources as "dead_domain"
/// so subsequent rounds skip them — retrying a dead site is wasted effort.
/// Transient failures (`network_error`, `*_fetch`, `rate_limited`, ...) are
/// left as `failed` so later rounds still retry them.
fn mark_dead_domains(eligible: &[types::BookSource], cache: &db::TestCache) {
    let mut domain_has_passed = std::collections::HashSet::new();

    // First pass: find domains with at least one passed source
    for src in eligible {
        let Ok(Some((status, _))) = cache.check(&src.bookSourceUrl, &src.bookSourceName) else {
            continue;
        };
        let domain = src
            .bookSourceUrl
            .split('/')
            .nth(2)
            .unwrap_or("")
            .to_lowercase();
        if domain.is_empty() {
            continue;
        }
        if status == "passed" {
            domain_has_passed.insert(domain);
        }
    }

    // Second pass: mark deterministically failed sources on dead domains
    for src in eligible {
        let domain = src
            .bookSourceUrl
            .split('/')
            .nth(2)
            .unwrap_or("")
            .to_lowercase();
        if domain.is_empty() || domain_has_passed.contains(&domain) {
            continue;
        }

        let Ok(Some((status, reason))) = cache.check(&src.bookSourceUrl, &src.bookSourceName)
        else {
            continue;
        };
        let is_deterministic = status == "failed"
            && reason.as_deref().is_some_and(|r| !is_transient_reason(r));
        if is_deterministic
            && let Err(e) = cache.save(
                &src.bookSourceUrl,
                &src.bookSourceName,
                "dead_domain",
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
///     `rate_limited`) that could succeed on a later attempt.
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
            status == "failed" && reason.as_deref().is_some_and(is_transient_reason)
        })
        .cloned()
        .collect()
}

fn is_transient_reason(reason: &str) -> bool {
    matches!(
        reason,
        "network_error"
            | "bookinfo_fetch"
            | "toc_fetch"
            | "chapter_fetch"
            | "rate_limited"
            | "server_error"
    )
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
}
