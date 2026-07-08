//! Completion engine for zsh-fzf-artisan.
//!
//! Invoked by the zsh shim as:
//!   artisan-comp complete --cwd "$PWD" --current $CURRENT -- "${words[@]}"
//!
//! Prints a prompt title on the first line and tab-separated
//! "candidate<TAB>description" items on the following lines. Exits non-zero
//! when it cannot function (no artisan, php failed with no usable cache) so
//! the shim can fall back to its pure-zsh path.

mod native;
mod values;
mod wellknown;

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::OnceLock;
use std::time::SystemTime;

use serde_json::Value;

use values::Kind;

/// Global options present on every artisan command — filtered from args completions.
const GLOBAL_OPTS: &[&str] = &[
    "help",
    "quiet",
    "verbose",
    "version",
    "ansi",
    "no-ansi",
    "no-interaction",
    "env",
];

const SKIP_ARGS: &[&str] = &["command"];

pub(crate) struct Project {
    pub artisan: PathBuf,
    pub dir: PathBuf,
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("complete") => {}
        Some("version") => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("usage: artisan-comp complete --cwd DIR --current N -- WORDS...");
            return ExitCode::from(2);
        }
    }

    let mut cwd = None;
    let mut current = 0usize;
    let mut words: Vec<String> = Vec::new();
    let mut it = args.into_iter().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--cwd" => cwd = it.next(),
            "--current" => current = it.next().and_then(|v| v.parse().ok()).unwrap_or(0),
            "--" => {
                words = it.collect();
                break;
            }
            _ => {}
        }
    }
    let cwd = cwd.map(PathBuf::from).or_else(|| env::current_dir().ok());

    let Some(cwd) = cwd else {
        return ExitCode::FAILURE;
    };
    match run(&cwd, current, &words) {
        Some(output) => {
            let mut stdout = std::io::stdout().lock();
            let _ = stdout.write_all(output.as_bytes());
            ExitCode::SUCCESS
        }
        None => ExitCode::FAILURE,
    }
}

fn run(cwd: &Path, current: usize, words: &[String]) -> Option<String> {
    let project = find_artisan(cwd)?;
    let cache_dir = cache_dir();
    let _ = fs::create_dir_all(&cache_dir);
    prune_cache(&cache_dir);
    let project_hash = fnv_hex(project.dir.as_os_str().as_encoded_bytes());

    if current <= 2 {
        complete_commands(&project, &cache_dir, &project_hash)
    } else {
        complete_args(&project, &cache_dir, &project_hash, current, words)
    }
}

/// Delete cache entries not touched in 30 days so `~/.cache/artisan` doesn't
/// grow unbounded as projects come and go. Cheap: one readdir of a small dir.
fn prune_cache(cache_dir: &Path) {
    const MAX_AGE: std::time::Duration = std::time::Duration::from_secs(30 * 24 * 60 * 60);
    let now = SystemTime::now();
    let Ok(entries) = fs::read_dir(cache_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let stale = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > MAX_AGE);
        if stale {
            let _ = fs::remove_file(&path);
        }
    }
}

