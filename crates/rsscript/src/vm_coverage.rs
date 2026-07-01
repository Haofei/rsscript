use std::collections::BTreeSet;

use crate::eval_types::CoverageBucket;
use crate::runtime_abi;

const REG_VM_RUNTIME_INTRINSICS: &[&str] = include!(concat!(
    env!("OUT_DIR"),
    "/rss-reg-vm-runtime-intrinsics.rs"
));
const REG_VM_SPECIAL_FORMS: &[&str] =
    include!(concat!(env!("OUT_DIR"), "/rss-reg-vm-special-forms.rs"));

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct VmCoverageReport {
    pub runtime_intrinsics: CoverageBucket,
    pub special_forms: CoverageBucket,
    pub hir_statements: CoverageBucket,
    pub hir_expressions: CoverageBucket,
    pub value_types: CoverageBucket,
    pub function_kinds: CoverageBucket,
    pub parity_features: CoverageBucket,
}

pub fn vm_coverage_report() -> VmCoverageReport {
    VmCoverageReport {
        runtime_intrinsics: vm_runtime_intrinsic_coverage(),
        special_forms: coverage_bucket_from_supported(REG_VM_SPECIAL_FORMS),
        hir_statements: coverage_bucket_from_all_supported(
            vec![
                "Assign".to_string(),
                "Break".to_string(),
                "Continue".to_string(),
                "Expr".to_string(),
                "For".to_string(),
                "If".to_string(),
                "Let".to_string(),
                "Loop".to_string(),
                "Match".to_string(),
                "Return".to_string(),
                "Select".to_string(),
                "With".to_string(),
            ],
            vec![
                "Assign".to_string(),
                "Break".to_string(),
                "Continue".to_string(),
                "Expr".to_string(),
                "For".to_string(),
                "If".to_string(),
                "Let".to_string(),
                "Loop".to_string(),
                "Return".to_string(),
                "With".to_string(),
            ],
        ),
        hir_expressions: coverage_bucket_from_all_supported(
            vec![
                "ArrayLiteral".to_string(),
                "Await".to_string(),
                "Binary".to_string(),
                "Call".to_string(),
                "Char".to_string(),
                "Closure".to_string(),
                "Effect".to_string(),
                "Field".to_string(),
                "Ident".to_string(),
                "Index".to_string(),
                "Manage".to_string(),
                "MapLiteral".to_string(),
                "Match".to_string(),
                "Number".to_string(),
                "ObjectLiteral".to_string(),
                "Spawn".to_string(),
                "String".to_string(),
                "Try".to_string(),
            ],
            vec![
                "ArrayLiteral".to_string(),
                "Binary".to_string(),
                "Call".to_string(),
                "Char".to_string(),
                "Closure".to_string(),
                "Effect".to_string(),
                "Field".to_string(),
                "Ident".to_string(),
                "Index".to_string(),
                "Manage".to_string(),
                "MapLiteral".to_string(),
                "Number".to_string(),
                "ObjectLiteral".to_string(),
                "String".to_string(),
                "Try".to_string(),
            ],
        ),
        value_types: coverage_bucket_from_supported(&[
            "Bool", "Bytes", "Char", "Closure", "Float", "Int", "Json", "List", "Map", "String",
            "Managed", "Native", "Struct", "Unit", "Variant",
        ]),
        function_kinds: coverage_bucket_from_all_supported(
            vec![
                "async".to_string(),
                "native".to_string(),
                "sync".to_string(),
            ],
            vec!["native".to_string(), "sync".to_string()],
        ),
        parity_features: vm_parity_feature_coverage(),
    }
}

