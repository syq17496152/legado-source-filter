use std::path::Path;

use reqwest::blocking::Client;

/// Remove pipeline output cache files (test DB + derived artifacts).
/// Must also remove WAL/SHM sidecar files for the SQLite DB.
pub fn clear_output_cache(output_dir: &Path) {
    for entry in &[
        "test_cache.db",
        "test_cache.db-wal",
        "test_cache.db-shm",
        "filtered.json",
    ] {
        let path = output_dir.join(entry);
        if path.exists() {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Ensure the local book source JSON is up to date.
/// Fetches the aoaostar index page, discovers the latest "全量书源" URL,
/// downloads if the remote filename has changed, and clears stale caches.
/// If the remote is unreachable, silently falls back to the local file.
pub fn ensure_fresh_source(input_path: &Path, output_dir: &Path) -> anyhow::Result<()> {
    let source_url_path = output_dir.join(".source_url");

    // Ensure data/ and output/ directories exist (for cargo install scenario)
    if let Some(parent) = input_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir_all(output_dir)?;

    // If we already have a local file, try remote update but don't fail if unreachable
    let has_local = input_path.exists();

    let html = match fetch_text("https://legado.aoaostar.com") {
        Ok(h) => h,
        Err(e) => {
            if has_local {
                println!("无法连接书源服务器，使用本地缓存");
                return Ok(());
            }
            return Err(e);
        }
    };

    let remote_url = discover_source_url(&html)?;
    let remote_name = remote_url.rsplit('/').next().unwrap_or("");

    // Compare with the last downloaded URL
    let last_url = std::fs::read_to_string(&source_url_path).ok();
    if let Some(ref last) = last_url
        && last.trim() == remote_url
        && has_local
    {
        tracing::info!("书源已是最新: {}", remote_name);
        return Ok(());
    }

    // Download new JSON to a temp file, then atomically replace
    tracing::info!("发现新书源: {}，正在下载...", remote_name);
    let temp_path = input_path.with_extension("tmp");
    download_to_file(&remote_url, &temp_path)
        .map_err(|e| anyhow::anyhow!("下载书源失败（服务器限速或网络问题，可稍后重试）: {}", e))?;
    std::fs::rename(&temp_path, input_path)?;
    std::fs::write(&source_url_path, &remote_url)?;

    tracing::info!("书源已更新 ({}), 清除旧缓存", remote_name);
    clear_output_cache(output_dir);
    Ok(())
}

fn fetch_text(url: &str) -> anyhow::Result<String> {
    let client = Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let resp = client.get(url).send()?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {} fetching {}", resp.status(), url);
    }
    Ok(resp.text()?)
}

fn download_to_file(url: &str, path: &Path) -> anyhow::Result<()> {
    let client = Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        // The aoaostar server throttles the ~21MB file heavily; allow up to 5min
        .timeout(std::time::Duration::from_secs(300))
        .build()?;
    let resp = client.get(url).send()?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {} downloading {}", resp.status(), url);
    }
    let bytes = resp.bytes()?;
    std::fs::write(path, &bytes)?;
    Ok(())
}

fn discover_source_url(html_str: &str) -> anyhow::Result<String> {
    use scraper::{Html, Selector};
    let doc = Html::parse_document(html_str);
    let li_sel = Selector::parse("li").unwrap();
    let p_sel = Selector::parse("p").unwrap();
    let a_sel = Selector::parse("a[href]").unwrap();

    for li in doc.root_element().select(&li_sel) {
        // Check if this <li> contains a <p> with text "全量书源"
        let mut has_header = false;
        for p in li.select(&p_sel) {
            let text: String = p.text().collect();
            if text.contains("全量书源") {
                has_header = true;
                break;
            }
        }
        if !has_header {
            continue;
        }

        // Within this <li>, find the first <a> with a .json href
        for a in li.select(&a_sel) {
            if let Some(href) = a.value().attr("href")
                && href.ends_with(".json")
            {
                return Ok(href.to_string());
            }
        }
    }

    anyhow::bail!("未在页面中找到全量书源链接");
}
