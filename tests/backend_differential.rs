//! Generative VM <-> compiler differential testing.
//!
//! The curated parity suite (vm_eval / vm_eval_parity / corpus) only compares
//! hand-picked inputs. This test *generates* valid RSScript programs and asserts
//! the register VM and the compiled Rust backend produce identical output — so a
//! divergence on an input nobody thought to write is still caught (proptest
//! shrinks any failure to a minimal reproducer).
//!
//! Programs are `let`-binding chains of checked integer arithmetic
//! (sum-of-products over literals and earlier variables), then print the result.
//! This stays inside the parser surface (no parens / unary minus), is always
//! type-correct, and skips overflowing cases (both backends error there).
//!
//! Each case compiles a generated crate, so the case count is modest by design;
//! raise it for a longer differential-fuzz run.

mod common;

use proptest::prelude::*;

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

/// Eligible function with parameters (exercises DeepCopy + integer arithmetic in
/// the JIT) — interp == jit == compiled.
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
    // Each case compiles a crate, so keep the count modest. Raise for a longer
    // differential-fuzz session (e.g. PROPTEST_CASES=200).
    #![proptest_config(ProptestConfig { cases: 24, max_shrink_iters: 64, ..ProptestConfig::default() })]

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
        source.push_str(&format!("    let b{index} = {}\n", render_bytes_expr(binding)));
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
