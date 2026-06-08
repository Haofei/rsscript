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

// NOTE: literal magnitudes and the binding-chain depth below are deliberately
// small. RSScript `Int` is i64, but the compiled backend currently lowers integer
// literals without an `i64` suffix, so an all-literal-derived sub-expression
// defaults to Rust `i32` and can *const-overflow at compile time* even though the
// i64 value is fine (e.g. `3528_i32 * 3457776_i32`). That is a real, separate
// VM<->compiler gap (tracked in docs/jit-todo.md); we keep generated values well
// under i32::MAX here so this differential exercises VM<->JIT parity rather than
// re-finding that one compiler bug on every run.
fn arb_factor(vars_in_scope: usize) -> impl Strategy<Value = Factor> {
    if vars_in_scope == 0 {
        (0i64..=4).prop_map(Factor::Lit).boxed()
    } else {
        prop_oneof![
            (0i64..=4).prop_map(Factor::Lit),
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
    source.push_str(&format!("    let mut acc = {}\n", render_expr(&program.result)));
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
}
