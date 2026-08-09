use std::process::{Command, Stdio};
use std::time::Duration;

use tracing::warn;

use crate::http_client::HttpClient;
use crate::search_url::{RequestSpec, resolve_url};

/// A single JS search-URL evaluation job.
pub struct JsJob {
    pub id: usize,
    pub base_url: String,
    pub keyword: String,
    pub js_code: String,
}

/// A JS rule-evaluation job (ruleBookInfo.tocUrl, ruleToc.*, ruleContent).
///
/// `result` is the fetched page body the rule operates on.
pub struct JsRuleJob {
    pub id: usize,
    pub base_url: String,
    pub result: String,
    pub js_code: String,
}

/// Outputs produced by evaluating a rule job.
#[derive(Debug, Default, Clone)]
pub struct JsRuleOutput {
    pub url: Option<String>,
    pub content: Option<String>,
    pub chapter_title: Option<String>,
    pub chapter_url: Option<String>,
    pub list: Vec<(String, String)>,
    pub error: Option<String>,
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

/// The node-side prologue that enables the java.ajax ↔ Rust bridge
/// (persistent stdin/stdout) when `bridge` is true.
fn bridge_prelude(bridge: bool) -> String {
    if bridge {
        r#"
globalThis.__BRIDGE__ = true;
globalThis.__AJAX_SEQ__ = 0;
const fs = require('fs');
let __stdinBuf = '';
function __writeLine(obj) {
    fs.writeSync(1, JSON.stringify(obj) + '\n');
}
function __readLineSync() {
    while (true) {
        const idx = __stdinBuf.indexOf('\n');
        if (idx >= 0) {
            const line = __stdinBuf.slice(0, idx);
            __stdinBuf = __stdinBuf.slice(idx + 1);
            return line;
        }
        const buf = Buffer.alloc(65536);
        const n = fs.readSync(0, buf, 0, buf.length, null);
        if (n <= 0) throw new Error('stdin closed');
        __stdinBuf += buf.toString('utf8', 0, n);
    }
}
"#
        .to_string()
    } else {
        r#"
globalThis.__BRIDGE__ = false;
"#
        .to_string()
    }
}

/// Build the node script that evaluates the given rule jobs.
/// `bridge: true` enables the java.ajax ↔ Rust request/response protocol
/// (persistent stdin/stdout); `false` uses the one-shot legacy stub.
fn build_rule_script(jobs: &[JsRuleJob], bridge: bool) -> String {
    let polyfill = include_str!("polyfill.js");

    let mut jobs_array = String::new();
    jobs_array.push('[');
    for (i, job) in jobs.iter().enumerate() {
        if i > 0 {
            jobs_array.push(',');
        }
        jobs_array.push_str(&format!(
            "{{id:{},base:{:?},result:{:?},code:{:?}}}",
            job.id,
            job.base_url,
            job.result,
            sanitize_js(&job.js_code),
        ));
    }
    jobs_array.push(']');

    let bridge_prelude = bridge_prelude(bridge);

    format!(
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

{bridge_prelude}

{polyfill}

function __evalRule(code) {{ return eval(code); }}

const jobs = {jobs_array};
for (const job of jobs) {{
    try {{
        globalThis.__BASE_URL__ = job.base;
        globalThis.baseUrl = job.base;
        globalThis.result = job.result;
        globalThis.__lastResponse = job.result;
        globalThis.page = 1;
        delete globalThis.url;
        delete globalThis.content;
        delete globalThis.chapter;
        delete globalThis.list;
        delete globalThis.tocUrl;
        // Direct eval inside a function scope: `var` declarations in the rule
        // stay local (Legado semantics), so a rule like `var url=...` does not
        // clobber the `url`/`content` globals we collect as the result.
        const __ret = __evalRule(job.code);
        const out = {{id: job.id, type: 'result'}};
        // Bare-expression rules (Legado: last value is the result) — fall back
        // to the eval completion value when no named global was assigned.
        let __fallback = (__ret !== undefined && __ret !== null) ? String(__ret) : '';
        if (typeof globalThis.url !== 'undefined' && globalThis.url !== null) out.url = String(globalThis.url);
        if (typeof globalThis.tocUrl !== 'undefined' && globalThis.tocUrl !== null) out.tocUrl = String(globalThis.tocUrl);
        if (typeof globalThis.content !== 'undefined' && globalThis.content !== null) out.content = String(globalThis.content);
        if (!out.url && !out.content && __fallback) out.content = __fallback;
        if (globalThis.chapter) {{
            if (globalThis.chapter.title !== undefined) out.chapterTitle = String(globalThis.chapter.title);
            if (globalThis.chapter.url !== undefined) out.chapterUrl = String(globalThis.chapter.url);
        }}
        if (Array.isArray(globalThis.list)) {{
            out.list = globalThis.list.map(it => {{
                let text = it && (it.text !== undefined ? it.text : (it.name !== undefined ? it.name : ''));
                let href = it && (it.href !== undefined ? it.href : (it.url !== undefined ? it.url : ''));
                return [String(text || ''), String(href || '')];
            }});
        }}
        {emit}
    }} catch(e) {{
        {emit_err}
    }}
}}
"#,
        bridge_prelude = bridge_prelude,
        polyfill = polyfill,
        jobs_array = jobs_array,
        emit = if bridge {
            "__writeLine(out);"
        } else {
            "console.log(JSON.stringify(out));"
        },
        emit_err = if bridge {
            "__writeLine({id: job.id, type: 'error', message: String(e)});"
        } else {
            "console.log(JSON.stringify({id: job.id, type: 'error', message: String(e)}));"
        },
    )
}

/// Parse node output lines into a map of job-id → JsRuleOutput.
fn collect_rule_outputs(stdout: &str) -> std::collections::HashMap<usize, JsRuleOutput> {
    let mut by_id: std::collections::HashMap<usize, JsRuleOutput> =
        std::collections::HashMap::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        let id = val.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
        let t = val.get("type").and_then(|v| v.as_str());
        if t == Some("error") {
            by_id.entry(id).or_default().error = Some(
                val.get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            );
        } else if t == Some("result") {
            let out = by_id.entry(id).or_default();
            out.url = val.get("url").and_then(|v| v.as_str()).map(str::to_string);
            if let Some(u) = val.get("tocUrl").and_then(|v| v.as_str()) {
                out.url = Some(u.to_string());
            }
            out.content = val
                .get("content")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            out.chapter_title = val
                .get("chapterTitle")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            out.chapter_url = val
                .get("chapterUrl")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            if let Some(list) = val.get("list").and_then(|v| v.as_array()) {
                for item in list {
                    if let Some(pair) = item.as_array()
                        && pair.len() >= 2
                        && let (Some(a), Some(b)) = (pair[0].as_str(), pair[1].as_str())
                    {
                        out.list.push((a.to_string(), b.to_string()));
                    }
                }
            }
        }
    }
    by_id
}

