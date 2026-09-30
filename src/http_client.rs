use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use encoding_rs::Encoding;
use reqwest::{Client, ClientBuilder};
use tokio::sync::Mutex as AsyncMutex;

use crate::search_url::RequestSpec;
use crate::status::FailReason;
use crate::url_fixer::normalized_domain;

/// Classify an HTTP status code for failure-reason reporting.
/// Returns a short label used in the cache `reason` column.
pub fn classify_status(status: u16) -> &'static str {
    match status {
        200..=299 => "ok",
        401 | 403 => "blocked", // anti-bot / auth wall
        404 => "not_found",
        408 | 429 => "rate_limited",
        500..=599 => "server_error",
        _ => "http_error",
    }
}

/// A categorised network failure. `fetch` returns `Ok` for any HTTP response
/// (including 4xx — those are classified by status code); `Err` is reserved
/// for transport-level failures and 5xx, so callers can tell "the site is
/// down / unreachable" apart from "the site answered but refused us".
#[derive(Debug)]
pub enum FetchError {
    /// Request timed out (timeout_secs exceeded).
    Timeout(reqwest::Error),
    /// DNS resolution or TCP connect failed.
    Connect(reqwest::Error),
    /// TLS handshake / certificate problem.
    Tls(reqwest::Error),
    /// Server returned 5xx (transient by nature).
    Server { status: u16, url: String },
    /// Body read or decode failure.
    Body(reqwest::Error),
    /// Anything else (request builder, redirect policy, ...).
    Other(reqwest::Error),
}

impl FetchError {
    /// Map a transport error to the cache reason it should be recorded under.
    /// 5xx → `server_error`; everything else is a network-level failure.
    pub fn fail_reason(&self) -> FailReason {
        match self {
            FetchError::Server { .. } => FailReason::ServerError,
            _ => FailReason::NetworkError,
        }
    }

    /// Classify a raw reqwest error into a `FetchError`.
    fn classify(err: reqwest::Error) -> FetchError {
        if err.is_timeout() {
            return FetchError::Timeout(err);
        }
        if err.is_connect() {
            return FetchError::Connect(err);
        }
        if let Some(status) = err.status()
            && status.as_u16() >= 500 {
                return FetchError::Server {
                    status: status.as_u16(),
                    url: err.url().map(|u| u.to_string()).unwrap_or_default(),
                };
            }
        if looks_like_tls(&err) {
            return FetchError::Tls(err);
        }
        if err.is_body() || err.is_decode() {
            return FetchError::Body(err);
        }
        FetchError::Other(err)
    }
}

impl From<reqwest::Error> for FetchError {
    fn from(err: reqwest::Error) -> Self {
        FetchError::classify(err)
    }
}

impl fmt::Display for FetchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FetchError::Timeout(e) => write!(f, "timeout: {e}"),
            FetchError::Connect(e) => write!(f, "connect failed: {e}"),
            FetchError::Tls(e) => write!(f, "tls error: {e}"),
            FetchError::Server { status, url } => write!(f, "HTTP {status} for {url}"),
            FetchError::Body(e) => write!(f, "body error: {e}"),
            FetchError::Other(e) => write!(f, "request error: {e}"),
        }
    }
}

impl std::error::Error for FetchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FetchError::Timeout(e)
            | FetchError::Connect(e)
            | FetchError::Tls(e)
            | FetchError::Body(e)
            | FetchError::Other(e) => Some(e),
            FetchError::Server { .. } => None,
        }
    }
}

/// reqwest doesn't expose "is this a TLS error", so walk the source chain and
/// look for characteristic error descriptions. Shallow (≤ 8 levels) and
/// best-effort — a miss just falls through to `Other`.
fn looks_like_tls(err: &reqwest::Error) -> bool {
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err);
    for _ in 0..8 {
        let Some(src) = cur else { break };
        let d = format!("{:?}", src).to_lowercase();
        if d.contains("tls")
            || d.contains("handshake")
            || d.contains("certificate")
            || d.contains("rustls")
            || d.contains("native-tls")
            || d.contains("ssl")
        {
            return true;
        }
        cur = src.source();
    }
    false
}

pub struct HttpClient {
    client: Client,
    no_redirect_client: Client,
    /// Minimum interval between two requests to the same (normalized) domain,
    /// honoring the `concurrentRate` declared by book sources. Empty = no
    /// rate limiting (default).
    rate_limits: HashMap<String, Duration>,
    /// Per-domain last-request timestamps, serialized per domain so requests
    /// to the same site queue up instead of bursting.
    last_request: HashMap<String, Arc<AsyncMutex<Option<Instant>>>>,
}

