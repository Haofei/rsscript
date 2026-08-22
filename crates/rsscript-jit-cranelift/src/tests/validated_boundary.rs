#[test]
fn validated_compile_boundary_rejects_malformed_ir_before_codegen() {
    let malformed = f(0, 1, vec![JitInstr::Jump { target: 9 }]);
    let error = match validate_function(&malformed) {
        Ok(_) => panic!("malformed IR must not mint a validation proof"),
        Err(error) => error,
    };
    assert!(error.0.contains("target 9"));

    let mut module = module();
    let error = module
        .compile(&malformed)
        .expect_err("the compatibility entry must validate before codegen");
    assert!(error.0.contains("target 9"));
    assert_eq!(module.compiled_function_count(), 0);
}

#[test]
fn validated_compile_boundary_accepts_a_sealed_proof() {
    let function = f(
        0,
        1,
        vec![
            JitInstr::LoadInt { dst: 0, value: 7 },
            JitInstr::Return { src: 0 },
        ],
    );
    let validated = validate_function(&function).expect("valid IR");
    let mut module = module();
    let id = module
        .compile_validated(&validated)
        .expect("sealed IR reaches codegen");
    assert_eq!(module.call(id, &[], &[]).completed(), Some(7));
}

#[test]
fn architecture_keeps_validation_and_codegen_in_separate_files() {
    let root = include_str!("../lib.rs");
    let module = include_str!("../module.rs");
    let validation = include_str!("../ir_validation.rs");
    let codegen = include_str!("../codegen.rs");
    let tests = include_str!("../tests.rs");

    assert!(root.contains("mod ir_validation;"));
    assert!(root.contains("mod codegen;"));
    assert!(!root.contains("include!("));
    assert!(root.contains("mod deopt;"));
    assert!(module.contains("pub use crate::deopt::{"));
    assert!(module.contains("validated: &ValidatedJitFunction<'_>"));
    assert!(!module.contains("fn validate(program: &JitFunction"));
    assert!(validation.contains("fn validate(program: &JitFunction"));
    assert!(codegen.contains("fn build_function("));
    assert!(codegen.contains("pub(crate) fn build_function("));
    for domain in [
        "host_and_memo",
        "calls_and_abi",
        "deopt",
        "validation",
        "fuzz",
        "ranges",
        "validated_boundary",
    ] {
        assert!(
            tests.contains(&format!("mod {domain};")),
            "VM JIT tests should retain the `{domain}` domain"
        );
    }
}
use super::*;
