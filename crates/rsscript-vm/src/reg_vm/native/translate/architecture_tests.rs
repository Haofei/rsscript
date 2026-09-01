#[test]
fn native_translation_is_partitioned_by_invariant() {
    let root = include_str!("../translate.rs");
    let jit_post = include_str!("jit_post.rs");
    let loop_regions = include_str!("loop_regions.rs");
    let osr_loop = include_str!("osr_loop.rs");

    assert!(root.contains("mod jit_post;"));
    assert!(root.contains("mod loop_regions;"));
    assert!(root.contains("mod osr_loop;"));
    assert!(!root.contains("fn native_forward_direct_list_store_loads("));
    assert!(!root.contains("fn native_memoize_loop_invariant_runtime_helper_calls("));
    assert!(!root.contains("fn detect_natural_loop_at("));
    assert!(!root.contains("fn translate_osr_loop_inner("));
    assert!(jit_post.contains("fn native_forward_direct_list_store_loads("));
    assert!(jit_post.contains("fn native_memoize_loop_invariant_runtime_helper_calls("));
    assert!(loop_regions.contains("fn detect_natural_loop_at("));
    assert!(loop_regions.contains("struct OsrEntry"));
    assert!(osr_loop.contains("fn translate_osr_loop_inner("));

    assert!(
        root.lines().count() <= 4_100,
        "translate.rs should remain an orchestration and lowering boundary"
    );
}

#[test]
fn alias_sensitive_loop_optimizations_consume_program_point_evidence() {
    let translator = include_str!("../translate.rs");
    let osr_loop = include_str!("osr_loop.rs");
    let jit_post = include_str!("jit_post.rs");
    let typed_region = include_str!("../typed_region.rs");

    // The OSR-loop translator (which consumes the program-point alias evidence)
    // now lives in the osr_loop child module.
    assert!(
        translator.contains("verified_alias_allows_bounds_elision(")
            || osr_loop.contains("verified_alias_allows_bounds_elision(")
    );
    assert!(jit_post.contains("program_point_value(helper_ip"));
    assert!(jit_post.contains("permits_readonly_hoist()"));
    assert!(typed_region.contains("Missing, conflicting or over-budget evidence"));
    assert!(typed_region.contains("typed.permits_bounds_elision(source_ip, reg, mutable)"));
}
