use std::collections::BTreeSet;

use crate::eval_types::CoverageBucket;
use crate::runtime_abi;

const REG_VM_RUNTIME_INTRINSICS: &[&str] = include!(concat!(
    env!("OUT_DIR"),
    "/rss-reg-vm-runtime-intrinsics.rs"
));

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct VmCoverageReport {
    pub runtime_intrinsics: CoverageBucket,
    pub hir_statements: CoverageBucket,
    pub hir_expressions: CoverageBucket,
    pub value_types: CoverageBucket,
    pub function_kinds: CoverageBucket,
    pub parity_features: CoverageBucket,
}

pub fn vm_coverage_report() -> VmCoverageReport {
    VmCoverageReport {
        runtime_intrinsics: vm_runtime_intrinsic_coverage(),
        hir_statements: coverage_bucket_from_supported(&[
            "Assign", "Break", "Continue", "Expr", "For", "If", "Let", "Loop", "Match", "Return",
            "Select", "With",
        ]),
        hir_expressions: coverage_bucket_from_supported(&[
            "ArrayLiteral",
            "Await",
            "Binary",
            "Call",
            "Closure",
            "Effect",
            "Field",
            "Ident",
            "Index",
            "Manage",
            "MapLiteral",
            "Match",
            "Number",
            "ObjectLiteral",
            "Spawn",
            "String",
            "Try",
        ]),
        value_types: coverage_bucket_from_supported(&[
            "Bool", "Bytes", "Char", "Closure", "Float", "Int", "Json", "List", "Map", "String",
            "Managed", "Native", "Struct", "Unit", "Variant",
        ]),
        function_kinds: coverage_bucket_from_supported(&["async", "native", "sync"]),
        parity_features: vm_parity_feature_coverage(),
    }
}

const NON_RUNTIME_PARITY_FEATURES: &[&str] = &[
    "function:async",
    "function:native",
    "function:sync",
    "hir_expr:ArrayLiteral",
    "hir_expr:Await",
    "hir_expr:Binary",
    "hir_expr:Call",
    "hir_expr:Closure",
    "hir_expr:Effect",
    "hir_expr:Field",
    "hir_expr:Ident",
    "hir_expr:Index",
    "hir_expr:Manage",
    "hir_expr:MapLiteral",
    "hir_expr:Match",
    "hir_expr:Number",
    "hir_expr:ObjectLiteral",
    "hir_expr:Spawn",
    "hir_expr:String",
    "hir_expr:Try",
    "hir_stmt:Assign",
    "hir_stmt:Break",
    "hir_stmt:Continue",
    "hir_stmt:Expr",
    "hir_stmt:For",
    "hir_stmt:If",
    "hir_stmt:Let",
    "hir_stmt:Loop",
    "hir_stmt:Match",
    "hir_stmt:Return",
    "hir_stmt:Select",
    "hir_stmt:With",
    "value:Bool",
    "value:Bytes",
    "value:Char",
    "value:Closure",
    "value:Float",
    "value:Int",
    "value:Json",
    "value:List",
    "value:Map",
    "value:Managed",
    "value:Native",
    "value:String",
    "value:Struct",
    "value:Unit",
    "value:Variant",
];

fn coverage_bucket_from_supported(supported: &[&str]) -> CoverageBucket {
    let mut supported = supported
        .iter()
        .map(|item| (*item).to_string())
        .collect::<Vec<_>>();
    supported.sort();
    supported.dedup();
    let all = supported.clone();
    CoverageBucket {
        all,
        supported,
        missing: Vec::new(),
    }
}

fn vm_parity_feature_coverage() -> CoverageBucket {
    let runtime = vm_runtime_intrinsic_coverage();
    let mut supported = NON_RUNTIME_PARITY_FEATURES
        .iter()
        .map(|feature| (*feature).to_string())
        .collect::<Vec<_>>();
    supported.extend(
        runtime
            .supported
            .into_iter()
            .map(|signature| format!("runtime:{signature}")),
    );
    coverage_bucket_from_owned(supported)
}

fn coverage_bucket_from_owned(mut supported: Vec<String>) -> CoverageBucket {
    supported.sort();
    supported.dedup();
    let all = supported.clone();
    CoverageBucket {
        all,
        supported,
        missing: Vec::new(),
    }
}

fn vm_runtime_intrinsic_coverage() -> CoverageBucket {
    let mut all = runtime_abi::runtime_intrinsic_signatures();
    all.sort();
    all.dedup();
    let all_set = all.iter().cloned().collect::<BTreeSet<_>>();
    let vm = REG_VM_RUNTIME_INTRINSICS
        .iter()
        .map(|signature| (*signature).to_string())
        .collect::<BTreeSet<_>>();
    let supported = all
        .iter()
        .filter(|signature| vm.contains(*signature))
        .cloned()
        .collect::<Vec<_>>();
    let missing = all_set.difference(&vm).cloned().collect::<Vec<_>>();
    CoverageBucket {
        all,
        supported,
        missing,
    }
}
