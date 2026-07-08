//! Extracts candidate values for artisan command arguments/options by parsing
//! the command's PHP source with mago-syntax and collecting string literals the
//! code compares against: `===`/`==`/`!=`/`!==`, `in_array()` (negated or not),
//! `match`, `switch` — through one level of variable aliasing
//! (`$x = $this->argument('name')`), same-file class constants
//! (`self::MODES`), and backed enums (`Source::Github->value`,
//! `Source::tryFrom(...)`, `Source::cases()`).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use mago_allocator::LocalArena;
use mago_database::file::FileId;
use mago_syntax::cst::cst::{
    Access, Argument, Call, ClassLikeConstant, ClassLikeConstantSelector, ClassLikeMember,
    ClassLikeMemberSelector, EnumCaseItem, Expression, Literal, MatchArm, NamespaceBody,
    Statement, SwitchCase, Variable,
};
use mago_syntax::walker::{walk_program, Walker};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Kind {
    Argument,
    Option,
}

pub type RefKey = (Kind, String);
pub type Values = HashMap<RefKey, Vec<String>>;

pub fn extract(project_dir: &Path, cmd: &str) -> Values {
    let mut out = Values::new();
    for path in candidate_files(project_dir) {
        // Single read per candidate: the same bytes serve the
        // defines-command check and the extraction parse.
        let Ok(src) = fs::read(&path) else { continue };
        if defines_command(&src, cmd) {
            extract_from(&src, project_dir, &mut out);
        }
    }
    out
}

/// PHP files that may define commands: every `.php` under any directory named
/// `Console` anywhere in `app/` (covers app/Console/Commands, module layouts
/// like app/Billing/Console/Commands, and app/Modules/**/Console), plus
/// routes/console.php. Matches the source set watched for cache staleness.
fn candidate_files(project_dir: &Path) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    collect_console_php(&project_dir.join("app"), &mut candidates);
    candidates.push(project_dir.join("routes/console.php"));
    candidates
}

fn collect_php_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_php_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "php") {
            out.push(path);
        }
    }
}

/// Recursively find directories named `Console` under `dir` and collect every
/// `.php` beneath each — so only command subtrees are read, not the whole app.
pub fn collect_console_php(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if path.file_name().is_some_and(|n| n == "Console") {
            collect_php_files(&path, out);
        } else {
            collect_console_php(&path, out);
        }
    }
}

/// Heuristic: a quoted occurrence of the command name near a signature/name/command() definition.
fn defines_command(src: &[u8], cmd: &str) -> bool {
    let needle = cmd.as_bytes();
    if needle.is_empty() {
        return false;
    }
    let mut from = 0;
    while let Some(pos) = find_sub(&src[from..], needle) {
        let at = from + pos;
        let before = at.checked_sub(1).map(|i| src[i]);
        let after = src.get(at + needle.len());
        if matches!(before, Some(b'\'' | b'"'))
            && matches!(after, Some(b'\'' | b'"' | b' ' | b'{' | b'\n' | b'\t'))
        {
            let window = &src[at.saturating_sub(300)..at];
            if find_sub(window, b"signature").is_some()
                || find_sub(window, b"$name").is_some()
                || find_sub(window, b"command(").is_some()
                || find_sub(window, b"AsCommand").is_some()
            {
                return true;
            }
        }
        from = at + 1;
    }
    false
}

fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn extract_from(src: &[u8], project_dir: &Path, out: &mut Values) {
    let arena = LocalArena::new();
    let program = mago_syntax::parser::parse_file_content(&arena, FileId::new(b"cmd.php"), src);

    let mut ctx = Ctx {
        aliases: HashMap::new(),
        consts: HashMap::new(),
        uses: parse_uses(src),
        enums: HashMap::new(),
        project_dir: project_dir.to_path_buf(),
        out: std::mem::take(out),
        collect: false,
    };
    // Pass 1 records aliases and class constants, pass 2 collects values — so
    // a definition after its use (rare, but legal) still resolves.
    walk_program(&Extractor, program, &mut ctx);
    ctx.collect = true;
    walk_program(&Extractor, program, &mut ctx);
    *out = ctx.out;
}

