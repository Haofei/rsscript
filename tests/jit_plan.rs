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
    // Both `add` and `main` are eligible: `add` is pure, and `main`'s only
    // non-pure instruction is a `CallKnown` to `add`, which the tier-0 executor
    // now runs cross-function (the call graph here is non-suspending + acyclic).
    assert_eq!(
        plan.eligible_functions, plan.total_functions,
        "pure `add` and its non-recursive caller `main` should both be JIT-eligible: {plan:?}"
    );
}

#[test]
fn jit_plan_marks_recursive_functions_ineligible() {
    // The tier-0 executor runs callees on the host stack, so a call cycle could
    // overflow where the stackless interpreter would not. Recursive functions are
    // therefore deliberately excluded and fall back to the interpreter.
    let source = "\
fn fact(n: Int) -> Int {
    if n <= 1 {
        return 1
    }
    return n * fact(n: read n - 1)
}

fn main() -> Int {
    return fact(n: read 5)
}
";
    let executable =
        rsscript::reg_vm_compile_source("jit-recursive.rss", source).expect("source compiles");
    let plan = executable.jit_plan();
    // `fact` is recursive (a self-cycle); `main` calls `fact`, so it can reach the
    // cycle. Both fall back; only the implicit helpers (if any) could be eligible.
    assert!(
        plan.fallback_functions >= 2,
        "recursive `fact` and its caller `main` should fall back: {plan:?}"
    );
}

#[test]
fn jit_plan_marks_collection_index_functions_eligible() {
    // List/Map get/set/index ops are closure-free and frame-free, so a function
    // built only from them (plus arithmetic/control) is in the tier-0 subset.
    // Construction (`List.new()`) is an intrinsic, so the collection is taken as a
    // parameter; the caller (which constructs) falls back as usual.
    let source = "\
fn process(xs: mut List<Int>) -> Int {
    List.set<Int>(list: mut xs, index: 0, value: read 9)
    let mut total = 0
    let mut j = 0
    while j < List.len<Int>(list: read xs) {
        total = total + List.get<Int>(list: read xs, index: j)
        j = j + 1
    }
    return total
}

fn main() -> Int {
    let mut xs = List<Int>.new()
    List.push<Int>(list: mut xs, value: read 1)
    List.push<Int>(list: mut xs, value: read 2)
    return process(xs: mut xs)
}
";
    let executable =
        rsscript::reg_vm_compile_source("jit-collections.rss", source).expect("source compiles");
    let plan = executable.jit_plan();
    assert!(
        plan.eligible_functions >= 1,
        "collection-index `process` should be JIT-eligible: {plan:?}"
    );
}
