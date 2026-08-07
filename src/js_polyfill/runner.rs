use std::process::{Command, Stdio};
use std::time::Duration;

use tracing::warn;

/// A single JS search-URL evaluation job.
pub struct JsJob {
    pub id: usize,
    pub base_url: String,
    pub keyword: String,
    pub js_code: String,
}

/// Execute a JS expression and return the resolved URL.
pub fn execute_js(base_url: &str, keyword: &str, js_code: &str) -> Option<String> {
    let job = JsJob {
        id: 0,
        base_url: base_url.to_string(),
        keyword: keyword.to_string(),
        js_code: sanitize_js(js_code).to_string(),
    };
    execute_js_batch(&[job]).into_iter().next().flatten()
}

/// Evaluate many JS search-URL expressions in a single node process.
///
/// One subprocess handles all jobs (instead of one per source/keyword),
/// amortizing node startup. Results are aligned with `jobs` by index.
pub fn execute_js_batch(jobs: &[JsJob]) -> Vec<Option<String>> {
    if jobs.is_empty() {
        return Vec::new();
    }
    let polyfill = include_str!("polyfill.js");

    // Embed each job's code as a JS string literal (Rust Debug escaping is
    // compatible with JS string literals).
    let mut jobs_array = String::new();
    jobs_array.push('[');
    for (i, job) in jobs.iter().enumerate() {
        if i > 0 {
            jobs_array.push(',');
        }
        jobs_array.push_str(&format!(
            "{{id:{},base:{:?},key:{:?},code:{:?}}}",
            job.id,
            job.base_url,
            job.keyword,
            sanitize_js(&job.js_code),
        ));
    }
    jobs_array.push(']');

    let script = format!(
        r#"
globalThis.__PAGE__ = 1;
globalThis.__SOURCE_KEY__ = '';
globalThis.__LOGIN_HEADER__ = '';
globalThis.__SOURCE_COMMENT__ = '';
globalThis.__lastResponse = '';
globalThis.__lastUrl = '';
globalThis.__lastMethod = 'GET';
globalThis.__lastHeaders = {{}};
globalThis.__lastBody = '';

{polyfill}

const jobs = {jobs_array};
for (const job of jobs) {{
    try {{
        globalThis.__BASE_URL__ = job.base;
        globalThis.__KEYWORD__ = job.key;
        globalThis.baseUrl = job.base;
        globalThis.key = job.key;
        globalThis.page = 1;
        globalThis.__lastResponse = '';
        globalThis.__lastUrl = '';
        delete globalThis.url;
        (0, eval)(job.code);
        let url = typeof globalThis.url !== 'undefined' ? globalThis.url : (java._store['url'] || '');
        console.log(JSON.stringify({{id: job.id, type: 'result', url: String(url || '')}}));
    }} catch(e) {{
        console.log(JSON.stringify({{id: job.id, type: 'error', message: String(e)}}));
    }}
}}
"#,
        polyfill = polyfill,
        jobs_array = jobs_array,
    );

    let mut child = match Command::new("node")
        .arg("-e")
        .arg(&script)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to spawn node: {}", e);
            return jobs.iter().map(|_| None).collect();
        }
    };

    // Wait with timeout
    let start = std::time::Instant::now();
    let timeout = Duration::from_secs(10);

    loop {
        if start.elapsed() > timeout {
            if let Err(e) = child.kill() {
                warn!("Failed to kill timed-out node process: {}", e);
            } else {
                // Wait briefly for the process to exit after kill
                let _ = child.wait();
            }
            warn!("Node subprocess timed out after 10s");
            return jobs.iter().map(|_| None).collect();
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => {
                warn!("Node subprocess error: {}", e);
                return jobs.iter().map(|_| None).collect();
            }
        }
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            warn!("Failed to read node output: {}", e);
            return jobs.iter().map(|_| None).collect();
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!("Node exited with error: {}", stderr.trim());
        return jobs.iter().map(|_| None).collect();
    }

    // Parse stdout — collect JSON result lines keyed by job id
    let mut by_id: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed)
            && val.get("type").and_then(|v| v.as_str()) == Some("result") {
                let id = val.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                if let Some(url) = val.get("url").and_then(|v| v.as_str()) {
                    by_id.insert(id, url.to_string());
                }
            }
    }

    jobs.iter()
        .map(|job| by_id.get(&job.id).cloned())
        .collect()
}

/// Strip Legado wrappers (`@js:` prefix, `<js>...</js>` block) from a code body.
fn sanitize_js(code: &str) -> &str {
    let code = code.strip_prefix("@js:").unwrap_or(code);
    if let Some(rel_start) = code.find("<js>") {
        let start = rel_start + 4;
        let end = code[start..]
            .find("</js>")
            .map(|e| start + e)
            .unwrap_or(code.len());
        &code[start..end]
    } else {
        code
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_js() {
        assert_eq!(sanitize_js("@js:url = baseUrl + '/a'"), "url = baseUrl + '/a'");
        assert_eq!(
            sanitize_js("<js>url = baseUrl + '/b'</js>"),
            "url = baseUrl + '/b'"
        );
        assert_eq!(sanitize_js("url = baseUrl + '/c'"), "url = baseUrl + '/c'");
    }

    #[test]
    fn test_execute_js_batch_simple() {
        if Command::new("node").arg("-e").arg("console.log(1)").output().is_err() {
            eprintln!("node not available; skipping JS batch test");
            return;
        }
        let jobs = vec![
            JsJob {
                id: 0,
                base_url: "https://example.com".into(),
                keyword: "测试".into(),
                js_code: "url = baseUrl + '/search?q=' + encodeURI(key)".into(),
            },
            JsJob {
                id: 1,
                base_url: "https://example.com".into(),
                keyword: "书".into(),
                js_code: "url = baseUrl + '/s?k=' + key".into(),
            },
        ];
        let results = execute_js_batch(&jobs);
        assert_eq!(results.len(), 2, "one result per job");
        assert_eq!(results[0].as_deref(), Some("https://example.com/search?q=%E6%B5%8B%E8%AF%95"));
        assert_eq!(results[1].as_deref(), Some("https://example.com/s?k=书"));
    }

    #[test]
    fn test_execute_js_batch_wrapper_forms() {
        if Command::new("node").arg("-e").arg("console.log(1)").output().is_err() {
            eprintln!("node not available; skipping JS batch test");
            return;
        }
        let jobs = vec![
            JsJob {
                id: 0,
                base_url: "https://a.com".into(),
                keyword: "k".into(),
                js_code: "@js:url = baseUrl + '/@js-path'" .into(),
            },
            JsJob {
                id: 1,
                base_url: "https://a.com".into(),
                keyword: "k".into(),
                js_code: "<js>url = baseUrl + '/block'</js>".into(),
            },
        ];
        let results = execute_js_batch(&jobs);
        assert_eq!(results[0].as_deref(), Some("https://a.com/@js-path"));
        assert_eq!(results[1].as_deref(), Some("https://a.com/block"));
    }
}
