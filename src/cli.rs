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

        /// Randomly sample N eligible sources instead of the first N —
        /// representative quick tests across the whole pool
        #[arg(long, conflicts_with = "limit")]
        sample: Option<usize>,

        /// Path to config.toml (optional; built-in defaults used if not specified)
        #[arg(long)]
        config: Option<PathBuf>,

        /// Verify the full reading chain (book info → TOC → chapter content)
        /// before marking a source passed. Default: on.
        #[arg(long, default_value = "true", action = clap::ArgAction::Set, num_args = 0..=1, require_equals = true, default_missing_value = "true")]
        verify_content: bool,

        /// Skip the reading-chain content verification (search-only mode).
        #[arg(long)]
        no_verify_content: bool,
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
