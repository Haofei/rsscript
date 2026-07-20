//! Generative VM <-> compiler differential testing.
//!
//! The curated parity suite (vm_eval / vm_eval_parity / corpus) only compares
//! hand-picked inputs. This test *generates* valid RSScript programs and asserts
//! the register VM and JIT produce identical output — so a divergence on an
//! input nobody thought to write is still caught (proptest shrinks any failure
//! to a minimal reproducer). Set `RSSCRIPT_FULL_BACKEND_PARITY=1` to include the
//! compiled Rust backend in the broad sweep.
//!
//! Programs are `let`-binding chains of checked integer arithmetic
//! (sum-of-products over literals and earlier variables), then print the result.
//! This stays inside the parser surface (no parens / unary minus), is always
//! type-correct, and skips overflowing cases (both backends error there).
//!
//! The default path stays in-process so it can run on every edit. Full backend
//! parity compiles a generated crate per case, so keep that as a longer
//! differential-fuzz run.

mod common;

use proptest::prelude::*;

fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn differential_proptest_cases() -> u32 {
    env_u32("RSS_DIFF_PROPTEST_CASES", 4)
}

fn differential_proptest_shrink_iters() -> u32 {
    env_u32("RSS_DIFF_SHRINK_ITERS", 16)
}

/// A factor in a product: a small literal or a reference to an earlier variable.
#[derive(Debug, Clone)]
enum Factor {
    Lit(i64),
    Var(usize),
}

/// One additive term: an operator (`+`/`-`, ignored for the first term) and the
/// factors multiplied together.
type Term = (char, Vec<Factor>);
/// A sum-of-products expression.
type Expr = Vec<Term>;

/// A comparison operator (renders to RSScript; evaluated by the oracle).
#[derive(Debug, Clone, Copy)]
enum Cmp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

impl Cmp {
    fn render(self) -> &'static str {
        match self {
            Cmp::Lt => "<",
            Cmp::Le => "<=",
            Cmp::Gt => ">",
            Cmp::Ge => ">=",
            Cmp::Eq => "==",
            Cmp::Ne => "!=",
        }
    }

    fn eval(self, lhs: i64, rhs: i64) -> bool {
        match self {
            Cmp::Lt => lhs < rhs,
            Cmp::Le => lhs <= rhs,
            Cmp::Gt => lhs > rhs,
            Cmp::Ge => lhs >= rhs,
            Cmp::Eq => lhs == rhs,
            Cmp::Ne => lhs != rhs,
        }
    }
}

/// A guarded adjustment: `if <lhs> <cmp> <rhs> { acc = acc + <adjustment> }`.
/// Exercises the JIT's comparison + jump instructions.
#[derive(Debug, Clone)]
struct Guard {
    lhs: Expr,
    cmp: Cmp,
    rhs: Expr,
    adjustment: Expr,
}

/// A program: a chain of `let` bindings, guarded conditional adjustments, and a
/// final result — all integer-typed, run inside a JIT-eligible `compute()`.
#[derive(Debug, Clone)]
struct Program {
    bindings: Vec<Expr>,
    guards: Vec<Guard>,
    result: Expr,
}

// Literal magnitudes deliberately span past `i32::MAX`: a product of two factors
// can exceed 2^31, so an all-literal sub-expression that defaults to Rust `i32`
// would *const-overflow at compile time* even though the i64 value is fine. That
// was a real VM<->compiler gap (integer literals lowered without an `i64`
// suffix); now that `rust_lower` emits `i64`-typed literals, this differential
// exercises that fix instead of avoiding it. The oracle uses checked arithmetic
// and `prop_assume!` skips the (now common) i64-overflow cases.
fn arb_factor(vars_in_scope: usize) -> impl Strategy<Value = Factor> {
    if vars_in_scope == 0 {
        (0i64..=60_000).prop_map(Factor::Lit).boxed()
    } else {
        prop_oneof![
            (0i64..=60_000).prop_map(Factor::Lit),
            (0..vars_in_scope).prop_map(Factor::Var),
        ]
        .boxed()
    }
}

fn arb_expr(vars_in_scope: usize) -> impl Strategy<Value = Expr> {
    let term = (
        prop_oneof![Just('+'), Just('-')],
        prop::collection::vec(arb_factor(vars_in_scope), 1..=2),
    );
    prop::collection::vec(term, 1..=3)
}

fn arb_program() -> impl Strategy<Value = Program> {
    (0usize..=1)
        .prop_flat_map(|binding_count| {
            // Build bindings sequentially so each can reference earlier vars.
            let mut strategy = Just(Vec::<Expr>::new()).boxed();
            for index in 0..binding_count {
                strategy = (strategy, arb_expr(index))
                    .prop_map(|(mut bindings, expr)| {
                        bindings.push(expr);
                        bindings
                    })
                    .boxed();
            }
            strategy.prop_flat_map(|bindings| {
                let count = bindings.len();
                let cmp = prop_oneof![
                    Just(Cmp::Lt),
                    Just(Cmp::Le),
                    Just(Cmp::Gt),
                    Just(Cmp::Ge),
                    Just(Cmp::Eq),
                    Just(Cmp::Ne),
                ];
                let guard = (arb_expr(count), cmp, arb_expr(count), arb_expr(count)).prop_map(
                    |(lhs, cmp, rhs, adjustment)| Guard {
                        lhs,
                        cmp,
                        rhs,
                        adjustment,
                    },
                );
                let guards = prop::collection::vec(guard, 0..=2);
                (guards, arb_expr(count)).prop_map(move |(guards, result)| Program {
                    bindings: bindings.clone(),
                    guards,
                    result,
                })
            })
        })
        .boxed()
}

/// Reference evaluation with checked arithmetic; `None` on overflow.
fn oracle(program: &Program) -> Option<i64> {
    let mut vars: Vec<i64> = Vec::new();
    for binding in &program.bindings {
        let value = eval_expr(binding, &vars)?;
        vars.push(value);
    }
    let mut acc = eval_expr(&program.result, &vars)?;
    for guard in &program.guards {
        let lhs = eval_expr(&guard.lhs, &vars)?;
        let rhs = eval_expr(&guard.rhs, &vars)?;
        if guard.cmp.eval(lhs, rhs) {
            let adjustment = eval_expr(&guard.adjustment, &vars)?;
            acc = acc.checked_add(adjustment)?;
        }
    }
    Some(acc)
}

fn eval_expr(expr: &Expr, vars: &[i64]) -> Option<i64> {
    let mut total: i64 = 0;
    for (index, (op, factors)) in expr.iter().enumerate() {
        let mut product: i64 = 1;
        for factor in factors {
            let value = match factor {
                Factor::Lit(n) => *n,
                Factor::Var(i) => vars[*i],
            };
            product = product.checked_mul(value)?;
        }
        total = if index == 0 {
            product
        } else if *op == '+' {
            total.checked_add(product)?
        } else {
            total.checked_sub(product)?
        };
    }
    Some(total)
}

