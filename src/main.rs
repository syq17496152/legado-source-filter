use clap::Parser;
use legado_source_filter::cli::{json_event, Cli, Commands};
use legado_source_filter::config::build_test_config;
use legado_source_filter::fetcher::{clear_output_cache, ensure_fresh_source};
use legado_source_filter::paths::{resolve_input, resolve_output};
use legado_source_filter::pipeline::{remove_stale_outputs, run_test_campaign};
use legado_source_filter::{db, preflight, reporter};
use tracing_subscriber::EnvFilter;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if !cli.json {
        tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env()
                .add_directive("html5ever=off".parse().unwrap())
                .add_directive(tracing::Level::ERROR.into()))
            .init();
    }

    let input_path = resolve_input();
    let output_path = cli.output.clone().unwrap_or_else(resolve_output);

    match &cli.command {
        Commands::Status => {
            let db_path = output_path.join("test_cache.db");

            if let Ok(cache) = db::TestCache::new(&db_path) {
                let n_total: usize = cache.load_meta("eligible_count").ok().flatten()
                    .and_then(|s| s.trim().parse().ok())
                    .or_else(|| {
                        cache.load_meta("eligible").ok().flatten()
                            .and_then(|s| serde_json::from_str::<Vec<legado_source_filter::types::BookSource>>(&s).ok())
                            .map(|v| v.len())
                    })
                    .unwrap_or(0);

                let rows = cache.summary().unwrap_or_default();
                let tested: usize = rows.iter().map(|(_, c)| c).sum();
                let mut counts = std::collections::BTreeMap::new();
                for (s, c) in &rows {
                    counts.insert(s.clone(), *c);
                }

                json_event(&cli, serde_json::json!({
                    "event": "status",
                    "preflight": if n_total > 0 { "done" } else { "not_run" },
                    "total": n_total,
                    "tested": tested,
                    "remaining": n_total.saturating_sub(tested),
                    "details": counts,
                }));

                if !cli.json {
                    println!("=== Pipeline Status ===");
                    if n_total > 0 {
                        println!("Preflight: done ({} eligible)", n_total);
                    } else {
                        println!("Preflight: not run yet — run `full` first");
                    }
                    if tested > 0 {
                        println!("Tested:    {} {}", tested,
                            if n_total > 0 { format!("/ {}", n_total) } else { String::new() });
                        for (status, count) in &rows {
                            println!("  {:<16} {}", status, count);
                        }
                    }
                }
            } else {
                json_event(&cli, serde_json::json!({
                    "event": "status",
                    "preflight": "not_run",
                    "total": 0,
                }));

                if !cli.json {
                    println!("=== Pipeline Status ===");
                    println!("Preflight: not run yet — run `full` first");
                }
            }
        }

        Commands::Full {
            concurrency, timeout, force, limit, config: config_path, rounds, verify_content,
        } => {
            json_event(&cli, serde_json::json!({"event": "phase", "phase": "full", "status": "started"}));

            // 默认断点续测：保留旧缓存，已测源自动跳过。
            // 仅 --force 时清除旧测试缓存和衍生输出，从头重测。
            if *force {
                clear_output_cache(&output_path);
            }
            // 清理旧版本遗留、当前不再产出的输出文件
            remove_stale_outputs(&output_path);

            // ── Phase 1: Preflight ──
            json_event(&cli, serde_json::json!({"event": "phase", "phase": "preflight", "status": "started"}));
            ensure_fresh_source(&input_path, &output_path)?;
            let file = std::fs::File::open(&input_path)?;
            let reader = std::io::BufReader::new(file);
            let sources: Vec<legado_source_filter::types::BookSource> = serde_json::from_reader(reader)?;
            let preflight_out = preflight::run(sources);
            let db_path = output_path.join("test_cache.db");
            reporter::write_outputs(&output_path, &db_path, &preflight_out)?;

            json_event(&cli, serde_json::json!({
                "event": "preflight_summary",
                "total_input": preflight_out.total_input,
                "eligible": preflight_out.eligible.len(),
                "skipped": preflight_out.skipped.len(),
                "explore_only": preflight_out.explore_only.len(),
            }));

            if !cli.json {
                println!("\n=== Preflight ===");
                println!("Eligible: {}", preflight_out.eligible.len());
            }

            if preflight_out.eligible.is_empty() {
                anyhow::bail!("No eligible sources after preflight");
            }

            // ── Phase 2: Build test config ──
            let test_config = build_test_config(
                *concurrency, *timeout, *force, *verify_content, config_path,
            )?;

            let eligible = if let Some(n) = limit {
                let n = (*n).min(preflight_out.eligible.len());
                preflight_out.eligible.into_iter().take(n).collect()
            } else {
                preflight_out.eligible
            };

            // ── Phase 3: Test campaign ──
            json_event(&cli, serde_json::json!({"event": "phase", "phase": "test", "status": "started"}));
            run_test_campaign(eligible, test_config, &output_path, cli.json, *rounds)?;
        }
    }

    Ok(())
}
