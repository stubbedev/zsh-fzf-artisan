//! Project-wide catalog of well-known Laravel values (class names, config keys,
//! test identifiers, migrations, environments), built once and cached. All
//! purely static — read from the filesystem, no artisan boot.
//!
//! `Catalog::build` computes every set; the caller caches it to disk and
//! invalidates only when the relevant directories change, so a tab press reads
//! a TSV instead of re-parsing every config and test file.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use mago_allocator::LocalArena;
use mago_database::file::FileId;
use mago_span::HasSpan;
use mago_syntax::cst::cst::{
    ArrayElement, AttributeList, Call, Expression, NamespaceBody, PartialArgument, Statement,
};
use mago_syntax::walker::{walk_program, Walker};

use crate::values::{find_sub, lit_str, Kind};

const HTTP_METHODS: &[&str] = &["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "HEAD"];

/// Directories never worth descending into during a scan.
fn is_skippable_dir(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "vendor" | "node_modules")
}

#[derive(Default)]
pub struct Catalog {
    models: Vec<String>,
    seeders: Vec<String>,
    providers: Vec<String>,
    config_keys: Vec<String>,
    queue_connections: Vec<String>,
    db_connections: Vec<String>,
    stores: Vec<String>,
    disks: Vec<String>,
    guards: Vec<String>,
    tests: Vec<String>,
    migrations: Vec<String>,
    envs: Vec<String>,
    events: Vec<String>,
    tables: Vec<String>,
    suites: Vec<String>,
    groups: Vec<String>,
    route_names: Vec<String>,
    queues: Vec<String>,
}

impl Catalog {
    pub fn build(project_dir: &Path) -> Self {
        let (tests, groups) = test_names_and_groups(&project_dir.join("tests"));
        Catalog {
            models: class_stems(&project_dir.join("app/Models")),
            seeders: class_stems(&project_dir.join("database/seeders")),
            providers: provider_fqns(&project_dir.join("app/Providers")),
            config_keys: config_dotted_keys(&project_dir.join("config")),
            queue_connections: config_keys(project_dir, "config/queue.php", "connections"),
            db_connections: config_keys(project_dir, "config/database.php", "connections"),
            stores: config_keys(project_dir, "config/cache.php", "stores"),
            disks: config_keys(project_dir, "config/filesystems.php", "disks"),
            guards: config_keys(project_dir, "config/auth.php", "guards"),
            tests,
            migrations: migration_paths(&project_dir.join("database/migrations")),
            envs: env_names(project_dir),
            events: class_stems(&project_dir.join("app/Events")),
            tables: table_names(&project_dir.join("database/migrations")),
            suites: testsuite_names(project_dir),
            groups,
            route_names: route_names(&project_dir.join("routes")),
            queues: queue_names(project_dir),
        }
    }

    /// Values for a given command + argument/option name.
    pub fn values(&self, subcmd: &str, kind: &Kind, name: &str) -> Vec<String> {
        match name {
            "model" => self.models.clone(),
            "class" if subcmd == "db:seed" => self.seeders.clone(),
            "provider" if subcmd == "vendor:publish" => self.providers.clone(),
            "connection" => self.queue_connections.clone(),
            "database" if *kind == Kind::Option => self.db_connections.clone(),
            "store" => self.stores.clone(),
            "disk" => self.disks.clone(),
            "guard" => self.guards.clone(),
            "config" if subcmd == "config:show" => self.config_keys.clone(),
            "filter" if subcmd == "test" => self.tests.clone(),
            "path" if subcmd.starts_with("migrate") => self.migrations.clone(),
            "method" if subcmd == "route:list" => {
                HTTP_METHODS.iter().map(|s| s.to_string()).collect()
            }
            "env" => self.envs.clone(),
            "event" if subcmd == "make:listener" => self.events.clone(),
            "table" => self.tables.clone(),
            "testsuite" => self.suites.clone(),
            "group" | "exclude-group" if subcmd == "test" => self.groups.clone(),
            "name" if subcmd == "route:list" => self.route_names.clone(),
            "queue" => self.queues.clone(),
            _ => Vec::new(),
        }
    }