/// Write via a temp file + rename so a concurrent reader (another shell tabbing
/// the same project) never sees a half-written cache.
fn write_atomic(path: &Path, data: &[u8]) {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    if fs::write(&tmp, data).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

fn complete_commands(project: &Project, cache_dir: &Path, project_hash: &str) -> Option<String> {
    let list = load_list(project, cache_dir, project_hash)?;
    Some(format!("Artisan Command\n{}", command_lines(&list)))
}

/// Cached `artisan list --format=json`. This single php run carries the full
/// definition (arguments + options) of every command, so no per-command
/// `help` boots are ever needed.
fn load_list(project: &Project, cache_dir: &Path, project_hash: &str) -> Option<Value> {
    let cache_file = cache_dir.join(format!("{project_hash}.list.json"));
    let raw = ensure_cache(&cache_file, newest_command_source(project), || {
        php_run(project, &["list", "--format=json"])
    })?;
    serde_json::from_slice(&raw).ok()
}

/// Well-known catalog, cached to a TSV that regenerates only when a catalog
/// source directory changes — so config/test parsing runs on change, not per tab.
fn load_catalog(project: &Project, cache_dir: &Path, project_hash: &str) -> wellknown::Catalog {
    let cache_file = cache_dir.join(format!("{project_hash}.catalog"));
    if !is_stale(&cache_file, newest_catalog_source(project)) {
        if let Ok(text) = fs::read_to_string(&cache_file) {
            return wellknown::Catalog::from_tsv(&text);
        }
    }
    let catalog = wellknown::Catalog::build(&project.dir);
    write_atomic(&cache_file, catalog.to_tsv().as_bytes());
    catalog
}

fn command_lines(list: &Value) -> String {
    let mut out = String::new();
    let Some(commands) = list.get("commands").and_then(Value::as_array) else {
        return out;
    };
    for cmd in commands {
        let name = cmd.get("name").and_then(Value::as_str).unwrap_or("");
        if name.is_empty() || name.starts_with('_') {
            continue;
        }
        let desc = cmd.get("description").and_then(Value::as_str).unwrap_or("");
        out.push_str(&format!("{name}\t{}\n", clean(desc)));
    }
    out
}

/// Source-extracted values, cached as a TSV sidecar with the same staleness
/// rules as the list cache — command sources are read and parsed only when
/// something changed, not on every tab press.
fn load_values(
    project: &Project,
    cache_dir: &Path,
    project_hash: &str,
    subcmd: &str,
) -> values::Values {
    let cache_file = cache_dir.join(format!(
        "{project_hash}_{}.vals",
        fnv_hex(subcmd.as_bytes())
    ));
    if !is_stale(&cache_file, newest_command_source(project)) {
        if let Ok(text) = fs::read_to_string(&cache_file) {
            let mut vals = values::Values::new();
            for line in text.lines().filter(|l| !l.starts_with('#')) {
                let mut parts = line.splitn(3, '\t');
                if let (Some(kind), Some(name), Some(value)) =
                    (parts.next(), parts.next(), parts.next())
                {
                    let kind = if kind == "option" {
                        Kind::Option
                    } else {
                        Kind::Argument
                    };
                    vals.entry((kind, name.to_string()))
                        .or_default()
                        .push(value.to_string());
                }
            }
            return vals;
        }
    }
    let vals = values::extract(&project.dir, subcmd);
    // Header line keeps the file non-empty when nothing was found, so an
    // empty result doesn't read as a stale cache.
    let mut text = String::from("# artisan-comp values\n");
    for ((kind, name), vs) in &vals {
        let kind = match kind {
            Kind::Argument => "argument",
            Kind::Option => "option",
        };
        for v in vs {
            text.push_str(&format!("{kind}\t{name}\t{v}\n"));
        }
    }
    write_atomic(&cache_file, text.as_bytes());
    vals
}

fn complete_args(
    project: &Project,
    cache_dir: &Path,
    project_hash: &str,
    current: usize,
    words: &[String],
) -> Option<String> {
    let subcmd = words.get(1)?.clone();
    if subcmd.is_empty() {
        return None;
    }

    let list = load_list(project, cache_dir, project_hash)?;
    let command = list
        .get("commands")
        .and_then(Value::as_array)
        .and_then(|cmds| {
            cmds.iter()
                .find(|c| c.get("name").and_then(Value::as_str) == Some(&subcmd))
        });
    let Some(def) = command.and_then(|c| c.get("definition")) else {
        // Namespace prefix or unknown command — offer prefix-filtered command list.
        let prefix = format!("{subcmd}:");
        let items: String = command_lines(&list)
            .lines()
            .filter(|l| l.starts_with(&prefix))
            .map(|l| format!("{l}\n"))
            .collect();
        if items.is_empty() {
            return Some(String::new());
        }
        return Some(format!("Artisan Namespace\n{items}"));
    };

    let vals = load_values(project, cache_dir, project_hash, &subcmd);
    let catalog = load_catalog(project, cache_dir, project_hash);
    // Values from the command source, then the cached well-known catalog
    // (models, seeders, config keys, test names, migrations, …).
    let combined = |kind: Kind, name: &str| -> Vec<String> {
        let mut v = vals
            .get(&(kind.clone(), name.to_string()))
            .cloned()
            .unwrap_or_default();
        for x in catalog.values(&subcmd, &kind, name) {
            if !v.contains(&x) {
                v.push(x);
            }
        }
        v
    };

    // Option lexicon: "--mode"/"-m" → definition key, for options that take a value.
    let mut value_opts: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    if let Some(opts) = def.get("options").and_then(Value::as_object) {
        for (key, opt) in opts {
            if !opt.is_object()
                || !opt
                    .get("accept_value")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            {
                continue;
            }
            if let Some(name) = opt.get("name").and_then(Value::as_str) {
                value_opts.insert(name, key);
            }
            match opt.get("shortcut").and_then(Value::as_str) {
                Some(s) if !s.is_empty() => {
                    value_opts.insert(s, key);
                }
                _ => {}
            }
        }
    }

    // Words already typed between the subcommand and the cursor.
    let prior_words: Vec<&str> = words
        .iter()
        .take(current.saturating_sub(1))
        .skip(2)
        .map(String::as_str)
        .collect();

    let (filled, in_positional_only) =
        scan_prior_words(&prior_words, |w| value_opts.contains_key(w));
    let current_word = current.checked_sub(1).and_then(|i| words.get(i));

    // Static values for an option, with an opt-in native `_complete` fallback
    // when the static sources find nothing (route names, queue names, tags…).
    let option_values = |key: &str| -> Vec<String> {
        let vals = combined(Kind::Option, key);
        if vals.is_empty() && native::enabled() {
            native::complete(project, words, current)
        } else {
            vals
        }
    };

    // Typing an option value inline: `--opt=<partial>` / `-m=<partial>`.
    if !in_positional_only {
        if let Some(word) = current_word {
            if word.starts_with('-') {
                if let Some((opt_word, _)) = word.split_once('=') {
                    if let Some(&key) = value_opts.get(opt_word) {
                        let opt_vals = option_values(key);
                        if !opt_vals.is_empty() {
                            let mut out = format!("Artisan {opt_word}\n");
                            for v in opt_vals {
                                out.push_str(&format!("{opt_word}={v}\tvalue for --{key}\n"));
                            }
                            return Some(out);
                        }
                    }
                }
            }
        }
        // Space-form value: the previous word is a bare value-taking option
        // (`artisan cmd --mode <Tab>`), so offer that option's values plainly.
        let previous_word = current.checked_sub(2).and_then(|i| words.get(i));
        if let Some(prev) = previous_word {
            if current_word.is_none_or(|w| !w.starts_with('-')) {
                if let Some(&key) = value_opts.get(prev.as_str()) {
                    let opt_vals = option_values(key);
                    if !opt_vals.is_empty() {
                        let mut out = format!("Artisan --{key}\n");
                        for v in opt_vals {
                            out.push_str(&format!("{v}\tvalue for --{key}\n"));
                        }
                        return Some(out);
                    }
                }
            }
        }
    }

    let mut out = String::from("Artisan Args\n");

    if let Some(args) = def.get("arguments").and_then(Value::as_object) {
        let mut position = 0usize;
        for (key, arg) in args {
            if !arg.is_object() || SKIP_ARGS.contains(&key.as_str()) {
                continue;
            }
            let is_array = arg
                .get("is_array")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            // Skip positionals already supplied on the line (array arguments
            // absorb the rest, so they always stay offered).
            let already_filled = position < filled && !is_array;
            position += 1;
            if already_filled {
                continue;
            }
            let suffix = if is_array {
                "..."
            } else if !arg
                .get("is_required")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "?"
            } else {
                ""
            };
            let desc = arg.get("description").and_then(Value::as_str).unwrap_or("");
            out.push_str(&format!("<{key}{suffix}>\t{}{}\n", clean(desc), hints(arg)));
            // Known values, right below their argument.
            for v in combined(Kind::Argument, key) {
                out.push_str(&format!("{v}\t<{key}> value\n"));
            }
        }
    }

    // After a bare `--`, options can no longer be passed — offer none.
    let opts = if in_positional_only {
        None
    } else {
        def.get("options").and_then(Value::as_object)
    };
    if let Some(opts) = opts {
        for (key, opt) in opts {
            if !opt.is_object() || GLOBAL_OPTS.contains(&key.as_str()) {
                continue;
            }
            let accepts = opt
                .get("accept_value")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let eq = if accepts { "=" } else { "" };
            let name = opt.get("name").and_then(Value::as_str).unwrap_or("");
            let shortcut = opt.get("shortcut").and_then(Value::as_str).unwrap_or("");
            // Drop options already present on the line, unless repeatable.
            let multiple = opt
                .get("is_multiple")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if !multiple && option_used(&prior_words, name, shortcut) {
                continue;
            }
            let desc = clean(opt.get("description").and_then(Value::as_str).unwrap_or(""));
            let hint = hints(opt);
            let shortcut_prefix = if shortcut.is_empty() {
                String::new()
            } else {
                format!("({shortcut}) ")
            };
            out.push_str(&format!("{name}{eq}\t{shortcut_prefix}{desc}{hint}\n"));
            if !shortcut.is_empty() {
                out.push_str(&format!("{shortcut}{eq}\t{desc}{hint}\n"));
            }
        }
    }

    Some(out)
}