#[derive(Debug)]
pub struct FetchResult {
    pub body_text: String,
    pub body_bytes: Vec<u8>,
    pub content_type: String,
    pub status_code: u16,
    pub elapsed_ms: u64,
    pub charset_detected: Option<String>,
    pub charset_uncertain: bool,
    pub headers: Vec<(String, String)>,
}

impl HttpClient {
    pub fn new(timeout_secs: u64) -> anyhow::Result<Self> {
        let client = ClientBuilder::new()
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_secs(timeout_secs))
            .connect_timeout(Duration::from_secs(10))
            .user_agent("Mozilla/5.0 (Linux; Android 13; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36")
            .gzip(true)
            .https_only(false)
            .build()?;

        // java.ajax/java.post bridge requests must NOT follow redirects so the
        // JS can read `Location` from the response headers itself.
        let no_redirect_client = ClientBuilder::new()
            .danger_accept_invalid_certs(true)
            .timeout(Duration::from_secs(timeout_secs))
            .connect_timeout(Duration::from_secs(10))
            .user_agent("Mozilla/5.0 (Linux; Android 13; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Mobile Safari/537.36")
            .gzip(true)
            .https_only(false)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;

        Ok(Self {
            client,
            no_redirect_client,
            rate_limits: HashMap::new(),
            last_request: HashMap::new(),
        })
    }

    /// Attach per-domain rate limits (domain → minimum inter-request interval).
    /// Domains not present are not throttled.
    pub fn with_rate_limits(mut self, limits: HashMap<String, Duration>) -> Self {
        self.last_request = limits
            .keys()
            .map(|d| (d.clone(), Arc::new(AsyncMutex::new(None))))
            .collect();
        self.rate_limits = limits;
        self
    }

    /// Wait out the per-domain interval before the request, if the domain is
    /// rate-limited. Serializes requests per domain via an async mutex.
    async fn rate_limit_acquire(&self, url: &str) {
        let Some(domain) = normalized_domain(url) else {
            return;
        };
        let Some(interval) = self.rate_limits.get(&domain).copied() else {
            return;
        };
        let Some(mutex) = self.last_request.get(&domain) else {
            return;
        };
        let mut last = mutex.lock().await;
        if let Some(prev) = *last {
            let elapsed = prev.elapsed();
            if elapsed < interval {
                tokio::time::sleep(interval - elapsed).await;
            }
        }
        *last = Some(Instant::now());
    }

    pub async fn fetch(&self, spec: &RequestSpec) -> Result<FetchResult, FetchError> {
        self.rate_limit_acquire(&spec.url).await;
        let start = std::time::Instant::now();
        let client = if spec.no_redirect {
            &self.no_redirect_client
        } else {
            &self.client
        };

        let response = match spec.method.as_str() {
            "POST" => {
                let mut req = client.post(&spec.url);
                if let Some(ref body) = spec.body {
                    // Replace templates in body too
                    req = req.body(body.clone());
                }
                // Form-style POST bodies (Legado `'body':'a=b&c=d'`) need the
                // urlencoded Content-Type, otherwise some sites return an
                // error page instead of search results.
                let has_ct = spec
                    .headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case("content-type"));
                if !has_ct {
                    req = req.header(
                        reqwest::header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded",
                    );
                }
                for (k, v) in &spec.headers {
                    req = req.header(k, v);
                }
                req.send().await?
            }
            _ => {
                let mut req = client.get(&spec.url);
                for (k, v) in &spec.headers {
                    req = req.header(k, v);
                }
                req.send().await?
            }
        };

        let status_code = response.status().as_u16();

        // Server errors (5xx) are transient — return error so tester retries
        if status_code >= 500 {
            return Err(FetchError::Server {
                status: status_code,
                url: spec.url.clone(),
            });
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let headers: Vec<(String, String)> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        let elapsed_ms = start.elapsed().as_millis() as u64;
        let body_bytes = response.bytes().await?.to_vec();

        // Decode bytes to string using charset detection
        let (body_text, charset_detected, charset_uncertain) =
            decode_body(&body_bytes, &content_type, &spec.charset);

        Ok(FetchResult {
            body_text,
            body_bytes,
            content_type,
            status_code,
            elapsed_ms,
            charset_detected,
            charset_uncertain,
            headers,
        })
    }
}