/// `use App\Enums\Source;` / `use App\Enums\Source as Src;` → short name → FQN.
/// ponytail: line-based, group imports (`use X\{A, B}`) not expanded.
fn parse_uses(src: &[u8]) -> HashMap<String, String> {
    let text = String::from_utf8_lossy(src);
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix("use ") else { continue };
        let Some(rest) = rest.strip_suffix(';') else { continue };
        if rest.contains('{') || rest.starts_with("function ") || rest.starts_with("const ") {
            continue;
        }
        let (fqn, alias) = match rest.split_once(" as ") {
            Some((f, a)) => (f.trim(), a.trim()),
            None => (rest.trim(), rest.trim().rsplit('\\').next().unwrap_or(rest)),
        };
        map.insert(alias.to_string(), fqn.trim_start_matches('\\').to_string());
    }
    map
}

struct Ctx {
    aliases: HashMap<Vec<u8>, RefKey>,
    /// Same-file class constants: NAME → string values (scalar = one, array = many).
    consts: HashMap<String, Vec<String>>,
    uses: HashMap<String, String>,
    /// Memoized enum lookups: written name → case name → backed value.
    enums: HashMap<String, Option<HashMap<String, String>>>,
    project_dir: PathBuf,
    out: Values,
    collect: bool,
}

impl Ctx {
    fn add(&mut self, key: RefKey, value: String) {
        if value.is_empty() || value.contains('\t') || value.contains('\n') {
            return;
        }
        let entry = self.out.entry(key).or_default();
        if !entry.iter().any(|v| v == &value) {
            entry.push(value);
        }
    }

    fn add_all(&mut self, key: &RefKey, values: Vec<String>) {
        for v in values {
            self.add(key.clone(), v);
        }
    }

    /// Resolve a written enum name (via use statements, PSR-4 App\ → app/) to
    /// its backed case values. ponytail: App\ prefix only; other autoload
    /// roots (packages, custom PSR-4 maps) resolve to nothing.
    fn enum_cases(&mut self, written: &str) -> Option<HashMap<String, String>> {
        if let Some(memo) = self.enums.get(written) {
            return memo.clone();
        }
        let fqn = self
            .uses
            .get(written)
            .cloned()
            .or_else(|| written.contains('\\').then(|| written.trim_start_matches('\\').to_string()));
        let result = fqn.and_then(|fqn| {
            let rel = fqn.strip_prefix("App\\")?.replace('\\', "/");
            let short = fqn.rsplit('\\').next().unwrap_or(&fqn).to_string();
            load_enum_cases(&self.project_dir.join("app").join(format!("{rel}.php")), &short)
        });
        self.enums.insert(written.to_string(), result.clone());
        result
    }
}

fn load_enum_cases(path: &Path, name: &str) -> Option<HashMap<String, String>> {
    let src = fs::read(path).ok()?;
    let arena = LocalArena::new();
    let program = mago_syntax::parser::parse_file_content(&arena, FileId::new(b"enum.php"), &src);

    fn scan<'a>(
        stmts: &'a [Statement<'a>],
        name: &str,
    ) -> Option<&'a mago_syntax::cst::cst::Enum<'a>> {
        for s in stmts {
            match s {
                Statement::Enum(e) if e.name.value == name.as_bytes() => return Some(e),
                Statement::Namespace(ns) => {
                    let inner = match &ns.body {
                        NamespaceBody::Implicit(b) => b.statements.as_slice(),
                        NamespaceBody::BraceDelimited(b) => b.statements.as_slice(),
                    };
                    if let Some(e) = scan(inner, name) {
                        return Some(e);
                    }
                }
                _ => {}
            }
        }
        None
    }

    let e = scan(program.statements.as_slice(), name)?;
    let mut map = HashMap::new();
    for member in e.members.iter() {
        if let ClassLikeMember::EnumCase(case) = member {
            if let EnumCaseItem::Backed(item) = &case.item {
                if let Some(v) = lit_str(item.value) {
                    map.insert(utf8(item.name.value), v);
                }
            }
        }
    }
    Some(map)
}