    pub fn to_tsv(&self) -> String {
        // Header keeps the file non-empty when every set is empty, so an empty
        // catalog doesn't read back as a stale cache.
        let mut out = String::from("# catalog\n");
        let mut section = |tag: &str, items: &[String]| {
            for v in items {
                // A tab or newline would split the TSV line and later be emitted
                // as a spurious completion candidate — drop such values.
                if !v.contains(['\n', '\t']) {
                    out.push_str(tag);
                    out.push('\t');
                    out.push_str(v);
                    out.push('\n');
                }
            }
        };
        section("model", &self.models);
        section("seeder", &self.seeders);
        section("provider", &self.providers);
        section("configkey", &self.config_keys);
        section("qconn", &self.queue_connections);
        section("dbconn", &self.db_connections);
        section("store", &self.stores);
        section("disk", &self.disks);
        section("guard", &self.guards);
        section("test", &self.tests);
        section("migration", &self.migrations);
        section("env", &self.envs);
        section("event", &self.events);
        section("table", &self.tables);
        section("suite", &self.suites);
        section("group", &self.groups);
        section("route", &self.route_names);
        section("queue", &self.queues);
        out
    }

    pub fn from_tsv(text: &str) -> Self {
        let mut c = Catalog::default();
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let Some((tag, value)) = line.split_once('\t') else {
                continue;
            };
            let bucket = match tag {
                "model" => &mut c.models,
                "seeder" => &mut c.seeders,
                "provider" => &mut c.providers,
                "configkey" => &mut c.config_keys,
                "qconn" => &mut c.queue_connections,
                "dbconn" => &mut c.db_connections,
                "store" => &mut c.stores,
                "disk" => &mut c.disks,
                "guard" => &mut c.guards,
                "test" => &mut c.tests,
                "migration" => &mut c.migrations,
                "env" => &mut c.envs,
                "event" => &mut c.events,
                "table" => &mut c.tables,
                "suite" => &mut c.suites,
                "group" => &mut c.groups,
                "route" => &mut c.route_names,
                "queue" => &mut c.queues,
                _ => continue,
            };
            bucket.push(value.to_string());
        }
        c
    }
}

// --- generic php-file walk -------------------------------------------------

/// Recurse `dir`, calling `f` with each `.php` file path. Uses `file_type()`
/// (no extra stat per entry) and skips dot-dirs, vendor, and node_modules.
fn for_each_php(dir: &Path, f: &mut impl FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            let name = entry.file_name();
            if !is_skippable_dir(&name.to_string_lossy()) {
                for_each_php(&entry.path(), f);
            }
        } else if ft.is_file() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "php") {
                f(&path);
            }
        }
    }
}

// --- class names -----------------------------------------------------------

fn class_stems(dir: &Path) -> Vec<String> {
    let mut out = BTreeSet::new();
    for_each_php(dir, &mut |path| {
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            out.insert(stem.to_string());
        }
    });
    out.into_iter().collect()
}

fn provider_fqns(dir: &Path) -> Vec<String> {
    let mut out = BTreeSet::new();
    for_each_php(dir, &mut |path| {
        if let Ok(src) = fs::read(path) {
            for fqn in class_fqns(&src) {
                out.insert(fqn);
            }
        }
    });
    out.into_iter().collect()
}

fn class_fqns(src: &[u8]) -> Vec<String> {
    let arena = LocalArena::new();
    let program = mago_syntax::parser::parse_file_content(&arena, FileId::new(b"p.php"), src);

    fn walk(stmts: &[Statement], ns: &str, out: &mut Vec<String>) {
        for s in stmts {
            match s {
                Statement::Class(c) => {
                    let name = String::from_utf8_lossy(c.name.value);
                    out.push(if ns.is_empty() {
                        name.into_owned()
                    } else {
                        format!("{ns}\\{name}")
                    });
                }
                Statement::Namespace(n) => {
                    let ns = n
                        .name
                        .as_ref()
                        .map(|id| String::from_utf8_lossy(id.value()).into_owned())
                        .unwrap_or_default();
                    let inner = match &n.body {
                        NamespaceBody::Implicit(b) => b.statements.as_slice(),
                        NamespaceBody::BraceDelimited(b) => b.statements.as_slice(),
                    };
                    walk(inner, &ns, out);
                }
                _ => {}
            }
        }
    }

    let mut out = Vec::new();
    walk(program.statements.as_slice(), "", &mut out);
    out
}

// --- config ----------------------------------------------------------------