fn render_factor(factor: &Factor) -> String {
    match factor {
        Factor::Lit(n) => n.to_string(),
        Factor::Var(i) => format!("x{i}"),
    }
}

fn render_expr(expr: &Expr) -> String {
    let mut rendered = String::new();
    for (index, (op, factors)) in expr.iter().enumerate() {
        if index > 0 {
            rendered.push_str(&format!(" {op} "));
        }
        let product = factors
            .iter()
            .map(render_factor)
            .collect::<Vec<_>>()
            .join(" * ");
        rendered.push_str(&product);
    }
    rendered
}

fn render(program: &Program) -> String {
    // The arithmetic lives in `compute()`, a pure integer/control function that
    // is JIT-eligible, so the JIT executor actually runs it; `main` calls it and
    // prints, so it stays on the interpreter. This makes the 3-way differential
    // exercise the JIT path, not just interp vs compiled.
    let mut source = String::from("fn compute() -> Int {\n");
    for (index, binding) in program.bindings.iter().enumerate() {
        source.push_str(&format!("    let x{index} = {}\n", render_expr(binding)));
    }
    source.push_str(&format!(
        "    let mut acc = {}\n",
        render_expr(&program.result)
    ));
    for guard in &program.guards {
        source.push_str(&format!(
            "    if {} {} {} {{\n        acc = acc + {}\n    }}\n",
            render_expr(&guard.lhs),
            guard.cmp.render(),
            render_expr(&guard.rhs),
            render_expr(&guard.adjustment),
        ));
    }
    source.push_str("    return acc\n}\n\n");
    source.push_str("fn main() -> Unit {\n");
    source.push_str("    Log.write(message: read String.from_int(value: compute()))\n");
    source.push_str("    return Unit\n}\n");
    source
}

/// Small forced full-backend smoke, so default runs still execute the generated
/// Rust backend without compiling a package for every differential case.
#[test]
fn compiled_backend_smoke_matches_vm_and_jit() {
    let source = "\
fn compute(seed: Int) -> Int {
    let mut acc = seed
    let mut i = 0
    while i < 5 {
        acc = acc * 3 - i
        i = i + 1
    }
    return acc
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: read compute(seed: read 7)))
    return Unit
}
";
    common::differential::assert_backends_agree_full("compiled-smoke.rss", source, &[]);
}

#[test]
fn named_call_binding_matches_the_language_oracle_on_every_backend() {
    let source = r#"
const fallback: Int = 7

struct Box { value: Int }
struct Cell { n: Int }

fn pair(first: Int, second: Int) -> Int { return first * 10 + second }
fn digits(a: Int = 1, b: Int, c: Int = 3) -> Int { return a * 100 + b * 10 + c }
fn pick(value: Int = fallback) -> Int { return value }
fn add_into(read_value: Int, target: mut Int) -> Unit { target = target + read_value }
fn Box.add(self: read Box, amount: Int = 1) -> Int { return self.value + amount }
fn Cell.combine(self: mut Cell, source: read Cell, target: mut Cell) -> Unit {
    target.n = target.n + source.n + self.n
}
fn record(order: mut Int, value: Int) -> Int {
    order = order * 10 + value
    Log.write(message: String.from_int(value: value))
    return value
}

pub fn main() -> Unit {
    let mut x: Int = 1
    let mut y: Int = 10
    add_into(target: mut y, read_value: x)

    let b = Box(value: 4)
    let mut base = Cell(n: 1)
    let source = Cell(n: 2)
    let mut target = Cell(n: 10)
    mut base.combine(target: mut target, source: read source)

    let fallback: Int = 99
    let mut order: Int = 0
    let encoded = pair(
        second: record(order: mut order, value: 2),
        first: record(order: mut order, value: 1)
    )

    Log.write(message: String.from_int(value: encoded))
    Log.write(message: String.from_int(value: order))
    Log.write(message: String.from_int(value: digits(c: 9, b: 2)))
    Log.write(message: String.from_int(value: x * 100 + y))
    Log.write(message: String.from_int(value: b.add()))
    Log.write(message: String.from_int(value: target.n))
    Log.write(message: String.from_int(value: pick()))
    return Unit
}
"#;
    let expected = "2\n1\n12\n21\n129\n111\n5\n13\n7\n";
    for backend in common::differential::full_backends() {
        let actual = backend
            .run_stdout("named-call-binding.rss", source, &[])
            .unwrap_or_else(|error| panic!("backend `{}` failed: {error}", backend.name()));
        assert_eq!(actual, expected, "backend `{}`", backend.name());
    }
}

/// Eligible function with parameters (exercises DeepCopy + integer arithmetic in
/// the JIT) — interp == jit by default, plus compiled when
/// `RSSCRIPT_FULL_BACKEND_PARITY=1`.
#[test]
fn backends_agree_on_parameterized_arithmetic() {
    let source = "\
fn combine(a: Int, b: Int, c: Int) -> Int {
    let scaled = a * 3 - b
    return scaled + c * 2
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: combine(a: read 7, b: read 4, c: read 5)))
    return Unit
}
";
    common::differential::assert_backends_agree("jit-params.rss", source, &[]);
}

/// Positional multi-field variant binding (SH-024): matching a 2-/3-field sum
/// variant by declared field order (`Rectangle(w, h)`, `Prism(w, _, d)`) and a
/// nested per-position pattern must agree across every backend. This surfaces
/// both danger zones: the reg-VM per-field `GetField` projection and the AOT
/// named-form lowering (`Sum::V { first: a, second: b }`).
#[test]
fn backends_agree_on_positional_multifield_variant() {
    let source = "\
sum Shape {
    Circle(radius: Int)
    Rectangle(width: Int, height: Int)
    Prism(width: Int, height: Int, depth: Int)
}

fn area(shape: read Shape) -> Int {
    match read shape {
        Circle(r) => { return read r * read r }
        Rectangle(w, h) => { return read w * read h }
        Prism(w, _, d) => { return read w * read d }
    }
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: area(shape: read Circle(radius: 4))))
    Log.write(message: read String.from_int(value: area(shape: read Rectangle(width: 3, height: 5))))
    Log.write(message: read String.from_int(value: area(shape: read Prism(width: 2, height: 9, depth: 7))))
    return Unit
}
";
    common::differential::assert_backends_agree("multifield-variant.rss", source, &[]);
}

/// Nested per-position destructuring inside a positional multi-field variant
/// (`Pair(k, Some(v))`) must agree across every backend.
#[test]
fn backends_agree_on_positional_multifield_nested_variant() {
    let source = "\
sum Entry {
    Pair(key: Int, value: Option<Int>)
}

fn value_or_zero(entry: read Entry) -> Int {
    match read entry {
        Pair(k, Some(v)) => { return read k + read v }
        Pair(k, None) => { return read k }
    }
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: value_or_zero(entry: read Pair(key: 10, value: Some(5)))))
    Log.write(message: read String.from_int(value: value_or_zero(entry: read Pair(key: 10, value: None))))
    return Unit
}
";
    common::differential::assert_backends_agree("multifield-variant-nested.rss", source, &[]);
}

