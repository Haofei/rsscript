//! Property-based tests.
//!
//! Generative differential checking: random integer-arithmetic programs are run
//! on the register VM and compared against a Rust oracle using the same checked
//! semantics. Programs are generated as a flat sum-of-products
//! (`f*f*f + f*f - f`) — no parentheses or unary minus (outside the parser
//! surface) and relying on standard `*`-before-`+/-` precedence, which the VM
//! and the oracle share. Overflowing cases are skipped.

mod common;

use proptest::prelude::*;

/// One additive term: an operator (`+`/`-`, ignored for the first term) and the
/// factors of a product.
type Term = (char, Vec<i64>);

fn arb_program() -> impl Strategy<Value = Vec<Term>> {
    let factor = 0i64..=20;
    let product = prop::collection::vec(factor, 1..=4);
    let term = (prop_oneof![Just('+'), Just('-')], product);
    prop::collection::vec(term, 1..=4)
}

/// Reference evaluation with checked arithmetic and standard precedence; `None`
/// on overflow.
fn oracle(terms: &[Term]) -> Option<i64> {
    let mut total: i64 = 0;
    for (index, (op, factors)) in terms.iter().enumerate() {
        let mut product: i64 = 1;
        for factor in factors {
            product = product.checked_mul(*factor)?;
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

fn render(terms: &[Term]) -> String {
    let mut rendered = String::new();
    for (index, (op, factors)) in terms.iter().enumerate() {
        if index > 0 {
            rendered.push_str(&format!(" {op} "));
        }
        let product = factors
            .iter()
            .map(i64::to_string)
            .collect::<Vec<_>>()
            .join(" * ");
        rendered.push_str(&product);
    }
    rendered
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn vm_integer_arithmetic_matches_oracle(program in arb_program()) {
        let expected = oracle(&program);
        prop_assume!(expected.is_some());
        let source = format!("fn main() -> Int {{\n    return {}\n}}\n", render(&program));
        let output = common::run_vm_source("property-arith.rss", &source, &[])
            .expect("generated arithmetic program should evaluate on the VM");
        prop_assert_eq!(output.display_value, expected.unwrap().to_string());
    }
}