/// Evaluate many rule jobs (tocUrl / chapterUrl / content) in a single node
/// process. Each job gets `result` (page body) and `baseUrl` injected, then
/// the resulting globals (`url`, `content`, `chapter`, `list`, `tocUrl`) are
/// collected back. Results are aligned with `jobs` by index.
///
/// This sync variant uses the legacy one-shot node process (no real HTTP for
/// `java.ajax` — it returns an empty string). Prefer [`execute_rule_batch_async`]
/// from async contexts where a client is available.
pub fn execute_rule_batch(jobs: &[JsRuleJob]) -> Vec<JsRuleOutput> {
    if jobs.is_empty() {
        return Vec::new();
    }
    let script = build_rule_script(jobs, false);
    let stdout = run_node(&script);
    let by_id = collect_rule_outputs(&stdout);
    jobs.iter()
        .map(|job| by_id.get(&job.id).cloned().unwrap_or_default())
        .collect()
}

/// Evaluate many rule jobs with live `java.ajax` support.
///
/// Runs one persistent node process with piped stdin/stdout. When a rule calls
/// `java.ajax(url)` / `java.post(url, ...)`, node writes a request line to
/// stdout and blocks reading stdin; this function performs the real HTTP fetch
/// (with `source_headers` + Referer set to the job's `baseUrl`) and writes the
/// response back. Returns outputs aligned with `jobs` by index.
pub async fn execute_rule_batch_async(
    jobs: &[JsRuleJob],
    client: &HttpClient,
    source_headers: &[(String, String)],
) -> Vec<JsRuleOutput> {
    if jobs.is_empty() {
        return Vec::new();
    }
    let script = build_rule_script(jobs, true);

    let mut child = match tokio::process::Command::new("node")
        .arg("-e")
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to spawn node: {}", e);
            return vec![JsRuleOutput::default(); jobs.len()];
        }
    };

    let mut stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let mut by_id: std::collections::HashMap<usize, JsRuleOutput> =
        std::collections::HashMap::new();

    // Drain stderr in the background so it can't fill the pipe.
    let stderr_task = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_end(&mut buf).await;
        }
        String::from_utf8_lossy(&buf).to_string()
    });

    // Read stdout line by line; respond to ajax requests synchronously.
    let overall = tokio::time::timeout(
        Duration::from_secs(45),
        run_rule_bridge(stdout, stdin.as_mut(), client, source_headers, &mut by_id),
    )
    .await;

    if overall.is_err() {
        warn!("node bridge timed out; killing subprocess");
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
    let _ = stderr_task.await;

    jobs.iter()
        .map(|job| by_id.get(&job.id).cloned().unwrap_or_default())
        .collect()
}