/// Eligible function with comparisons and branches (LessInt / JumpIfBool /
/// JumpIfIntCompare in the JIT).
#[test]
fn backends_agree_on_comparison_and_branches() {
    let source = "\
fn classify(n: Int) -> Int {
    if n < 10 {
        return 0
    }
    if n <= 100 {
        return 1
    }
    return 2
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: classify(n: read 3)))
    Log.write(message: read String.from_int(value: classify(n: read 42)))
    Log.write(message: read String.from_int(value: classify(n: read 999)))
    return Unit
}
";
    common::differential::assert_backends_agree("jit-branches.rss", source, &[]);
}

#[test]
fn backends_agree_on_if_expression() {
    let source = "\
fn choose(flag: Bool) -> Int {
    return if flag {
        7
    } else {
        11
    }
}

fn main() -> Unit {
    Log.write(message: String.from_int(value: choose(flag: true)))
    Log.write(message: String.from_int(value: choose(flag: false)))
    return Unit
}
";
    common::differential::assert_backends_agree("if-expression.rss", source, &[]);
}

/// Eligible function with a loop (jumps + reassignment in the JIT).
#[test]
fn backends_agree_on_loop() {
    let source = "\
fn sum_to(n: Int) -> Int {
    let mut total = 0
    let mut i = 1
    while i <= n {
        total = total + i
        i = i + 1
    }
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: sum_to(n: read 10)))
    return Unit
}
";
    common::differential::assert_backends_agree("jit-loop.rss", source, &[]);
}

/// Eligible function exercising the collection get/set/index ops now in the
/// tier-0 subset (List push/set/get/len/append/pop/clear, Map insert/get/remove)
/// — interp == jit == compiled. The work lives in `compute()` (pure: no closures,
/// no calls), so the JIT executor actually runs these instructions.
#[test]
fn backends_agree_on_collection_ops() {
    // `compute` takes the collections as parameters so it is JIT-eligible
    // (construction via `List.new()`/`Map.new()` is an intrinsic and stays on the
    // interpreter in `main`). It exercises every collection op now in the tier-0
    // subset: List set/append/len/get/pop/clear and Map insert/get/remove.
    let source = "\
fn compute(xs: mut List<Int>, ys: read List<Int>, table: mut Map<Int, Int>) -> Int {
    List.set<Int>(list: mut xs, index: 2, value: read 100)
    List.append<Int>(list: mut xs, values: read ys)
    let mut total = 0
    let mut j = 0
    while j < List.len<Int>(list: read xs) {
        total = total + List.get<Int>(list: read xs, index: j)
        j = j + 1
    }
    match List.pop<Int>(list: mut xs) {
        Some(v) => {
            total = total + v
        }
        None => {
            total = total
        }
    }
    Map.insert<Int, Int>(map: mut table, key: read 9, value: read total)
    match Map.remove<Int, Int>(map: mut table, key: read 1) {
        Some(v) => {
            total = total + v
        }
        None => {
            total = total
        }
    }
    match Map.get<Int, Int>(map: read table, key: read 2) {
        Some(v) => {
            total = total + v
        }
        None => {
            total = total
        }
    }
    List.clear<Int>(list: mut xs)
    total = total + List.len<Int>(list: read xs)
    return total
}

fn main() -> Unit {
    let mut xs = List<Int>.new()
    let mut i = 0
    while i < 6 {
        let sq = i * i
        List.push<Int>(list: mut xs, value: read sq)
        i = i + 1
    }
    let mut ys = List<Int>.new()
    List.push<Int>(list: mut ys, value: read 7)
    let mut table = Map<Int, Int>.new()
    Map.insert<Int, Int>(map: mut table, key: read 1, value: read 50)
    Map.insert<Int, Int>(map: mut table, key: read 2, value: read 25)
    Log.write(message: read String.from_int(value: compute(xs: mut xs, ys: read ys, table: mut table)))
    return Unit
}
";
    common::differential::assert_backends_agree("jit-collections.rss", source, &[]);
}

/// A mut-heavy struct field-write loop plus mut/read parameter passing — gates the
/// copy-on-write `SetField` (mutate in place when the struct `Rc` is uniquely
/// owned, clone when shared). Full parity mode also checks generated Rust's
/// owned-struct lowering.
#[test]
fn backends_agree_on_struct_field_writes() {
    let source = "\
struct Box {
    v: Int,
    w: Int
}

fn total(b: read Box) -> Int {
    return b.v + b.w
}

fn main() -> Unit {
    let mut a = Box(v: 1, w: 2)
    let mut i = 0
    while i < 20 {
        a.v = a.v + a.w
        a.w = a.w + i + a.v
        // `total` borrows `a` (read) mid-loop — exercises the shared-Rc path
        // before the next in-place write.
        let snapshot = total(b: read a)
        a.v = a.v + snapshot
        i = i + 1
    }
    Log.write(message: read String.from_int(value: a.v))
    Log.write(message: read String.from_int(value: a.w))
    Log.write(message: read String.from_int(value: total(b: read a)))
    return Unit
}
";
    common::differential::assert_backends_agree("struct-field-writes.rss", source, &[]);
}

/// A `derives(Eq, Hash)` struct (with nested `List`/`Option` fields) used as a
/// `Map` key and `Set` element. The VM keys on a structural projection of the
/// value while generated Rust keys on the derived `Hash`/`Eq`; all enabled
/// backends agree on dedup (a structurally-equal, separately-constructed key overwrites),
/// membership, and the round-trip through `Map.keys()` (key value reconstruction).
/// Output is reduced to counts/booleans so hash *iteration order* — which legitimately
/// differs between the backends — is not under test.
#[test]
fn backends_agree_on_struct_keyed_collections() {
    let source = "\
struct Point derives(Clone, Eq, Hash) {
    x: Int,
    y: Int,
    tags: List<Int>
}

fn main() -> Unit {
    let counts = Map.new<Point, Int>()
    let p1 = Point(x: 1, y: 2, tags: List.new<Int>())
    let p2 = Point(x: 3, y: 4, tags: List.new<Int>())
    // Structurally identical to `p1` but a distinct allocation.
    let p1_again = Point(x: 1, y: 2, tags: List.new<Int>())

    Map.insert(map: mut counts, key: read p1, value: read 10)
    Map.insert(map: mut counts, key: read p2, value: read 20)
    // Same structural key: overwrites rather than adds.
    Map.insert(map: mut counts, key: read p1_again, value: read 99)

    Log.write(message: read String.from_int(value: Map.len(map: read counts)))
    Log.write(message: read String.from_bool(value: Map.contains_key(map: read counts, key: read p1_again)))
    let absent = Point(x: 9, y: 9, tags: List.new<Int>())
    Log.write(message: read String.from_bool(value: Map.contains_key(map: read counts, key: read absent)))
    // Round-trips keys back to `Point` values (key reconstruction).
    Log.write(message: read String.from_int(value: List.len(list: read Map.keys(map: read counts))))

    let seen = Set.new<Point>()
    Set.insert(set: mut seen, value: read p1)
    Set.insert(set: mut seen, value: read p2)
    Set.insert(set: mut seen, value: read p1_again)
    Log.write(message: read String.from_int(value: Set.len(set: read seen)))
    Log.write(message: read String.from_bool(value: Set.contains(set: read seen, value: read p1_again)))
    return Unit
}
";
    common::differential::assert_backends_agree("struct-keyed-collections.rss", source, &[]);
}

