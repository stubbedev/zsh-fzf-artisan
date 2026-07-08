//! Opt-in bridge to Symfony's hidden `_complete` command (what
//! `artisan completion` uses). Yields runtime-only values a static parse can't
//! know — route names, queue names, publish tags, option `suggestedValues` —
//! at the cost of one artisan boot per query. Enabled with ARTISAN_COMP_NATIVE=1
//! and only consulted when the static sources return nothing.

use std::env;
use std::process::Command;

use crate::Project;

pub fn enabled() -> bool {
    matches!(
        env::var("ARTISAN_COMP_NATIVE").ok().as_deref(),
        Some("1" | "true" | "yes")
    )
}

/// Query `_complete` for the cursor position. `words` is argv-style
/// (words[0] = "artisan"); `current` is the 1-based cursor index used
/// everywhere else, converted here to Symfony's 0-based `--current`.
pub fn complete(project: &Project, words: &[String], current: usize) -> Vec<String> {
    let php = env::var("_ARTISAN_PHP_BIN").unwrap_or_else(|_| "php".into());
    let mut cmd = Command::new(php);
    cmd.arg(&project.artisan)
        .args(["_complete", "--no-interaction", "-sbash", "-a1"])
        .arg(format!("-c{}", current.saturating_sub(1)))
        .current_dir(&project.dir);
    for w in words {
        cmd.arg("-i").arg(w);
    }

    let Ok(out) = cmd.output() else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    let mut seen = Vec::new();
    for line in String::from_utf8_lossy(&out.stdout).lines() {
        // Bash output is one suggestion per line; guard against any tab-tagged
        // descriptions leaking in from other writers.
        let value = line.split('\t').next().unwrap_or("").trim();
        if !value.is_empty() && !seen.iter().any(|v| v == value) {
            seen.push(value.to_string());
        }
    }
    seen
}