/// The read loop: consume node stdout, answer ajax requests by fetching via
/// `client`, and collect `result`/`error` lines into `by_id`.
async fn run_rule_bridge(
    stdout: Option<tokio::process::ChildStdout>,
    stdin: Option<&mut tokio::process::ChildStdin>,
    client: &HttpClient,
    source_headers: &[(String, String)],
    by_id: &mut std::collections::HashMap<usize, JsRuleOutput>,
) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let Some(stdout) = stdout else { return };
    let Some(stdin) = stdin else { return };
    let reader = BufReader::new(stdout);
    let mut lines = reader.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let trimmed = line.trim();
        let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        let t = val.get("type").and_then(|v| v.as_str());

        if t == Some("ajax") {
            let id = val.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let url = val
                .get("url")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let method = val
                .get("method")
                .and_then(|v| v.as_str())
                .unwrap_or("GET")
                .to_string();
            let body = val
                .get("body")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let base = val
                .get("base")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let extra_headers: Vec<(String, String)> = val
                .get("headers")
                .and_then(|v| v.as_object())
                .map(|obj| {
                    obj.iter()
                        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                        .collect()
                })
                .unwrap_or_default();

            let mut headers = source_headers.to_vec();
            headers.push(("Referer".to_string(), base.clone()));
            headers.extend(extra_headers);

            let full_url = resolve_url(&url, &base);
            let spec = RequestSpec {
                url: full_url,
                method: method.clone(),
                body: if method.eq_ignore_ascii_case("POST") {
                    Some(body)
                } else {
                    None
                },
                headers,
                charset: None,
                // java.ajax/java.post in Legado do NOT follow redirects — the
                // JS reads `Location` etc. from the response headers itself.
                no_redirect: true,
            };

            let response = match client.fetch(&spec).await {
                Ok(r) => {
                    let headers_json: serde_json::Value = r
                        .headers
                        .iter()
                        .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
                        .collect::<serde_json::Map<String, serde_json::Value>>()
                        .into();
                    format!(
                        "{{\"type\":\"ajaxResp\",\"id\":{},\"body\":{},\"headers\":{}}}",
                        id,
                        serde_json::to_string(&r.body_text).unwrap_or_else(|_| "\"\"".to_string()),
                        serde_json::to_string(&headers_json).unwrap_or_else(|_| "{}".to_string())
                    )
                }
                Err(e) => format!(
                    "{{\"type\":\"ajaxErr\",\"id\":{},\"error\":{}}}",
                    id,
                    serde_json::to_string(&e.to_string()).unwrap_or_else(|_| "\"\"".to_string())
                ),
            };
            let _ = stdin.write_all(format!("{}\n", response).as_bytes()).await;
            let _ = stdin.flush().await;
        } else {
            // result / error lines
            let id = val.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            if t == Some("error") {
                by_id.entry(id).or_default().error = Some(
                    val.get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                );
            } else if t == Some("result") {
                let out = by_id.entry(id).or_default();
                out.url = val.get("url").and_then(|v| v.as_str()).map(str::to_string);
                if let Some(u) = val.get("tocUrl").and_then(|v| v.as_str()) {
                    out.url = Some(u.to_string());
                }
                out.content = val
                    .get("content")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                out.chapter_title = val
                    .get("chapterTitle")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                out.chapter_url = val
                    .get("chapterUrl")
                    .and_then(|v| v.as_str())
                    .map(str::to_string);
                if let Some(list) = val.get("list").and_then(|v| v.as_array()) {
                    for item in list {
                        if let Some(pair) = item.as_array()
                            && pair.len() >= 2
                            && let (Some(a), Some(b)) = (pair[0].as_str(), pair[1].as_str())
                        {
                            out.list.push((a.to_string(), b.to_string()));
                        }
                    }
                }
            }
        }
    }
    let _ = stdin.shutdown().await;
}