fn hints(v: &Value) -> String {
    let mut h = String::new();
    let accepts = v
        .get("accept_value")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let required = v
        .get("is_value_required")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    if accepts && !required {
        h.push_str(" [optional value]");
    }
    if v.get("is_multiple")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        h.push_str(" [repeatable]");
    }
    if let Some(d) = v.get("default") {
        let display = match d {
            Value::Null | Value::Bool(false) | Value::Array(_) | Value::Object(_) => None,
            Value::String(s) if s.is_empty() => None,
            Value::String(s) => Some(s.clone()),
            Value::Bool(true) => Some("true".to_string()),
            Value::Number(n) => Some(n.to_string()),
        };
        if let Some(d) = display {
            h.push_str(&format!(" (default: {d})"));
        }
    }
    h
}

fn clean(s: &str) -> String {
    s.replace(['\n', '\t'], " ")
}

// --- line analysis ---------------------------------------------------------

/// Given the words between the subcommand and the cursor, return how many
/// positional arguments are already filled and whether a bare `--` has switched
/// parsing into positional-only mode. `takes_value(word)` reports whether an
/// option token consumes the following word as its value (`--opt value`).
fn scan_prior_words(prior_words: &[&str], takes_value: impl Fn(&str) -> bool) -> (usize, bool) {
    let separator = prior_words.iter().position(|w| *w == "--");
    let mut filled = 0usize;
    let mut consumed_by_option = false;
    for (i, w) in prior_words.iter().enumerate() {
        if consumed_by_option {
            consumed_by_option = false;
            continue;
        }
        let after_separator = separator.is_some_and(|s| i > s);
        if !after_separator && w.starts_with('-') {
            consumed_by_option = takes_value(w);
            continue;
        }
        if !w.is_empty() && *w != "--" {
            filled += 1;
        }
    }
    (filled, separator.is_some())
}