/// Deque / Set / SortedSet / SortedMap mutations with out-of-order inserts,
/// duplicates, and removes, then sorted/ordered dumps. This gates the collection
/// backing/representation optimizations: the VM (`Vec`-backed) and the compiled
/// backend (real `VecDeque`/ordered structures) must produce identical ordered
/// output, so any in-place / binary-search rewrite that broke ordering would fail
/// here. interp == jit == compiled (== native/force-deopt under the feature).
#[test]
fn backends_agree_on_ordered_collection_ops() {
    let source = "\
features: local

fn main() -> Unit {
    local deque = Deque<Int>.new()
    Deque.push_back<Int>(deque: mut deque, value: read 1)
    Deque.push_front<Int>(deque: mut deque, value: read 2)
    Deque.push_back<Int>(deque: mut deque, value: read 3)
    Deque.push_front<Int>(deque: mut deque, value: read 4)
    match Deque.pop_front<Int>(deque: mut deque) {
        Some(v) => { Log.write(message: read String.from_int(value: v)) }
        None => { Log.write(message: read \"none\") }
    }
    match Deque.pop_back<Int>(deque: mut deque) {
        Some(v) => { Log.write(message: read String.from_int(value: v)) }
        None => { Log.write(message: read \"none\") }
    }
    let dq = Deque.to_list<Int>(deque: read deque)
    let mut d = 0
    while d < Deque.len<Int>(deque: read deque) {
        Log.write(message: read String.from_int(value: dq[d]))
        d = d + 1
    }

    local sset = SortedSet<Int>.new()
    let order = [5, 1, 3, 1, 9, 2, 5, 7, 0]
    let mut k = 0
    while k < List.len<Int>(list: read order) {
        let _ins = SortedSet.insert<Int>(set: mut sset, value: read order[k])
        k = k + 1
    }
    let _r1 = SortedSet.remove<Int>(set: mut sset, value: read 3)
    let _r2 = SortedSet.remove<Int>(set: mut sset, value: read 42)
    let xs = SortedSet.to_list<Int>(set: read sset)
    let mut i = 0
    while i < SortedSet.len<Int>(set: read sset) {
        Log.write(message: read String.from_int(value: xs[i]))
        i = i + 1
    }

    local smap = SortedMap<Int, Int>.new()
    let mut m = 0
    while m < List.len<Int>(list: read order) {
        let entry_value = order[m] * 10
        SortedMap.insert<Int, Int>(map: mut smap, key: read order[m], value: read entry_value)
        m = m + 1
    }
    let _rm = SortedMap.remove<Int, Int>(map: mut smap, key: read 9)
    let ks = SortedMap.keys<Int, Int>(map: read smap)
    let vs = SortedMap.values<Int, Int>(map: read smap)
    let mut j = 0
    while j < SortedMap.len<Int, Int>(map: read smap) {
        Log.write(message: read String.from_int(value: ks[j]))
        Log.write(message: read String.from_int(value: vs[j]))
        j = j + 1
    }
    return Unit
}
";
    common::differential::assert_backends_agree("ordered-collections.rss", source, &[]);
}

/// Native heap reads: a loop function with a **struct parameter** (field reads via
/// `GetFieldSlot`) and a **list parameter** (`List.len`/`List.get`). These compile
/// natively now via host-helper calls (the struct/list passed as a handle), with
/// out-of-the-subset reads falling back. interp == jit == native == force-deopt ==
/// compiled.
#[test]
fn backends_agree_on_native_heap_reads() {
    let source = "\
struct Vec2 {
    x: Int,
    y: Int
}

fn blend(p: read Vec2, xs: read List<Int>, n: Int) -> Int {
    let mut acc = 0
    let mut i = 0
    let len = List.len<Int>(list: read xs)
    while i < n {
        acc = acc + p.x * 2 + p.y
        if i < len {
            acc = acc + List.get<Int>(list: read xs, index: i)
        }
        acc = acc - i
        i = i + 1
    }
    return acc
}

fn main() -> Unit {
    let xs = [10, 20, 30, 40]
    let total = blend(p: read Vec2(x: 5, y: 9), xs: read xs, n: read 64)
    Log.write(message: read String.from_int(value: total))
    return Unit
}
";
    common::differential::assert_backends_agree("jit-native-heap-reads.rss", source, &[]);
}

/// Native **Float** heap reads (Phase 3.2): a loop function with a `read List<Float>`
/// parameter and a `read Vec2f` struct parameter, reading and accumulating `f64`
/// scalars in a hot loop. The list-element and field reads compile to the new
/// `ListGetFloat`/`FieldFloat` host-helper calls (the list/struct passed as a
/// handle), with their `f64` results bail-checked exactly like the Int helpers.
/// These are side-effect-free reads, so the §7.2 deopt-from-top fallback stays
/// valid. interp == jit == native == force-deopt == compiled.
#[test]
fn backends_agree_on_native_float_heap_reads() {
    let source = "\
struct Vec2f {
    x: Float,
    y: Float
}

fn blend(p: read Vec2f, xs: read List<Float>, n: Int) -> Float {
    let mut acc = 0.0
    let mut i = 0
    let len = List.len<Float>(list: read xs)
    while i < n {
        acc = acc + p.x * 2.0 + p.y
        if i < len {
            acc = acc + List.get<Float>(list: read xs, index: i)
        }
        i = i + 1
    }
    return acc
}

fn main() -> Unit {
    let xs = [1.5, 2.25, 3.75, 4.0]
    let total = blend(p: read Vec2f(x: 0.5, y: 9.0), xs: read xs, n: read 64)
    Log.write(message: read String.from_float(value: total))
    return Unit
}
";
    common::differential::assert_backends_agree("jit-native-float-heap-reads.rss", source, &[]);
}

/// Failure-path differential for Float reads: an out-of-bounds `List.get<Float>`
/// in a hot loop must error identically on every backend — the native tier's
/// Float read helper must signal an immediate bail (not loop forever or use a
/// garbage 0.0), so the interpreter re-runs and produces the same error.
#[test]
fn backends_all_fail_on_out_of_bounds_float_list_get() {
    let source = "\
fn sum_n(xs: read List<Float>, n: Int) -> Float {
    let mut i = 0
    let mut acc = 0.0
    while i < n {
        acc = acc + List.get<Float>(list: read xs, index: i)
        i = i + 1
    }
    return acc
}

fn main() -> Unit {
    let xs = [1.0, 2.0, 3.0]
    Log.write(message: read String.from_float(value: sum_n(xs: read xs, n: read 8)))
    return Unit
}
";
    common::differential::assert_backends_all_fail("oob-float-list-get.rss", source, &[]);
}

/// TV2 (Valhalla) direct flat-array reads: a `read List<Float>` and a
/// `read List<Int>` param, each summed in its own hot loop, so the native tier
/// reclassifies them as flat-array params (`FlatFloat`/`FlatInt`) and lowers
/// `List.get`/`List.len` to **direct in-register loads** off the raw `f64`/`i64`
/// buffer — no per-element host call. The direct read must be bit-identical to the
/// interpreter's `TypedVec::get` materialization. interp == jit == native ==
/// force-deopt == compiled (the force-deopt mode proves the §7.2 fallback still
/// reproduces the exact result when the native path is suppressed).
#[test]
fn backends_agree_on_tv2_direct_flat_reads() {
    let source = "\
fn sum_floats(xs: read List<Float>, n: Int) -> Float {
    let mut i = 0
    let mut acc = 0.0
    let len = List.len<Float>(list: read xs)
    while i < n {
        acc = acc + List.get<Float>(list: read xs, index: i % len)
        i = i + 1
    }
    return acc
}

fn sum_ints(xs: read List<Int>, n: Int) -> Int {
    let mut i = 0
    let mut acc = 0
    let len = List.len<Int>(list: read xs)
    while i < n {
        acc = acc + List.get<Int>(list: read xs, index: i % len)
        i = i + 1
    }
    return acc
}

fn main() -> Unit {
    let fs = [1.5, 2.25, 3.75, 4.0, 5.5]
    let is = [10, 20, 30, 40]
    let f = sum_floats(xs: read fs, n: read 96)
    let s = sum_ints(xs: read is, n: read 96)
    Log.write(message: read String.from_float(value: f))
    Log.write(message: read String.from_int(value: s))
    return Unit
}
";
    common::differential::assert_backends_agree("tv2-direct-flat-reads.rss", source, &[]);
}

