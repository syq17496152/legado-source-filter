use std::path::PathBuf;

use crate::tester;

const DEFAULT_CONFIG: &str = include_str!("../data/config.toml");

/// Load config.toml: from explicit path, default `data/config.toml`, or embedded default.
pub fn load_config(config: &Option<PathBuf>) -> anyhow::Result<toml::Value> {
    let content = if let Some(path) = config {
        std::fs::read_to_string(path)?
    } else {
        let default_path = PathBuf::from("data/config.toml");
        if default_path.exists() {
            std::fs::read_to_string(&default_path)?
        } else {
            DEFAULT_CONFIG.to_string()
        }
    };
    Ok(content.parse()?)
}

/// Build a TestConfig from config.toml.
pub fn build_test_config(
    concurrency: usize,
    timeout_secs: u64,
    force: bool,
    verify_content: bool,
    config_path: &Option<PathBuf>,
) -> anyhow::Result<tester::TestConfig> {
    let config_toml = load_config(config_path)?;

    let mut test_books = Vec::new();
    if let Some(searches) = config_toml.get("search").and_then(|v| v.as_array()) {
        for entry in searches {
            let name = entry
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if !name.is_empty() {
                test_books.push(tester::TestBook {
                    name,
                    author: entry
                        .get("author")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    // Optional `domain` hint matches the site the book lives
                    // on; sources on that domain try this book first.
                    domain_hint: entry
                        .get("domain")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string),
                });
            }
        }
    }

    // Generic fallback keywords are configurable via `[test]
    // generic_keywords = [...]`; default keeps the classic high-frequency set.
    let mut generic_keywords = vec!["重生".into(), "系统".into(), "穿越".into()];
    let test_section = config_toml.get("test");
    if let Some(kws) = test_section
        .and_then(|v| v.get("generic_keywords"))
        .and_then(|v| v.as_array())
    {
        let parsed: Vec<String> = kws
            .iter()
            .filter_map(|v| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string))
            .collect();
        if !parsed.is_empty() {
            generic_keywords = parsed;
        }
    }

    // Rare keywords used for the static-list probe; must differ from the
    // primary keyword or the probe is skipped. Default is a string that no
    // real novel title contains.
    let mut static_probe_keywords = vec!["zzzzz".into()];
    if let Some(kws) = test_section
        .and_then(|v| v.get("static_probe_keywords"))
        .and_then(|v| v.as_array())
    {
        let parsed: Vec<String> = kws
            .iter()
            .filter_map(|v| v.as_str().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string))
            .collect();
        if !parsed.is_empty() {
            static_probe_keywords = parsed;
        }
    }

    // Re-test `passed` sources older than this many days (0 = never).
    let cache_ttl_days = test_section
        .and_then(|v| v.get("cache_ttl_days"))
        .and_then(|v| v.as_integer())
        .unwrap_or(0)
        .max(0) as u64;

    Ok(tester::TestConfig {
        concurrency,
        timeout_secs,
        test_books,
        generic_keywords,
        force,
        verify_content,
        static_probe_keywords,
        cache_ttl_days,
    })
}
