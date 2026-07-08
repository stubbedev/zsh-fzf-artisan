//! Extracts candidate values for artisan command arguments/options by parsing
//! the command's PHP source with mago-syntax and collecting string literals the
//! code compares against: `===`/`==`/`!=`/`!==`, `in_array()` (negated or not),
//! `match`, `switch`.
//!
//! The compared expression is traced back to an option/argument through
//! variable aliasing (`$x = $this->argument('name')`), `explode(',', ...)` +
//! `array_map`/`array_filter` closure params + `foreach` (multi-value options),
//! scalar wrappers (`trim`/`strtolower`/...), `(string)` casts, and ternaries
//! (`... ? null : $x`).
//!
//! The value set is resolved from same-file class constants (`self::MODES`),
//! constants imported from another file (`Data::SITE_X`), variables holding a
//! literal array or enum chain (`$m = ['a','b']`), and backed enums
//! (`Source::Github->value`, `Source::tryFrom(...)`, `Source::cases()`,
//! `array_column(...,'value')`,
//! `collect(Source::cases())->pluck('name'|'value')->toArray()`).

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use mago_allocator::LocalArena;
use mago_database::file::FileId;
use mago_syntax::cst::cst::{
    Access, Argument, Call, ClassLikeConstant, ClassLikeConstantSelector, ClassLikeMember,
    ClassLikeMemberSelector, EnumCaseItem, Expression, Foreach, ForeachTarget, Literal, MatchArm,
    NamespaceBody, Statement, SwitchCase, Variable,
};
use mago_syntax::walker::{walk_program, Walker};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Kind {
    Argument,
    Option,
}

pub type RefKey = (Kind, String);
pub type Values = HashMap<RefKey, Vec<String>>;

/// A resolved enum's cases: `values` maps case name → backed value (backed
/// cases only); `names` lists every case name in source order (backed or not).
/// Both are needed because commands validate against either — `Enum::cases()`
/// / `Enum::Case->value` want the values, `->pluck('name')` wants the names.
#[derive(Clone, Default)]
struct EnumCases {
    values: HashMap<String, String>,
    names: Vec<String>,
}

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
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
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
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
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
        value_aliases: HashMap::new(),
        consts: HashMap::new(),
        uses: parse_uses(src),
        enums: HashMap::new(),
        class_consts: HashMap::new(),
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
        let Some(rest) = line.strip_prefix("use ") else {
            continue;
        };
        let Some(rest) = rest.strip_suffix(';') else {
            continue;
        };
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
    /// Variables holding a known set of strings — a literal array or an enum
    /// chain assigned to a var (`$modes = ['a','b']`, `$names = collect(...)->
    /// pluck('name')->toArray()`), so `in_array($x, $modes)` resolves the set.
    value_aliases: HashMap<Vec<u8>, Vec<String>>,
    /// Same-file class constants: NAME → string values (scalar = one, array = many).
    consts: HashMap<String, Vec<String>>,
    uses: HashMap<String, String>,
    /// Memoized enum lookups: written name → resolved cases.
    enums: HashMap<String, Option<EnumCases>>,
    /// Memoized cross-file class constants: written class name → (const → values).
    class_consts: HashMap<String, Option<HashMap<String, Vec<String>>>>,
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

    /// Resolve a written class-like name (via use statements, PSR-4 App\ → app/)
    /// to (file path, short class name). ponytail: App\ prefix only; other
    /// autoload roots (packages, custom PSR-4 maps) resolve to nothing.
    fn resolve_app_class(&self, written: &str) -> Option<(PathBuf, String)> {
        let fqn = self.uses.get(written).cloned().or_else(|| {
            written
                .contains('\\')
                .then(|| written.trim_start_matches('\\').to_string())
        })?;
        let rel = fqn.strip_prefix("App\\")?.replace('\\', "/");
        let short = fqn.rsplit('\\').next().unwrap_or(&fqn).to_string();
        Some((
            self.project_dir.join("app").join(format!("{rel}.php")),
            short,
        ))
    }

    /// Resolve a written enum name to its cases (memoized).
    fn enum_cases(&mut self, written: &str) -> Option<EnumCases> {
        if let Some(memo) = self.enums.get(written) {
            return memo.clone();
        }
        let result = self
            .resolve_app_class(written)
            .and_then(|(path, short)| load_enum_cases(&path, &short));
        self.enums.insert(written.to_string(), result.clone());
        result
    }

    /// String value(s) of `Class::CONST` where `Class` is imported from another
    /// file — resolved by loading that class and reading the constant (memoized).
    fn class_const(&mut self, class: &str, name: &str) -> Vec<String> {
        if !self.class_consts.contains_key(class) {
            let loaded = self
                .resolve_app_class(class)
                .map(|(path, short)| load_class_consts(&path, &short));
            self.class_consts.insert(class.to_string(), loaded);
        }
        self.class_consts
            .get(class)
            .and_then(|m| m.as_ref())
            .and_then(|m| m.get(name))
            .cloned()
            .unwrap_or_default()
    }
}

