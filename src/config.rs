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
            let name = entry.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
            if !name.is_empty() {
                test_books.push(tester::TestBook {
                    name,
                    author: entry.get("author").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    domain_hint: None,
                });
            }
        }
    }

    Ok(tester::TestConfig {
        concurrency,
        timeout_secs,
        test_books,
        generic_keywords: vec!["重生".into(), "系统".into(), "穿越".into()],
        force,
        verify_content,
    })
}