/// Whether an option (by long name and shortcut) already appears on the line,
/// in either `--opt`/`-o` or `--opt=x`/`-o=x` form.
fn option_used(prior_words: &[&str], name: &str, shortcut: &str) -> bool {
    let hit = |w: &str, flag: &str| {
        !flag.is_empty() && (w == flag || w.strip_prefix(flag).is_some_and(|r| r.starts_with('=')))
    };
    prior_words.iter().any(|w| hit(w, name) || hit(w, shortcut))
}

// --- cache -----------------------------------------------------------------

/// Return cache contents, regenerating when stale relative to `newest` (the
/// newest mtime of the sources this cache depends on). A regeneration failure
/// falls back to the existing cache file if one is present.
fn ensure_cache(
    cache_file: &Path,
    newest: Option<SystemTime>,
    regen: impl FnOnce() -> Option<Vec<u8>>,
) -> Option<Vec<u8>> {
    if !is_stale(cache_file, newest) {
        return fs::read(cache_file).ok();
    }
    match regen() {
        Some(data) if !data.is_empty() => {
            write_atomic(cache_file, &data);
            Some(data)
        }
        _ => fs::read(cache_file).ok().filter(|d| !d.is_empty()),
    }
}

/// Stale when the cache is missing/empty or any watched source is newer.
fn is_stale(cache_file: &Path, newest: Option<SystemTime>) -> bool {
    let Some(cache_time) = mtime(cache_file) else {
        return true;
    };
    if fs::metadata(cache_file)
        .map(|m| m.len() == 0)
        .unwrap_or(true)
    {
        return true;
    }
    newest.is_some_and(|src| src > cache_time)
}

/// Newest mtime of command-definition sources (drives the list.json and .vals
/// caches): artisan itself, composer.lock, routes/console.php, bootstrap/app.php,
/// and every `.php` under any `Console` directory in `app/`. Computed once per
/// process — a completion run touches one project.
fn newest_command_source(project: &Project) -> Option<SystemTime> {
    static CACHE: OnceLock<Option<SystemTime>> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let mut newest = None;
        for p in [
            project.artisan.clone(),
            project.dir.join("composer.lock"),
            project.dir.join("routes/console.php"),
            project.dir.join("bootstrap/app.php"),
        ] {
            bump(&mut newest, mtime(&p));
        }
        bump(&mut newest, newest_console_php(&project.dir.join("app")));
        newest
    })
}

/// Newest mtime of the well-known catalog sources (drives the .catalog cache),
/// kept separate from command sources so editing a test doesn't force an
/// artisan re-list. Computed once per process.
fn newest_catalog_source(project: &Project) -> Option<SystemTime> {
    static CACHE: OnceLock<Option<SystemTime>> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let mut newest = None;
        for rel in [
            "config",
            "tests",
            "database/migrations",
            "database/seeders",
            "app/Models",
            "app/Providers",
        ] {
            bump(&mut newest, newest_php_in(&project.dir.join(rel)));
        }
        // .env.* files live in the project root.
        if let Ok(entries) = fs::read_dir(&project.dir) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().starts_with(".env.") {
                    bump(
                        &mut newest,
                        entry.metadata().and_then(|m| m.modified()).ok(),
                    );
                }
            }
        }
        newest
    })
}

