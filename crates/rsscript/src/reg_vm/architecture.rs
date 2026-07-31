use std::path::Path;

fn source(path: &str) -> &'static str {
    match path {
        "mod.rs" => include_str!("mod.rs"),
        "exec.rs" => include_str!("exec.rs"),
        "exec/storage_accounting.rs" => include_str!("exec/storage_accounting.rs"),
        "host_adapters.rs" => include_str!("host_adapters.rs"),
        "intrinsics/mod.rs" => include_str!("intrinsics/mod.rs"),
        "lower.rs" => include_str!("lower.rs"),
        "lower/closure_analysis.rs" => include_str!("lower/closure_analysis.rs"),
        "model.rs" => include_str!("model.rs"),
        "model/deep_copy.rs" => include_str!("model/deep_copy.rs"),
        "planning.rs" => include_str!("planning.rs"),
        "tier.rs" => include_str!("tier.rs"),
        "model/profile.rs" => include_str!("model/profile.rs"),
        "tier/compile_result.rs" => include_str!("tier/compile_result.rs"),
        "tier/osr_plan.rs" => include_str!("tier/osr_plan.rs"),
        _ => panic!("unknown register VM source path: {path}"),
    }
}

#[test]
fn register_vm_invariants_stay_in_owned_modules() {
    let boundaries = [
        ("mod.rs", "pub struct JitPlan", false),
        ("planning.rs", "pub struct JitPlan", true),
        ("model.rs", "pub(crate) struct FunctionProfile", false),
        (
            "model/profile.rs",
            "pub(crate) struct FunctionProfile",
            true,
        ),
        (
            "tier.rs",
            "pub(in crate::reg_vm) struct NativeCompileTelemetry",
            false,
        ),
        (
            "tier/compile_result.rs",
            "pub(in crate::reg_vm) struct NativeCompileTelemetry",
            true,
        ),
        ("exec.rs", "fn retained_storage_bytes_inner", false),
        (
            "exec/storage_accounting.rs",
            "fn retained_storage_bytes_inner",
            true,
        ),
        ("lower.rs", "fn closure_capture_names", false),
        (
            "lower/closure_analysis.rs",
            "fn closure_capture_names",
            true,
        ),
        ("model.rs", "fn deepcopy_elidable_param_regs", false),
        (
            "model/deep_copy.rs",
            "fn deepcopy_elidable_param_regs",
            true,
        ),
        ("tier.rs", "fn osr_materialize_recipe_is_supported", false),
        (
            "tier/osr_plan.rs",
            "fn osr_materialize_recipe_is_supported",
            true,
        ),
    ];

    for (path, invariant, expected) in boundaries {
        assert_eq!(
            source(path).contains(invariant),
            expected,
            "{invariant:?} has crossed its {path} ownership boundary",
        );
    }
}

#[test]
fn register_vm_boundary_modules_remain_reviewable() {
    let limits = [
        ("host_adapters.rs", 450usize),
        ("planning.rs", 700usize),
        ("exec/storage_accounting.rs", 400),
        ("lower/closure_analysis.rs", 250),
        ("model/deep_copy.rs", 900),
        ("model/profile.rs", 450),
        ("tier/compile_result.rs", 200),
        ("tier/osr_plan.rs", 250),
    ];

    for (path, max_lines) in limits {
        let line_count = source(path).lines().count();
        assert!(
            line_count <= max_lines,
            "{} grew to {line_count} lines; split by invariant before exceeding {max_lines}",
            Path::new(path).display(),
        );
    }
}

#[test]
fn restricted_host_dispatch_stays_behind_scoped_adapters() {
    let dispatch = source("intrinsics/mod.rs");
    let adapters = source("host_adapters.rs");

    assert!(
        dispatch.contains("self.authorize_intrinsic_host_access(intrinsic, args, base)?;"),
        "intrinsic dispatch must authorize concrete host resources before its match arms"
    );
    for capability in [
        ".filesystem_path(&authorized)",
        ".network_endpoint(&authorized)",
        ".process_executable(&authorized)",
    ] {
        assert!(
            adapters.contains(capability),
            "scoped host adapter is missing the {capability} consumption boundary"
        );
    }
}