/// TV2 failure path: an out-of-bounds direct flat read (`List<Int>` indexed past
/// its length in a hot loop) must error identically on every tier — the native
/// tier's direct read must branch to fallback on OOB (not load garbage or loop),
/// so the interpreter re-runs and produces the same error across interp == jit ==
/// native == force-deopt == compiled.
#[test]
fn backends_all_fail_on_tv2_out_of_bounds_direct_read() {
    let source = "\
fn sum_n(xs: read List<Int>, n: Int) -> Int {
    let mut i = 0
    let mut acc = 0
    while i < n {
        acc = acc + List.get<Int>(list: read xs, index: i)
        i = i + 1
    }
    return acc
}

fn main() -> Unit {
    let xs = [7, 8, 9]
    Log.write(message: read String.from_int(value: sum_n(xs: read xs, n: read 64)))
    return Unit
}
";
    common::differential::assert_backends_all_fail("tv2-oob-direct-read.rss", source, &[]);
}

/// Real self-hosted tool, cross-backend: the RSS package manifest inspector
/// (`benchmarks/micro/selfhost_manifest_inspector.rss`) parses a fixture `rsspkg.toml`
/// and emits a JSON report. This is a hardening check on a realistic
/// intrinsic/IO/error-handling workload (not a numeric microbenchmark). An
/// absolute fixture path keeps every enabled backend, including the AOT
/// subprocess in full mode, reading the same file.
#[test]
fn backends_agree_on_manifest_inspector() {
    let source = include_str!("../../../benchmarks/micro/selfhost_manifest_inspector.rss");
    let fixture = common::workspace_root()
        .join("benchmarks/micro/fixtures/package-medium/rsspkg.toml")
        .display()
        .to_string();
    common::differential::assert_backends_agree(
        "selfhost-manifest-inspector.rss",
        source,
        &[fixture.as_str()],
    );
}

/// Scalar field assignment through a `mut` parameter must propagate to the caller
/// on every enabled backend (VM write-back == AOT `&mut` in full mode).
/// Regression for ledger SH-013.
#[test]
fn backends_agree_on_mut_param_field_assignment() {
    let source = "features: local\n\
\n\
struct Tally {\n\
    n: Int\n\
}\n\
\n\
fn bump(c: mut Tally) -> Unit {\n\
    c.n = c.n + 1\n\
}\n\
\n\
fn add(c: mut Tally, amount: Int) -> Unit {\n\
    c.n = c.n + amount\n\
}\n\
\n\
fn main() -> Unit {\n\
    let mut c = Tally(n: 0)\n\
    bump(c: mut c)\n\
    bump(c: mut c)\n\
    add(c: mut c, amount: 10)\n\
    Log.write(message: read String.from_int(value: c.n))\n\
    return Unit\n\
}\n";
    common::differential::assert_backends_agree("mut-param-field.rss", source, &[]);
}

/// A generic collection implemented in RSS itself: `benchmarks/micro/selfhost_mailbox.rss`
/// (a fixed-capacity `Mailbox<T>` over parallel lists, with oldest-first and
/// source-filtered takes). Regression for the generic-call lowering fix (a generic
/// function call with explicit type args used to be mis-lowered as a struct
/// construction). interp == jit == native == force-deopt == compiled.
#[test]
fn backends_agree_on_selfhost_mailbox() {
    let source = include_str!("../../../benchmarks/micro/selfhost_mailbox.rss");
    common::differential::assert_backends_agree("selfhost-mailbox.rss", source, &[]);
}

/// Real self-hosted tool, cross-backend: the stdlib conformance reporter
/// (`benchmarks/micro/selfhost_stdlib_reporter.rss`) — an IO-free, loop-heavy battery of
/// List/Map/Option checks emitting a JSON summary. interp == jit == native ==
/// force-deopt == compiled.
#[test]
fn backends_agree_on_stdlib_reporter() {
    let source = include_str!("../../../benchmarks/micro/selfhost_stdlib_reporter.rss");
    common::differential::assert_backends_agree("selfhost-stdlib-reporter.rss", source, &[]);
}

/// Failure-path differential for a real tool (gates ledger SH-005): the manifest
/// inspector must fail on every backend when the manifest is malformed (missing
/// `[package]`) or absent — a `main` returning `Err` is now a failed run on the
/// VM, JIT, native, and AOT alike.
#[test]
fn backends_all_fail_on_bad_manifest() {
    let source = include_str!("../../../benchmarks/micro/selfhost_manifest_inspector.rss");
    let missing_field = common::workspace_root()
        .join("benchmarks/micro/fixtures/package-bad/rsspkg.toml")
        .display()
        .to_string();
    common::differential::assert_backends_all_fail(
        "bad-manifest.rss",
        source,
        &[missing_field.as_str()],
    );

    let absent = common::workspace_root()
        .join("benchmarks/micro/fixtures/does-not-exist/rsspkg.toml")
        .display()
        .to_string();
    common::differential::assert_backends_all_fail(
        "absent-manifest.rss",
        source,
        &[absent.as_str()],
    );
}