/// Top-level string keys of `<top_key> => [...]` inside a config file's
/// returned array, e.g. the connection names in config/database.php.
fn config_keys(project_dir: &Path, rel: &str, top_key: &str) -> Vec<String> {
    let Ok(src) = fs::read(project_dir.join(rel)) else {
        return Vec::new();
    };
    let arena = LocalArena::new();
    let program = mago_syntax::parser::parse_file_content(&arena, FileId::new(b"c.php"), &src);
    for stmt in program.statements.as_slice() {
        let Statement::Return(ret) = stmt else {
            continue;
        };
        let Some(value) = ret.value else { continue };
        if let Some(keys) = nested_keys(unparen(value), top_key) {
            return keys;
        }
    }
    Vec::new()
}

fn nested_keys(expr: &Expression, top_key: &str) -> Option<Vec<String>> {
    for element in array_elements(expr)? {
        let ArrayElement::KeyValue(kv) = element else {
            continue;
        };
        if lit_str(kv.key).as_deref() != Some(top_key) {
            continue;
        }
        let inner = array_elements(unparen(kv.value))?;
        return Some(
            inner
                .iter()
                .filter_map(|e| match e {
                    ArrayElement::KeyValue(kv) => lit_str(kv.key),
                    _ => None,
                })
                .collect(),
        );
    }
    None
}

/// Dotted keys across every config file, e.g. `app`, `app.name`,
/// `database.connections.mysql`. Used for `config:show`. No cap — config trees
/// are bounded in practice and the result is cached.
fn config_dotted_keys(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: BTreeSet<String> = BTreeSet::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "php") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(src) = fs::read(&path) else { continue };
        out.insert(stem.to_string());
        let arena = LocalArena::new();
        let program = mago_syntax::parser::parse_file_content(&arena, FileId::new(b"c.php"), &src);
        for stmt in program.statements.as_slice() {
            if let Statement::Return(ret) = stmt {
                if let Some(value) = ret.value {
                    emit_dotted(unparen(value), stem, &mut out);
                }
            }
        }
    }
    out.into_iter().collect()
}

fn emit_dotted(expr: &Expression, prefix: &str, out: &mut BTreeSet<String>) {
    let Some(elements) = array_elements(expr) else {
        return;
    };
    for element in elements {
        let ArrayElement::KeyValue(kv) = element else {
            continue;
        };
        let Some(key) = lit_str(kv.key) else { continue };
        let dotted = format!("{prefix}.{key}");
        emit_dotted(unparen(kv.value), &dotted, out);
        out.insert(dotted);
    }
}

// --- migrations / env ------------------------------------------------------

fn migration_paths(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.ends_with(".php")
                .then(|| format!("database/migrations/{name}"))
        })
        .collect();
    out.sort();
    out
}

fn env_names(project_dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(project_dir) else {
        return Vec::new();
    };
    let mut out: Vec<String> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let rest = name.strip_prefix(".env.")?;
            (!matches!(rest, "example" | "backup" | "bak" | "dist") && !rest.contains('.'))
                .then(|| rest.to_string())
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

// --- tables / testsuites / routes / queues -----------------------------------

/// After every occurrence of `needle`, capture the immediately following
/// single- or double-quoted literal. Byte scan — no parse, runs only inside the
/// cached catalog build.
fn scan_quoted_calls(src: &[u8], needle: &[u8], out: &mut BTreeSet<String>) {
    let mut from = 0;
    while let Some(pos) = find_sub(&src[from..], needle) {
        let mut i = from + pos + needle.len();
        while src.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
            i += 1;
        }
        if let Some(&q @ (b'\'' | b'"')) = src.get(i) {
            let start = i + 1;
            if let Some(len) = find_sub(&src[start..], &[q]) {
                if let Ok(s) = std::str::from_utf8(&src[start..start + len]) {
                    if !s.is_empty() {
                        out.insert(s.to_string());
                    }
                }
            }
        }
        from = from + pos + needle.len();
    }
}

/// Table names from `Schema::create('x')` in migrations → `db:table`,
/// `make:migration --table=`.
fn table_names(dir: &Path) -> Vec<String> {
    let mut out = BTreeSet::new();
    for_each_php(dir, &mut |path| {
        if let Ok(src) = fs::read(path) {
            scan_quoted_calls(&src, b"Schema::create(", &mut out);
        }
    });
    out.into_iter().collect()
}

/// Route names from `->name('x')` / `Route::name('x')` in routes/ →
/// `route:list --name=`. Group name prefixes (trailing dot) are dropped;
/// names inside prefixed groups come out unprefixed — partial beats booting.
fn route_names(dir: &Path) -> Vec<String> {
    let mut out = BTreeSet::new();
    for_each_php(dir, &mut |path| {
        // routes/console.php names scheduled tasks, not routes.
        if path.file_name().is_some_and(|n| n == "console.php") {
            return;
        }
        if let Ok(src) = fs::read(path) {
            let arena = LocalArena::new();
            let program =
                mago_syntax::parser::parse_file_content(&arena, FileId::new(b"r.php"), &src);
            scan_route_stmts(program.statements.as_slice(), "", &mut out);
        }
    });
    out.into_iter().filter(|n| !n.ends_with('.')).collect()
}