const NON_RUNTIME_PARITY_FEATURES: &[&str] = &[
    "function:native",
    "function:sync",
    "hir_expr:ArrayLiteral",
    "hir_expr:Binary",
    "hir_expr:Call",
    "hir_expr:Closure",
    "hir_expr:Effect",
    "hir_expr:Field",
    "hir_expr:Ident",
    "hir_expr:Index",
    "hir_expr:Manage",
    "hir_expr:MapLiteral",
    "hir_expr:Number",
    "hir_expr:ObjectLiteral",
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
    "hir_stmt:Return",
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
    let mut all = NON_RUNTIME_PARITY_FEATURES
        .iter()
        .map(|feature| (*feature).to_string())
        .collect::<Vec<_>>();
    all.extend(
        runtime
            .all
            .into_iter()
            .map(|signature| format!("runtime:{signature}")),
    );
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
    coverage_bucket_from_all_supported(all, supported)
}

fn coverage_bucket_from_all_supported(
    mut all: Vec<String>,
    mut supported: Vec<String>,
) -> CoverageBucket {
    all.sort();
    all.dedup();
    supported.sort();
    supported.dedup();
    let all_set = all.iter().cloned().collect::<BTreeSet<_>>();
    let supported_set = supported.iter().cloned().collect::<BTreeSet<_>>();
    let missing = all_set
        .difference(&supported_set)
        .cloned()
        .collect::<Vec<_>>();
    CoverageBucket {
        all,
        supported,
        missing,
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::interfaces::default_interfaces;
    use crate::syntax::ast::Item;
    use crate::syntax::parse_source;

    use super::{REG_VM_RUNTIME_INTRINSICS, REG_VM_SPECIAL_FORMS, runtime_abi};

    #[test]
    fn generated_reg_vm_runtime_intrinsics_do_not_drift_from_runtime_abi() {
        let abi = runtime_abi::runtime_intrinsic_signatures()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let vm = REG_VM_RUNTIME_INTRINSICS
            .iter()
            .map(|signature| (*signature).to_string())
            .collect::<BTreeSet<_>>();
        let special_forms = REG_VM_SPECIAL_FORMS
            .iter()
            .map(|signature| (*signature).to_string())
            .collect::<BTreeSet<_>>();

        let missing = abi.difference(&vm).cloned().collect::<Vec<_>>();
        let extra = vm.difference(&abi).cloned().collect::<Vec<_>>();
        let special_form_overlap = special_forms
            .intersection(&abi)
            .cloned()
            .collect::<Vec<_>>();

        assert!(
            missing.is_empty(),
            "runtime ABI entries missing from reg VM resolver: {missing:?}"
        );
        assert!(
            extra.is_empty(),
            "reg VM resolver entries not declared in runtime ABI: {extra:?}"
        );
        assert!(
            special_form_overlap.is_empty(),
            "reg VM special forms should not be runtime ABI entries: {special_form_overlap:?}"
        );
    }

    #[test]
    fn generated_reg_vm_special_forms_match_core_public_non_native_functions() {
        let special_forms = REG_VM_SPECIAL_FORMS
            .iter()
            .map(|signature| (*signature).to_string())
            .collect::<BTreeSet<_>>();
        let core_non_native = core_public_non_native_function_signatures();

        let missing_core_signature = special_forms
            .difference(&core_non_native)
            .cloned()
            .collect::<Vec<_>>();
        assert!(
            missing_core_signature.is_empty(),
            "reg VM special forms without bundled public non-native core signatures: {missing_core_signature:?}"
        );
    }

    fn core_public_non_native_function_signatures() -> BTreeSet<String> {
        let mut signatures = BTreeSet::new();
        for (path, source) in default_interfaces() {
            let program = parse_source(path, source);
            let protocol_names = program
                .protocols
                .iter()
                .map(|protocol| protocol.name.as_str())
                .collect::<BTreeSet<_>>();
            for item in program.items {
                if let Item::Function(function) = item
                    && !function.is_native
                    && (function.is_public
                        || function
                            .name
                            .rsplit_once('.')
                            .is_some_and(|(namespace, _)| protocol_names.contains(namespace)))
                {
                    signatures.insert(function.name);
                }
            }
        }
        signatures
    }
}
