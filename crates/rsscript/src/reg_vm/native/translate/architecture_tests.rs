#[test]
fn native_translation_is_partitioned_by_invariant() {
    let root = include_str!("../translate.rs");
    let jit_post = include_str!("jit_post.rs");
    let loop_regions = include_str!("loop_regions.rs");

    assert!(root.contains("mod jit_post;"));
    assert!(root.contains("mod loop_regions;"));
    assert!(!root.contains("fn native_forward_direct_list_store_loads("));
    assert!(!root.contains("fn native_memoize_loop_invariant_host_calls("));
    assert!(!root.contains("fn detect_natural_loop_at("));
    assert!(jit_post.contains("fn native_forward_direct_list_store_loads("));
    assert!(jit_post.contains("fn native_memoize_loop_invariant_host_calls("));
    assert!(loop_regions.contains("fn detect_natural_loop_at("));
    assert!(loop_regions.contains("struct OsrEntry"));

    assert!(
        root.lines().count() <= 4_100,
        "translate.rs should remain an orchestration and lowering boundary"
    );
}