struct Extractor;

impl<'ast, 'arena> Walker<'ast, 'arena, Ctx> for Extractor {
    fn walk_in_class_like_constant(
        &self,
        constant: &'ast ClassLikeConstant<'arena>,
        ctx: &mut Ctx,
    ) {
        if ctx.collect {
            return;
        }
        for item in constant.items.iter() {
            let vals = const_strings(item.value);
            if !vals.is_empty() {
                ctx.consts.insert(utf8(item.name.value), vals);
            }
        }
    }

    fn walk_in_expression(&self, expression: &'ast Expression<'arena>, ctx: &mut Ctx) {
        match expression {
            // $var = $this->argument('name');  (alias, recorded in pass 1)
            Expression::Assignment(a) if a.operator.is_assign() => {
                if ctx.collect {
                    return;
                }
                if let Expression::Variable(Variable::Direct(dv)) = a.lhs {
                    if let Some(r) = expr_ref(a.rhs, ctx) {
                        ctx.aliases.insert(dv.name.to_vec(), r);
                    }
                }
            }
            _ if !ctx.collect => {}
            // $this->argument('x') === 'value' / self::MODE_X / Enum::Case->value  (either side)
            Expression::Binary(b)
                if b.operator.is_equality() || b.operator.is_negated_equality() =>
            {
                if let Some(r) = expr_ref(b.lhs, ctx) {
                    let vals = lit_strings(b.rhs, ctx);
                    ctx.add_all(&r, vals);
                } else if let Some(r) = expr_ref(b.rhs, ctx) {
                    let vals = lit_strings(b.lhs, ctx);
                    ctx.add_all(&r, vals);
                }
            }
            // in_array($this->argument('x'), [...] | self::MODES | Enum::cases() forms)
            // Negation (!in_array) needs no handling: the walker visits the
            // inner call either way.
            Expression::Call(Call::Function(fc)) => {
                let Expression::Identifier(id) = fc.function else { return };
                if !id.last_segment().eq_ignore_ascii_case(b"in_array") {
                    return;
                }
                let mut args = fc.argument_list.arguments.iter();
                let (Some(first), Some(second)) = (args.next(), args.next()) else { return };
                let Some(r) = expr_ref(first.value(), ctx) else { return };
                let vals = collect_strings(second.value(), ctx);
                ctx.add_all(&r, vals);
            }
            // Source::from($this->argument('x')) / Source::tryFrom(...) → all case values
            Expression::Call(Call::StaticMethod(smc)) => {
                let ClassLikeMemberSelector::Identifier(m) = &smc.method else { return };
                if !matches!(m.value, b"from" | b"tryFrom") {
                    return;
                }
                let Expression::Identifier(cid) = smc.class else { return };
                let Some(first) = smc.argument_list.arguments.iter().next() else { return };
                let Some(r) = expr_ref(first.value(), ctx) else { return };
                let written = utf8(cid.value());
                if let Some(cases) = ctx.enum_cases(&written) {
                    ctx.add_all(&r, cases.into_values().collect());
                }
            }
            // match ($this->argument('x')) { 'a', self::B, Enum::C->value => ... }
            Expression::Match(m) => {
                let Some(r) = expr_ref(m.expression, ctx) else { return };
                for arm in m.arms.iter() {
                    if let MatchArm::Expression(arm) = arm {
                        for cond in arm.conditions.iter() {
                            let vals = lit_strings(cond, ctx);
                            ctx.add_all(&r, vals);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // switch ($this->argument('x')) { case 'a': ... }
    fn walk_in_statement(&self, statement: &'ast Statement<'arena>, ctx: &mut Ctx) {
        if !ctx.collect {
            return;
        }
        let Statement::Switch(s) = statement else { return };
        let Some(r) = expr_ref(s.expression, ctx) else { return };
        for case in s.body.cases() {
            if let SwitchCase::Expression(c) = case {
                let vals = lit_strings(c.expression, ctx);
                ctx.add_all(&r, vals);
            }
        }
    }
}

/// Resolve an expression to an argument/option reference:
/// `$this->argument('x')`, `$this->option('x')`, an aliased variable,
/// unwrapping parentheses and `?? default`.
fn expr_ref(expr: &Expression, ctx: &Ctx) -> Option<RefKey> {
    match expr {
        Expression::Parenthesized(p) => expr_ref(p.expression, ctx),
        Expression::Binary(b) if b.operator.is_null_coalesce() => expr_ref(b.lhs, ctx),
        Expression::Call(Call::Method(mc)) => {
            let Expression::Variable(Variable::Direct(dv)) = mc.object else { return None };
            if dv.name != b"$this" {
                return None;
            }
            let ClassLikeMemberSelector::Identifier(id) = &mc.method else { return None };
            let kind = match id.value {
                b"argument" => Kind::Argument,
                b"option" => Kind::Option,
                _ => return None,
            };
            let first = mc.argument_list.arguments.iter().next()?;
            let name = lit_str_arg(first)?;
            Some((kind, name))
        }
        Expression::Variable(Variable::Direct(dv)) => ctx.aliases.get(dv.name as &[u8]).cloned(),
        _ => None,
    }
}

fn lit_str_arg(arg: &Argument) -> Option<String> {
    lit_str(arg.value())
}

pub(crate) fn lit_str(expr: &Expression) -> Option<String> {
    match expr {
        Expression::Parenthesized(p) => lit_str(p.expression),
        Expression::Literal(Literal::String(s)) => {
            let bytes: &[u8] = match s.value {
                Some(v) => v,
                None => {
                    let r = s.raw;
                    if r.len() >= 2 { &r[1..r.len() - 1] } else { r }
                }
            };
            Some(String::from_utf8_lossy(bytes).into_owned())
        }
        _ => None,
    }
}

/// Strings an expression can evaluate to: literal, same-file class constant
/// (scalar or array), `Enum::Case->value`.
fn lit_strings(expr: &Expression, ctx: &mut Ctx) -> Vec<String> {
    if let Some(s) = lit_str(expr) {
        return vec![s];
    }
    match expr {
        Expression::Parenthesized(p) => lit_strings(p.expression, ctx),
        // self::CONST / static::CONST / SomeClass::CONST — resolved against
        // this file's constants regardless of the written class name.
        Expression::Access(Access::ClassConstant(cca)) => {
            let ClassLikeConstantSelector::Identifier(sel) = &cca.constant else {
                return Vec::new();
            };
            ctx.consts.get(&utf8(sel.value)).cloned().unwrap_or_default()
        }
        // Enum::Case->value
        Expression::Access(Access::Property(pa)) => {
            let ClassLikeMemberSelector::Identifier(prop) = &pa.property else {
                return Vec::new();
            };
            if prop.value != b"value" {
                return Vec::new();
            }
            let Expression::Access(Access::ClassConstant(cca)) = pa.object else {
                return Vec::new();
            };
            let Expression::Identifier(cid) = cca.class else { return Vec::new() };
            let ClassLikeConstantSelector::Identifier(case) = &cca.constant else {
                return Vec::new();
            };
            let written = utf8(cid.value());
            let case = utf8(case.value);
            ctx.enum_cases(&written)
                .and_then(|cases| cases.get(&case).cloned())
                .map(|v| vec![v])
                .unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

/// Strings inside an in_array() haystack: array literal (elements may be
/// constants/enum values), constant array, or an enum-cases expression
/// (`Enum::cases()`, `array_column(Enum::cases(), 'value')`,
/// `array_map(fn, Enum::cases())`).
fn collect_strings(expr: &Expression, ctx: &mut Ctx) -> Vec<String> {
    let elements = match expr {
        Expression::Parenthesized(p) => return collect_strings(p.expression, ctx),
        Expression::Array(a) => a.elements.as_slice(),
        Expression::LegacyArray(a) => a.elements.as_slice(),
        _ => {
            let mut vals = lit_strings(expr, ctx);
            vals.extend(enum_all_values(expr, ctx));
            return vals;
        }
    };
    elements
        .iter()
        .filter_map(|e| e.get_value())
        .flat_map(|v| lit_strings(v, ctx))
        .collect()
}

fn enum_all_values(expr: &Expression, ctx: &mut Ctx) -> Vec<String> {
    match expr {
        Expression::Parenthesized(p) => enum_all_values(p.expression, ctx),
        // Enum::cases()
        Expression::Call(Call::StaticMethod(smc)) => {
            let ClassLikeMemberSelector::Identifier(m) = &smc.method else { return Vec::new() };
            if m.value != b"cases" {
                return Vec::new();
            }
            let Expression::Identifier(cid) = smc.class else { return Vec::new() };
            ctx.enum_cases(&utf8(cid.value()))
                .map(|cases| cases.into_values().collect())
                .unwrap_or_default()
        }
        // array_column(Enum::cases(), 'value') / array_map(fn, Enum::cases())
        Expression::Call(Call::Function(fc)) => {
            let Expression::Identifier(id) = fc.function else { return Vec::new() };
            let args: Vec<_> = fc.argument_list.arguments.iter().collect();
            let inner = match id.last_segment() {
                n if n.eq_ignore_ascii_case(b"array_column") => args.first(),
                n if n.eq_ignore_ascii_case(b"array_map") => args.get(1),
                _ => return Vec::new(),
            };
            inner.map(|a| enum_all_values(a.value(), ctx)).unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

/// Constant definition values: string literal or array of string literals.
fn const_strings(expr: &Expression) -> Vec<String> {
    if let Some(s) = lit_str(expr) {
        return vec![s];
    }
    let elements = match expr {
        Expression::Parenthesized(p) => return const_strings(p.expression),
        Expression::Array(a) => a.elements.as_slice(),
        Expression::LegacyArray(a) => a.elements.as_slice(),
        _ => return Vec::new(),
    };
    elements.iter().filter_map(|e| lit_str(e.get_value()?)).collect()
}

fn utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<?php
class SyncCommand extends Command
{
    protected $signature = 'app:sync {source} {--mode=}';

    private const MODE_HIDDEN = 'hidden';
    private const EXTRA_MODES = ['debug', 'trace'];

    public function handle(): int
    {
        $src = $this->argument('source');
        if ($src === 'github' || $src == 'gitlab') {
            return 1;
        }
        if ($this->argument('source') !== 'bitbucket') {
            return 1;
        }
        if (in_array($this->argument('source'), ['svn', 'hg'], true)) {
            return 1;
        }
        if (!in_array($this->argument('source'), ['zip', 'rar'])) {
            return 1;
        }
        $n = match ($src) {
            'a', 'b' => 1,
            'c' => 2,
            default => 0,
        };
        switch ($this->option('mode')) {
            case 'fast':
                break;
            case 'slow':
                break;
        }
        $mode = $this->option('mode') ?? 'ignored-default';
        if ($mode === 'medium') {
            return 1;
        }
        if ('turbo' === $mode) {
            return 1;
        }
        if ($mode === self::MODE_HIDDEN) {
            return 1;
        }
        if (in_array($mode, self::EXTRA_MODES)) {
            return 1;
        }
        return 0;
    }
}
"#;

    fn extract_fixture(src: &str) -> Values {
        let mut out = Values::new();
        extract_from(src.as_bytes(), Path::new("/nonexistent"), &mut out);
        out
    }

    #[test]
    fn extracts_all_patterns() {
        let out = extract_fixture(FIXTURE);

        let source = &out[&(Kind::Argument, "source".to_string())];
        for v in ["github", "gitlab", "bitbucket", "svn", "hg", "zip", "rar", "a", "b", "c"] {
            assert!(source.iter().any(|s| s == v), "missing argument value {v}: {source:?}");
        }

        let mode = &out[&(Kind::Option, "mode".to_string())];
        for v in ["fast", "slow", "medium", "turbo", "hidden", "debug", "trace"] {
            assert!(mode.iter().any(|s| s == v), "missing option value {v}: {mode:?}");
        }
        assert!(!mode.iter().any(|s| s == "ignored-default"));
    }

    #[test]
    fn finds_command_definitions() {
        assert!(defines_command(FIXTURE.as_bytes(), "app:sync"));
        assert!(!defines_command(FIXTURE.as_bytes(), "app:other"));
        assert!(defines_command(
            br#"Artisan::command('mail:send {user}', function () {});"#,
            "mail:send"
        ));
    }

    #[test]
    fn discovers_commands_in_nonstandard_console_dirs() {
        let dir = std::env::temp_dir().join(format!("artisan-comp-disc-{}", std::process::id()));
        // Domain-style layout: app/Billing/Console/Commands, not app/Console/Commands.
        let cmds = dir.join("app/Billing/Console/Commands");
        fs::create_dir_all(&cmds).unwrap();
        fs::write(
            cmds.join("ChargeCommand.php"),
            r#"<?php
class ChargeCommand extends Command
{
    protected $signature = 'billing:charge {tier}';
    public function handle(): int
    {
        return in_array($this->argument('tier'), ['free', 'paid']) ? 0 : 1;
    }
}
"#,
        )
        .unwrap();

        let out = extract(&dir, "billing:charge");
        let _ = fs::remove_dir_all(&dir);

        let tier = &out[&(Kind::Argument, "tier".to_string())];
        for v in ["free", "paid"] {
            assert!(tier.iter().any(|s| s == v), "missing {v} from nonstandard dir: {tier:?}");
        }
    }

    #[test]
    fn resolves_enums_across_files() {
        let dir = std::env::temp_dir().join(format!("artisan-comp-test-{}", std::process::id()));
        let enums = dir.join("app/Enums");
        fs::create_dir_all(&enums).unwrap();
        fs::write(
            enums.join("Source.php"),
            r#"<?php

namespace App\Enums;

enum Source: string
{
    case Github = 'github';
    case Gitlab = 'gitlab';
    case Svn = 'svn';
}
"#,
        )
        .unwrap();

        let cmd = r#"<?php

namespace App\Console\Commands;

use App\Enums\Source;

class SyncCommand extends Command
{
    protected $signature = 'app:sync {source} {--kind=}';

    public function handle(): int
    {
        if ($this->argument('source') === Source::Github->value) {
            return 1;
        }
        Source::tryFrom($this->option('kind'));
        if (in_array($this->argument('source'), array_column(Source::cases(), 'value'))) {
            return 1;
        }
        return 0;
    }
}
"#;
        let mut out = Values::new();
        extract_from(cmd.as_bytes(), &dir, &mut out);
        let _ = fs::remove_dir_all(&dir);

        let source = &out[&(Kind::Argument, "source".to_string())];
        for v in ["github", "gitlab", "svn"] {
            assert!(source.iter().any(|s| s == v), "missing enum value {v}: {source:?}");
        }
        let kind = &out[&(Kind::Option, "kind".to_string())];
        for v in ["github", "gitlab", "svn"] {
            assert!(kind.iter().any(|s| s == v), "missing tryFrom value {v}: {kind:?}");
        }
    }
}
