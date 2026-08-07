//! Regression check: parse → serialize must be a faithful round-trip for the
//! full source feed, preserving every field (including unmodeled rule fields).
//! Run: `cargo run --example rt_check`

use legado_source_filter::types::BookSource;

fn main() {
    let raw = std::fs::read_to_string("data/b778fe6b.json")
        .expect("source feed missing; run the fetcher first");
    let list: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
    let mut missing: std::collections::BTreeMap<String, usize> = Default::default();
    let mut diff: std::collections::BTreeMap<String, usize> = Default::default();

    for v in &list {
        let s: BookSource = serde_json::from_value(v.clone()).unwrap();
        let back = serde_json::to_value(&s).unwrap();
        for (k, vv) in v.as_object().unwrap() {
            match back.get(k) {
                None => *missing.entry(k.clone()).or_default() += 1,
                Some(b) if b != vv => *diff.entry(k.clone()).or_default() += 1,
                _ => {}
            }
        }
    }

    if missing.is_empty() && diff.is_empty() {
        println!("round-trip OK for {} sources", list.len());
        return;
    }
    for (k, c) in &missing {
        eprintln!("MISSING {c:5}  {k}");
    }
    for (k, c) in &diff {
        eprintln!("DIFF    {c:5}  {k}");
    }
    std::process::exit(1);
}
