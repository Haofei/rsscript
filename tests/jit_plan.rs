//! Tier-0 JIT eligibility analysis (the seam where native codegen plugs in).

#[test]
fn jit_plan_marks_pure_integer_functions_eligible() {
    let source = "\
fn add(a: Int, b: Int) -> Int {
    return a + b
}

fn main() -> Int {
    return add(a: read 2, b: read 3)
}
";
    let executable = rsscript::reg_vm_compile_source("jit.rss", source).expect("source compiles");
    let plan = executable.jit_plan();
    assert!(plan.total_functions >= 2, "{plan:?}");
    assert_eq!(
        plan.eligible_functions + plan.fallback_functions,
        plan.total_functions,
        "plan must classify every function: {plan:?}"
    );
    assert!(
        plan.eligible_functions >= 1,
        "pure-integer `add` should be JIT-eligible: {plan:?}"
    );
    assert!(
        plan.fallback_functions >= 1,
        "`main` (which calls a function) should fall back: {plan:?}"
    );
}