/// Walk route statements, composing `Route::name('admin.')->group(fn)` name
/// prefixes into the names declared inside the group. Each file is scanned
/// independently — prefixes applied via `require`d files are not composed.
fn scan_route_stmts(stmts: &[Statement], prefix: &str, out: &mut BTreeSet<String>) {
    use mago_syntax::cst::cst::ClassLikeMemberSelector;
    for s in stmts {
        match s {
            Statement::Expression(es) => scan_route_expr(es.expression, prefix, out),
            Statement::Block(b) => scan_route_stmts(b.statements.as_slice(), prefix, out),
            Statement::Namespace(ns) => {
                let inner = match &ns.body {
                    NamespaceBody::Implicit(b) => b.statements.as_slice(),
                    NamespaceBody::BraceDelimited(b) => b.statements.as_slice(),
                };
                scan_route_stmts(inner, prefix, out);
            }
            _ => {}
        }
    }

    /// One fluent chain: collect its `name('x')` part and `group(fn)` body,
    /// then either recurse into the group with the composed prefix or emit
    /// the full route name.
    fn scan_route_expr(expr: &Expression, prefix: &str, out: &mut BTreeSet<String>) {
        let mut name_part: Option<String> = None;
        let mut group_body: Option<&Expression> = None;
        let mut cur = expr;
        loop {
            match cur {
                Expression::Parenthesized(p) => cur = p.expression,
                Expression::Call(Call::Method(mc)) => {
                    if let ClassLikeMemberSelector::Identifier(id) = &mc.method {
                        match id.value {
                            // Walking outer→inner: the innermost name() is the
                            // group prefix / route name, so later (inner) wins.
                            b"name" | b"as" => {
                                if let Some(n) = mc
                                    .argument_list
                                    .arguments
                                    .iter()
                                    .next()
                                    .and_then(|a| lit_str(a.value()))
                                {
                                    name_part = Some(n);
                                }
                            }
                            b"group" => {
                                group_body =
                                    mc.argument_list.arguments.iter().next().map(|a| a.value());
                            }
                            _ => {}
                        }
                    }
                    cur = mc.object;
                }
                Expression::Call(Call::StaticMethod(smc)) => {
                    if let ClassLikeMemberSelector::Identifier(id) = &smc.method {
                        match id.value {
                            b"name" | b"as" => {
                                if let Some(n) = smc
                                    .argument_list
                                    .arguments
                                    .iter()
                                    .next()
                                    .and_then(|a| lit_str(a.value()))
                                {
                                    name_part = Some(n);
                                }
                            }
                            b"group" => {
                                group_body =
                                    smc.argument_list.arguments.iter().next().map(|a| a.value());
                            }
                            _ => {}
                        }
                    }
                    break;
                }
                _ => break,
            }
        }
        if let Some(body) = group_body {
            let composed = match &name_part {
                Some(n) => format!("{prefix}{n}"),
                None => prefix.to_string(),
            };
            if let Expression::Closure(c) = unparen(body) {
                scan_route_stmts(c.body.statements.as_slice(), &composed, out);
            }
        } else if let Some(n) = name_part {
            let full = format!("{prefix}{n}");
            if !full.is_empty() {
                out.insert(full);
            }
        }
    }
}

/// `<testsuite name="...">` entries from phpunit.xml(.dist) → `test --testsuite=`.
fn testsuite_names(project_dir: &Path) -> Vec<String> {
    let mut out = BTreeSet::new();
    for f in ["phpunit.xml", "phpunit.xml.dist", "phpunit.dist.xml"] {
        let Ok(src) = fs::read(project_dir.join(f)) else {
            continue;
        };
        let mut from = 0;
        while let Some(pos) = find_sub(&src[from..], b"<testsuite ") {
            let at = from + pos;
            let end = find_sub(&src[at..], b">").map_or(src.len(), |e| at + e);
            let tag = &src[at..end];
            if let Some(np) = find_sub(tag, b"name=\"") {
                let start = np + 6;
                if let Some(len) = find_sub(&tag[start..], b"\"") {
                    if let Ok(s) = std::str::from_utf8(&tag[start..start + len]) {
                        if !s.is_empty() {
                            out.insert(s.to_string());
                        }
                    }
                }
            }
            from = at + 1;
        }
    }
    out.into_iter().collect()
}