/// Evaluate many JS search-URL expressions in a single node process.
///
/// One subprocess handles all jobs (instead of one per source/keyword),
/// amortizing node startup. Results are aligned with `jobs` by index.
pub fn execute_js_batch(jobs: &[JsJob]) -> Vec<Option<String>> {
    if jobs.is_empty() {
        return Vec::new();
    }
    let script = build_search_script(jobs, false);
    let stdout = run_node(&script);

    // Parse stdout — collect JSON result lines keyed by job id
    let mut by_id: std::collections::HashMap<usize, String> = std::collections::HashMap::new();
    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed)
            && val.get("type").and_then(|v| v.as_str()) == Some("result")
        {
            let id = val.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            if let Some(url) = val.get("url").and_then(|v| v.as_str()) {
                by_id.insert(id, url.to_string());
            }
        }
    }

    jobs.iter().map(|job| by_id.get(&job.id).cloned()).collect()
}

/// Build the node script that evaluates the given search-URL jobs.
/// `bridge: true` enables java.ajax ↔ Rust for `@js` search rules that need
/// live HTTP (e.g. `java.post(...).header("Location")`).
fn build_search_script(jobs: &[JsJob], bridge: bool) -> String {
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

    let bridge_prelude = bridge_prelude(bridge);
    let emit = if bridge {
        "__writeLine(out);"
    } else {
        "console.log(JSON.stringify(out));"
    };
    let emit_err = if bridge {
        "__writeLine({id: job.id, type: 'error', message: String(e)});"
    } else {
        "console.log(JSON.stringify({id: job.id, type: 'error', message: String(e)}));"
    };

    format!(
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

{bridge_prelude}

{polyfill}

function __evalRule(code) {{ return eval(code); }}

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
        // Direct eval in a function scope so `var` declarations in the rule
        // stay local (Legado semantics), instead of leaking onto globalThis
        // where they'd be misread as the collected result. Defined at top level
        // so the `url`/`content` let-bindings below (in the loop body) are not
        // in scope, avoiding TDZ ReferenceErrors.
        const __ret = __evalRule(job.code);
        let url = typeof globalThis.url !== 'undefined' ? globalThis.url : (java._store['url'] || __ret);
        const out = {{id: job.id, type: 'result', url: String(url || '')}};
        {emit}
    }} catch(e) {{
        {emit_err}
    }}
}}
"#,
        polyfill = polyfill,
        bridge_prelude = bridge_prelude,
        jobs_array = jobs_array,
        emit = emit,
        emit_err = emit_err,
    )
}

/// Evaluate many search-URL jobs with live `java.ajax` support.
///
/// Runs one persistent node process with piped stdin/stdout, exactly like
/// [`execute_rule_batch_async`], so `@js` search rules that issue their own
/// HTTP (POST + `.header("Location")` redirect chasing, token handshakes, …)
/// get real responses back. Results are aligned with `jobs` by index.
pub async fn execute_js_batch_async(
    jobs: &[JsJob],
    client: &HttpClient,
    source_headers: &[(String, String)],
) -> Vec<Option<String>> {
    if jobs.is_empty() {
        return Vec::new();
    }
    let script = build_search_script(jobs, true);

    let mut child = match tokio::process::Command::new("node")
        .arg("-e")
        .arg(&script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to spawn node: {}", e);
            return vec![None; jobs.len()];
        }
    };

    let mut stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let mut by_id: std::collections::HashMap<usize, JsRuleOutput> =
        std::collections::HashMap::new();

    // Drain stderr in the background so it can't fill the pipe.
    let stderr_task = tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        if let Some(mut stderr) = stderr {
            let _ = stderr.read_to_end(&mut buf).await;
        }
        String::from_utf8_lossy(&buf).to_string()
    });

    // Read stdout line by line; respond to ajax requests synchronously.
    let overall = tokio::time::timeout(
        Duration::from_secs(45),
        run_rule_bridge(stdout, stdin.as_mut(), client, source_headers, &mut by_id),
    )
    .await;

    if overall.is_err() {
        warn!("node bridge timed out; killing subprocess");
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
    let _ = stderr_task.await;

    jobs.iter()
        .map(|job| by_id.get(&job.id).and_then(|o| o.url.clone()))
        .collect()
}

