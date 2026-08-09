use std::path::PathBuf;

/// Resolve the input book-source JSON path.
/// Priority: 1) CWD data/*.json, 2) XDG default
pub fn resolve_input() -> PathBuf {
    let xdg = {
        let home = || std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let base =
            std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| format!("{}/.local/share", home()));
        PathBuf::from(base)
            .join("legado-source-filter")
            .join("sources.json")
    };
    // Check for project-relative data files (convenience for running from repo root)
    if let Ok(cwd) = std::env::current_dir() {
        let data_dir = cwd.join("data");
        if data_dir.is_dir() {
            // Prefer sources.json, else any .json file
            let named = data_dir.join("sources.json");
            if named.exists() {
                return named;
            }
            if let Ok(entries) = std::fs::read_dir(&data_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|e| e == "json") {
                        return path;
                    }
                }
            }
        }
    }
    xdg
}

/// Resolve the output directory.
/// Priority: 1) explicit --output flag, 2) CWD output/ if it exists or data/
/// exists, 3) XDG default
pub fn resolve_output() -> PathBuf {
    if let Ok(cwd) = std::env::current_dir() {
        let cwd_output = cwd.join("output");
        if cwd_output.is_dir() {
            return cwd_output;
        }
        // If running from repo root with data/ dir, default to output/ even if not yet created
        if cwd.join("data").is_dir() {
            return cwd_output;
        }
    }
    let home = || std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let base = std::env::var("XDG_CACHE_HOME").unwrap_or_else(|_| format!("{}/.cache", home()));
    PathBuf::from(base).join("legado-source-filter")
}