/// Queue names from config/queue.php: each connection's `queue` key, unwrapping
/// `env('X', 'default')` to the default → `--queue=`.
fn queue_names(project_dir: &Path) -> Vec<String> {
    let Ok(src) = fs::read(project_dir.join("config/queue.php")) else {
        return Vec::new();
    };
    let arena = LocalArena::new();
    let program = mago_syntax::parser::parse_file_content(&arena, FileId::new(b"q.php"), &src);
    let mut out = BTreeSet::new();
    for stmt in program.statements.as_slice() {
        let Statement::Return(ret) = stmt else {
            continue;
        };
        let Some(value) = ret.value else { continue };
        let Some(elements) = array_elements(unparen(value)) else {
            continue;
        };
        for element in elements {
            let ArrayElement::KeyValue(kv) = element else {
                continue;
            };
            if lit_str(kv.key).as_deref() != Some("connections") {
                continue;
            }
            let Some(conns) = array_elements(unparen(kv.value)) else {
                continue;
            };
            for conn in conns {
                let ArrayElement::KeyValue(c) = conn else {
                    continue;
                };
                let Some(fields) = array_elements(unparen(c.value)) else {
                    continue;
                };
                for field in fields {
                    let ArrayElement::KeyValue(fkv) = field else {
                        continue;
                    };
                    if lit_str(fkv.key).as_deref() == Some("queue") {
                        if let Some(q) = scalar_or_env_default(unparen(fkv.value)) {
                            out.insert(q);
                        }
                    }
                }
            }
        }
    }
    out.into_iter().collect()
}

/// A string literal, or the default argument of `env('X', 'default')`.
fn scalar_or_env_default(expr: &Expression) -> Option<String> {
    if let Some(s) = lit_str(expr) {
        return Some(s);
    }
    let Expression::Call(Call::Function(fc)) = expr else {
        return None;
    };
    let Expression::Identifier(id) = fc.function else {
        return None;
    };
    if !id.last_segment().eq_ignore_ascii_case(b"env") {
        return None;
    }
    fc.argument_list
        .arguments
        .iter()
        .nth(1)
        .and_then(|a| lit_str(a.value()))
}

// --- tests -----------------------------------------------------------------

/// Test identifiers for `test --filter` (class basenames, PHPUnit test methods
/// — name `test*`, `#[Test]` attribute, or `@test` docblock — and Pest
/// `it()`/`test()` descriptions), plus group names for `test --group` from
/// `#[Group('x')]` attributes and `@group x` docblocks.
fn test_names_and_groups(dir: &Path) -> (Vec<String>, Vec<String>) {
    let mut tests: BTreeSet<String> = BTreeSet::new();
    let mut groups: BTreeSet<String> = BTreeSet::new();
    for_each_php(dir, &mut |path| {
        if let Ok(src) = fs::read(path) {
            scan_tests(&src, &mut tests, &mut groups);
        }
    });
    (tests.into_iter().collect(), groups.into_iter().collect())
}

struct TestCtx {
    names: BTreeSet<String>,
    groups: BTreeSet<String>,
    /// `(end_offset, is_test_docblock)` for every docblock, sorted by end.
    docblocks: Vec<(u32, bool)>,
    /// End offset of the previously visited method, so a docblock is only
    /// attributed to a method when it sits after the prior method's body.
    prev_method_end: u32,
}

fn scan_tests(src: &[u8], out: &mut BTreeSet<String>, groups: &mut BTreeSet<String>) {
    let arena = LocalArena::new();
    let program = mago_syntax::parser::parse_file_content(&arena, FileId::new(b"t.php"), src);

    let mut docblocks: Vec<(u32, bool)> = Vec::new();
    for t in program.trivia.iter().filter(|t| t.kind.is_comment()) {
        docblocks.push((t.span.end.offset, contains_test_tag(t.value)));
        collect_group_tags(t.value, groups);
    }
    docblocks.sort_by_key(|(end, _)| *end);

    let mut ctx = TestCtx {
        names: std::mem::take(out),
        groups: std::mem::take(groups),
        docblocks,
        prev_method_end: 0,
    };
    walk_program(&TestScan, program, &mut ctx);
    *out = ctx.names;
    *groups = ctx.groups;
}