fn load_enum_cases(path: &Path, name: &str) -> Option<EnumCases> {
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
    let mut cases = EnumCases::default();
    for member in e.members.iter() {
        if let ClassLikeMember::EnumCase(case) = member {
            match &case.item {
                EnumCaseItem::Backed(item) => {
                    let name = utf8(item.name.value);
                    if let Some(v) = lit_str(item.value) {
                        cases.values.insert(name.clone(), v);
                    }
                    cases.names.push(name);
                }
                EnumCaseItem::Unit(item) => cases.names.push(utf8(item.name.value)),
            }
        }
    }
    Some(cases)
}

/// Load a class/interface/trait's string constants (name → value(s)) from its
/// file. Mirrors the same-file constant handling but for an imported class.
fn load_class_consts(path: &Path, name: &str) -> HashMap<String, Vec<String>> {
    let mut out = HashMap::new();
    let Ok(src) = fs::read(path) else {
        return out;
    };
    let arena = LocalArena::new();
    let program = mago_syntax::parser::parse_file_content(&arena, FileId::new(b"class.php"), &src);

    fn members<'a>(stmts: &'a [Statement<'a>], name: &str) -> Option<&'a [ClassLikeMember<'a>]> {
        for s in stmts {
            match s {
                Statement::Class(c) if c.name.value == name.as_bytes() => {
                    return Some(c.members.as_slice())
                }
                Statement::Interface(i) if i.name.value == name.as_bytes() => {
                    return Some(i.members.as_slice())
                }
                Statement::Trait(t) if t.name.value == name.as_bytes() => {
                    return Some(t.members.as_slice())
                }
                Statement::Namespace(ns) => {
                    let inner = match &ns.body {
                        NamespaceBody::Implicit(b) => b.statements.as_slice(),
                        NamespaceBody::BraceDelimited(b) => b.statements.as_slice(),
                    };
                    if let Some(m) = members(inner, name) {
                        return Some(m);
                    }
                }
                _ => {}
            }
        }
        None
    }

    let Some(members) = members(program.statements.as_slice(), name) else {
        return out;
    };
    for member in members {
        if let ClassLikeMember::Constant(constant) = member {
            for item in constant.items.iter() {
                let vals = const_strings(item.value);
                if !vals.is_empty() {
                    out.insert(utf8(item.name.value), vals);
                }
            }
        }
    }
    out
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
                    // Var holding a literal array or enum chain → record its set
                    // so `in_array($x, $var)` can resolve the values.
                    let vals = collect_strings(a.rhs, ctx);
                    if !vals.is_empty() {
                        ctx.value_aliases.insert(dv.name.to_vec(), vals);
                    }
                }
            }
            // array_map($fn, $arr) / array_filter($arr, $fn): bind the callable's
            // first parameter to the array's ref, so a comparison on that param
            // inside the closure attaches to the option/argument. Pass 1 only.
            Expression::Call(Call::Function(fc)) if !ctx.collect => {
                if let Expression::Identifier(id) = fc.function {
                    let (cb_idx, arr_idx) = match id.last_segment().to_ascii_lowercase().as_slice()
                    {
                        b"array_map" => (0usize, 1usize),
                        b"array_filter" | b"array_walk" => (1, 0),
                        _ => return,
                    };
                    let args: Vec<_> = fc.argument_list.arguments.iter().collect();
                    if let (Some(cb), Some(arr)) = (args.get(cb_idx), args.get(arr_idx)) {
                        if let (Some(param), Some(r)) =
                            (first_param_name(cb.value()), expr_ref(arr.value(), ctx))
                        {
                            ctx.aliases.insert(param, r);
                        }
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
                let Expression::Identifier(id) = fc.function else {
                    return;
                };
                if !id.last_segment().eq_ignore_ascii_case(b"in_array") {
                    return;
                }
                let mut args = fc.argument_list.arguments.iter();
                let (Some(first), Some(second)) = (args.next(), args.next()) else {
                    return;
                };
                let Some(r) = expr_ref(first.value(), ctx) else {
                    return;
                };
                let vals = collect_strings(second.value(), ctx);
                ctx.add_all(&r, vals);
            }
            // Source::from($this->argument('x')) / Source::tryFrom(...) → all case values
            Expression::Call(Call::StaticMethod(smc)) => {
                let ClassLikeMemberSelector::Identifier(m) = &smc.method else {
                    return;
                };
                if !matches!(m.value, b"from" | b"tryFrom") {
                    return;
                }
                let Expression::Identifier(cid) = smc.class else {
                    return;
                };
                let Some(first) = smc.argument_list.arguments.iter().next() else {
                    return;
                };
                let Some(r) = expr_ref(first.value(), ctx) else {
                    return;
                };
                let written = utf8(cid.value());
                if let Some(cases) = ctx.enum_cases(&written) {
                    ctx.add_all(&r, cases.values.into_values().collect());
                }
            }
            // match ($this->argument('x')) { 'a', self::B, Enum::C->value => ... }
            Expression::Match(m) => {
                let Some(r) = expr_ref(m.expression, ctx) else {
                    return;
                };
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
            // foreach (<ref-array> as $item): bind $item to the array's ref so a
            // comparison on $item in the loop body attaches to the option/arg.
            if let Statement::Foreach(f) = statement {
                bind_foreach(f, ctx);
            }
            return;
        }
        let Statement::Switch(s) = statement else {
            return;
        };
        let Some(r) = expr_ref(s.expression, ctx) else {
            return;
        };
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
            let Expression::Variable(Variable::Direct(dv)) = mc.object else {
                return None;
            };
            if dv.name != b"$this" {
                return None;
            }
            let ClassLikeMemberSelector::Identifier(id) = &mc.method else {
                return None;
            };
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
        // `(string) $x` / `(int) $x` — casts preserve the ref.
        Expression::UnaryPrefix(u) if u.operator.is_cast() => expr_ref(u.operand, ctx),
        // `cond ? $a : $b` — either branch may carry the ref (e.g. `... ? null : $x`).
        Expression::Conditional(c) => c
            .then
            .and_then(|t| expr_ref(t, ctx))
            .or_else(|| expr_ref(c.r#else, ctx)),
        // See through calls whose result carries the same ref as one argument:
        // scalar wrappers (`trim($x)`) and element-preserving array ops
        // (`explode(',', $x)`, `array_map($fn, $x)`, `array_filter($x)`). This
        // lets `in_array(trim($x), [...])` and multi-value options resolve.
        Expression::Call(Call::Function(fc)) => {
            let Expression::Identifier(id) = fc.function else {
                return None;
            };
            let idx = passthrough_arg(id.last_segment())?;
            let arg = fc.argument_list.arguments.iter().nth(idx)?;
            expr_ref(arg.value(), ctx)
        }
        _ => None,
    }
}

/// For functions whose result values equal the values (or elements) of one
/// argument, the index of that argument. Used to trace a ref through wrappers.
fn passthrough_arg(seg: &[u8]) -> Option<usize> {
    match seg.to_ascii_lowercase().as_slice() {
        b"trim" | b"ltrim" | b"rtrim" | b"strtolower" | b"strtoupper" | b"mb_strtolower"
        | b"mb_strtoupper" | b"strval" | b"ucfirst" | b"lcfirst" => Some(0),
        b"explode" | b"array_map" => Some(1),
        b"array_filter" | b"array_values" | b"array_unique" | b"array_reverse" => Some(0),
        _ => None,
    }
}

fn bind_foreach(f: &Foreach, ctx: &mut Ctx) {
    let Some(r) = expr_ref(f.expression, ctx) else {
        return;
    };
    let target = match &f.target {
        ForeachTarget::Value(v) => v.value,
        ForeachTarget::KeyValue(kv) => kv.value,
    };
    if let Expression::Variable(Variable::Direct(dv)) = target {
        ctx.aliases.insert(dv.name.to_vec(), r);
    }
}

/// The first parameter variable name (`$x`) of a closure or arrow function.
fn first_param_name(expr: &Expression) -> Option<Vec<u8>> {
    let params = match expr {
        Expression::Closure(c) => &c.parameter_list,
        Expression::ArrowFunction(f) => &f.parameter_list,
        _ => return None,
    };
    params
        .parameters
        .iter()
        .next()
        .map(|p| p.variable.name.to_vec())
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
                    if r.len() >= 2 {
                        &r[1..r.len() - 1]
                    } else {
                        r
                    }
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
        // self::CONST / static::CONST — resolved against this file's constants.
        // Foo::CONST for an imported Foo — resolved by loading Foo's file.
        Expression::Access(Access::ClassConstant(cca)) => {
            let ClassLikeConstantSelector::Identifier(sel) = &cca.constant else {
                return Vec::new();
            };
            let name = utf8(sel.value);
            if let Some(vals) = ctx.consts.get(&name) {
                return vals.clone();
            }
            // Cross-file: Foo::CONST where Foo is imported (not self/static/parent).
            if let Expression::Identifier(cid) = cca.class {
                let written = utf8(cid.value());
                if !matches!(written.as_str(), "self" | "static" | "parent") {
                    return ctx.class_const(&written, &name);
                }
            }
            Vec::new()
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
            let Expression::Identifier(cid) = cca.class else {
                return Vec::new();
            };
            let ClassLikeConstantSelector::Identifier(case) = &cca.constant else {
                return Vec::new();
            };
            let written = utf8(cid.value());
            let case = utf8(case.value);
            ctx.enum_cases(&written)
                .and_then(|cases| cases.values.get(&case).cloned())
                .map(|v| vec![v])
                .unwrap_or_default()
        }
        _ => Vec::new(),
    }
}

/// Strings inside an in_array() haystack: array literal (elements may be
/// constants/enum values), constant array, or an enum-cases expression
/// (`Enum::cases()`, `array_column(Enum::cases(), 'value')`,
/// `array_map(fn, Enum::cases())`, `collect(Enum::cases())->pluck('name')->toArray()`).
fn collect_strings(expr: &Expression, ctx: &mut Ctx) -> Vec<String> {
    let elements = match expr {
        Expression::Parenthesized(p) => return collect_strings(p.expression, ctx),
        Expression::Array(a) => a.elements.as_slice(),
        Expression::LegacyArray(a) => a.elements.as_slice(),
        // A variable holding a known set (recorded in pass 1).
        Expression::Variable(Variable::Direct(dv)) => {
            return ctx
                .value_aliases
                .get(dv.name as &[u8])
                .cloned()
                .unwrap_or_default();
        }
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

/// Which enum-case field an expression selects.
#[derive(Clone, Copy)]
enum EnumField {
    Value,
    Name,
}

/// All strings an enum-cases expression can yield, defaulting to backed values.
fn enum_all_values(expr: &Expression, ctx: &mut Ctx) -> Vec<String> {
    enum_chain(expr, ctx, EnumField::Value)
}

/// Walk an enum-cases chain to the terminal `Enum::cases()`, selecting `field`.
/// Sees through `collect(...)`, `array_column/array_map/array_values`, and
/// collection methods (`->toArray()`, `->all()`, `->values()`); `->pluck('name')`
/// switches the selection to case names for the rest of the (inner) chain, so
/// `collect(Enum::cases())->pluck('name')->toArray()` yields the case names.
fn enum_chain(expr: &Expression, ctx: &mut Ctx, field: EnumField) -> Vec<String> {
    match expr {
        Expression::Parenthesized(p) => enum_chain(p.expression, ctx, field),
        // Terminal: Enum::cases()
        Expression::Call(Call::StaticMethod(smc)) => {
            let ClassLikeMemberSelector::Identifier(m) = &smc.method else {
                return Vec::new();
            };
            if m.value != b"cases" {
                return Vec::new();
            }
            let Expression::Identifier(cid) = smc.class else {
                return Vec::new();
            };
            match ctx.enum_cases(&utf8(cid.value())) {
                Some(cases) => match field {
                    EnumField::Value => cases.values.into_values().collect(),
                    EnumField::Name => cases.names,
                },
                None => Vec::new(),
            }
        }
        // collect(X) / array_column(X, _) / array_map(_, X) / array_values(X)
        Expression::Call(Call::Function(fc)) => {
            let Expression::Identifier(id) = fc.function else {
                return Vec::new();
            };
            let args: Vec<_> = fc.argument_list.arguments.iter().collect();
            let inner = match id.last_segment().to_ascii_lowercase().as_slice() {
                b"collect" | b"array_column" | b"array_values" | b"array_unique" => args.first(),
                b"array_map" => args.get(1),
                _ => return Vec::new(),
            };
            inner
                .map(|a| enum_chain(a.value(), ctx, field))
                .unwrap_or_default()
        }
        // ->toArray()/->all()/->values(): identity. ->pluck('name'|'value'): pick field.
        Expression::Call(Call::Method(mc)) => {
            let ClassLikeMemberSelector::Identifier(m) = &mc.method else {
                return Vec::new();
            };
            match m.value {
                b"toArray" | b"all" | b"values" => enum_chain(mc.object, ctx, field),
                b"pluck" => {
                    let f = match mc
                        .argument_list
                        .arguments
                        .iter()
                        .next()
                        .and_then(|a| lit_str(a.value()))
                        .as_deref()
                    {
                        Some("name") => EnumField::Name,
                        _ => EnumField::Value,
                    };
                    enum_chain(mc.object, ctx, f)
                }
                _ => Vec::new(),
            }
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
    elements
        .iter()
        .filter_map(|e| lit_str(e.get_value()?))
        .collect()
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
        for v in [
            "github",
            "gitlab",
            "bitbucket",
            "svn",
            "hg",
            "zip",
            "rar",
            "a",
            "b",
            "c",
        ] {
            assert!(
                source.iter().any(|s| s == v),
                "missing argument value {v}: {source:?}"
            );
        }

        let mode = &out[&(Kind::Option, "mode".to_string())];
        for v in [
            "fast", "slow", "medium", "turbo", "hidden", "debug", "trace",
        ] {
            assert!(
                mode.iter().any(|s| s == v),
                "missing option value {v}: {mode:?}"
            );
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
            assert!(
                tier.iter().any(|s| s == v),
                "missing {v} from nonstandard dir: {tier:?}"
            );
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
            assert!(
                source.iter().any(|s| s == v),
                "missing enum value {v}: {source:?}"
            );
        }
        let kind = &out[&(Kind::Option, "kind".to_string())];
        for v in ["github", "gitlab", "svn"] {
            assert!(
                kind.iter().any(|s| s == v),
                "missing tryFrom value {v}: {kind:?}"
            );
        }
    }

    #[test]
    fn resolves_values_through_explode_and_array_map() {
        // Multi-value option: explode the comma list, validate each element in a
        // closure via in_array. The literal set must attach to `resources` even
        // though the comparison is on the closure param, not the option itself.
        let cmd = r#"<?php

namespace App\Console\Commands;

use Illuminate\Console\Command;

class Reindex extends Command
{
    protected $signature = 'app:reindex {--resources= : list}';

    public function handle(): int
    {
        $resources = $this->option('resources');
        $resources = explode(',', $resources);
        $resources = array_filter(array_map(function ($resource) {
            $resource = trim($resource);

            return in_array($resource, ['folders', 'files', 'file-content', 'all']) ? $resource : null;
        }, $resources));

        return count($resources);
    }
}
"#;
        let dir =
            std::env::temp_dir().join(format!("artisan-comp-map-test-{}", std::process::id()));
        let mut out = Values::new();
        extract_from(cmd.as_bytes(), &dir, &mut out);

        let res = &out[&(Kind::Option, "resources".to_string())];
        for v in ["folders", "files", "file-content", "all"] {
            assert!(
                res.iter().any(|s| s == v),
                "missing resource value {v}: {res:?}"
            );
        }
    }

    #[test]
    fn resolves_values_through_foreach() {
        // foreach over the exploded option binds the loop var to the option ref.
        let cmd = r#"<?php

namespace App\Console\Commands;

use Illuminate\Console\Command;

class Sync extends Command
{
    protected $signature = 'app:sync {--modes= : list}';

    public function handle(): int
    {
        foreach (explode(',', $this->option('modes')) as $mode) {
            if (!in_array($mode, ['fast', 'slow', 'safe'])) {
                return 1;
            }
        }

        return 0;
    }
}
"#;
        let dir = std::env::temp_dir().join(format!("artisan-comp-fe-test-{}", std::process::id()));
        let mut out = Values::new();
        extract_from(cmd.as_bytes(), &dir, &mut out);

        let modes = &out[&(Kind::Option, "modes".to_string())];
        for v in ["fast", "slow", "safe"] {
            assert!(modes.iter().any(|s| s == v), "missing mode {v}: {modes:?}");
        }
    }

    #[test]
    fn resolves_values_from_enum_pluck_name_via_variable() {
        // Real kontainer idiom (DAMReindex --pattern): the valid set is the enum
        // case NAMES, built with collect(Enum::cases())->pluck('name')->toArray()
        // and stored in a variable that in_array then checks against.
        let dir =
            std::env::temp_dir().join(format!("artisan-comp-pluck-test-{}", std::process::id()));
        let enums = dir.join("app/Enums");
        fs::create_dir_all(&enums).unwrap();
        fs::write(
            enums.join("Pattern.php"),
            r#"<?php

namespace App\Enums;

enum Pattern: string
{
    case Alpha = 'a';
    case Beta = 'b';
    case Gamma = 'c';
}
"#,
        )
        .unwrap();

        let cmd = r#"<?php

namespace App\Console\Commands;

use App\Enums\Pattern;
use Illuminate\Console\Command;

class Reindex extends Command
{
    protected $signature = 'app:reindex {--pattern=}';

    public function handle(): int
    {
        $validNames = collect(Pattern::cases())->pluck('name')->toArray();
        if (!in_array($this->option('pattern'), $validNames)) {
            return 1;
        }

        return 0;
    }
}
"#;
        let mut out = Values::new();
        extract_from(cmd.as_bytes(), &dir, &mut out);
        let _ = fs::remove_dir_all(&dir);

        let p = &out[&(Kind::Option, "pattern".to_string())];
        for v in ["Alpha", "Beta", "Gamma"] {
            assert!(p.iter().any(|s| s == v), "missing case name {v}: {p:?}");
        }
    }

    #[test]
    fn resolves_values_from_imported_class_constants() {
        // Real kontainer-custom-scripts idiom (SoyaConcept `site`): the value set
        // is class constants defined in a DIFFERENT file, imported via `use`.
        let dir =
            std::env::temp_dir().join(format!("artisan-comp-xconst-test-{}", std::process::id()));
        let data_dir = dir.join("app/Console/Commands/Soya/Dam");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(
            data_dir.join("Data.php"),
            r#"<?php

namespace App\Console\Commands\Soya\Dam;

class Data
{
    public const string SITE_A = 'soyaconcept';
    public const SITE_B = 'levete';
    public const SITE_C = 'wasabi';
}
"#,
        )
        .unwrap();

        let cmd = r#"<?php

namespace App\Console\Commands\Soya;

use App\Console\Commands\Soya\Dam\Data;
use Illuminate\Console\Command;

class Soya extends Command
{
    protected $signature = 'client:soya {site}';

    public function handle(): int
    {
        $site = $this->argument('site');
        if (!in_array($site, [Data::SITE_A, Data::SITE_B, Data::SITE_C])) {
            return 1;
        }

        return 0;
    }
}
"#;
        let mut out = Values::new();
        extract_from(cmd.as_bytes(), &dir, &mut out);
        let _ = fs::remove_dir_all(&dir);

        let site = &out[&(Kind::Argument, "site".to_string())];
        for v in ["soyaconcept", "levete", "wasabi"] {
            assert!(site.iter().any(|s| s == v), "missing site {v}: {site:?}");
        }
    }

    #[test]
    fn resolves_values_through_ternary_and_cast() {
        // Real kontainer idiom (DamRegenerateFileEmbeddings --type): the option is
        // normalized through a ternary + (string) cast + strtolower/trim before
        // in_array. The ref must survive all of those to attach the value set.
        let cmd = r#"<?php

namespace App\Console\Commands;

use Illuminate\Console\Command;

class Embed extends Command
{
    protected $signature = 'app:embed {--type=}';

    public function handle(): int
    {
        $typeOption = $this->option('type');
        $typeFilter = ($typeOption === null || $typeOption === '')
            ? null
            : strtolower(trim((string) $typeOption));
        if ($typeFilter !== null && !in_array($typeFilter, ['images', 'documents'], true)) {
            return 1;
        }

        return 0;
    }
}
"#;
        let dir = std::env::temp_dir().join(format!("artisan-comp-tc-test-{}", std::process::id()));
        let mut out = Values::new();
        extract_from(cmd.as_bytes(), &dir, &mut out);

        let ty = &out[&(Kind::Option, "type".to_string())];
        for v in ["images", "documents"] {
            assert!(ty.iter().any(|s| s == v), "missing type {v}: {ty:?}");
        }
    }
}