fn bump(newest: &mut Option<SystemTime>, candidate: Option<SystemTime>) {
    if let Some(t) = candidate {
        if newest.is_none_or(|n| t > n) {
            *newest = Some(t);
        }
    }
}

/// Newest `.php` mtime under any `Console` directory in the tree.
fn newest_console_php(dir: &Path) -> Option<SystemTime> {
    let mut newest = None;
    let Ok(entries) = fs::read_dir(dir) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let found = if path.file_name().is_some_and(|n| n == "Console") {
            newest_php_in(&path)
        } else {
            newest_console_php(&path)
        };
        bump(&mut newest, found);
    }
    newest
}

fn newest_php_in(dir: &Path) -> Option<SystemTime> {
    let mut newest = None;
    let Ok(entries) = fs::read_dir(dir) else {
        return None;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            bump(&mut newest, newest_php_in(&path));
        } else if path.extension().is_some_and(|e| e == "php") {
            bump(&mut newest, mtime(&path));
        }
    }
    newest
}

fn mtime(p: &Path) -> Option<SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn cache_dir() -> PathBuf {
    if let Ok(dir) = env::var("ARTISAN_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    let home = env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".cache/artisan")
}

// --- helpers ---------------------------------------------------------------

fn find_artisan(cwd: &Path) -> Option<Project> {
    let mut dir = cwd.canonicalize().ok()?;
    loop {
        let artisan = dir.join("artisan");
        if artisan.is_file() {
            return Some(Project { artisan, dir });
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn php_run(project: &Project, args: &[&str]) -> Option<Vec<u8>> {
    let php = env::var("_ARTISAN_PHP_BIN").unwrap_or_else(|_| "php".into());
    let out = Command::new(php)
        .arg(&project.artisan)
        .args(args)
        .current_dir(&project.dir)
        .output()
        .ok()?;
    if !out.status.success() || out.stdout.is_empty() {
        return None;
    }
    Some(out.stdout)
}

fn fnv_hex(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // Options that take a value in these cases: --mode/-m, --queue.
    fn takes_value(w: &str) -> bool {
        HashSet::from(["--mode", "-m", "--queue"]).contains(w)
    }

    #[test]
    fn counts_plain_positionals() {
        let (filled, positional_only) = scan_prior_words(&["github", "web"], takes_value);
        assert_eq!(filled, 2);
        assert!(!positional_only);
    }

    #[test]
    fn trailing_empty_word_is_not_a_positional() {
        // `artisan app:sync <cursor>` — the empty current word is excluded upstream,
        // but a stray empty token must never inflate the count.
        let (filled, _) = scan_prior_words(&["github", ""], takes_value);
        assert_eq!(filled, 1);
    }

    #[test]
    fn value_taking_option_consumes_next_word() {
        // `--mode fast source` → mode consumes "fast"; only "source" is positional.
        let (filled, _) = scan_prior_words(&["--mode", "fast", "source"], takes_value);
        assert_eq!(filled, 1);
        // Inline form doesn't consume a following word.
        let (filled, _) = scan_prior_words(&["--mode=fast", "source"], takes_value);
        assert_eq!(filled, 1);
        // A boolean option consumes nothing.
        let (filled, _) = scan_prior_words(&["--force", "source"], takes_value);
        assert_eq!(filled, 1);
    }

    #[test]
    fn double_dash_switches_to_positional_only() {
        // After `--`, dash-words count as positionals and option parsing stops.
        let (filled, positional_only) = scan_prior_words(&["--", "-x", "value"], takes_value);
        assert_eq!(filled, 2);
        assert!(positional_only);
    }

    #[test]
    fn option_used_matches_long_short_and_inline() {
        assert!(option_used(&["--mode"], "--mode", "-m"));
        assert!(option_used(&["-m"], "--mode", "-m"));
        assert!(option_used(&["--mode=fast"], "--mode", "-m"));
        assert!(option_used(&["-m=fast"], "--mode", "-m"));
        assert!(!option_used(&["--modeless"], "--mode", "-m"));
        assert!(!option_used(&["source"], "--mode", "-m"));
        // Empty shortcut must never match a bare word.
        assert!(!option_used(&["source"], "--force", ""));
    }
}