/// Failure-path differential (the case that matters most for semantic hardening):
/// an out-of-bounds `List.get` must error on *every* backend. The loop-condition
/// variant specifically guards the native tier's immediate-bail behaviour — a
/// failed read must fall back at once, not loop forever or use a garbage 0.
#[test]
fn backends_all_fail_on_out_of_bounds_list_get_in_condition() {
    let source = "\
fn scan(xs: read List<Int>) -> Int {
    let mut i = 0
    let mut acc = 0
    while List.get<Int>(list: read xs, index: i) >= 0 {
        acc = acc + i
        i = i + 1
    }
    return acc
}

fn main() -> Unit {
    let xs = [1, 2, 3]
    Log.write(message: read String.from_int(value: scan(xs: read xs)))
    return Unit
}
";
    common::differential::assert_backends_all_fail("oob-list-get-loop.rss", source, &[]);
}

/// Failure-path differential: a plain out-of-bounds `List.get` in a hot loop body
/// (the index runs past the list) errors on every backend.
#[test]
fn backends_all_fail_on_out_of_bounds_list_get() {
    let source = "\
fn sum_n(xs: read List<Int>, n: Int) -> Int {
    let mut i = 0
    let mut acc = 0
    while i < n {
        acc = acc + List.get<Int>(list: read xs, index: i)
        i = i + 1
    }
    return acc
}

fn main() -> Unit {
    let xs = [1, 2, 3]
    Log.write(message: read String.from_int(value: sum_n(xs: read xs, n: read 8)))
    return Unit
}
";
    common::differential::assert_backends_all_fail("oob-list-get.rss", source, &[]);
}

/// JIT-eligible functions that call other JIT-eligible functions: the tier-0
/// executor now drives non-suspending, non-recursive callees in-line. `accumulate`
/// has a loop (so it is JIT'd) and calls two leaf helpers — interp == jit ==
/// compiled.
#[test]
fn backends_agree_on_cross_function_calls() {
    let source = "\
fn square(n: Int) -> Int {
    return n * n
}

fn weight(n: Int) -> Int {
    if n < 0 {
        return 0 - n
    }
    return n
}

fn accumulate(limit: Int) -> Int {
    let mut total = 0
    let mut i = 0
    while i < limit {
        total = total + square(n: read i) - weight(n: read i)
        i = i + 1
    }
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: accumulate(limit: read 8)))
    return Unit
}
";
    common::differential::assert_backends_agree("jit-cross-call.rss", source, &[]);
}

/// Inlining of callees with **internal control flow** (branches and a loop) — the
/// native inliner splices the callee body into a fresh register window, remaps its
/// internal jump targets, and routes each `Return` to the post-call join. `driver`
/// has a loop (so it is JIT'd) and calls `clampish` (two branches) and `digits`
/// (a loop). interp == jit == native == force-deopt == compiled.
#[test]
fn backends_agree_on_branchy_inlined_calls() {
    let source = "\
fn clampish(x: Int, lo: Int, hi: Int) -> Int {
    if x < lo {
        return lo
    }
    if x > hi {
        return hi
    }
    return x
}

fn digits(value: Int) -> Int {
    let mut n = value
    if n < 0 {
        n = 0 - n
    }
    let mut count = 0
    while n > 0 {
        count = count + 1
        n = n / 10
    }
    return count
}

fn driver(limit: Int) -> Int {
    let mut total = 0
    let mut i = 0
    while i < limit {
        total = total + clampish(x: read i, lo: read 3, hi: read 17) + digits(value: read i)
        i = i + 1
    }
    return total
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: driver(limit: read 250)))
    return Unit
}
";
    common::differential::assert_backends_agree("jit-branchy-inline.rss", source, &[]);
}

/// A function with **float parameters** plus an `Int` loop counter — exercises the
/// native JIT's unification-based parameter typing (the float params are inferred
/// `Float` via the `bias` anchor). interp == jit == native == compiled.
#[test]
fn backends_agree_on_float_param_function() {
    let source = "\
fn blend(x: Float, k: Float, n: Int) -> Float {
    let bias = 0.5
    let mut acc = x
    let mut i = 0
    while i < n {
        acc = acc * k + bias - x
        i = i + 1
    }
    return acc
}

fn main() -> Unit {
    Log.write(message: read String.from_float(value: blend(x: read 1.25, k: read 0.5, n: read 16)))
    return Unit
}
";
    common::differential::assert_backends_agree("jit-float-params.rss", source, &[]);
}

proptest! {
    // Default cases stay small and in-process. Raise RSS_DIFF_PROPTEST_CASES
    // for soak runs; with RSSCRIPT_FULL_BACKEND_PARITY=1 each case also compiles
    // a generated crate, so keep that mode explicit.
    #![proptest_config(ProptestConfig {
        cases: differential_proptest_cases(),
        max_shrink_iters: differential_proptest_shrink_iters(),
        ..ProptestConfig::default()
    })]

    #[test]
    fn backends_agree_on_integer_programs(program in arb_program()) {
        // Skip cases that overflow i64 — every backend errors there, and the goal
        // is to compare successful runs.
        prop_assume!(oracle(&program).is_some());
        let source = render(&program);
        // N-way: VM interpreter == JIT == compiled Rust. A future native JIT tier
        // is checked here automatically.
        common::differential::assert_backends_agree("backend-diff.rss", &source, &[]);
    }

    #[test]
    fn backends_agree_on_float_programs(program in arb_float_program()) {
        // Float arithmetic + comparisons, three-way. No division (avoids
        // div-by-zero) and no NaN-producing ops, so every case succeeds and is
        // deterministic; the result is reduced to an Int count of true
        // comparisons so float *formatting* parity is not the thing under test.
        let source = render_float(&program);
        common::differential::assert_backends_agree("backend-diff-float.rss", &source, &[]);
    }

    #[test]
    fn backends_agree_on_string_programs(program in arb_string_program()) {
        // String concatenation chains + length, three-way. Concatenation cannot
        // fail, so no case is skipped.
        let source = render_string(&program);
        common::differential::assert_backends_agree("backend-diff-string.rss", &source, &[]);
    }

    #[test]
    fn backends_agree_on_bytes_programs(program in arb_bytes_program()) {
        // `Bytes.from_string`/`concat`/`slice` chains; the result's *length* is
        // printed (raw-byte display isn't the thing under test). N-way.
        let source = render_bytes(&program);
        common::differential::assert_backends_agree("backend-diff-bytes.rss", &source, &[]);
    }

    /// Coverage-style fuzz seed: a raw byte string is decoded into a program by
    /// [`program_from_seed`], then run through the N-way differential. This is the
    /// Fuzzilli-style `seed(bytes) -> program` shape; proptest supplies the random
    /// seeds and shrinking (the "mutators"). Pointing a coverage-guided engine
    /// (cargo-fuzz / libFuzzer) at `program_from_seed` is the deployment step — the
    /// decoder is total (any byte string yields a valid program), which is exactly
    /// what such engines require.
    #[test]
    fn backends_agree_on_seed_decoded_programs(seed in prop::collection::vec(any::<u8>(), 0..64)) {
        let program = program_from_seed(&seed);
        prop_assume!(oracle(&program).is_some());
        let source = render(&program);
        common::differential::assert_backends_agree("backend-diff-seed.rss", &source, &[]);
    }
}

