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

/// A program: a chain of `let` bindings (each an expression over earlier vars)
/// plus a final expression that is printed.
#[derive(Debug, Clone)]
struct Program {
    bindings: Vec<Expr>,
    result: Expr,
}

fn arb_factor(vars_in_scope: usize) -> impl Strategy<Value = Factor> {
    if vars_in_scope == 0 {
        (0i64..=12).prop_map(Factor::Lit).boxed()
    } else {
        prop_oneof![
            (0i64..=12).prop_map(Factor::Lit),
            (0..vars_in_scope).prop_map(Factor::Var),
        ]
        .boxed()
    }
}

fn arb_expr(vars_in_scope: usize) -> impl Strategy<Value = Expr> {
    let term = (
        prop_oneof![Just('+'), Just('-')],
        prop::collection::vec(arb_factor(vars_in_scope), 1..=3),
    );
    prop::collection::vec(term, 1..=3)
}

fn arb_program() -> impl Strategy<Value = Program> {
    (0usize..=3)
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
                arb_expr(count).prop_map(move |result| Program {
                    bindings: bindings.clone(),
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
    eval_expr(&program.result, &vars)
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
    source.push_str(&format!("    let result = {}\n", render_expr(&program.result)));
    source.push_str("    return result\n}\n\n");
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
