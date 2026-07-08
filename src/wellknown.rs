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
use mago_syntax::cst::cst::{ArrayElement, Expression, NamespaceBody, Statement};
use mago_syntax::walker::{walk_program, Walker};

use crate::values::{lit_str, Kind};

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
}

impl Catalog {
    pub fn build(project_dir: &Path) -> Self {
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
            tests: test_names(&project_dir.join("tests")),
            migrations: migration_paths(&project_dir.join("database/migrations")),
            envs: env_names(project_dir),
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
            _ => Vec::new(),
        }
    }

    pub fn to_tsv(&self) -> String {
        // Header keeps the file non-empty when every set is empty, so an empty
        // catalog doesn't read back as a stale cache.
        let mut out = String::from("# catalog\n");
        let mut section = |tag: &str, items: &[String]| {
            for v in items {
                if !v.contains('\n') {
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

// --- tests -----------------------------------------------------------------

/// Test identifiers for `test --filter`: class basenames, PHPUnit test methods
/// (name `test*`, `#[Test]` attribute, or `@test` docblock), and Pest
/// `it()`/`test()` descriptions.
fn test_names(dir: &Path) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    for_each_php(dir, &mut |path| {
        if let Ok(src) = fs::read(path) {
            scan_tests(&src, &mut out);
        }
    });
    out.into_iter().collect()
}

struct TestCtx {
    names: BTreeSet<String>,
    /// `(end_offset, is_test_docblock)` for every docblock, sorted by end.
    docblocks: Vec<(u32, bool)>,
    /// End offset of the previously visited method, so a docblock is only
    /// attributed to a method when it sits after the prior method's body.
    prev_method_end: u32,
}

fn scan_tests(src: &[u8], out: &mut BTreeSet<String>) {
    let arena = LocalArena::new();
    let program = mago_syntax::parser::parse_file_content(&arena, FileId::new(b"t.php"), src);

    let mut docblocks: Vec<(u32, bool)> = program
        .trivia
        .iter()
        .filter(|t| t.kind.is_comment())
        .map(|t| (t.span.end.offset, contains_test_tag(t.value)))
        .collect();
    docblocks.sort_by_key(|(end, _)| *end);

    let mut ctx = TestCtx {
        names: std::mem::take(out),
        docblocks,
        prev_method_end: 0,
    };
    walk_program(&TestScan, program, &mut ctx);
    *out = ctx.names;
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
    }

    fn walk_in_method(
        &self,
        method: &'ast mago_syntax::cst::cst::Method<'arena>,
        ctx: &mut TestCtx,
    ) {
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
}