/// `@group name` tags in a docblock — group names are project-global, so no
/// attribution to a specific method is needed.
fn collect_group_tags(bytes: &[u8], out: &mut BTreeSet<String>) {
    let mut from = 0;
    while let Some(pos) = find_sub(&bytes[from..], b"@group") {
        let mut i = from + pos + 6;
        let had_space = bytes.get(i).is_some_and(|b| b.is_ascii_whitespace());
        while bytes.get(i).is_some_and(|b| *b == b' ' || *b == b'\t') {
            i += 1;
        }
        let start = i;
        while bytes
            .get(i)
            .is_some_and(|b| !b.is_ascii_whitespace() && *b != b'*' && *b != b'}')
        {
            i += 1;
        }
        if had_space && i > start {
            if let Ok(s) = std::str::from_utf8(&bytes[start..i]) {
                out.insert(s.to_string());
            }
        }
        from = from + pos + 6;
    }
}

/// `#[Group('x')]` attribute values (PHPUnit\Framework\Attributes\Group).
fn collect_group_attrs<'a, 'b>(
    lists: impl Iterator<Item = &'a AttributeList<'b>>,
    out: &mut BTreeSet<String>,
) where
    'b: 'a,
{
    for list in lists {
        for attr in list.attributes.iter() {
            if !attr.name.last_segment().eq_ignore_ascii_case(b"group") {
                continue;
            }
            let Some(args) = &attr.argument_list else {
                continue;
            };
            for arg in args.arguments.iter() {
                let expr = match arg {
                    PartialArgument::Positional(p) => p.value,
                    PartialArgument::Named(n) => n.value,
                    _ => continue,
                };
                if let Some(s) = lit_str(expr) {
                    if !s.is_empty() {
                        out.insert(s);
                    }
                }
            }
        }
    }
}

/// `@test` as a whole docblock tag (not a substring of a longer word).
fn contains_test_tag(bytes: &[u8]) -> bool {
    let mut i = 0;
    while let Some(pos) = bytes[i..].windows(5).position(|w| w == b"@test") {
        let at = i + pos;
        let after = bytes.get(at + 5);
        // Tag boundary: end, whitespace, or `}` (annotation close).
        if after.is_none_or(|c| c.is_ascii_whitespace() || *c == b'}' || *c == b'*') {
            return true;
        }
        i = at + 5;
    }
    false
}

struct TestScan;

impl<'ast, 'arena> Walker<'ast, 'arena, TestCtx> for TestScan {
    fn walk_in_class(&self, class: &'ast mago_syntax::cst::cst::Class<'arena>, ctx: &mut TestCtx) {
        ctx.names
            .insert(String::from_utf8_lossy(class.name.value).into_owned());
        collect_group_attrs(class.attribute_lists.iter(), &mut ctx.groups);
    }

    fn walk_in_method(
        &self,
        method: &'ast mago_syntax::cst::cst::Method<'arena>,
        ctx: &mut TestCtx,
    ) {
        collect_group_attrs(method.attribute_lists.iter(), &mut ctx.groups);
        let name = String::from_utf8_lossy(method.name.value);
        let span = method.span();
        let is_test = (name.starts_with("test") && name.len() > 4)
            || has_test_attribute(method)
            || preceded_by_test_docblock(span.start.offset, ctx.prev_method_end, &ctx.docblocks);
        if is_test {
            ctx.names.insert(name.into_owned());
        }
        ctx.prev_method_end = span.end.offset;
    }

    // Pest: it('...') / test('...') — the first string argument is the name.
    fn walk_in_function_call(
        &self,
        call: &'ast mago_syntax::cst::cst::FunctionCall<'arena>,
        ctx: &mut TestCtx,
    ) {
        let Expression::Identifier(id) = call.function else {
            return;
        };
        let fname = id.last_segment();
        if fname != b"it" && fname != b"test" {
            return;
        }
        if let Some(arg) = call.argument_list.arguments.iter().next() {
            if let Some(desc) = lit_str(arg.value()) {
                if !desc.is_empty() {
                    ctx.names.insert(desc);
                }
            }
        }
    }
}

fn has_test_attribute(method: &mago_syntax::cst::cst::Method) -> bool {
    method.attribute_lists.iter().any(|list| {
        list.attributes
            .iter()
            .any(|attr| attr.name.last_segment().eq_ignore_ascii_case(b"test"))
    })
}