// --- Coverage-style seed -> program decoder -------------------------------

/// A cursor over a fuzz seed. Reads wrap around (and an empty seed reads as all
/// zeroes), so **every** byte string decodes to a valid program — the totality a
/// coverage-guided fuzzer needs.
struct SeedReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> SeedReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn next(&mut self) -> u8 {
        if self.bytes.is_empty() {
            return 0;
        }
        let b = self.bytes[self.pos % self.bytes.len()];
        self.pos = self.pos.wrapping_add(1);
        b
    }

    /// A value in `0..n` (`0` when `n == 0`).
    fn range(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { self.next() as usize % n }
    }
}

/// Decode a fuzz seed into an integer [`Program`] (same shape as `arb_program`).
fn program_from_seed(seed: &[u8]) -> Program {
    let mut r = SeedReader::new(seed);
    let binding_count = r.range(3); // 0..=2
    let mut bindings = Vec::new();
    for index in 0..binding_count {
        bindings.push(expr_from_seed(&mut r, index));
    }
    let count = bindings.len();
    let guard_count = r.range(3); // 0..=2
    let mut guards = Vec::new();
    for _ in 0..guard_count {
        guards.push(Guard {
            lhs: expr_from_seed(&mut r, count),
            cmp: cmp_from_seed(&mut r),
            rhs: expr_from_seed(&mut r, count),
            adjustment: expr_from_seed(&mut r, count),
        });
    }
    let result = expr_from_seed(&mut r, count);
    Program {
        bindings,
        guards,
        result,
    }
}

fn expr_from_seed(r: &mut SeedReader, vars_in_scope: usize) -> Expr {
    let terms = 1 + r.range(3); // 1..=3
    (0..terms)
        .map(|_| {
            let op = if r.next() & 1 == 0 { '+' } else { '-' };
            let factor_count = 1 + r.range(2); // 1..=2
            let factors = (0..factor_count)
                .map(|_| factor_from_seed(r, vars_in_scope))
                .collect();
            (op, factors)
        })
        .collect()
}

fn factor_from_seed(r: &mut SeedReader, vars_in_scope: usize) -> Factor {
    if vars_in_scope > 0 && r.next() & 1 == 0 {
        Factor::Var(r.range(vars_in_scope))
    } else {
        Factor::Lit(i64::from(r.next()) % 64)
    }
}

fn cmp_from_seed(r: &mut SeedReader) -> Cmp {
    match r.range(6) {
        0 => Cmp::Lt,
        1 => Cmp::Le,
        2 => Cmp::Gt,
        3 => Cmp::Ge,
        4 => Cmp::Eq,
        _ => Cmp::Ne,
    }
}

// --- Float programs -------------------------------------------------------

/// Exact-in-binary float literals so arithmetic is reproducible bit-for-bit
/// across backends (all are sums of small powers of two).
const FLOAT_LITS: [f64; 6] = [0.0, 0.5, 1.0, 1.5, 2.0, 2.5];

#[derive(Debug, Clone)]
enum FloatFactor {
    Lit(usize),
    Var(usize),
}

type FloatTerm = (char, Vec<FloatFactor>);
type FloatExpr = Vec<FloatTerm>;

/// A float program: a chain of `let` bindings (float sum-of-products) and a set
/// of comparisons whose true-count is returned as an `Int`.
#[derive(Debug, Clone)]
struct FloatProgram {
    bindings: Vec<FloatExpr>,
    comparisons: Vec<(FloatExpr, Cmp, FloatExpr)>,
}

fn arb_float_factor(vars_in_scope: usize) -> impl Strategy<Value = FloatFactor> {
    if vars_in_scope == 0 {
        (0..FLOAT_LITS.len()).prop_map(FloatFactor::Lit).boxed()
    } else {
        prop_oneof![
            (0..FLOAT_LITS.len()).prop_map(FloatFactor::Lit),
            (0..vars_in_scope).prop_map(FloatFactor::Var),
        ]
        .boxed()
    }
}

fn arb_float_expr(vars_in_scope: usize) -> impl Strategy<Value = FloatExpr> {
    // Only `+`/`-` between terms and `*` within a term — no division, so no
    // div-by-zero and no irrational/long-decimal intermediate values.
    let term = (
        prop_oneof![Just('+'), Just('-')],
        prop::collection::vec(arb_float_factor(vars_in_scope), 1..=2),
    );
    prop::collection::vec(term, 1..=3)
}

fn arb_float_program() -> impl Strategy<Value = FloatProgram> {
    (0usize..=2)
        .prop_flat_map(|binding_count| {
            let mut strategy = Just(Vec::<FloatExpr>::new()).boxed();
            for index in 0..binding_count {
                strategy = (strategy, arb_float_expr(index))
                    .prop_map(|(mut bindings, expr)| {
                        bindings.push(expr);
                        bindings
                    })
                    .boxed();
            }
            strategy.prop_flat_map(|bindings| {
                let count = bindings.len();
                let cmp = prop_oneof![
                    Just(Cmp::Lt),
                    Just(Cmp::Le),
                    Just(Cmp::Gt),
                    Just(Cmp::Ge),
                    Just(Cmp::Eq),
                    Just(Cmp::Ne),
                ];
                let comparison = (arb_float_expr(count), cmp, arb_float_expr(count));
                prop::collection::vec(comparison, 1..=3).prop_map(move |comparisons| FloatProgram {
                    bindings: bindings.clone(),
                    comparisons,
                })
            })
        })
        .boxed()
}

fn render_float_factor(factor: &FloatFactor) -> String {
    match factor {
        // `{:?}` always prints a decimal point (e.g. `2.0`), keeping it a Float
        // literal rather than an Int.
        FloatFactor::Lit(index) => format!("{:?}", FLOAT_LITS[*index]),
        FloatFactor::Var(index) => format!("f{index}"),
    }
}

fn render_float_expr(expr: &FloatExpr) -> String {
    let mut rendered = String::new();
    for (index, (op, factors)) in expr.iter().enumerate() {
        if index > 0 {
            rendered.push_str(&format!(" {op} "));
        }
        let product = factors
            .iter()
            .map(render_float_factor)
            .collect::<Vec<_>>()
            .join(" * ");
        rendered.push_str(&product);
    }
    rendered
}

fn render_float(program: &FloatProgram) -> String {
    let mut source = String::from("fn compute() -> Int {\n");
    for (index, binding) in program.bindings.iter().enumerate() {
        source.push_str(&format!(
            "    let f{index} = {}\n",
            render_float_expr(binding)
        ));
    }
    source.push_str("    let mut count = 0\n");
    for (lhs, cmp, rhs) in &program.comparisons {
        source.push_str(&format!(
            "    if {} {} {} {{\n        count = count + 1\n    }}\n",
            render_float_expr(lhs),
            cmp.render(),
            render_float_expr(rhs),
        ));
    }
    source.push_str("    return count\n}\n\n");
    source.push_str("fn main() -> Unit {\n");
    source.push_str("    Log.write(message: read String.from_int(value: compute()))\n");
    source.push_str("    return Unit\n}\n");
    source
}

