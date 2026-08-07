use std::io::Write;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "legado-source-filter", about = "筛选 Legado 可用书源")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Emit machine-readable JSON Lines to stdout
    #[arg(long, global = true)]
    pub json: bool,

    /// Output directory (default: XDG_CACHE_HOME/legado-source-filter)
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Show current pipeline status from cache (no network)
    Status,

    /// One-shot: preflight + test with N rounds
    Full {
        #[arg(short, long, default_value = "50")]
        concurrency: usize,

        #[arg(short, long, default_value = "15")]
        timeout: u64,

        /// Re-test all sources, ignoring cache
        #[arg(long)]
        force: bool,

        /// Number of test rounds. Failed sources are retried each round.
        #[arg(long, default_value = "1")]
        rounds: u32,

        /// Limit to first N sources (for quick tests)
        #[arg(long)]
        limit: Option<usize>,

        /// Path to config.toml (optional; built-in defaults used if not specified)
        #[arg(long)]
        config: Option<PathBuf>,

        /// Also verify ruleContent extracts readable content from the first
        /// search result's book page before marking a source as passed
        #[arg(long)]
        verify_content: bool,
    },
}

/// Emit a JSON event to stdout if --json is enabled.
pub fn json_event(cli: &Cli, value: serde_json::Value) {
    if !cli.json {
        return;
    }
    println!("{}", serde_json::to_string(&value).unwrap());
    let _ = std::io::stdout().flush();
}
