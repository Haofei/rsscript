use std::collections::{BTreeMap, BTreeSet};

use crate::syntax::ast::{DataEffect, EffectDecl, FileMode, FunctionDecl, Item, Param, TypeRef};
use crate::syntax::parse_source;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewFinding {
    pub code: String,
    pub summary: String,
}

pub fn review_sources(
    old_file: &str,
    old_source: &str,
    new_file: &str,
    new_source: &str,
) -> Vec<ReviewFinding> {
    let old_program = parse_source(old_file, old_source);
    let new_program = parse_source(new_file, new_source);
    let mut findings = Vec::new();

    if old_program.mode != new_program.mode {
        findings.push(ReviewFinding {
            code: "RSR001".to_string(),
            summary: format!(
                "file mode changed from {} to {}.",
                file_mode_label(old_program.mode),
                file_mode_label(new_program.mode)
            ),
        });
    }

    let old_functions = collect_function_sigs(&old_program.items);
    let new_functions = collect_function_sigs(&new_program.items);
    let names: BTreeSet<_> = old_functions
        .keys()
        .chain(new_functions.keys())
        .cloned()
        .collect();

    for name in names {
        match (old_functions.get(&name), new_functions.get(&name)) {
            (Some(_), None) => findings.push(ReviewFinding {
                code: "RSR002".to_string(),
                summary: format!("function `{name}` was removed."),
            }),
            (None, Some(_)) => findings.push(ReviewFinding {
                code: "RSR003".to_string(),
                summary: format!("function `{name}` was added."),
            }),
            (Some(old), Some(new)) => compare_function(old, new, &mut findings),
            (None, None) => {}
        }
    }

    findings
}

pub fn format_review_human(findings: &[ReviewFinding]) -> String {
    if findings.is_empty() {
        return "review: no API changes detected\n".to_string();
    }

    let mut output = String::new();
    for finding in findings {
        output.push_str(&format!("{}: {}\n", finding.code, finding.summary));
    }
    output
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FunctionSig {
    name: String,
    params: Vec<ParamSig>,
    return_type: Option<String>,
    returns_fresh: bool,
    effects: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParamSig {
    name: String,
    effect: Option<&'static str>,
    type_name: String,
}

fn collect_function_sigs(items: &[Item]) -> BTreeMap<String, FunctionSig> {
    items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some((function.name.clone(), function_sig(function))),
            Item::Type(_) => None,
        })
        .collect()
}

fn function_sig(function: &FunctionDecl) -> FunctionSig {
    FunctionSig {
        name: function.name.clone(),
        params: function.params.iter().map(param_sig).collect(),
        return_type: function.return_ty.as_ref().map(type_name),
        returns_fresh: function.returns_fresh,
        effects: function.effects.iter().map(effect_name).collect(),
    }
}

fn param_sig(param: &Param) -> ParamSig {
    ParamSig {
        name: param.name.clone(),
        effect: param.effect.map(effect_label),
        type_name: type_name(&param.ty),
    }
}

fn compare_function(old: &FunctionSig, new: &FunctionSig, findings: &mut Vec<ReviewFinding>) {
    if old.params != new.params {
        findings.push(ReviewFinding {
            code: "RSR004".to_string(),
            summary: format!("function `{}` parameters changed.", old.name),
        });
    }
    if old.return_type != new.return_type || old.returns_fresh != new.returns_fresh {
        findings.push(ReviewFinding {
            code: "RSR005".to_string(),
            summary: format!("function `{}` return contract changed.", old.name),
        });
    }
    if old.effects != new.effects {
        findings.push(ReviewFinding {
            code: "RSR006".to_string(),
            summary: format!("function `{}` effects changed.", old.name),
        });
    }
}

fn file_mode_label(mode: Option<FileMode>) -> &'static str {
    match mode {
        Some(FileMode::Managed) => "managed",
        Some(FileMode::UsesLocal) => "uses-local",
        None => "<missing>",
    }
}

fn effect_label(effect: DataEffect) -> &'static str {
    match effect {
        DataEffect::Read => "read",
        DataEffect::Mut => "mut",
        DataEffect::Take => "take",
    }
}

fn type_name(ty: &TypeRef) -> String {
    if ty.args.is_empty() {
        return ty.name.clone();
    }

    let args = ty.args.iter().map(type_name).collect::<Vec<_>>().join(", ");
    format!("{}<{args}>", ty.name)
}

fn effect_name(effect: &EffectDecl) -> String {
    match effect {
        EffectDecl::Name(name) => name.clone(),
        EffectDecl::Retains(param) => format!("retains({param})"),
    }
}
