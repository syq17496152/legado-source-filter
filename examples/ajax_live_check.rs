//! Live smoke test of the java.ajax bridge against a real public HTTP API.
//! Run: `cargo run --example ajax_live_check`

use legado_source_filter::http_client::HttpClient;
use legado_source_filter::js_polyfill::runner::{JsRuleJob, execute_rule_batch_async};

fn main() -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let client = HttpClient::new(20)?;
        // Real public JSON API — the JS rule fetches it via java.ajax and extracts a field.
        let job = JsRuleJob {
            id: 0,
            base_url: "https://jsonplaceholder.typicode.com".to_string(),
            result: "".to_string(),
            js_code:
                r#"content = JSON.parse(java.ajax('https://jsonplaceholder.typicode.com/todos/1')).title"#
                    .to_string(),
        };
        let out = execute_rule_batch_async(&[job], &client, &[]).await;
        match out.into_iter().next() {
            Some(o) => {
                let val = o.content.unwrap_or_default();
                println!("bridge live check -> {:?}", val);
                if val == "delectus aut autem" {
                    Ok(())
                } else {
                    anyhow::bail!("unexpected value: {val}")
                }
            }
            None => anyhow::bail!("no output"),
        }
    })
}