/// Spawn a node process for the given script and return stdout (or empty on
/// any failure). Shared by search-URL and rule-evaluation batches.
fn run_node(script: &str) -> String {
    let mut child = match Command::new("node")
        .arg("-e")
        .arg(script)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            warn!("Failed to spawn node: {}", e);
            return String::new();
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
            return String::new();
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => {
                warn!("Node subprocess error: {}", e);
                return String::new();
            }
        }
    }

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => {
            warn!("Failed to read node output: {}", e);
            return String::new();
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!("Node exited with error: {}", stderr.trim());
        return String::new();
    }

    String::from_utf8_lossy(&output.stdout).to_string()
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
        assert_eq!(
            sanitize_js("@js:url = baseUrl + '/a'"),
            "url = baseUrl + '/a'"
        );
        assert_eq!(
            sanitize_js("<js>url = baseUrl + '/b'</js>"),
            "url = baseUrl + '/b'"
        );
        assert_eq!(sanitize_js("url = baseUrl + '/c'"), "url = baseUrl + '/c'");
    }

    #[test]
    fn test_execute_js_batch_simple() {
        if Command::new("node")
            .arg("-e")
            .arg("console.log(1)")
            .output()
            .is_err()
        {
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
        assert_eq!(
            results[0].as_deref(),
            Some("https://example.com/search?q=%E6%B5%8B%E8%AF%95")
        );
        assert_eq!(results[1].as_deref(), Some("https://example.com/s?k=书"));
    }

    #[test]
    fn test_execute_js_batch_wrapper_forms() {
        if Command::new("node")
            .arg("-e")
            .arg("console.log(1)")
            .output()
            .is_err()
        {
            eprintln!("node not available; skipping JS batch test");
            return;
        }
        let jobs = vec![
            JsJob {
                id: 0,
                base_url: "https://a.com".into(),
                keyword: "k".into(),
                js_code: "@js:url = baseUrl + '/@js-path'".into(),
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

    #[test]
    fn test_execute_js_batch_last_expression_value() {
        // Legado @js searchUrls often end with a bare expression (no `url=`):
        // the last evaluated value becomes the URL. Indirect eval returns it.
        if Command::new("node")
            .arg("-e")
            .arg("console.log(1)")
            .output()
            .is_err()
        {
            eprintln!("node not available; skipping JS batch test");
            return;
        }
        let jobs = vec![JsJob {
            id: 0,
            base_url: "http://www.example.com/".into(),
            keyword: "重生".into(),
            js_code: r#"@js:
option={"method":"POST","body":{"keyword":key}}
"http://www.example.com/"+"search,"+JSON.stringify(option)"#
                .into(),
        }];
        let results = execute_js_batch(&jobs);
        assert_eq!(
            results[0].as_deref(),
            Some(
                "http://www.example.com/search,{\"method\":\"POST\",\"body\":{\"keyword\":\"重生\"}}"
            )
        );
    }

    #[test]
    fn test_execute_rule_batch_content_and_list() {
        if Command::new("node")
            .arg("-e")
            .arg("console.log(1)")
            .output()
            .is_err()
        {
            eprintln!("node not available; skipping JS rule test");
            return;
        }
        let jobs = vec![
            JsRuleJob {
                id: 0,
                base_url: "https://a.com/novel".into(),
                result: r#"{"data":[{"id":1,"name":"A"}]}"#.into(),
                js_code: "url = baseUrl + '/' + java.getString('$.data[0].id')".into(),
            },
            JsRuleJob {
                id: 1,
                base_url: "https://a.com/novel".into(),
                result: "<div class='chapter'>Hello 正文</div>".into(),
                js_code: "content = java.getString('class.chapter@text')".into(),
            },
            JsRuleJob {
                id: 2,
                base_url: "https://a.com/novel".into(),
                result: "".into(),
                js_code: "list = [{text:'1',href:'/c/1'},{text:'2',href:'/c/2'}]".into(),
            },
        ];
        let out = execute_rule_batch(&jobs);
        assert_eq!(out[0].url.as_deref(), Some("https://a.com/novel/1"));
        assert_eq!(out[1].content.as_deref(), Some("Hello 正文"));
        assert_eq!(out[2].list.len(), 2);
        assert_eq!(out[2].list[0], ("1".to_string(), "/c/1".to_string()));
    }

    #[test]
    fn test_execute_rule_batch_chapter_vars() {
        if Command::new("node")
            .arg("-e")
            .arg("console.log(1)")
            .output()
            .is_err()
        {
            eprintln!("node not available; skipping JS rule test");
            return;
        }
        let jobs = vec![JsRuleJob {
            id: 0,
            base_url: "https://a.com".into(),
            result: "".into(),
            js_code: "chapter={title:'第一章',url:'/c/1'}".into(),
        }];
        let out = execute_rule_batch(&jobs);
        assert_eq!(out[0].chapter_title.as_deref(), Some("第一章"));
        assert_eq!(out[0].chapter_url.as_deref(), Some("/c/1"));
    }

    #[test]
    fn test_execute_rule_batch_bare_expression() {
        // Legado rules sometimes end with a bare expression (no `content=` /
        // `url=` assignment); the last value is the result.
        if Command::new("node")
            .arg("-e")
            .arg("console.log(1)")
            .output()
            .is_err()
        {
            eprintln!("node not available; skipping JS rule test");
            return;
        }
        let jobs = vec![JsRuleJob {
            id: 0,
            base_url: "http://s.nshkedu.com".into(),
            result: r#"{"book_id":"12345"}"#.into(),
            js_code: r#"let bid=parseInt(java.getString('$.book_id'));
let subPath=parseInt(bid/1000);
"http://s.nshkedu.com/api/book/chapter/"+subPath+"/"+bid+"/list.json""#
                .into(),
        }];
        let out = execute_rule_batch(&jobs);
        assert_eq!(
            out[0].content.as_deref(),
            Some("http://s.nshkedu.com/api/book/chapter/12/12345/list.json")
        );
    }

    #[tokio::test]
    async fn test_execute_rule_batch_jsoup_token_extraction() {
        // Sources use org.jsoup.Jsoup.parse(html).select(...).attr(...) to pull
        // a hidden form field (CSRF token / action) before building a POST.
        if Command::new("node")
            .arg("-e")
            .arg("console.log(1)")
            .output()
            .is_err()
        {
            eprintln!("node not available; skipping JS rule test");
            return;
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 4096];
                    let _ = socket.read(&mut buf).await;
                    let body = r#"<html><form method="post" action="/search"><input type="hidden" name="_token" value="TOK123"></form></html>"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                });
            }
        });

        let client = HttpClient::new(10).unwrap();
        let jobs = vec![JsRuleJob {
            id: 0,
            base_url: format!("http://{}", addr),
            result: "".into(),
            js_code: r#"var url = baseUrl;
var html = java.ajax(url);
token = org.jsoup.Jsoup.parse(html).select('input[name=_token]').attr('value');
url + "/search," + JSON.stringify({ "body": "_token=" + token + "&keyword=key", "method": "post" })"#
                .into(),
        }];
        let out = execute_rule_batch_async(&jobs, &client, &[]).await;
        assert_eq!(
            out[0].content.as_deref(),
            Some(format!(
                "http://{}/search,{{\"body\":\"_token=TOK123&keyword=key\",\"method\":\"post\"}}",
                addr
            ))
            .as_deref()
        );
    }

    #[test]
    fn test_execute_rule_batch_jsoup_class_and_eq() {
        if Command::new("node")
            .arg("-e")
            .arg("console.log(1)")
            .output()
            .is_err()
        {
            eprintln!("node not available; skipping JS rule test");
            return;
        }
        let jobs = vec![JsRuleJob {
            id: 0,
            base_url: "https://a.com".into(),
            result: r#"<ul class="chapter"><li><a href="/c/1">一</a></li><li><a href="/c/2">二</a></li></ul>"#.into(),
            js_code: "content = org.jsoup.Jsoup.parse(result).select('.chapter li a').eq(1).attr('href')".into(),
        }];
        let out = execute_rule_batch(&jobs);
        assert_eq!(out[0].content.as_deref(), Some("/c/2"));
    }

    #[tokio::test]
    async fn test_execute_rule_batch_async_with_java_ajax() {
        if Command::new("node")
            .arg("-e")
            .arg("console.log(1)")
            .output()
            .is_err()
        {
            eprintln!("node not available; skipping async bridge test");
            return;
        }
        // Tiny HTTP server that answers any request with a fixed JSON body.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 4096];
                    let _ = socket.read(&mut buf).await;
                    let body = r#"{"data":[{"name":"正道潜龙","url":"/book/1"}]}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                });
            }
        });

        let client = HttpClient::new(10).unwrap();
        let jobs = vec![JsRuleJob {
            id: 0,
            base_url: format!("http://{}", addr),
            result: "".into(),
            // The rule calls java.ajax() to fetch data — the bridge must fetch
            // from Rust, feed the body back, and let JS parse it.
            js_code: "content = JSON.parse(java.ajax('/api/list')).data[0].name".into(),
        }];
        let out = execute_rule_batch_async(&jobs, &client, &[]).await;
        assert_eq!(out[0].content.as_deref(), Some("正道潜龙"));
    }

    #[tokio::test]
    async fn test_execute_rule_batch_async_post() {
        if Command::new("node")
            .arg("-e")
            .arg("console.log(1)")
            .output()
            .is_err()
        {
            eprintln!("node not available; skipping async bridge POST test");
            return;
        }
        // Echo the request body back so we can assert POST payload reached Rust.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 8192];
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let body = if req.contains("q=\u{4e66}") {
                        "ok-post"
                    } else {
                        "bad"
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                });
            }
        });

        let client = HttpClient::new(10).unwrap();
        let jobs = vec![JsRuleJob {
            id: 0,
            base_url: format!("http://{}", addr),
            result: "".into(),
            js_code: "content = java.post('/api', 'q=书')".into(),
        }];
        let out = execute_rule_batch_async(&jobs, &client, &[]).await;
        assert_eq!(out[0].content.as_deref(), Some("ok-post"));
    }

    #[tokio::test]
    async fn test_execute_rule_batch_async_post_header_location() {
        // `java.post(...).header("Location")` must read the response header.
        if Command::new("node")
            .arg("-e")
            .arg("console.log(1)")
            .output()
            .is_err()
        {
            eprintln!("node not available; skipping async bridge header test");
            return;
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 8192];
                    let _ = socket.read(&mut buf).await;
                    let resp = "HTTP/1.1 302 Found\r\nLocation: /redirected/page\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = socket.write_all(resp.as_bytes()).await;
                });
            }
        });

        let client = HttpClient::new(10).unwrap();
        let jobs = vec![JsRuleJob {
            id: 0,
            base_url: format!("http://{}", addr),
            result: "".into(),
            js_code: "content = java.post('/api', 'a=1').header('Location')".into(),
        }];
        let out = execute_rule_batch_async(&jobs, &client, &[]).await;
        assert_eq!(out[0].content.as_deref(), Some("/redirected/page"));
    }

    #[tokio::test]
    async fn test_execute_js_batch_async_post_header_location() {
        // @js searchUrl ending with `java.post(...).header("Location")` must
        // yield the Location header as the search URL.
        if Command::new("node")
            .arg("-e")
            .arg("console.log(1)")
            .output()
            .is_err()
        {
            eprintln!("node not available; skipping async search header test");
            return;
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 8192];
                    let _ = socket.read(&mut buf).await;
                    let resp = "HTTP/1.1 302 Found\r\nLocation: /go/to/results\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = socket.write_all(resp.as_bytes()).await;
                });
            }
        });

        let client = HttpClient::new(10).unwrap();
        let jobs = vec![JsJob {
            id: 0,
            base_url: format!("http://{}", addr),
            keyword: "书".into(),
            js_code: "@js:\njava.post(baseUrl + '/search', 'q=' + key).header('Location')".into(),
        }];
        let out = execute_js_batch_async(&jobs, &client, &[]).await;
        assert_eq!(out[0].as_deref(), Some("/go/to/results"));
    }

    #[tokio::test]
    async fn test_rule_batch_desede_cipher_polyfill() {
        // Legado sources decrypt paginated content with the classic Java API
        // (SecretKeySpec / IvParameterSpec / Base64 / Cipher). The polyfill must
        // let such a rule actually decrypt the body fetched via java.ajax.
        if Command::new("node")
            .arg("-e")
            .arg("console.log(1)")
            .output()
            .is_err()
        {
            eprintln!("node not available; skipping DESede cipher test");
            return;
        }
        // Ciphertext of "第一章 风起 夜风卷起落叶…" under key ZKYm5vSUhvcG9IbXNZTG1pb2 / iv 01234567.
        let ciphertext = "riG5B3O77ilQm0WI4sF8a7lRx1s4NSKnlXiQOTKBG0nQR9dekwZ5idofSkItQf9ypZKC2ONiewgCQc8EegkimTyEINFsvSpijoD2RlojNrRf/AN16T0tuQ==";
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = format!("{{\"data\":\"{}\"}}", ciphertext);
        let body_clone = body.clone();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 8192];
            let _ = socket.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body_clone.len(),
                body_clone
            );
            let _ = socket.write_all(resp.as_bytes()).await;
        });

        let client = HttpClient::new(10).unwrap();
        let jobs = vec![JsRuleJob {
            id: 0,
            base_url: format!("http://{}", addr),
            result: "".into(),
            js_code: r#"result = java.ajax('/api');
var data = JSON.parse(result).data;
var key = SecretKeySpec(String("ZKYm5vSUhvcG9IbXNZTG1pb2").getBytes(), "DESede");
var iv = IvParameterSpec(String("01234567").getBytes());
var bytes = Base64.decode(String(data).getBytes(), 2);
var chipher = Cipher.getInstance("DESede/CBC/PKCS5Padding");
chipher.init(2, key, iv);
content = String(chipher.doFinal(bytes));"#
                .into(),
        }];
        let out = execute_rule_batch_async(&jobs, &client, &[]).await;
        let content = out[0].content.as_deref().unwrap_or("");
        assert!(
            content.contains("第一章 风起"),
            "expected decrypted text, got {:?}",
            content
        );
        assert!(content.contains("夜风卷起落叶"), "got {:?}", content);
    }

    #[tokio::test]
    async fn test_rule_batch_aes_cipher_polyfill() {
        // 七猫-style: AES/CBC, iv embedded as the first 16 bytes of the payload,
        // ciphertext is the remainder. Uses Arrays.copyOfRange like the real rules.
        if Command::new("node")
            .arg("-e")
            .arg("console.log(1)")
            .output()
            .is_err()
        {
            eprintln!("node not available; skipping AES cipher test");
            return;
        }
        let plaintext = "夜色渐深，城墙上火把依次亮起";
        // Pre-compute the ciphertext at test time via `node -e` (mirrors the
        // 七猫 payload layout: 16-byte iv followed by the AES-CBC ciphertext).
        let script = format!(
            r#"const c=require('crypto');
const key=Buffer.from('242ccb8230d709e1','utf8');
const iv=Buffer.from('000102030405060708090a0b0c0d0e0f','hex');
const ci=c.createCipheriv('aes-128-cbc',key,iv);
let e=ci.update({:?},'utf8','base64');e+=ci.final('base64');
const payload=Buffer.concat([iv,Buffer.from(e,'base64')]).toString('base64');
console.log(payload);"#,
            plaintext
        );
        let out = std::process::Command::new("node")
            .arg("-e")
            .arg(&script)
            .output();
        let payload = String::from_utf8_lossy(&out.unwrap().stdout)
            .trim()
            .to_string();
        assert!(!payload.is_empty(), "failed to pre-compute payload");

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = format!("{{\"data\":\"{}\"}}", payload);
        let body_clone = body.clone();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 8192];
            let _ = socket.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body_clone.len(),
                body_clone
            );
            let _ = socket.write_all(resp.as_bytes()).await;
        });

        let client = HttpClient::new(10).unwrap();
        let jobs = vec![JsRuleJob {
            id: 0,
            base_url: format!("http://{}", addr),
            result: "".into(),
            js_code: r#"result = java.ajax('/api');
content = JSON.parse(result).data;
var ivEncData = java.base64DecodeToByteArray(content);
var key = SecretKeySpec(String("242ccb8230d709e1").getBytes(), "AES");
var iv = IvParameterSpec(Arrays.copyOfRange(ivEncData, 0, 16));
var chipher = Cipher.getInstance("AES/CBC/PKCS5Padding");
chipher.init(2, key, iv);
content = String(chipher.doFinal(Arrays.copyOfRange(ivEncData, 16, ivEncData.length)));"#
                .into(),
        }];
        let out = execute_rule_batch_async(&jobs, &client, &[]).await;
        let content = out[0].content.as_deref().unwrap_or("");
        assert!(
            content.contains("夜色渐深"),
            "expected decrypted AES text, got {:?}",
            content
        );
    }
}