// --- String programs ------------------------------------------------------

const STRING_LITS: [&str; 5] = ["", "a", "bc", "Z9", "  "];

#[derive(Debug, Clone)]
enum StringAtom {
    Lit(usize),
    Var(usize),
}

/// A non-empty left-folded concatenation of atoms.
type StringExpr = Vec<StringAtom>;

#[derive(Debug, Clone)]
struct StringProgram {
    bindings: Vec<StringExpr>,
    result: StringExpr,
}

fn arb_string_atom(vars_in_scope: usize) -> impl Strategy<Value = StringAtom> {
    if vars_in_scope == 0 {
        (0..STRING_LITS.len()).prop_map(StringAtom::Lit).boxed()
    } else {
        prop_oneof![
            (0..STRING_LITS.len()).prop_map(StringAtom::Lit),
            (0..vars_in_scope).prop_map(StringAtom::Var),
        ]
        .boxed()
    }
}

fn arb_string_expr(vars_in_scope: usize) -> impl Strategy<Value = StringExpr> {
    prop::collection::vec(arb_string_atom(vars_in_scope), 1..=3)
}

fn arb_string_program() -> impl Strategy<Value = StringProgram> {
    (0usize..=2)
        .prop_flat_map(|binding_count| {
            let mut strategy = Just(Vec::<StringExpr>::new()).boxed();
            for index in 0..binding_count {
                strategy = (strategy, arb_string_expr(index))
                    .prop_map(|(mut bindings, expr)| {
                        bindings.push(expr);
                        bindings
                    })
                    .boxed();
            }
            strategy.prop_flat_map(|bindings| {
                let count = bindings.len();
                arb_string_expr(count).prop_map(move |result| StringProgram {
                    bindings: bindings.clone(),
                    result,
                })
            })
        })
        .boxed()
}

fn render_string_atom(atom: &StringAtom) -> String {
    match atom {
        StringAtom::Lit(index) => format!("{:?}", STRING_LITS[*index]),
        StringAtom::Var(index) => format!("s{index}"),
    }
}

/// Left-fold the atoms into nested `String.concat` calls, seeded with an empty
/// string. Folding from `""` means every atom (including a lone variable) is
/// passed through `read`, i.e. cloned — so a variable stays usable after being
/// referenced, sidestepping String move semantics that are not under test here.
fn render_string_expr(expr: &StringExpr) -> String {
    let mut acc = String::from("\"\"");
    for atom in expr {
        acc = format!(
            "String.concat(left: read {acc}, right: read {})",
            render_string_atom(atom)
        );
    }
    acc
}

fn render_string(program: &StringProgram) -> String {
    let mut source = String::from("fn main() -> Unit {\n");
    for (index, binding) in program.bindings.iter().enumerate() {
        source.push_str(&format!(
            "    let s{index} = {}\n",
            render_string_expr(binding)
        ));
    }
    source.push_str(&format!(
        "    let result = {}\n",
        render_string_expr(&program.result)
    ));
    source.push_str("    Log.write(message: read result)\n");
    source.push_str(
        "    Log.write(message: read String.from_int(value: String.len(value: read result)))\n",
    );
    source.push_str("    return Unit\n}\n");
    source
}

// --- Bytes programs -------------------------------------------------------

const BYTES_LITS: [&str; 5] = ["", "a", "byte", "Z9", "  "];

#[derive(Debug, Clone)]
enum BytesAtom {
    Lit(usize),
    Var(usize),
}

type BytesExpr = Vec<BytesAtom>;

#[derive(Debug, Clone)]
struct BytesProgram {
    bindings: Vec<BytesExpr>,
    result: BytesExpr,
    /// Optional `(start, len)` slice applied to the result before measuring.
    slice: Option<(i64, i64)>,
}

fn arb_bytes_atom(vars_in_scope: usize) -> impl Strategy<Value = BytesAtom> {
    if vars_in_scope == 0 {
        (0..BYTES_LITS.len()).prop_map(BytesAtom::Lit).boxed()
    } else {
        prop_oneof![
            (0..BYTES_LITS.len()).prop_map(BytesAtom::Lit),
            (0..vars_in_scope).prop_map(BytesAtom::Var),
        ]
        .boxed()
    }
}

fn arb_bytes_expr(vars_in_scope: usize) -> impl Strategy<Value = BytesExpr> {
    prop::collection::vec(arb_bytes_atom(vars_in_scope), 1..=3)
}

fn arb_bytes_program() -> impl Strategy<Value = BytesProgram> {
    (0usize..=2)
        .prop_flat_map(|binding_count| {
            let mut strategy = Just(Vec::<BytesExpr>::new()).boxed();
            for index in 0..binding_count {
                strategy = (strategy, arb_bytes_expr(index))
                    .prop_map(|(mut bindings, expr)| {
                        bindings.push(expr);
                        bindings
                    })
                    .boxed();
            }
            strategy.prop_flat_map(|bindings| {
                let count = bindings.len();
                let slice = prop::option::of((0i64..=6, 0i64..=6));
                (arb_bytes_expr(count), slice).prop_map(move |(result, slice)| BytesProgram {
                    bindings: bindings.clone(),
                    result,
                    slice,
                })
            })
        })
        .boxed()
}

fn render_bytes_atom(atom: &BytesAtom) -> String {
    match atom {
        BytesAtom::Lit(index) => {
            format!("Bytes.from_string(value: read {:?})", BYTES_LITS[*index])
        }
        BytesAtom::Var(index) => format!("b{index}"),
    }
}

/// Left-fold the atoms into nested `Bytes.concat`, seeded with empty bytes so
/// every atom is `read` (cloned) — same move-sidestep as the string generator.
fn render_bytes_expr(expr: &BytesExpr) -> String {
    let mut acc = String::from("Bytes.from_string(value: read \"\")");
    for atom in expr {
        acc = format!(
            "Bytes.concat(left: read {acc}, right: read {})",
            render_bytes_atom(atom)
        );
    }
    acc
}

fn render_bytes(program: &BytesProgram) -> String {
    let mut source = String::from("fn main() -> Unit {\n");
    for (index, binding) in program.bindings.iter().enumerate() {
        source.push_str(&format!(
            "    let b{index} = {}\n",
            render_bytes_expr(binding)
        ));
    }
    source.push_str(&format!(
        "    let result = {}\n",
        render_bytes_expr(&program.result)
    ));
    if let Some((start, len)) = program.slice {
        source.push_str(&format!(
            "    let sliced = Bytes.slice(value: read result, start: {start}, len: {len})\n",
        ));
        source.push_str(
            "    Log.write(message: read String.from_int(value: Bytes.len(value: read sliced)))\n",
        );
    } else {
        source.push_str(
            "    Log.write(message: read String.from_int(value: Bytes.len(value: read result)))\n",
        );
    }
    source.push_str("    return Unit\n}\n");
    source
}