/// Parse the Python-dict style header string into key-value pairs.
/// Format: `{'User-Agent': 'Mozilla/5.0'}` or `{"key": "value"}`
pub fn parse_headers(header_str: &str) -> Vec<(String, String)> {
    use regex::Regex;
    let mut headers = Vec::new();
    if header_str.is_empty() {
        return headers;
    }

    // Match `'key': 'value'` or `"key": "value"`
    let re = Regex::new(r#"['"]([^'"]+)['"]\s*:\s*['"]([^'"]*)['"]"#).unwrap();
    for cap in re.captures_iter(header_str) {
        headers.push((cap[1].to_string(), cap[2].to_string()));
    }
    headers
}

fn decode_body(
    bytes: &[u8],
    content_type: &str,
    url_charset: &Option<String>,
) -> (String, Option<String>, bool) {
    // Priority 1: Content-Type header charset
    if let Some(charset_str) = extract_charset_from_header(content_type)
        && let Some(encoding) = Encoding::for_label(charset_str.as_bytes())
    {
        let (text, _, _) = encoding.decode(bytes);
        return (text.into_owned(), Some(charset_str), false);
    }

    // Priority 2: Charset hint from searchUrl config
    if let Some(charset) = url_charset
        && let Some(encoding) = Encoding::for_label(charset.as_bytes())
    {
        let (text, _, had_errors) = encoding.decode(bytes);
        if !had_errors {
            return (text.into_owned(), Some(charset.clone()), false);
        }
    }

    // Priority 3: chardetng detection
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(bytes, bytes.len() < 1024);
    let encoding = detector.guess(None, true);
    let (text, _, had_errors) = encoding.decode(bytes);
    let name = encoding.name().to_lowercase();
    let uncertain = had_errors || bytes.len() < 64;

    (text.into_owned(), Some(name), uncertain)
}

fn extract_charset_from_header(content_type: &str) -> Option<String> {
    let lower = content_type.to_lowercase();
    if let Some(pos) = lower.find("charset=") {
        let rest = &lower[pos + 8..];
        let charset = rest.split([';', ' ', ',']).next().unwrap_or("");
        if !charset.is_empty() {
            return Some(charset.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_headers_simple() {
        let h = parse_headers("{'User-Agent': 'Mozilla/5.0'}");
        assert_eq!(
            h,
            vec![("User-Agent".to_string(), "Mozilla/5.0".to_string())]
        );
    }

    #[test]
    fn test_parse_headers_multi() {
        let h = parse_headers("{'key1': 'val1', 'key2': 'val2'}");
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].0, "key1");
        assert_eq!(h[1].0, "key2");
    }

    #[test]
    fn test_extract_charset_from_header() {
        assert_eq!(
            extract_charset_from_header("text/html; charset=gbk"),
            Some("gbk".into())
        );
        assert_eq!(extract_charset_from_header("text/html"), None);
    }

    #[tokio::test]
    async fn test_rate_limit_paces_requests() {
        // Mock server records arrival timestamps; the client must space
        // requests to the same domain by >= the configured interval.
        let arrivals = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Instant>::new()));
        let arrivals2 = arrivals.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let arrivals = arrivals2.clone();
                tokio::spawn(async move {
                    use tokio::io::{AsyncReadExt, AsyncWriteExt};
                    let mut buf = [0u8; 8192];
                    let _ = socket.read(&mut buf).await;
                    arrivals.lock().unwrap().push(Instant::now());
                    let body = "ok";
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                });
            }
        });

        // Rate-limit the domain (host part only, no port — matching
        // normalized_domain).
        let mut limits = HashMap::new();
        limits.insert("127.0.0.1".to_string(), Duration::from_millis(250));

        let client = std::sync::Arc::new(HttpClient::new(10).unwrap().with_rate_limits(limits));
        let url = format!("http://{}/x", addr);
        let spec = RequestSpec {
            url: url.clone(),
            method: "GET".to_string(),
            body: None,
            headers: Vec::new(),
            charset: None,
            no_redirect: false,
        };

        // Three concurrent requests to the same (rate-limited) domain.
        let mut handles = Vec::new();
        for _ in 0..3 {
            let client = client.clone();
            let spec = spec.clone();
            handles.push(tokio::spawn(async move {
                client.fetch(&spec).await.unwrap()
            }));
        }
        for h in handles {
            h.await.unwrap();
        }

        let times = arrivals.lock().unwrap().clone();
        assert_eq!(times.len(), 3);
        // Sorted arrival times must be >= ~250ms apart.
        let mut sorted = times;
        sorted.sort();
        for pair in sorted.windows(2) {
            let gap = pair[1].duration_since(pair[0]);
            assert!(
                gap >= Duration::from_millis(200),
                "requests too close together: {gap:?}"
            );
        }
    }

    #[test]
    fn test_no_rate_limits_by_default() {
        // A client without rate limits must pass requests straight through.
        let client = HttpClient::new(10).unwrap();
        assert!(client.rate_limits.is_empty());
    }
}
