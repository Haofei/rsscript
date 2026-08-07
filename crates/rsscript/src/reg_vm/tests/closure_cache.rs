#[cfg(test)]
mod closure_cache_tests {
    use super::super::*;

    /// Lower a source program to a `RegUnit`, exactly as `reg_vm_compile_source`
    /// does, so tests can inspect the closure-identity gate and the cache.
    fn unit(source: &str) -> RegUnit {
        let mut program = parse_source("test.rss", source);
        crate::syntax::isolate_module_namespaces(&mut program);
        let hir = crate::hir::Hir::from_syntax_with_standard_package_interfaces(&program);
        RegUnit::lower(&rsscript_lowering::ExecutableIr::from_validated_hir(&hir))
            .expect("lowering should succeed")
    }

    /// A program that never compares closures must leave the gate OFF, so the
    /// non-capturing-closure cache is permitted to share one `Rc`.
    #[test]
    fn gate_off_when_no_closure_equality() {
        let source = r#"
fn apply(f: noescape Fn(Int) -> Int, x: Int) -> Int {
    return f(x)
}

fn main() -> Unit {
    let mut i = 0
    let mut total = 0
    while i < 3 {
        let g = |x| { return x * 2 + 1 }
        total = total + apply(f: read g, x: read i)
        i = i + 1
    }
    Output.write(message: read String.from_int(value: total))
    return Unit
}
"#;
        assert!(
            !unit(source).closure_identity_observable,
            "no `==`/`!=` over a closure ⇒ identity is unobservable ⇒ gate off",
        );
    }

    /// Comparing two closure-typed values with `==` makes pointer identity
    /// observable, so the gate must turn ON (disabling the cache). The RSScript
    /// analyzer permits this expression (the compiled backend later rejects bare
    /// `Fn ==`), so the gate is what keeps the VM bit-identical to that backend's
    /// distinct-allocation semantics when the program *does* reach an equality.
    #[test]
    fn gate_on_when_closure_compared() {
        let source = r#"
fn main() -> Unit {
    let f: Fn(Int) -> Int = |x| { return x + 1 }
    let g: Fn(Int) -> Int = |x| { return x + 1 }
    if f == g {
        Output.write(message: read String.from_int(value: 1))
    }
    return Unit
}
"#;
        assert!(
            unit(source).closure_identity_observable,
            "a user `==` over closure-typed operands ⇒ identity observable ⇒ gate on",
        );
    }

    /// With the gate off, repeated `MakeClosure` of the same non-capturing
    /// function shares ONE `Rc` (pointer-identical), proving the allocation was
    /// eliminated. We drive the handler directly so we can read back the register.
    #[test]
    fn cache_shares_one_rc_when_gate_off() {
        // Hand-build a unit whose closure-identity gate is off and a function 0
        // that the closure refers to (its body is irrelevant for this test).
        let func = RegFunction::placeholder("noop".into());
        let unit = RegUnit {
            functions: vec![Rc::new(func)],
            function_ids: HashMap::new(),
            resource_drop_functions: HashMap::new(),
            types: HashMap::new(),
            native_signatures: HashMap::new(),
            closure_identity_observable: false,
        };
        let mut vm = RegVm::new(Rc::new(unit), Vec::new(), HashMap::new());

        let a = vm.cached_noncapturing_closure(0);
        let b = vm.cached_noncapturing_closure(0);
        assert!(
            Rc::ptr_eq(&a, &b),
            "non-capturing closures of the same function must share one cached Rc",
        );
        assert!(
            a.captures.is_empty(),
            "cached closure must be non-capturing"
        );
    }
}