/// True when the closest docblock ending before `start` is a `@test` one and
/// it sits after the previous method's body (so it belongs to this method,
/// not a trailing docblock of an earlier one).
fn preceded_by_test_docblock(start: u32, prev_method_end: u32, docblocks: &[(u32, bool)]) -> bool {
    match docblocks.partition_point(|(end, _)| *end <= start) {
        0 => false,
        i => {
            let (end, is_test) = docblocks[i - 1];
            is_test && end > prev_method_end
        }
    }
}

// --- shared array helpers --------------------------------------------------

fn array_elements<'a>(expr: &'a Expression<'a>) -> Option<&'a [ArrayElement<'a>]> {
    match expr {
        Expression::Array(a) => Some(a.elements.as_slice()),
        Expression::LegacyArray(a) => Some(a.elements.as_slice()),
        _ => None,
    }
}

fn unparen<'a>(expr: &'a Expression<'a>) -> &'a Expression<'a> {
    match expr {
        Expression::Parenthesized(p) => unparen(p.expression),
        _ => expr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_config_keys_and_class_stems() {
        let dir = std::env::temp_dir().join(format!("artisan-comp-wk-test-{}", std::process::id()));
        fs::create_dir_all(dir.join("config")).unwrap();
        fs::create_dir_all(dir.join("app/Models/Billing")).unwrap();
        fs::write(
            dir.join("config/cache.php"),
            r#"<?php
return [
    'default' => env('CACHE_STORE', 'database'),
    'stores' => [
        'array' => ['driver' => 'array'],
        'database' => ['driver' => 'database'],
        'redis' => ['driver' => 'redis'],
    ],
];
"#,
        )
        .unwrap();
        fs::write(dir.join("app/Models/User.php"), "<?php class User {}").unwrap();
        fs::write(
            dir.join("app/Models/Billing/Invoice.php"),
            "<?php class Invoice {}",
        )
        .unwrap();

        let cat = Catalog::build(&dir);
        assert_eq!(
            cat.values("cache:clear", &Kind::Argument, "store"),
            vec!["array", "database", "redis"]
        );
        assert_eq!(
            cat.values("make:controller", &Kind::Option, "model"),
            vec!["Invoice", "User"]
        );
        let dotted = cat.values("config:show", &Kind::Argument, "config");
        for k in [
            "cache",
            "cache.default",
            "cache.stores",
            "cache.stores.redis",
        ] {
            assert!(
                dotted.iter().any(|s| s == k),
                "missing dotted key {k}: {dotted:?}"
            );
        }

        // Round-trips through the on-disk TSV.
        let restored = Catalog::from_tsv(&cat.to_tsv());
        assert_eq!(
            restored.values("config:show", &Kind::Argument, "config"),
            dotted
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detects_test_attribute_and_docblock() {
        let mut out = BTreeSet::new();
        let mut groups = BTreeSet::new();
        scan_tests(
            br#"<?php
class ThingTest extends TestCase
{
    public function testClassic() {}

    #[Test]
    public function itUsesAttribute() {}

    #[PHPUnit\Framework\Attributes\Test]
    public function itUsesFqcnAttribute() {}

    /** @test */
    public function itUsesDocblock() {}

    public function notATest() {}

    /** just a comment */
    public function alsoNotATest() {}
}
"#,
            &mut out,
            &mut groups,
        );
        for t in [
            "testClassic",
            "itUsesAttribute",
            "itUsesFqcnAttribute",
            "itUsesDocblock",
        ] {
            assert!(out.contains(t), "missing test {t}: {out:?}");
        }
        assert!(!out.contains("notATest"));
        assert!(!out.contains("alsoNotATest"));
    }

    #[test]
    fn reads_tests_migrations_env_providers() {
        let dir =
            std::env::temp_dir().join(format!("artisan-comp-wk2-test-{}", std::process::id()));
        fs::create_dir_all(dir.join("tests/Feature")).unwrap();
        fs::create_dir_all(dir.join("database/migrations")).unwrap();
        fs::create_dir_all(dir.join("app/Providers")).unwrap();
        fs::write(
            dir.join("tests/Feature/PestTest.php"),
            r#"<?php
it('adds numbers', function () {});
test('subtracts numbers', function () {});
"#,
        )
        .unwrap();
        fs::write(
            dir.join("database/migrations/2024_01_01_000000_create_users_table.php"),
            "<?php",
        )
        .unwrap();
        fs::write(dir.join(".env.local"), "").unwrap();
        fs::write(dir.join(".env.production"), "").unwrap();
        fs::write(dir.join(".env.example"), "").unwrap();
        fs::write(
            dir.join("app/Providers/AppServiceProvider.php"),
            "<?php\nnamespace App\\Providers;\nclass AppServiceProvider {}",
        )
        .unwrap();

        let cat = Catalog::build(&dir);
        let tests = cat.values("test", &Kind::Option, "filter");
        for t in ["adds numbers", "subtracts numbers"] {
            assert!(
                tests.iter().any(|s| s == t),
                "missing test name {t}: {tests:?}"
            );
        }
        assert_eq!(
            cat.values("migrate", &Kind::Option, "path"),
            vec!["database/migrations/2024_01_01_000000_create_users_table.php"]
        );
        assert_eq!(
            cat.values("app:sync", &Kind::Option, "env"),
            vec!["local", "production"]
        );
        assert_eq!(
            cat.values("vendor:publish", &Kind::Option, "provider"),
            vec!["App\\Providers\\AppServiceProvider"]
        );
        assert!(cat
            .values("route:list", &Kind::Option, "method")
            .iter()
            .any(|s| s == "GET"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_tables_suites_groups_routes_queues_events() {
        let dir =
            std::env::temp_dir().join(format!("artisan-comp-wk3-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("database/migrations")).unwrap();
        fs::create_dir_all(dir.join("routes")).unwrap();
        fs::create_dir_all(dir.join("tests")).unwrap();
        fs::create_dir_all(dir.join("config")).unwrap();
        fs::create_dir_all(dir.join("app/Events")).unwrap();

        fs::write(
            dir.join("database/migrations/2024_01_01_000000_create_users_table.php"),
            r#"<?php
Schema::create('users', function (Blueprint $table) {});
Schema::create("orders", function (Blueprint $table) {});
"#,
        )
        .unwrap();
        fs::write(
            dir.join("routes/web.php"),
            r#"<?php
Route::get('/', HomeController::class)->name('home');
Route::name('admin.')->group(function () {
    Route::get('/admin', AdminController::class)->name('dashboard');
});
"#,
        )
        .unwrap();
        fs::write(
            dir.join("phpunit.xml"),
            r#"<phpunit>
  <testsuites>
    <testsuite name="Unit"><directory>tests/Unit</directory></testsuite>
    <testsuite name="Feature"><directory>tests/Feature</directory></testsuite>
  </testsuites>
</phpunit>
"#,
        )
        .unwrap();
        fs::write(
            dir.join("tests/GroupTest.php"),
            r#"<?php
use PHPUnit\Framework\Attributes\Group;

#[Group('slow')]
class GroupTest extends TestCase
{
    #[Group('integration')]
    public function testOne() {}

    /** @group legacy */
    public function testTwo() {}
}
"#,
        )
        .unwrap();
        fs::write(
            dir.join("config/queue.php"),
            r#"<?php
return [
    'connections' => [
        'redis' => ['driver' => 'redis', 'queue' => env('REDIS_QUEUE', 'default')],
        'database' => ['driver' => 'database', 'queue' => 'jobs'],
    ],
];
"#,
        )
        .unwrap();
        fs::write(
            dir.join("app/Events/UserRegistered.php"),
            "<?php class UserRegistered {}",
        )
        .unwrap();

        let cat = Catalog::build(&dir);
        assert_eq!(
            cat.values("db:table", &Kind::Argument, "table"),
            vec!["orders", "users"]
        );
        assert_eq!(
            cat.values("test", &Kind::Option, "testsuite"),
            vec!["Feature", "Unit"]
        );
        assert_eq!(
            cat.values("test", &Kind::Option, "group"),
            vec!["integration", "legacy", "slow"]
        );
        // Group name prefixes compose into the names declared inside.
        assert_eq!(
            cat.values("route:list", &Kind::Option, "name"),
            vec!["admin.dashboard", "home"]
        );
        assert_eq!(
            cat.values("queue:work", &Kind::Option, "queue"),
            vec!["default", "jobs"]
        );
        assert_eq!(
            cat.values("make:listener", &Kind::Option, "event"),
            vec!["UserRegistered"]
        );

        // Round-trips through the on-disk TSV.
        let restored = Catalog::from_tsv(&cat.to_tsv());
        assert_eq!(
            restored.values("test", &Kind::Option, "group"),
            vec!["integration", "legacy", "slow"]
        );
        assert_eq!(
            restored.values("queue:work", &Kind::Option, "queue"),
            vec!["default", "jobs"]
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
