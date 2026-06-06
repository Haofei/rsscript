use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Serialize;

use crate::diagnostic::{Span, code};
use crate::hir::{
    CallResolution, FunctionSig as HirFunctionSig, Hir, HirBindingKind, HirBlock, HirExpr, HirStmt,
    ParamEffect, ResolvedCalleeKind,
};
use crate::interfaces::standard_package_interfaces;
use crate::syntax::ast::{
    Block, CallArg, Callee, DataEffect, EffectDecl, Expr, FieldDecl, FileFeature, FunctionDecl,
    GenericBound, Item, LetKind, MatchPattern, Param, Program, ProtocolImpl, Stmt, TypeDecl,
    TypeKind, TypeRef, merge_programs,
};
use crate::syntax::parse_source;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewFinding {
    pub code: String,
    pub risk: ReviewRisk,
    pub summary: String,
    pub spans: Vec<ReviewSpan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    pub fixes: Vec<ReviewFix>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewSpan {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub length: usize,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewFix {
    pub kind: String,
    pub title: String,
    pub applicability: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewMap {
    pub summary: ReviewMapSummary,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<ReviewMapModule>,
    pub files: Vec<ReviewMapFile>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReviewMapSummary {
    pub total_functions: usize,
    pub total_lines: usize,
    pub must_review_lines: usize,
    pub low_semantic_risk_lines: usize,
    pub unknown_lines: usize,
    pub suggested_review_lines: usize,
    pub review_ratio: ReviewRatio,
    pub unknown_ratio: ReviewRatio,
    pub unknown_function_ratio: ReviewRatio,
    #[serde(rename = "must_review")]
    pub review_required: ReviewMapCategorySummary,
    #[serde(rename = "low_semantic_risk")]
    pub foldable: ReviewMapCategorySummary,
    pub unknown: ReviewMapCategorySummary,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReviewRatio {
    scaled: u32,
}

impl Serialize for ReviewRatio {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_f64(f64::from(self.scaled) / 1000.0)
    }
}

impl ReviewRatio {
    fn from_parts(numerator: usize, denominator: usize) -> Self {
        if denominator == 0 {
            return Self { scaled: 0 };
        }
        let scaled = ((numerator.saturating_mul(1000)) / denominator).min(1000) as u32;
        Self { scaled }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ReviewMapCategorySummary {
    pub functions: usize,
    pub lines: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewMapFile {
    pub file: String,
    pub features: Vec<String>,
    pub risk: ReviewMapFileRisk,
    pub reasons: Vec<String>,
    pub regions: Vec<ReviewMapRegion>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewMapModule {
    pub file: String,
    pub module_path: String,
    pub line: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uses: Vec<ReviewMapUse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewMapUse {
    pub path: String,
    pub line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewMapFileRisk {
    Low,
    Elevated,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewMapRegion {
    pub function: String,
    pub classification: ReviewMapClassification,
    pub line: usize,
    pub line_count: usize,
    pub reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub receiver_calls: Vec<ReviewMapReceiverCall>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewMapReceiverCall {
    pub line: usize,
    pub column: usize,
    pub source: String,
    pub canonical_callee: String,
    pub self_effect: String,
    pub resolution: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewMapClassification {
    #[serde(rename = "must_review")]
    ReviewRequired,
    #[serde(rename = "low_semantic_risk")]
    Foldable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReviewRisk {
    Feature,
    Api,
    TypeLayout,
    Effect,
    Boundary,
    Unsafe,
    Guarantee,
}

impl ReviewRisk {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Feature => "feature",
            Self::Api => "api",
            Self::TypeLayout => "type-layout",
            Self::Effect => "effect",
            Self::Boundary => "boundary",
            Self::Unsafe => "unsafe",
            Self::Guarantee => "guarantee",
        }
    }
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

    if feature_label(&old_program.features) != feature_label(&new_program.features) {
        findings.push(review_finding(
            code::REVIEW_FEATURES_CHANGED,
            ReviewRisk::Feature,
            format!(
                "file features changed from {} to {}.",
                feature_label(&old_program.features),
                feature_label(&new_program.features)
            ),
            Vec::new(),
            Some(feature_label(&old_program.features)),
            Some(feature_label(&new_program.features)),
        ));
    }

    let old_types = collect_type_sigs(&old_program.items);
    let new_types = collect_type_sigs(&new_program.items);
    let type_names: BTreeSet<_> = old_types.keys().chain(new_types.keys()).cloned().collect();

    for name in type_names {
        match (old_types.get(&name), new_types.get(&name)) {
            (Some(old), None) => findings.push(review_finding(
                code::REVIEW_TYPE_REMOVED,
                ReviewRisk::TypeLayout,
                format!("type `{name}` was removed."),
                vec![review_span(&old.span, "removed type")],
                Some(type_contract(old)),
                None,
            )),
            (None, Some(new)) => findings.push(review_finding(
                code::REVIEW_TYPE_ADDED,
                ReviewRisk::TypeLayout,
                format!("type `{name}` was added."),
                vec![review_span(&new.span, "added type")],
                None,
                Some(type_contract(new)),
            )),
            (Some(old), Some(new)) => compare_type(old, new, &mut findings),
            (None, None) => {}
        }
    }

    let old_functions = collect_function_sigs(&old_program.items);
    let new_functions = collect_function_sigs(&new_program.items);
    let function_names: BTreeSet<_> = old_functions
        .keys()
        .chain(new_functions.keys())
        .cloned()
        .collect();

    for name in function_names {
        match (old_functions.get(&name), new_functions.get(&name)) {
            (Some(old), None) => findings.push(review_finding(
                code::REVIEW_FUNCTION_REMOVED,
                ReviewRisk::Api,
                format!("function `{name}` was removed."),
                vec![review_span(&old.span, "removed function")],
                Some(function_contract(old)),
                None,
            )),
            (None, Some(new)) => findings.push(review_finding(
                code::REVIEW_FUNCTION_ADDED,
                ReviewRisk::Api,
                format!("function `{name}` was added."),
                vec![review_span(&new.span, "added function")],
                None,
                Some(function_contract(new)),
            )),
            (Some(old), Some(new)) => compare_function(old, new, &mut findings),
            (None, None) => {}
        }
    }

    let old_protocol_impls = collect_protocol_impl_sigs(&old_program.protocol_impls);
    let new_protocol_impls = collect_protocol_impl_sigs(&new_program.protocol_impls);
    let protocol_impl_names: BTreeSet<_> = old_protocol_impls
        .keys()
        .chain(new_protocol_impls.keys())
        .cloned()
        .collect();
    for name in protocol_impl_names {
        match (old_protocol_impls.get(&name), new_protocol_impls.get(&name)) {
            (Some(old), None) => findings.push(review_finding(
                code::REVIEW_PROTOCOL_IMPL_CHANGED,
                ReviewRisk::Api,
                format!("protocol implementation `{name}` was removed."),
                vec![review_span(&old.span, "removed protocol impl")],
                Some(protocol_impl_contract(old)),
                None,
            )),
            (None, Some(new)) => findings.push(review_finding(
                code::REVIEW_PROTOCOL_IMPL_CHANGED,
                ReviewRisk::Api,
                format!("protocol implementation `{name}` was added."),
                vec![review_span(&new.span, "added protocol impl")],
                None,
                Some(protocol_impl_contract(new)),
            )),
            (Some(old), Some(new)) if old.mappings != new.mappings => {
                findings.push(review_finding(
                    code::REVIEW_PROTOCOL_IMPL_CHANGED,
                    ReviewRisk::Api,
                    format!("protocol implementation `{name}` mappings changed."),
                    paired_spans(
                        &old.span,
                        &new.span,
                        "old protocol impl",
                        "new protocol impl",
                    ),
                    Some(protocol_impl_contract(old)),
                    Some(protocol_impl_contract(new)),
                ));
            }
            (Some(_), Some(_)) | (None, None) => {}
        }
    }

    // Track sum type changes
    let old_sum_types = collect_sum_type_sigs(&old_program.items);
    let new_sum_types = collect_sum_type_sigs(&new_program.items);
    let sum_type_names: BTreeSet<_> = old_sum_types
        .keys()
        .chain(new_sum_types.keys())
        .cloned()
        .collect();
    for name in sum_type_names {
        match (old_sum_types.get(&name), new_sum_types.get(&name)) {
            (Some(old), None) => findings.push(review_finding(
                code::REVIEW_SUM_TYPE_CHANGED,
                ReviewRisk::Api,
                format!("sum type `{name}` was removed."),
                vec![review_span(&old.span, "removed sum type")],
                Some(format!("sum {name}")),
                None,
            )),
            (None, Some(new)) => findings.push(review_finding(
                code::REVIEW_SUM_TYPE_CHANGED,
                ReviewRisk::Api,
                format!("sum type `{name}` was added."),
                vec![review_span(&new.span, "added sum type")],
                None,
                Some(format!("sum {name}")),
            )),
            (Some(old), Some(new)) if old.variants != new.variants => {
                findings.push(review_finding(
                    code::REVIEW_SUM_TYPE_CHANGED,
                    ReviewRisk::Api,
                    format!("sum type `{name}` variants changed."),
                    paired_spans(&old.span, &new.span, "old sum type", "new sum type"),
                    Some(format!("variants: {}", old.variants.join(", "))),
                    Some(format!("variants: {}", new.variants.join(", "))),
                ));
            }
            _ => {}
        }
    }

    // Track const changes
    let old_consts = collect_const_sigs(&old_program.items);
    let new_consts = collect_const_sigs(&new_program.items);
    let const_names: BTreeSet<_> = old_consts
        .keys()
        .chain(new_consts.keys())
        .cloned()
        .collect();
    for name in const_names {
        match (old_consts.get(&name), new_consts.get(&name)) {
            (Some(old), None) => findings.push(review_finding(
                code::REVIEW_CONST_CHANGED,
                ReviewRisk::Api,
                format!("const `{name}` was removed."),
                vec![review_span(&old.span, "removed const")],
                Some(format!("const {name}")),
                None,
            )),
            (None, Some(new)) => findings.push(review_finding(
                code::REVIEW_CONST_CHANGED,
                ReviewRisk::Api,
                format!("const `{name}` was added."),
                vec![review_span(&new.span, "added const")],
                None,
                Some(format!("const {name}")),
            )),
            (Some(old), Some(new)) if old.type_name != new.type_name => {
                findings.push(review_finding(
                    code::REVIEW_CONST_CHANGED,
                    ReviewRisk::Api,
                    format!("const `{name}` type changed."),
                    paired_spans(&old.span, &new.span, "old const", "new const"),
                    Some(format!("const {name}: {}", old.type_name)),
                    Some(format!("const {name}: {}", new.type_name)),
                ));
            }
            _ => {}
        }
    }

    // Track type alias changes
    let old_aliases = collect_type_alias_sigs(&old_program.items);
    let new_aliases = collect_type_alias_sigs(&new_program.items);
    let alias_names: BTreeSet<_> = old_aliases
        .keys()
        .chain(new_aliases.keys())
        .cloned()
        .collect();
    for name in alias_names {
        match (old_aliases.get(&name), new_aliases.get(&name)) {
            (Some(old), None) => findings.push(review_finding(
                code::REVIEW_TYPE_ALIAS_CHANGED,
                ReviewRisk::Api,
                format!("type alias `{name}` was removed."),
                vec![review_span(&old.span, "removed type alias")],
                Some(format!("type {name} = {}", old.target)),
                None,
            )),
            (None, Some(new)) => findings.push(review_finding(
                code::REVIEW_TYPE_ALIAS_CHANGED,
                ReviewRisk::Api,
                format!("type alias `{name}` was added."),
                vec![review_span(&new.span, "added type alias")],
                None,
                Some(format!("type {name} = {}", new.target)),
            )),
            (Some(old), Some(new)) if old.target != new.target => {
                findings.push(review_finding(
                    code::REVIEW_TYPE_ALIAS_CHANGED,
                    ReviewRisk::Api,
                    format!("type alias `{name}` target changed."),
                    paired_spans(&old.span, &new.span, "old alias", "new alias"),
                    Some(format!("type {name} = {}", old.target)),
                    Some(format!("type {name} = {}", new.target)),
                ));
            }
            _ => {}
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
        output.push_str(&format!(
            "{}[{}]: {}\n",
            finding.code,
            finding.risk.as_str(),
            finding.summary
        ));
    }
    output
}

pub fn format_review_json(findings: &[ReviewFinding]) -> String {
    serde_json::to_string(findings).expect("review JSON serialization should not fail")
}

pub fn review_map_sources(sources: Vec<(&str, &str)>) -> ReviewMap {
    let interfaces = standard_package_interfaces().collect::<Vec<_>>();
    review_map_sources_with_interfaces(sources, &interfaces)
}

pub(crate) fn review_map_sources_with_interfaces(
    sources: Vec<(&str, &str)>,
    interfaces: &[(&str, &str)],
) -> ReviewMap {
    let parsed_sources = sources
        .into_iter()
        .map(|(file, source)| ReviewMapParsedSource {
            file: file.to_string(),
            total_lines: source.lines().count().max(1),
            program: parse_source(file, source),
        })
        .collect::<Vec<_>>();
    let merged_program = merge_programs(parsed_sources.iter().map(|source| source.program.clone()));
    let interface_programs = interfaces
        .iter()
        .map(|(file, source)| parse_source(file, source))
        .collect::<Vec<_>>();
    let hir = Hir::from_syntax_with_interfaces(&merged_program, &interface_programs);
    let mut region_drafts = parsed_sources
        .iter()
        .flat_map(|source| review_map_file_region_drafts(source, &hir))
        .collect::<Vec<_>>();
    propagate_review_map_call_classifications(&mut region_drafts);

    let files = parsed_sources
        .iter()
        .map(|source| {
            let regions = region_drafts
                .iter()
                .filter(|draft| draft.file == source.file)
                .map(|draft| draft.region.clone())
                .collect();
            let features = feature_names(&source.program.features);
            ReviewMapFile {
                file: source.file.clone(),
                features,
                risk: review_map_file_risk(&source.program.features),
                reasons: review_map_file_reasons(&source.program.features),
                regions,
            }
        })
        .collect::<Vec<_>>();

    ReviewMap {
        summary: review_map_summary(&files),
        modules: review_map_modules(&parsed_sources),
        files,
    }
}

pub fn format_review_map_human(map: &ReviewMap) -> String {
    if map
        .files
        .iter()
        .all(|file| file.regions.is_empty() && file.reasons.is_empty())
    {
        return "review map: no functions detected\n".to_string();
    }

    let mut output = String::new();
    output.push_str(&format!(
        "summary: must-review {} functions/{} lines; low-semantic-risk {} functions/{} lines; unknown {} functions/{} lines; total {} functions/{} lines\n",
        map.summary.review_required.functions,
        map.summary.review_required.lines,
        map.summary.foldable.functions,
        map.summary.foldable.lines,
        map.summary.unknown.functions,
        map.summary.unknown.lines,
        map.summary.total_functions,
        map.summary.total_lines
    ));
    for file in &map.files {
        output.push_str(&format!("{}:", file.file));
        if !file.features.is_empty() {
            output.push_str(&format!(
                " features {}; risk {}",
                file.features.join(", "),
                review_map_file_risk_label(file.risk)
            ));
        }
        if !file.reasons.is_empty() {
            output.push_str(&format!("; {}", file.reasons.join("; ")));
        }
        output.push('\n');
        for region in &file.regions {
            output.push_str(&format!(
                "  {} [{}] line {} ({} lines): {}\n",
                region.function,
                review_map_classification_label(region.classification),
                region.line,
                region.line_count,
                region.reasons.join("; ")
            ));
        }
    }
    output
}

pub fn format_review_map_json(map: &ReviewMap) -> String {
    serde_json::to_string(map).expect("review map JSON serialization should not fail")
}

fn review_map_summary(files: &[ReviewMapFile]) -> ReviewMapSummary {
    let mut summary = ReviewMapSummary::default();
    for region in files.iter().flat_map(|file| file.regions.iter()) {
        summary.total_functions += 1;
        summary.total_lines += region.line_count;
        let category = match region.classification {
            ReviewMapClassification::ReviewRequired => &mut summary.review_required,
            ReviewMapClassification::Foldable => &mut summary.foldable,
            ReviewMapClassification::Unknown => &mut summary.unknown,
        };
        category.functions += 1;
        category.lines += region.line_count;
    }
    summary.must_review_lines = summary.review_required.lines;
    summary.low_semantic_risk_lines = summary.foldable.lines;
    summary.unknown_lines = summary.unknown.lines;
    summary.suggested_review_lines = summary.review_required.lines + summary.unknown.lines;
    summary.review_ratio =
        ReviewRatio::from_parts(summary.suggested_review_lines, summary.total_lines);
    summary.unknown_ratio = ReviewRatio::from_parts(summary.unknown.lines, summary.total_lines);
    summary.unknown_function_ratio =
        ReviewRatio::from_parts(summary.unknown.functions, summary.total_functions);
    summary
}

fn review_map_modules(sources: &[ReviewMapParsedSource]) -> Vec<ReviewMapModule> {
    let mut modules = Vec::new();
    for source in sources {
        let uses = source
            .program
            .items
            .iter()
            .filter_map(|item| match item {
                Item::Use(use_decl) => Some(ReviewMapUse {
                    path: use_decl.path.join("."),
                    line: use_decl.span.line,
                }),
                _ => None,
            })
            .collect::<Vec<_>>();
        for item in &source.program.items {
            if let Item::Module(module) = item {
                modules.push(ReviewMapModule {
                    file: source.file.clone(),
                    module_path: module.path.join("."),
                    line: module.span.line,
                    uses: uses.clone(),
                });
            }
        }
    }
    modules
}

#[derive(Debug, Clone)]
struct ReviewMapParsedSource {
    file: String,
    total_lines: usize,
    program: Program,
}

fn review_map_file_region_drafts(
    source: &ReviewMapParsedSource,
    hir: &Hir,
) -> Vec<ReviewMapRegionDraft> {
    let mut function_lines = source
        .program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some((function.name.clone(), function.span.line)),
            Item::Type(_)
            | Item::Module(_)
            | Item::Use(_)
            | Item::SumType(_)
            | Item::TypeAlias(_)
            | Item::Const(_) => None,
        })
        .collect::<Vec<_>>();
    function_lines.sort_by_key(|(_, line)| *line);

    source
        .program
        .items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some(review_map_region_draft(
                &source.file,
                function,
                &hir,
                &function_lines,
                source.total_lines,
            )),
            Item::Type(_)
            | Item::Module(_)
            | Item::Use(_)
            | Item::SumType(_)
            | Item::TypeAlias(_)
            | Item::Const(_) => None,
        })
        .collect()
}

fn review_map_region_draft(
    file: &str,
    function: &FunctionDecl,
    hir: &Hir,
    function_lines: &[(String, usize)],
    total_lines: usize,
) -> ReviewMapRegionDraft {
    let mut facts = ReviewMapFacts::default();
    // Build value_types for receiver-call resolution
    for param in &function.params {
        facts
            .value_types
            .insert(param.name.clone(), type_ref_display_name(&param.ty));
    }
    for type_param in &function.type_params {
        if let Some(GenericBound::Protocol(protocol)) = &type_param.bound {
            facts.value_types.insert(
                format!("__protocol_bound__{}", type_param.name),
                protocol.clone(),
            );
        }
    }
    let callback_params = review_map_callback_params(function);
    let local_closure_bindings = review_map_local_closure_bindings(&function.body);
    collect_review_map_facts_block(
        &function.body,
        hir,
        &callback_params,
        &local_closure_bindings,
        &mut facts,
    );
    if let Some(hir_body) = hir.function_body(&function.name) {
        let local_bindings = hir_body
            .bindings
            .iter()
            .filter(|binding| binding.kind == HirBindingKind::LocalLet)
            .map(|binding| binding.name.as_str())
            .collect::<BTreeSet<_>>();
        if let Some(body) = hir_body.block.as_ref() {
            collect_review_map_hir_facts_block(body, &local_bindings, &mut facts);
        }
    }

    let mut reasons = Vec::new();
    if function.is_public {
        reasons.push("public entry point".to_string());
    }
    if let Some(reason) = &function.deprecated_reason {
        reasons.push(format!("deprecated: {reason}"));
    }
    if is_entry_function(&function.name) {
        reasons.push("entry point".to_string());
    }
    for param in &function.params {
        if matches!(param.effect, Some(DataEffect::Mut | DataEffect::Take)) {
            reasons.push(format!(
                "{} parameter `{}`",
                effect_label(param.effect.expect("effect matched")),
                param.name
            ));
        }
        if type_ref_contains_name(&param.ty, "ResourcePool") {
            reasons.push(format!("ResourcePool parameter `{}`", param.name));
        }
    }
    if let Some(return_ty) = &function.return_ty
        && type_ref_contains_name(return_ty, "ResourcePool")
    {
        reasons.push("ResourcePool return type".to_string());
    }
    if function.returns_fresh {
        reasons.push("fresh guarantee boundary".to_string());
    }
    if function.is_async {
        reasons.push("async function boundary".to_string());
    }
    for effect in &function.effects {
        match effect {
            EffectDecl::Retains(param) => reasons.push(format!("retains `{param}`")),
            EffectDecl::Name(name) if matches!(name.as_str(), "native" | "unsafe" | "parallel") => {
                reasons.push(format!("{name} boundary"))
            }
            EffectDecl::Name(name) if is_runtime_guarantee_boundary(name) => {
                reasons.push(format!("guarantee `{name}`"))
            }
            _ => {}
        }
    }
    for call in &facts.native_calls {
        reasons.push(format!("native call `{call}`"));
    }
    for call in &facts.unsafe_calls {
        reasons.push(format!("unsafe call `{call}`"));
    }
    if facts.has_local {
        reasons.push("local binding".to_string());
    }
    if facts.has_manage {
        reasons.push("manage boundary".to_string());
    }
    if facts.has_spawn {
        reasons.push("spawn task boundary".to_string());
    }
    if facts.has_await {
        reasons.push("await suspension boundary".to_string());
    }
    if !facts.spawn_captures.is_empty() {
        let captures = facts
            .spawn_captures
            .iter()
            .map(|capture| format!("`{capture}`"))
            .collect::<Vec<_>>()
            .join(", ");
        reasons.push(format!("spawn retains-until-task-complete {captures}"));
    }
    if !facts.managed_closure_captures.is_empty() {
        let captures = facts
            .managed_closure_captures
            .iter()
            .map(|capture| format!("`{capture}`"))
            .collect::<Vec<_>>()
            .join(", ");
        reasons.push(format!("managed closure retains {captures}"));
    }
    for contract in &facts.explicit_closure_contracts {
        reasons.push(format!("explicit closure {contract}"));
    }
    if facts.has_with {
        reasons.push("resource with block".to_string());
    }
    if facts.has_take {
        reasons.push("take effect".to_string());
    }
    if facts.has_mut {
        reasons.push("mut call-site effect".to_string());
    }
    if facts.has_resource_pool {
        reasons.push("ResourcePool usage".to_string());
    }
    if facts.has_handle_field_write {
        reasons.push("writes through handle field".to_string());
    }
    if facts.has_managed_state_write {
        reasons.push("writes to managed state".to_string());
    }
    if facts.has_error_boundary {
        reasons.push("error handling boundary".to_string());
    }
    if facts.has_capability_object {
        reasons.push("capability object construction".to_string());
    }
    if facts.has_dynamic_protocol_dispatch {
        reasons.push("dynamic protocol dispatch".to_string());
    }
    for callback in &facts.callback_calls {
        reasons.push(format!("noescape callback call `{callback}`"));
    }

    let classification = if !facts.unresolved_calls.is_empty() {
        let calls = facts
            .unresolved_calls
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        reasons.push(format!("unresolved call(s): {calls}"));
        ReviewMapClassification::Unknown
    } else if reasons.is_empty() {
        reasons.push("private pure helper with no retention or resource boundary".to_string());
        ReviewMapClassification::Foldable
    } else {
        ReviewMapClassification::ReviewRequired
    };

    ReviewMapRegionDraft {
        file: file.to_string(),
        region: ReviewMapRegion {
            function: function.name.clone(),
            classification,
            line: function.span.line,
            line_count: review_map_line_count(
                &function.name,
                function.span.line,
                function_lines,
                total_lines,
            ),
            reasons,
            receiver_calls: facts.receiver_calls.clone(),
        },
        facts,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ReviewMapRegionDraft {
    file: String,
    region: ReviewMapRegion,
    facts: ReviewMapFacts,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ReviewMapFacts {
    has_local: bool,
    has_manage: bool,
    has_spawn: bool,
    has_await: bool,
    has_with: bool,
    has_mut: bool,
    has_take: bool,
    has_resource_pool: bool,
    has_handle_field_write: bool,
    has_managed_state_write: bool,
    has_error_boundary: bool,
    has_capability_object: bool,
    has_dynamic_protocol_dispatch: bool,
    user_calls: BTreeSet<String>,
    unresolved_calls: BTreeSet<String>,
    callback_calls: BTreeSet<String>,
    receiver_calls: Vec<ReviewMapReceiverCall>,
    native_calls: BTreeSet<String>,
    unsafe_calls: BTreeSet<String>,
    spawn_captures: BTreeSet<String>,
    managed_closure_captures: BTreeSet<String>,
    explicit_closure_contracts: BTreeSet<String>,
    /// Value types for receiver-call resolution (param_name -> type_name,
    /// plus `__protocol_bound__<T>` -> protocol for generic bounds).
    value_types: HashMap<String, String>,
}

fn propagate_review_map_call_classifications(drafts: &mut [ReviewMapRegionDraft]) {
    propagate_unknown_calls(drafts);
    propagate_review_required_calls(drafts);
}

fn propagate_unknown_calls(drafts: &mut [ReviewMapRegionDraft]) {
    loop {
        let classifications = drafts
            .iter()
            .map(|draft| (draft.region.function.clone(), draft.region.classification))
            .collect::<BTreeMap<_, _>>();
        let mut changed = false;

        for draft in drafts.iter_mut() {
            if draft.region.classification == ReviewMapClassification::Unknown {
                continue;
            }
            let Some(callee) = draft.facts.user_calls.iter().find(|callee| {
                classifications.get(*callee) == Some(&ReviewMapClassification::Unknown)
            }) else {
                continue;
            };
            draft.region.classification = ReviewMapClassification::Unknown;
            draft.region.reasons.retain(|reason| {
                reason != "private pure helper with no retention or resource boundary"
            });
            draft
                .region
                .reasons
                .push(format!("calls unknown `{callee}`"));
            changed = true;
        }

        if !changed {
            break;
        }
    }
}

fn propagate_review_required_calls(drafts: &mut [ReviewMapRegionDraft]) {
    loop {
        let classifications = drafts
            .iter()
            .map(|draft| (draft.region.function.clone(), draft.region.classification))
            .collect::<BTreeMap<_, _>>();
        let mut changed = false;

        for draft in drafts.iter_mut() {
            if draft.region.classification != ReviewMapClassification::Foldable {
                continue;
            }
            let Some(callee) = draft.facts.user_calls.iter().find(|callee| {
                classifications.get(*callee) == Some(&ReviewMapClassification::ReviewRequired)
            }) else {
                continue;
            };
            draft.region.classification = ReviewMapClassification::ReviewRequired;
            draft.region.reasons.retain(|reason| {
                reason != "private pure helper with no retention or resource boundary"
            });
            draft
                .region
                .reasons
                .push(format!("calls must-review `{callee}`"));
            changed = true;
        }

        if !changed {
            break;
        }
    }
}

fn receiver_call_resolution_label(resolution: &CallResolution) -> &'static str {
    match resolution {
        CallResolution::Resolved { kind, .. } => match kind {
            ResolvedCalleeKind::UserFunction => "user_function",
            ResolvedCalleeKind::BuiltinFunction => "builtin_function",
            ResolvedCalleeKind::Constructor { .. } => "constructor",
        },
        CallResolution::EnumVariant => "enum_variant",
        CallResolution::Ambiguous { .. } => "ambiguous",
        CallResolution::Unknown => "unknown",
    }
}

fn review_map_callback_params(function: &FunctionDecl) -> BTreeSet<String> {
    function
        .params
        .iter()
        .filter(|param| param.ty.is_noescape && param.ty.name == "Fn")
        .map(|param| param.name.clone())
        .collect()
}

fn review_map_local_closure_bindings(block: &Block) -> BTreeSet<String> {
    let mut bindings = BTreeSet::new();
    collect_review_map_local_closure_bindings_block(block, &mut bindings);
    bindings
}

fn collect_review_map_local_closure_bindings_block(block: &Block, bindings: &mut BTreeSet<String>) {
    for statement in &block.statements {
        collect_review_map_local_closure_bindings_stmt(statement, bindings);
    }
}

fn collect_review_map_local_closure_bindings_stmt(stmt: &Stmt, bindings: &mut BTreeSet<String>) {
    match stmt {
        Stmt::Let(stmt) => {
            if stmt.kind == LetKind::Local && matches!(stmt.value, Some(Expr::Closure { .. })) {
                bindings.insert(stmt.name.clone());
            }
            if let Some(value) = &stmt.value {
                collect_review_map_local_closure_bindings_expr(value, bindings);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_review_map_local_closure_bindings_expr(value, bindings);
            }
        }
        Stmt::With(stmt) => {
            collect_review_map_local_closure_bindings_expr(&stmt.resource, bindings);
            collect_review_map_local_closure_bindings_block(&stmt.body, bindings);
        }
        Stmt::If(stmt) => {
            collect_review_map_local_closure_bindings_expr(&stmt.condition, bindings);
            collect_review_map_local_closure_bindings_block(&stmt.then_body, bindings);
            if let Some(else_body) = &stmt.else_body {
                collect_review_map_local_closure_bindings_block(else_body, bindings);
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                collect_review_map_local_closure_bindings_expr(condition, bindings);
            }
            collect_review_map_local_closure_bindings_block(&stmt.body, bindings);
        }
        Stmt::For(stmt) => {
            collect_review_map_local_closure_bindings_expr(&stmt.iterable, bindings);
            collect_review_map_local_closure_bindings_block(&stmt.body, bindings);
        }
        Stmt::TaskGroup(stmt) => {
            collect_review_map_local_closure_bindings_block(&stmt.body, bindings);
        }
        Stmt::Select(stmt) => {
            for arm in &stmt.arms {
                collect_review_map_local_closure_bindings_expr(&arm.operation, bindings);
                collect_review_map_local_closure_bindings_block(&arm.body, bindings);
            }
        }
        Stmt::Match(stmt) => {
            collect_review_map_local_closure_bindings_expr(&stmt.value, bindings);
            for arm in &stmt.arms {
                collect_review_map_local_closure_bindings_block(&arm.body, bindings);
            }
        }
        Stmt::LetElse(stmt) => {
            collect_review_map_local_closure_bindings_expr(&stmt.value, bindings);
            collect_review_map_local_closure_bindings_block(&stmt.else_body, bindings);
        }
        Stmt::Assign(stmt) => {
            collect_review_map_local_closure_bindings_expr(&stmt.target, bindings);
            collect_review_map_local_closure_bindings_expr(&stmt.value, bindings);
        }
        Stmt::Expr(expr) => collect_review_map_local_closure_bindings_expr(expr, bindings),
        Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Unknown(_) => {}
    }
}

fn collect_review_map_local_closure_bindings_expr(expr: &Expr, bindings: &mut BTreeSet<String>) {
    match expr {
        Expr::Call { args, .. } => {
            for arg in args {
                collect_review_map_local_closure_bindings_expr(&arg.value, bindings);
            }
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. }
        | Expr::Field { base: value, .. } => {
            collect_review_map_local_closure_bindings_expr(value, bindings);
        }
        Expr::Index { base, index, .. }
        | Expr::Binary {
            left: base,
            right: index,
            ..
        } => {
            collect_review_map_local_closure_bindings_expr(base, bindings);
            collect_review_map_local_closure_bindings_expr(index, bindings);
        }
        Expr::Closure { body, .. } => {
            collect_review_map_local_closure_bindings_block(body, bindings)
        }
        Expr::Match { value, arms, .. } => {
            collect_review_map_local_closure_bindings_expr(value, bindings);
            for arm in arms {
                collect_review_map_local_closure_bindings_block(&arm.body, bindings);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_review_map_local_closure_bindings_expr(&entry.key, bindings);
                collect_review_map_local_closure_bindings_expr(&entry.value, bindings);
            }
        }
        Expr::ObjectLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

fn collect_review_map_facts_block(
    block: &Block,
    hir: &Hir,
    callback_params: &BTreeSet<String>,
    local_closure_bindings: &BTreeSet<String>,
    facts: &mut ReviewMapFacts,
) {
    for statement in &block.statements {
        collect_review_map_facts_stmt(
            statement,
            hir,
            callback_params,
            local_closure_bindings,
            facts,
        );
    }
}

fn collect_review_map_facts_stmt(
    statement: &Stmt,
    hir: &Hir,
    callback_params: &BTreeSet<String>,
    local_closure_bindings: &BTreeSet<String>,
    facts: &mut ReviewMapFacts,
) {
    match statement {
        Stmt::Let(stmt) => {
            if stmt.kind == LetKind::Local {
                facts.has_local = true;
            }
            if let Some(ty) = &stmt.type_annotation {
                facts
                    .value_types
                    .insert(stmt.name.clone(), type_ref_display_name(ty));
            }
            if let Some(value) = &stmt.value {
                if !facts.value_types.contains_key(&stmt.name)
                    && let Some(type_name) =
                        review_map_expr_type_name_with_facts(value, hir, &facts.value_types)
                {
                    facts.value_types.insert(stmt.name.clone(), type_name);
                }
                collect_review_map_facts_expr(
                    value,
                    hir,
                    callback_params,
                    local_closure_bindings,
                    facts,
                );
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_review_map_facts_expr(
                    value,
                    hir,
                    callback_params,
                    local_closure_bindings,
                    facts,
                );
            }
        }
        Stmt::With(stmt) => {
            facts.has_with = true;
            collect_review_map_facts_expr(
                &stmt.resource,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
            collect_review_map_facts_block(
                &stmt.body,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
        }
        Stmt::If(stmt) => {
            collect_review_map_facts_expr(
                &stmt.condition,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
            collect_review_map_facts_block(
                &stmt.then_body,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
            if let Some(else_body) = &stmt.else_body {
                collect_review_map_facts_block(
                    else_body,
                    hir,
                    callback_params,
                    local_closure_bindings,
                    facts,
                );
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                collect_review_map_facts_expr(
                    condition,
                    hir,
                    callback_params,
                    local_closure_bindings,
                    facts,
                );
            }
            collect_review_map_facts_block(
                &stmt.body,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
        }
        Stmt::For(stmt) => {
            collect_review_map_facts_expr(
                &stmt.iterable,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
            collect_review_map_facts_block(
                &stmt.body,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
        }
        Stmt::TaskGroup(stmt) => {
            collect_review_map_facts_block(
                &stmt.body,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
        }
        Stmt::Select(stmt) => {
            for arm in &stmt.arms {
                collect_review_map_facts_expr(
                    &arm.operation,
                    hir,
                    callback_params,
                    local_closure_bindings,
                    facts,
                );
                collect_review_map_facts_block(
                    &arm.body,
                    hir,
                    callback_params,
                    local_closure_bindings,
                    facts,
                );
            }
        }
        Stmt::Match(stmt) => {
            collect_review_map_facts_expr(
                &stmt.value,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
            let value_type =
                review_map_expr_type_name_with_facts(&stmt.value, hir, &facts.value_types);
            for arm in &stmt.arms {
                let scoped_binding =
                    review_map_match_binding_type(&arm.pattern, value_type.as_deref());
                let previous = scoped_binding
                    .as_ref()
                    .and_then(|(binding, _)| facts.value_types.get(binding).cloned());
                if let Some((binding, type_name)) = &scoped_binding {
                    facts.value_types.insert(binding.clone(), type_name.clone());
                }
                collect_review_map_facts_block(
                    &arm.body,
                    hir,
                    callback_params,
                    local_closure_bindings,
                    facts,
                );
                if let Some((binding, _)) = scoped_binding {
                    if let Some(previous) = previous {
                        facts.value_types.insert(binding, previous);
                    } else {
                        facts.value_types.remove(&binding);
                    }
                }
            }
        }
        Stmt::LetElse(stmt) => {
            collect_review_map_facts_expr(
                &stmt.value,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
            collect_review_map_facts_block(
                &stmt.else_body,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
        }
        Stmt::Assign(stmt) => {
            collect_review_map_facts_expr(
                &stmt.target,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
            collect_review_map_facts_expr(
                &stmt.value,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
        }
        Stmt::Expr(expr) => {
            collect_review_map_facts_expr(expr, hir, callback_params, local_closure_bindings, facts)
        }
        Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Unknown(_) => {}
    }
}

fn review_map_expr_type_name(expr: &Expr, hir: &Hir) -> Option<String> {
    match expr {
        Expr::Call { callee, .. } => match hir.resolve_call(callee) {
            CallResolution::Resolved { signature, kind } => {
                if let ResolvedCalleeKind::Constructor { .. } = kind
                    && let Callee::Qualified { namespace, .. } = callee
                {
                    return Some(namespace.clone());
                }
                signature.return_type.clone()
            }
            CallResolution::EnumVariant
            | CallResolution::Ambiguous { .. }
            | CallResolution::Unknown => None,
        },
        Expr::Effect { value, .. } => review_map_expr_type_name(value, hir),
        Expr::Try { value, .. } => {
            review_map_expr_type_name(value, hir).and_then(|ty| result_ok_type_name(&ty))
        }
        Expr::Await { value, .. } => review_map_expr_type_name(value, hir),
        Expr::String(_, _) | Expr::MultilineString(_, _) => Some("String".to_string()),
        Expr::Number(value, _) => Some(crate::hir::number_literal_type_name(value).to_string()),
        Expr::Ident(name, _) if matches!(name.as_str(), "true" | "false") => {
            Some("Bool".to_string())
        }
        Expr::Ident(name, _) => hir.sum_type_for_variant(name).map(str::to_string),
        Expr::ArrayLiteral { items, .. } => items
            .first()
            .and_then(|item| review_map_expr_type_name(item, hir))
            .map(|item| format!("List<{item}>"))
            .or_else(|| Some("List".to_string())),
        Expr::MapLiteral { .. } => Some("MapLiteral".to_string()),
        Expr::ObjectLiteral { .. } => Some("JsonLiteral".to_string()),
        _ => None,
    }
}

fn collect_review_map_facts_expr(
    expr: &Expr,
    hir: &Hir,
    callback_params: &BTreeSet<String>,
    local_closure_bindings: &BTreeSet<String>,
    facts: &mut ReviewMapFacts,
) {
    match expr {
        Expr::Call { callee, args, span } => {
            if is_resource_pool_callee(callee) {
                facts.has_resource_pool = true;
            }
            if is_capability_from_callee(callee) {
                facts.has_capability_object = true;
            }
            if let Some(callback) = review_map_callback_call(callee, callback_params) {
                facts.callback_calls.insert(callback.to_string());
            } else if review_map_local_closure_call(callee, local_closure_bindings).is_none() {
                let resolution = match callee {
                    Callee::ReceiverCall {
                        receiver,
                        method,
                        effect,
                    } => {
                        let receiver_label = review_expr_label(receiver);
                        if let Some(receiver_type) =
                            review_map_expr_type_name_with_facts(receiver, hir, &facts.value_types)
                        {
                            let (resolution, namespace) = hir.resolve_receiver_call(
                                &receiver_type,
                                method,
                                &facts.value_types,
                            );
                            if capability_protocol_name(&receiver_type)
                                .is_some_and(|protocol| namespace.as_deref() == Some(protocol))
                            {
                                facts.has_dynamic_protocol_dispatch = true;
                            }
                            facts.receiver_calls.push(ReviewMapReceiverCall {
                                line: span.line,
                                column: span.column,
                                source: format!("{} {receiver_label}.{method}", effect.as_str()),
                                canonical_callee: namespace
                                    .map(|namespace| format!("{namespace}.{method}"))
                                    .unwrap_or_else(|| format!("<unresolved>.{method}")),
                                self_effect: effect.as_str().to_string(),
                                resolution: receiver_call_resolution_label(&resolution).to_string(),
                            });
                            resolution
                        } else {
                            facts.receiver_calls.push(ReviewMapReceiverCall {
                                line: span.line,
                                column: span.column,
                                source: format!("{} {receiver_label}.{method}", effect.as_str()),
                                canonical_callee: format!("<unresolved>.{method}"),
                                self_effect: effect.as_str().to_string(),
                                resolution: "unknown".to_string(),
                            });
                            CallResolution::Unknown
                        }
                    }
                    _ => hir.resolve_call(callee),
                };
                match resolution {
                    CallResolution::Resolved { signature, kind } => {
                        collect_call_boundary_facts(&signature, facts);
                        if is_capability_protocol_call(callee, args, &facts.value_types) {
                            facts.has_dynamic_protocol_dispatch = true;
                        }
                        if kind == ResolvedCalleeKind::UserFunction {
                            facts.user_calls.insert(function_sig_key(&signature));
                        }
                    }
                    CallResolution::Ambiguous { .. } | CallResolution::Unknown => {
                        facts.unresolved_calls.insert(review_callee_display(callee));
                    }
                    CallResolution::EnumVariant => {}
                }
            }
            for arg in args {
                collect_review_map_facts_expr(
                    &arg.value,
                    hir,
                    callback_params,
                    local_closure_bindings,
                    facts,
                );
            }
        }
        Expr::Effect { effect, value, .. } => {
            match effect {
                DataEffect::Mut => facts.has_mut = true,
                DataEffect::Take => facts.has_take = true,
                DataEffect::Read => {}
            }
            collect_review_map_facts_expr(
                value,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
        }
        Expr::Manage { value, .. } => {
            facts.has_manage = true;
            collect_review_map_facts_expr(
                value,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
        }
        Expr::Spawn { value, .. } => {
            facts.has_spawn = true;
            collect_spawn_capture_names(value, &mut facts.spawn_captures);
            collect_review_map_facts_expr(
                value,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
        }
        Expr::Await { value, .. } => {
            facts.has_await = true;
            collect_review_map_facts_expr(
                value,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
        }
        Expr::Try { value, .. } => {
            facts.has_error_boundary = true;
            collect_review_map_facts_expr(
                value,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
        }
        Expr::Match { value, arms, .. } => {
            collect_review_map_facts_expr(
                value,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
            for arm in arms {
                collect_review_map_facts_block(
                    &arm.body,
                    hir,
                    callback_params,
                    local_closure_bindings,
                    facts,
                );
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_review_map_facts_expr(
                left,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
            collect_review_map_facts_expr(
                right,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
        }
        Expr::Field { base, .. } => {
            collect_review_map_facts_expr(base, hir, callback_params, local_closure_bindings, facts)
        }
        Expr::Index { base, index, .. } => {
            collect_review_map_facts_expr(
                base,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
            collect_review_map_facts_expr(
                index,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            );
        }
        Expr::Closure {
            body,
            captures,
            declared_effects,
            explicit,
            ..
        } => {
            if *explicit {
                for capture in captures {
                    facts.explicit_closure_contracts.insert(format!(
                        "captures {} `{}`",
                        effect_label(capture.effect),
                        capture.name
                    ));
                }
                if !declared_effects.is_empty() {
                    facts
                        .explicit_closure_contracts
                        .insert(format!("effects({})", declared_effects.join(", ")));
                }
            }
            collect_review_map_facts_block(
                body,
                hir,
                callback_params,
                local_closure_bindings,
                facts,
            )
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_review_map_facts_expr(
                    &entry.key,
                    hir,
                    callback_params,
                    local_closure_bindings,
                    facts,
                );
                collect_review_map_facts_expr(
                    &entry.value,
                    hir,
                    callback_params,
                    local_closure_bindings,
                    facts,
                );
            }
        }
        Expr::ObjectLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

fn collect_call_boundary_facts(signature: &HirFunctionSig, facts: &mut ReviewMapFacts) {
    let callee = function_sig_key(signature);
    if signature.effects.iter().any(|effect| effect == "native") {
        facts.native_calls.insert(callee.clone());
    }
    if signature.effects.iter().any(|effect| effect == "unsafe") {
        facts.unsafe_calls.insert(callee);
    }
}

fn review_map_callback_call<'a>(
    callee: &Callee,
    callback_params: &'a BTreeSet<String>,
) -> Option<&'a str> {
    match callee {
        Callee::Name(name) => callback_params.get(name).map(String::as_str),
        Callee::Qualified { .. } | Callee::ReceiverCall { .. } => None,
    }
}

fn review_map_local_closure_call<'a>(
    callee: &Callee,
    local_closure_bindings: &'a BTreeSet<String>,
) -> Option<&'a str> {
    match callee {
        Callee::Name(name) => local_closure_bindings.get(name).map(String::as_str),
        Callee::Qualified { .. } | Callee::ReceiverCall { .. } => None,
    }
}

fn collect_spawn_capture_names(expr: &Expr, captures: &mut BTreeSet<String>) {
    match expr {
        Expr::Ident(name, _) => {
            captures.insert(name.clone());
        }
        Expr::Effect { value, .. } | Expr::Try { value, .. } => {
            collect_spawn_capture_names(value, captures);
        }
        Expr::Manage { .. } => {}
        Expr::Call { args, .. } => {
            for arg in args {
                collect_spawn_capture_names(&arg.value, captures);
            }
        }
        Expr::Field { base, name, .. } => {
            if let Some(base_name) = spawn_capture_path(base) {
                captures.insert(format!("{base_name}.{name}"));
            } else {
                collect_spawn_capture_names(base, captures);
            }
        }
        Expr::Index { base, index, .. } => {
            collect_spawn_capture_names(base, captures);
            collect_spawn_capture_names(index, captures);
        }
        Expr::Binary { left, right, .. } => {
            collect_spawn_capture_names(left, captures);
            collect_spawn_capture_names(right, captures);
        }
        Expr::Match { value, arms, .. } => {
            collect_spawn_capture_names(value, captures);
            for arm in arms {
                for statement in &arm.body.statements {
                    collect_spawn_capture_names_from_stmt(statement, captures);
                }
            }
        }
        Expr::Spawn { value, .. } => collect_spawn_capture_names(value, captures),
        Expr::Await { value, .. } => collect_spawn_capture_names(value, captures),
        Expr::Closure { body, .. } => {
            for statement in &body.statements {
                collect_spawn_capture_names_from_stmt(statement, captures);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_spawn_capture_names(&entry.key, captures);
                collect_spawn_capture_names(&entry.value, captures);
            }
        }
        Expr::ObjectLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

fn collect_spawn_capture_names_from_stmt(stmt: &Stmt, captures: &mut BTreeSet<String>) {
    match stmt {
        Stmt::Let(stmt) => {
            if let Some(value) = &stmt.value {
                collect_spawn_capture_names(value, captures);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_spawn_capture_names(value, captures);
            }
        }
        Stmt::Assign(stmt) => {
            collect_spawn_capture_names(&stmt.target, captures);
            collect_spawn_capture_names(&stmt.value, captures);
        }
        Stmt::Expr(value) => collect_spawn_capture_names(value, captures),
        Stmt::With(stmt) => {
            collect_spawn_capture_names(&stmt.resource, captures);
            for statement in &stmt.body.statements {
                collect_spawn_capture_names_from_stmt(statement, captures);
            }
        }
        Stmt::If(stmt) => {
            collect_spawn_capture_names(&stmt.condition, captures);
            for statement in &stmt.then_body.statements {
                collect_spawn_capture_names_from_stmt(statement, captures);
            }
            if let Some(else_body) = &stmt.else_body {
                for statement in &else_body.statements {
                    collect_spawn_capture_names_from_stmt(statement, captures);
                }
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                collect_spawn_capture_names(condition, captures);
            }
            for statement in &stmt.body.statements {
                collect_spawn_capture_names_from_stmt(statement, captures);
            }
        }
        Stmt::For(stmt) => {
            collect_spawn_capture_names(&stmt.iterable, captures);
            for statement in &stmt.body.statements {
                collect_spawn_capture_names_from_stmt(statement, captures);
            }
        }
        Stmt::TaskGroup(stmt) => {
            for statement in &stmt.body.statements {
                collect_spawn_capture_names_from_stmt(statement, captures);
            }
        }
        Stmt::Select(stmt) => {
            for arm in &stmt.arms {
                collect_spawn_capture_names(&arm.operation, captures);
                for statement in &arm.body.statements {
                    collect_spawn_capture_names_from_stmt(statement, captures);
                }
            }
        }
        Stmt::Match(stmt) => {
            collect_spawn_capture_names(&stmt.value, captures);
            for arm in &stmt.arms {
                for statement in &arm.body.statements {
                    collect_spawn_capture_names_from_stmt(statement, captures);
                }
            }
        }
        Stmt::LetElse(stmt) => {
            collect_spawn_capture_names(&stmt.value, captures);
            for statement in &stmt.else_body.statements {
                collect_spawn_capture_names_from_stmt(statement, captures);
            }
        }
        Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Unknown(_) => {}
    }
}

fn spawn_capture_path(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(name, _) => Some(name.clone()),
        Expr::Field { base, name, .. } => {
            spawn_capture_path(base).map(|base| format!("{base}.{name}"))
        }
        Expr::Effect { value, .. } | Expr::Try { value, .. } => spawn_capture_path(value),
        Expr::Manage { .. }
        | Expr::MapLiteral { .. }
        | Expr::ObjectLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Spawn { .. }
        | Expr::Await { .. }
        | Expr::Index { .. }
        | Expr::Call { .. }
        | Expr::Binary { .. }
        | Expr::Closure { .. }
        | Expr::Match { .. }
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => None,
    }
}

fn collect_review_map_hir_facts_block(
    block: &HirBlock,
    local_bindings: &BTreeSet<&str>,
    facts: &mut ReviewMapFacts,
) {
    for statement in &block.statements {
        collect_review_map_hir_facts_stmt(statement, local_bindings, facts);
    }
}

fn function_sig_key(signature: &HirFunctionSig) -> String {
    if let Some(namespace) = &signature.namespace {
        format!("{namespace}.{}", signature.name)
    } else {
        signature.name.clone()
    }
}

fn collect_review_map_hir_facts_stmt(
    statement: &HirStmt,
    local_bindings: &BTreeSet<&str>,
    facts: &mut ReviewMapFacts,
) {
    match statement {
        HirStmt::Let {
            kind: HirBindingKind::ManagedLet,
            value: Some(HirExpr::Closure { body, .. }),
            ..
        } => {
            collect_managed_closure_capture_names(body, local_bindings, facts);
            collect_review_map_hir_facts_block(body, local_bindings, facts);
        }
        HirStmt::Let {
            kind: HirBindingKind::LocalLet,
            value: Some(HirExpr::Closure { body, .. }),
            ..
        } => {
            collect_review_map_hir_facts_block(body, local_bindings, facts);
        }
        HirStmt::Let { value, .. } | HirStmt::Return { value, .. } => {
            if let Some(value) = value {
                collect_review_map_hir_facts_expr(value, local_bindings, facts);
            }
        }
        HirStmt::With { resource, body, .. } => {
            collect_review_map_hir_facts_expr(resource, local_bindings, facts);
            collect_review_map_hir_facts_block(body, local_bindings, facts);
        }
        HirStmt::If {
            condition,
            then_body,
            else_body,
            ..
        } => {
            collect_review_map_hir_facts_expr(condition, local_bindings, facts);
            collect_review_map_hir_facts_block(then_body, local_bindings, facts);
            if let Some(else_body) = else_body {
                collect_review_map_hir_facts_block(else_body, local_bindings, facts);
            }
        }
        HirStmt::Loop {
            condition, body, ..
        } => {
            if let Some(condition) = condition {
                collect_review_map_hir_facts_expr(condition, local_bindings, facts);
            }
            collect_review_map_hir_facts_block(body, local_bindings, facts);
        }
        HirStmt::For { iterable, body, .. } => {
            collect_review_map_hir_facts_expr(iterable, local_bindings, facts);
            collect_review_map_hir_facts_block(body, local_bindings, facts);
        }
        HirStmt::Match { value, arms, .. } => {
            collect_review_map_hir_facts_expr(value, local_bindings, facts);
            for arm in arms {
                collect_review_map_hir_facts_block(&arm.body, local_bindings, facts);
            }
        }
        HirStmt::Select { arms, .. } => {
            for arm in arms {
                collect_review_map_hir_facts_expr(&arm.operation, local_bindings, facts);
                collect_review_map_hir_facts_block(&arm.body, local_bindings, facts);
            }
        }
        HirStmt::Expr(expr) | HirStmt::Assign { value: expr, .. } => {
            collect_review_map_hir_facts_expr(expr, local_bindings, facts)
        }
        HirStmt::Break(_) | HirStmt::Continue(_) | HirStmt::Unknown(_) => {}
    }
}

fn collect_review_map_hir_facts_expr(
    expr: &HirExpr,
    local_bindings: &BTreeSet<&str>,
    facts: &mut ReviewMapFacts,
) {
    if hir_expr_writes_through_handle_field(expr) {
        facts.has_handle_field_write = true;
    }
    if hir_expr_writes_to_managed_state(expr, local_bindings) {
        facts.has_managed_state_write = true;
    }
    match expr {
        HirExpr::Binary { left, right, .. } => {
            collect_review_map_hir_facts_expr(left, local_bindings, facts);
            collect_review_map_hir_facts_expr(right, local_bindings, facts);
        }
        HirExpr::Field { base, .. } => {
            collect_review_map_hir_facts_expr(base, local_bindings, facts)
        }
        HirExpr::Index { base, index, .. } => {
            collect_review_map_hir_facts_expr(base, local_bindings, facts);
            collect_review_map_hir_facts_expr(index, local_bindings, facts);
        }
        HirExpr::Call {
            args, resolution, ..
        } => {
            if let CallResolution::Resolved { signature, .. } = resolution {
                collect_call_boundary_facts(signature, facts);
            }
            for (index, arg) in args.iter().enumerate() {
                if let Some(body) = noescape_call_closure_body(arg, index, resolution) {
                    collect_review_map_hir_facts_block(body, local_bindings, facts);
                } else {
                    collect_review_map_hir_facts_expr(&arg.value, local_bindings, facts);
                }
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_review_map_hir_facts_expr(value, local_bindings, facts);
        }
        HirExpr::Closure { body, .. } => {
            collect_managed_closure_capture_names(body, local_bindings, facts);
            collect_review_map_hir_facts_block(body, local_bindings, facts);
        }
        HirExpr::Match { value, arms, .. } => {
            collect_review_map_hir_facts_expr(value, local_bindings, facts);
            for arm in arms {
                collect_review_map_hir_facts_block(&arm.body, local_bindings, facts);
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_review_map_hir_facts_expr(&entry.key, local_bindings, facts);
                collect_review_map_hir_facts_expr(&entry.value, local_bindings, facts);
            }
        }
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn noescape_call_closure_body<'a>(
    arg: &'a crate::hir::HirCallArg,
    index: usize,
    resolution: &CallResolution,
) -> Option<&'a HirBlock> {
    if !call_arg_is_noescape_param(arg, index, resolution) {
        return None;
    }
    hir_closure_body(&arg.value)
}

fn call_arg_is_noescape_param(
    arg: &crate::hir::HirCallArg,
    index: usize,
    resolution: &CallResolution,
) -> bool {
    let CallResolution::Resolved { signature, .. } = resolution else {
        return false;
    };
    let Some(param) = arg
        .name
        .as_ref()
        .and_then(|name| signature.params.iter().find(|param| param.name == *name))
        .or_else(|| signature.params.get(index))
    else {
        return false;
    };
    param
        .type_name
        .strip_prefix("noescape Fn(")
        .and_then(|rest| rest.split_once(')'))
        .is_some()
}

fn hir_closure_body(expr: &HirExpr) -> Option<&HirBlock> {
    match expr {
        HirExpr::Closure { body, .. } => Some(body),
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => hir_closure_body(value),
        _ => None,
    }
}

fn collect_managed_closure_capture_names(
    body: &HirBlock,
    local_bindings: &BTreeSet<&str>,
    facts: &mut ReviewMapFacts,
) {
    let mut closure_locals = BTreeSet::new();
    collect_managed_closure_capture_names_block(body, local_bindings, &mut closure_locals, facts);
}

fn collect_managed_closure_capture_names_block(
    block: &HirBlock,
    local_bindings: &BTreeSet<&str>,
    closure_locals: &mut BTreeSet<String>,
    facts: &mut ReviewMapFacts,
) {
    for statement in &block.statements {
        match statement {
            HirStmt::Let { name, value, .. } => {
                if let Some(value) = value {
                    collect_managed_closure_capture_names_expr(
                        value,
                        local_bindings,
                        closure_locals,
                        facts,
                    );
                }
                closure_locals.insert(name.clone());
            }
            HirStmt::Return {
                value: Some(value), ..
            }
            | HirStmt::Expr(value)
            | HirStmt::Assign { value, .. } => {
                collect_managed_closure_capture_names_expr(
                    value,
                    local_bindings,
                    closure_locals,
                    facts,
                );
            }
            HirStmt::With {
                resource,
                binding,
                body,
                ..
            } => {
                collect_managed_closure_capture_names_expr(
                    resource,
                    local_bindings,
                    closure_locals,
                    facts,
                );
                closure_locals.insert(binding.clone());
                collect_managed_closure_capture_names_block(
                    body,
                    local_bindings,
                    closure_locals,
                    facts,
                );
                closure_locals.remove(binding);
            }
            HirStmt::If {
                condition,
                then_body,
                else_body,
                ..
            } => {
                collect_managed_closure_capture_names_expr(
                    condition,
                    local_bindings,
                    closure_locals,
                    facts,
                );
                let mut then_locals = closure_locals.clone();
                collect_managed_closure_capture_names_block(
                    then_body,
                    local_bindings,
                    &mut then_locals,
                    facts,
                );
                if let Some(else_body) = else_body {
                    let mut else_locals = closure_locals.clone();
                    collect_managed_closure_capture_names_block(
                        else_body,
                        local_bindings,
                        &mut else_locals,
                        facts,
                    );
                }
            }
            HirStmt::Loop {
                condition, body, ..
            } => {
                if let Some(condition) = condition {
                    collect_managed_closure_capture_names_expr(
                        condition,
                        local_bindings,
                        closure_locals,
                        facts,
                    );
                }
                let mut body_locals = closure_locals.clone();
                collect_managed_closure_capture_names_block(
                    body,
                    local_bindings,
                    &mut body_locals,
                    facts,
                );
            }
            HirStmt::For {
                binding,
                iterable,
                body,
                ..
            } => {
                collect_managed_closure_capture_names_expr(
                    iterable,
                    local_bindings,
                    closure_locals,
                    facts,
                );
                let mut body_locals = closure_locals.clone();
                body_locals.insert(binding.clone());
                collect_managed_closure_capture_names_block(
                    body,
                    local_bindings,
                    &mut body_locals,
                    facts,
                );
            }
            HirStmt::Match { value, arms, .. } => {
                collect_managed_closure_capture_names_expr(
                    value,
                    local_bindings,
                    closure_locals,
                    facts,
                );
                for arm in arms {
                    let mut arm_locals = closure_locals.clone();
                    for binding in arm.pattern.binding_names() {
                        arm_locals.insert(binding.to_string());
                    }
                    collect_managed_closure_capture_names_block(
                        &arm.body,
                        local_bindings,
                        &mut arm_locals,
                        facts,
                    );
                }
            }
            HirStmt::Select { arms, .. } => {
                for arm in arms {
                    collect_managed_closure_capture_names_expr(
                        &arm.operation,
                        local_bindings,
                        closure_locals,
                        facts,
                    );
                    let mut arm_locals = closure_locals.clone();
                    if arm.binding != "_" {
                        arm_locals.insert(arm.binding.clone());
                    }
                    collect_managed_closure_capture_names_block(
                        &arm.body,
                        local_bindings,
                        &mut arm_locals,
                        facts,
                    );
                }
            }
            HirStmt::Return { value: None, .. }
            | HirStmt::Break(_)
            | HirStmt::Continue(_)
            | HirStmt::Unknown(_) => {}
        }
    }
}

fn collect_managed_closure_capture_names_expr(
    expr: &HirExpr,
    local_bindings: &BTreeSet<&str>,
    closure_locals: &BTreeSet<String>,
    facts: &mut ReviewMapFacts,
) {
    if let Some((root, path)) = hir_capture_path(expr)
        && !local_bindings.contains(root)
        && !closure_locals.contains(root)
    {
        facts.managed_closure_captures.insert(path);
        return;
    }

    match expr {
        HirExpr::Binary { left, right, .. } => {
            collect_managed_closure_capture_names_expr(left, local_bindings, closure_locals, facts);
            collect_managed_closure_capture_names_expr(
                right,
                local_bindings,
                closure_locals,
                facts,
            );
        }
        HirExpr::Index { base, index, .. } => {
            collect_managed_closure_capture_names_expr(base, local_bindings, closure_locals, facts);
            collect_managed_closure_capture_names_expr(
                index,
                local_bindings,
                closure_locals,
                facts,
            );
        }
        HirExpr::Call { args, .. } => {
            for arg in args {
                collect_managed_closure_capture_names_expr(
                    &arg.value,
                    local_bindings,
                    closure_locals,
                    facts,
                );
            }
        }
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => {
            collect_managed_closure_capture_names_expr(
                value,
                local_bindings,
                closure_locals,
                facts,
            );
        }
        HirExpr::Closure { body, .. } => {
            let mut nested_locals = closure_locals.clone();
            collect_managed_closure_capture_names_block(
                body,
                local_bindings,
                &mut nested_locals,
                facts,
            );
        }
        HirExpr::Match { value, arms, .. } => {
            collect_managed_closure_capture_names_expr(
                value,
                local_bindings,
                closure_locals,
                facts,
            );
            for arm in arms {
                let mut arm_locals = closure_locals.clone();
                collect_managed_closure_capture_names_block(
                    &arm.body,
                    local_bindings,
                    &mut arm_locals,
                    facts,
                );
            }
        }
        HirExpr::MapLiteral { entries, .. } => {
            for entry in entries {
                collect_managed_closure_capture_names_expr(
                    &entry.key,
                    local_bindings,
                    closure_locals,
                    facts,
                );
                collect_managed_closure_capture_names_expr(
                    &entry.value,
                    local_bindings,
                    closure_locals,
                    facts,
                );
            }
        }
        HirExpr::Field { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Ident { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => {}
    }
}

fn hir_capture_path(expr: &HirExpr) -> Option<(&str, String)> {
    match expr {
        HirExpr::Ident { name, .. } => Some((name.as_str(), name.clone())),
        HirExpr::Field { base, name, .. } => {
            let (root, path) = hir_capture_path(base)?;
            Some((root, format!("{path}.{name}")))
        }
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => hir_capture_path(value),
        _ => None,
    }
}

fn hir_expr_writes_to_managed_state(expr: &HirExpr, local_bindings: &BTreeSet<&str>) -> bool {
    match expr {
        HirExpr::Effect {
            effect: ParamEffect::Mut | ParamEffect::Take,
            value,
            ..
        } => hir_place_path_root(value).is_some_and(|root| !local_bindings.contains(root)),
        _ => false,
    }
}

fn hir_expr_writes_through_handle_field(expr: &HirExpr) -> bool {
    match expr {
        HirExpr::Effect {
            effect: ParamEffect::Mut | ParamEffect::Take,
            value,
            ..
        } => hir_place_path_crosses_handle_field(value),
        _ => false,
    }
}

fn hir_place_path_root(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident { name, .. } => Some(name),
        HirExpr::Field { base, .. } | HirExpr::Index { base, .. } => hir_place_path_root(base),
        HirExpr::Effect { value, .. }
        | HirExpr::Manage { value, .. }
        | HirExpr::Spawn { value, .. }
        | HirExpr::Await { value, .. }
        | HirExpr::Try { value, .. } => hir_place_path_root(value),
        HirExpr::Binary { .. }
        | HirExpr::MapLiteral { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Call { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Match { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn hir_place_path_crosses_handle_field(expr: &HirExpr) -> bool {
    match expr {
        HirExpr::Field { base, access, .. } => {
            access.is_handle || hir_place_path_crosses_handle_field(base)
        }
        HirExpr::Index { base, .. }
        | HirExpr::Manage { value: base, .. }
        | HirExpr::Spawn { value: base, .. }
        | HirExpr::Await { value: base, .. } => hir_place_path_crosses_handle_field(base),
        HirExpr::Effect { value, .. } | HirExpr::Try { value, .. } => {
            hir_place_path_crosses_handle_field(value)
        }
        HirExpr::Ident { .. } => false,
        HirExpr::Binary { .. }
        | HirExpr::MapLiteral { .. }
        | HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Call { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Match { .. }
        | HirExpr::Number { .. }
        | HirExpr::String { .. }
        | HirExpr::Unknown(_) => false,
    }
}

fn review_callee_display(callee: &Callee) -> String {
    match callee {
        Callee::Name(name) => name.clone(),
        Callee::Qualified { namespace, name } => format!("{namespace}.{name}"),
        Callee::ReceiverCall {
            receiver,
            method,
            effect,
        } => format!(
            "{} {}.{method}",
            effect.as_str(),
            review_expr_label(receiver)
        ),
    }
}

fn review_map_expr_type_name_with_facts(
    expr: &Expr,
    hir: &Hir,
    value_types: &HashMap<String, String>,
) -> Option<String> {
    match expr {
        Expr::Ident(name, _) => value_types
            .get(name)
            .cloned()
            .or_else(|| hir.sum_type_for_variant(name).map(str::to_string)),
        Expr::Call {
            callee: Callee::ReceiverCall {
                receiver, method, ..
            },
            ..
        } => {
            let receiver_type = review_map_expr_type_name_with_facts(receiver, hir, value_types)?;
            match hir
                .resolve_receiver_call(&receiver_type, method, value_types)
                .0
            {
                CallResolution::Resolved { signature, .. } => signature.return_type,
                CallResolution::Ambiguous { .. }
                | CallResolution::EnumVariant
                | CallResolution::Unknown => None,
            }
        }
        Expr::Call { .. } => review_map_expr_type_name(expr, hir),
        Expr::Effect { value, .. } | Expr::Manage { value, .. } => {
            review_map_expr_type_name_with_facts(value, hir, value_types)
        }
        Expr::Try { value, .. } => review_map_expr_type_name_with_facts(value, hir, value_types)
            .and_then(|ty| result_ok_type_name(&ty)),
        Expr::Await { value, .. } => review_map_expr_type_name_with_facts(value, hir, value_types),
        Expr::Field { base, name, .. } => {
            let base_type = review_map_expr_type_name_with_facts(base, hir, value_types)?;
            hir.type_info(&base_type)
                .and_then(|info| info.fields.get(name))
                .map(|field| field.type_name.clone())
        }
        _ => review_map_expr_type_name(expr, hir),
    }
}

fn review_expr_label(expr: &Expr) -> String {
    match expr {
        Expr::Ident(name, _) => name.clone(),
        Expr::String(value, _) | Expr::MultilineString(value, _) => format!("{value:?}"),
        Expr::Field { base, name, .. } => format!("{}.{}", review_expr_label(base), name),
        Expr::Index { base, .. } => format!("{}[]", review_expr_label(base)),
        Expr::Call { callee, .. } => format!("{}()", review_callee_display(callee)),
        Expr::Effect { value, .. } | Expr::Manage { value, .. } | Expr::Try { value, .. } => {
            review_expr_label(value)
        }
        _ => "<expr>".to_string(),
    }
}

fn is_resource_pool_callee(callee: &Callee) -> bool {
    match callee {
        Callee::Name(name) => type_root_name(name) == "ResourcePool",
        Callee::Qualified { namespace, .. } => type_root_name(namespace) == "ResourcePool",
        Callee::ReceiverCall { .. } => false,
    }
}

fn is_capability_from_callee(callee: &Callee) -> bool {
    matches!(
        callee,
        Callee::Qualified { namespace, name }
            if type_root_name(namespace) == "Capability" && type_root_name(name) == "from"
    )
}

fn is_capability_protocol_call(
    callee: &Callee,
    args: &[CallArg],
    value_types: &HashMap<String, String>,
) -> bool {
    let Callee::Qualified { namespace, .. } = callee else {
        return false;
    };
    let Some(self_arg) = args
        .iter()
        .find(|arg| arg.name.as_deref() == Some("self"))
        .or_else(|| args.first())
    else {
        return false;
    };
    expr_ident_name(&self_arg.value)
        .and_then(|name| value_types.get(name))
        .and_then(|type_name| capability_protocol_name(type_name))
        .is_some_and(|protocol| protocol == namespace)
}

fn expr_ident_name(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name),
        Expr::Effect { value, .. } | Expr::Try { value, .. } => expr_ident_name(value),
        _ => None,
    }
}

fn type_ref_contains_name(ty: &TypeRef, name: &str) -> bool {
    ty.name == name || ty.args.iter().any(|arg| type_ref_contains_name(arg, name))
}

fn review_map_match_binding_type(
    pattern: &MatchPattern,
    value_type: Option<&str>,
) -> Option<(String, String)> {
    if let MatchPattern::Binding { name, .. } = pattern {
        return value_type.map(|ty| (name.clone(), ty.to_string()));
    }
    let MatchPattern::Variant {
        name,
        binding: Some(binding),
        ..
    } = pattern
    else {
        return None;
    };
    let value_type = value_type?;
    let args = type_arg_names(value_type)?;
    match name.as_str() {
        "Some" if type_root_name(value_type) == "Option" => args
            .first()
            .and_then(|ty| review_map_match_binding_type(binding, Some(ty))),
        "Ok" if type_root_name(value_type) == "Result" => args
            .first()
            .and_then(|ty| review_map_match_binding_type(binding, Some(ty))),
        "Err" if type_root_name(value_type) == "Result" => args
            .get(1)
            .and_then(|ty| review_map_match_binding_type(binding, Some(ty))),
        _ => None,
    }
}

fn type_ref_display_name(ty: &TypeRef) -> String {
    if ty.args.is_empty() {
        return ty.name.clone();
    }
    format!(
        "{}<{}>",
        ty.name,
        ty.args
            .iter()
            .map(type_ref_display_name)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn type_root_name(type_name: &str) -> &str {
    let type_name = type_name
        .trim()
        .strip_prefix("fresh ")
        .unwrap_or(type_name.trim());
    type_name
        .split_once('<')
        .map_or(type_name, |(root, _)| root)
}

fn type_arg_names(type_name: &str) -> Option<Vec<&str>> {
    let inner = type_name
        .split_once('<')
        .and_then(|(_, rest)| rest.strip_suffix('>'))?;
    Some(split_top_level_type_args(inner))
}

fn result_ok_type_name(type_name: &str) -> Option<String> {
    if type_root_name(type_name) != "Result" {
        return None;
    }
    type_arg_names(type_name).and_then(|args| args.first().map(|ty| (*ty).to_string()))
}

fn split_top_level_type_args(args: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    for (index, ch) in args.char_indices() {
        match ch {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(args[start..index].trim());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(args[start..].trim());
    parts
}

fn capability_protocol_name(type_name: &str) -> Option<&str> {
    if type_root_name(type_name) != "Capability" {
        return None;
    }
    type_arg_names(type_name).and_then(|args| args.first().copied())
}

fn review_map_line_count(
    function_name: &str,
    start_line: usize,
    function_lines: &[(String, usize)],
    total_lines: usize,
) -> usize {
    let next_line = function_lines
        .iter()
        .find(|(name, line)| name != function_name && *line > start_line)
        .map(|(_, line)| *line)
        .unwrap_or(total_lines + 1);
    next_line.saturating_sub(start_line).max(1)
}

fn review_map_classification_label(classification: ReviewMapClassification) -> &'static str {
    match classification {
        ReviewMapClassification::ReviewRequired => "must-review",
        ReviewMapClassification::Foldable => "low-semantic-risk",
        ReviewMapClassification::Unknown => "unknown",
    }
}

fn review_map_file_risk_label(risk: ReviewMapFileRisk) -> &'static str {
    match risk {
        ReviewMapFileRisk::Low => "low",
        ReviewMapFileRisk::Elevated => "elevated",
        ReviewMapFileRisk::High => "high",
    }
}

fn is_entry_function(name: &str) -> bool {
    matches!(name, "main" | "run")
        || name.starts_with("run_")
        || name.starts_with("handle_")
        || name.ends_with("_handler")
}

fn is_runtime_guarantee_boundary(effect: &str) -> bool {
    matches!(effect, "no_panic" | "noalloc" | "no_block" | "pure")
}

fn review_finding(
    code: &str,
    risk: ReviewRisk,
    summary: impl Into<String>,
    spans: Vec<ReviewSpan>,
    before: Option<String>,
    after: Option<String>,
) -> ReviewFinding {
    ReviewFinding {
        code: code.to_string(),
        risk,
        summary: summary.into(),
        spans,
        before,
        after,
        fixes: review_fixes(code),
    }
}

fn review_fixes(code: &str) -> Vec<ReviewFix> {
    let (kind, title) = match code {
        code::REVIEW_FEATURES_CHANGED => (
            "review_file_features",
            "Review whether this file should enable the changed advanced capabilities.",
        ),
        code::REVIEW_FUNCTION_REMOVED => (
            "restore_or_migrate_function",
            "Restore the function or migrate all call sites.",
        ),
        code::REVIEW_FUNCTION_ADDED => (
            "review_new_api",
            "Review the new API surface and ownership contract.",
        ),
        code::REVIEW_PARAMS_CHANGED => (
            "update_call_sites",
            "Update call sites for the changed parameter contract.",
        ),
        code::REVIEW_RETURN_CHANGED => (
            "review_return_contract",
            "Review callers that depend on the old return and freshness contract.",
        ),
        code::REVIEW_EFFECTS_CHANGED => (
            "review_effect_contract",
            "Review added or removed effects and update callers.",
        ),
        code::REVIEW_TYPE_REMOVED => (
            "restore_or_migrate_type",
            "Restore the type or migrate all consumers.",
        ),
        code::REVIEW_TYPE_ADDED => (
            "review_type_exposure",
            "Review the new type's ownership and resource fields.",
        ),
        code::REVIEW_TYPE_KIND_CHANGED => (
            "review_type_kind",
            "Review class, struct, or resource lifetime semantics for this type.",
        ),
        code::REVIEW_TYPE_FIELDS_CHANGED => (
            "review_type_layout",
            "Review field ownership, handle markers, and resource containment.",
        ),
        code::REVIEW_BOUNDARY_CHANGED => (
            "review_local_manage_boundary",
            "Review the changed local ownership and manage boundary.",
        ),
        code::REVIEW_UNSAFE_ADDED => (
            "review_unsafe_boundary",
            "Review the new unsafe boundary and require explicit justification.",
        ),
        code::REVIEW_NATIVE_ADDED => (
            "review_native_boundary",
            "Review the new native boundary and wrapper contract.",
        ),
        code::REVIEW_GUARANTEE_REMOVED => (
            "review_removed_guarantee",
            "Review callers that relied on the removed runtime guarantee.",
        ),
        code::REVIEW_FUNCTION_KIND_CHANGED => (
            "review_function_kind",
            "Review callers and scheduling assumptions for the changed function kind.",
        ),
        code::REVIEW_PROTOCOL_IMPL_CHANGED => (
            "review_protocol_impl",
            "Review the changed protocol implementation mapping and dispatch target.",
        ),
        code::REVIEW_SUM_TYPE_CHANGED => (
            "review_sum_type",
            "Review the changed sum type variants and update all match sites.",
        ),
        code::REVIEW_CONST_CHANGED => {
            ("review_const", "Review the changed constant value or type.")
        }
        code::REVIEW_TYPE_ALIAS_CHANGED => {
            ("review_type_alias", "Review the changed type alias target.")
        }
        _ => ("review_change", "Review this source-level contract change."),
    };
    vec![ReviewFix {
        kind: kind.to_string(),
        title: title.to_string(),
        applicability: "manual".to_string(),
    }]
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FunctionSig {
    name: String,
    is_async: bool,
    params: Vec<ParamSig>,
    return_type: Option<String>,
    returns_fresh: bool,
    effects: BTreeSet<String>,
    boundary: BoundarySig,
    span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParamSig {
    name: String,
    effect: Option<&'static str>,
    type_name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct BoundarySig {
    events: BTreeSet<BoundaryEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct BoundaryEvent {
    kind: BoundaryEventKind,
    subject: Option<String>,
    path: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum BoundaryEventKind {
    LocalBinding,
    Manage,
    Take,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TypeSig {
    name: String,
    kind: TypeKind,
    fields: BTreeMap<String, FieldSig>,
    span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FieldSig {
    type_name: String,
    is_handle: bool,
    is_weak: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProtocolImplSig {
    protocol: String,
    type_name: String,
    mappings: BTreeMap<String, String>,
    span: Span,
}

fn collect_type_sigs(items: &[Item]) -> BTreeMap<String, TypeSig> {
    items
        .iter()
        .filter_map(|item| match item {
            Item::Type(type_decl) => Some((type_decl.name.clone(), type_sig(type_decl))),
            Item::Function(_)
            | Item::Module(_)
            | Item::Use(_)
            | Item::SumType(_)
            | Item::TypeAlias(_)
            | Item::Const(_) => None,
        })
        .collect()
}

fn type_sig(type_decl: &TypeDecl) -> TypeSig {
    TypeSig {
        name: type_decl.name.clone(),
        kind: type_decl.kind,
        fields: type_decl
            .fields
            .iter()
            .map(|field| (field.name.clone(), field_sig(field)))
            .collect(),
        span: type_decl.span.clone(),
    }
}

fn field_sig(field: &FieldDecl) -> FieldSig {
    FieldSig {
        type_name: type_name(&field.ty),
        is_handle: field.is_handle,
        is_weak: field.is_weak,
    }
}

fn collect_protocol_impl_sigs(impls: &[ProtocolImpl]) -> BTreeMap<String, ProtocolImplSig> {
    impls
        .iter()
        .map(|protocol_impl| {
            let sig = protocol_impl_sig(protocol_impl);
            (protocol_impl_key(&sig.protocol, &sig.type_name), sig)
        })
        .collect()
}

fn protocol_impl_sig(protocol_impl: &ProtocolImpl) -> ProtocolImplSig {
    ProtocolImplSig {
        protocol: protocol_impl.protocol.clone(),
        type_name: protocol_impl.type_name.clone(),
        mappings: protocol_impl
            .mappings
            .iter()
            .map(|mapping| (mapping.method.clone(), mapping.target.clone()))
            .collect(),
        span: protocol_impl.span.clone(),
    }
}

fn protocol_impl_key(protocol: &str, type_name: &str) -> String {
    format!("{protocol} for {type_name}")
}

fn collect_function_sigs(items: &[Item]) -> BTreeMap<String, FunctionSig> {
    items
        .iter()
        .filter_map(|item| match item {
            Item::Function(function) => Some((function.name.clone(), function_sig(function))),
            Item::Type(_)
            | Item::Module(_)
            | Item::Use(_)
            | Item::SumType(_)
            | Item::TypeAlias(_)
            | Item::Const(_) => None,
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SumTypeSig {
    variants: Vec<String>,
    span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConstSig {
    type_name: String,
    span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TypeAliasSig {
    target: String,
    span: Span,
}

fn collect_sum_type_sigs(items: &[Item]) -> BTreeMap<String, SumTypeSig> {
    items
        .iter()
        .filter_map(|item| match item {
            Item::SumType(sum) => Some((
                sum.name.clone(),
                SumTypeSig {
                    variants: sum_variant_contracts(sum),
                    span: sum.span.clone(),
                },
            )),
            _ => None,
        })
        .collect()
}

fn sum_variant_contracts(sum: &crate::syntax::ast::SumTypeDecl) -> Vec<String> {
    sum.variants
        .iter()
        .map(|variant| {
            if variant.fields.is_empty() {
                variant.name.clone()
            } else {
                format!(
                    "{}({})",
                    variant.name,
                    variant
                        .fields
                        .iter()
                        .map(field_contract)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        })
        .collect()
}

fn field_contract(field: &FieldDecl) -> String {
    let type_name = type_name(&field.ty);
    if field.is_weak {
        format!("{}: weak {type_name}", field.name)
    } else if field.is_handle {
        format!("{}: handle {type_name}", field.name)
    } else {
        format!("{}: {type_name}", field.name)
    }
}

fn collect_const_sigs(items: &[Item]) -> BTreeMap<String, ConstSig> {
    items
        .iter()
        .filter_map(|item| match item {
            Item::Const(decl) => Some((
                decl.name.clone(),
                ConstSig {
                    type_name: decl
                        .type_annotation
                        .as_ref()
                        .map(type_name)
                        .unwrap_or_else(|| "inferred".to_string()),
                    span: decl.span.clone(),
                },
            )),
            _ => None,
        })
        .collect()
}

fn collect_type_alias_sigs(items: &[Item]) -> BTreeMap<String, TypeAliasSig> {
    items
        .iter()
        .filter_map(|item| match item {
            Item::TypeAlias(decl) => Some((
                decl.name.clone(),
                TypeAliasSig {
                    target: type_name(&decl.target),
                    span: decl.span.clone(),
                },
            )),
            _ => None,
        })
        .collect()
}

fn function_sig(function: &FunctionDecl) -> FunctionSig {
    FunctionSig {
        name: function.name.clone(),
        is_async: function.is_async,
        params: function.params.iter().map(param_sig).collect(),
        return_type: function.return_ty.as_ref().map(type_name),
        returns_fresh: function.returns_fresh,
        effects: function.effects.iter().map(effect_name).collect(),
        boundary: boundary_sig(&function.body),
        span: function.span.clone(),
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
    if old.is_async != new.is_async {
        findings.push(review_finding(
            code::REVIEW_FUNCTION_KIND_CHANGED,
            ReviewRisk::Api,
            format!("function `{}` kind changed.", old.name),
            paired_spans(
                &old.span,
                &new.span,
                "old function kind",
                "new function kind",
            ),
            Some(function_kind_contract(old)),
            Some(function_kind_contract(new)),
        ));
    }
    if old.params != new.params {
        findings.push(review_finding(
            code::REVIEW_PARAMS_CHANGED,
            ReviewRisk::Api,
            format!("function `{}` parameters changed.", old.name),
            paired_spans(&old.span, &new.span, "old function", "new function"),
            Some(params_contract(&old.params)),
            Some(params_contract(&new.params)),
        ));
    }
    if old.return_type != new.return_type || old.returns_fresh != new.returns_fresh {
        findings.push(review_finding(
            code::REVIEW_RETURN_CHANGED,
            ReviewRisk::Api,
            format!("function `{}` return contract changed.", old.name),
            paired_spans(&old.span, &new.span, "old function", "new function"),
            Some(return_contract(old)),
            Some(return_contract(new)),
        ));
    }
    if old.effects != new.effects {
        findings.push(review_finding(
            code::REVIEW_EFFECTS_CHANGED,
            ReviewRisk::Effect,
            format!("function `{}` effects changed.", old.name),
            paired_spans(&old.span, &new.span, "old function", "new function"),
            Some(effects_contract(&old.effects)),
            Some(effects_contract(&new.effects)),
        ));
    }
    if !has_effect(&old.effects, "unsafe") && has_effect(&new.effects, "unsafe") {
        findings.push(review_finding(
            code::REVIEW_UNSAFE_ADDED,
            ReviewRisk::Unsafe,
            format!("function `{}` added unsafe boundary.", old.name),
            paired_spans(&old.span, &new.span, "old function", "new unsafe boundary"),
            Some(if has_effect(&old.effects, "unsafe") {
                "unsafe".to_string()
            } else {
                "<none>".to_string()
            }),
            Some("unsafe".to_string()),
        ));
    }
    if !has_effect(&old.effects, "native") && has_effect(&new.effects, "native") {
        findings.push(review_finding(
            code::REVIEW_NATIVE_ADDED,
            ReviewRisk::Boundary,
            format!("function `{}` added native boundary.", old.name),
            paired_spans(&old.span, &new.span, "old function", "new native boundary"),
            Some(if has_effect(&old.effects, "native") {
                "native".to_string()
            } else {
                "<none>".to_string()
            }),
            Some("native".to_string()),
        ));
    }
    if !has_effect(&old.effects, "parallel") && has_effect(&new.effects, "parallel") {
        findings.push(review_finding(
            code::REVIEW_PARALLEL_ADDED,
            ReviewRisk::Boundary,
            format!("function `{}` added parallel boundary.", old.name),
            paired_spans(
                &old.span,
                &new.span,
                "old function",
                "new parallel boundary",
            ),
            Some(if has_effect(&old.effects, "parallel") {
                "parallel".to_string()
            } else {
                "<none>".to_string()
            }),
            Some("parallel".to_string()),
        ));
    }
    let old_guarantees = guarantee_effects(&old.effects);
    let new_guarantees = guarantee_effects(&new.effects);
    let removed_guarantees: BTreeSet<_> = old_guarantees
        .difference(&new_guarantees)
        .cloned()
        .collect();
    if !removed_guarantees.is_empty() {
        findings.push(review_finding(
            code::REVIEW_GUARANTEE_REMOVED,
            ReviewRisk::Guarantee,
            format!(
                "function `{}` removed guarantee(s): {}.",
                old.name,
                effects_contract(&removed_guarantees)
            ),
            paired_spans(&old.span, &new.span, "old guarantees", "new guarantees"),
            Some(effects_contract(&old_guarantees)),
            Some(effects_contract(&new_guarantees)),
        ));
    }
    if old.boundary != new.boundary {
        findings.push(review_finding(
            code::REVIEW_BOUNDARY_CHANGED,
            ReviewRisk::Boundary,
            format!(
                "function `{}` local/manage boundary changed: {}.",
                old.name,
                boundary_change_summary(&old.boundary, &new.boundary)
            ),
            paired_spans(&old.span, &new.span, "old function", "new function"),
            Some(boundary_contract(&old.boundary)),
            Some(boundary_contract(&new.boundary)),
        ));
    }
}

fn compare_type(old: &TypeSig, new: &TypeSig, findings: &mut Vec<ReviewFinding>) {
    if old.kind != new.kind {
        findings.push(review_finding(
            code::REVIEW_TYPE_KIND_CHANGED,
            ReviewRisk::TypeLayout,
            format!(
                "type `{}` kind changed from {} to {}.",
                old.name,
                type_kind_label(old.kind),
                type_kind_label(new.kind)
            ),
            paired_spans(&old.span, &new.span, "old type", "new type"),
            Some(type_kind_label(old.kind).to_string()),
            Some(type_kind_label(new.kind).to_string()),
        ));
    }
    if old.fields != new.fields {
        findings.push(review_finding(
            code::REVIEW_TYPE_FIELDS_CHANGED,
            ReviewRisk::TypeLayout,
            format!("type `{}` field layout changed.", old.name),
            paired_spans(&old.span, &new.span, "old type", "new type"),
            Some(fields_contract(&old.fields)),
            Some(fields_contract(&new.fields)),
        ));
    }
}

fn feature_label(features: &[FileFeature]) -> String {
    let labels = feature_names(features);
    if labels.is_empty() {
        return "<none>".to_string();
    }
    labels.join(",")
}

fn feature_names(features: &[FileFeature]) -> Vec<String> {
    let mut labels = features
        .iter()
        .map(feature_name)
        .map(str::to_string)
        .collect::<Vec<_>>();
    labels.sort_unstable();
    labels.dedup();
    labels
}

fn feature_name(feature: &FileFeature) -> &'static str {
    match feature {
        FileFeature::Local => "local",
        FileFeature::Native => "native",
        FileFeature::Unsafe => "unsafe",
        FileFeature::Async => "async",
        FileFeature::Device => "device",
        FileFeature::Ffi => "ffi",
        FileFeature::Reflection => "reflection",
    }
}

fn review_map_file_risk(features: &[FileFeature]) -> ReviewMapFileRisk {
    if features.iter().any(|feature| {
        matches!(
            feature,
            FileFeature::Native | FileFeature::Unsafe | FileFeature::Device | FileFeature::Ffi
        )
    }) {
        ReviewMapFileRisk::High
    } else if features.iter().any(|feature| {
        matches!(
            feature,
            FileFeature::Local | FileFeature::Async | FileFeature::Reflection
        )
    }) {
        ReviewMapFileRisk::Elevated
    } else {
        ReviewMapFileRisk::Low
    }
}

fn review_map_file_reasons(features: &[FileFeature]) -> Vec<String> {
    feature_names(features)
        .into_iter()
        .filter_map(|feature| review_map_feature_reason(&feature).map(str::to_string))
        .collect()
}

fn review_map_feature_reason(feature: &str) -> Option<&'static str> {
    match feature {
        "local" => Some("local capability enabled"),
        "native" => Some("native boundary capability enabled"),
        "unsafe" => Some("unsafe capability enabled"),
        "async" => Some("async control-flow capability enabled"),
        "device" => Some("reserved device review marker enabled"),
        "ffi" => Some("reserved ffi review marker enabled"),
        "reflection" => Some("reserved reflection review marker enabled"),
        _ => None,
    }
}

fn review_span(span: &Span, label: &str) -> ReviewSpan {
    ReviewSpan {
        file: span.file.clone(),
        line: span.line,
        column: span.column,
        length: span.length,
        label: label.to_string(),
    }
}

fn paired_spans(old: &Span, new: &Span, old_label: &str, new_label: &str) -> Vec<ReviewSpan> {
    vec![review_span(old, old_label), review_span(new, new_label)]
}

fn type_kind_label(kind: TypeKind) -> &'static str {
    match kind {
        TypeKind::Class => "class",
        TypeKind::Struct => "struct",
        TypeKind::Resource => "resource",
    }
}

fn effect_label(effect: DataEffect) -> &'static str {
    match effect {
        DataEffect::Read => "read",
        DataEffect::Mut => "mut",
        DataEffect::Take => "take",
    }
}

fn function_contract(function: &FunctionSig) -> String {
    let mut contract = format!(
        "{} {}({})",
        function_kind_contract(function),
        function.name,
        params_contract(&function.params)
    );
    if let Some(return_type) = &function.return_type {
        if function.returns_fresh {
            contract.push_str(&format!(" -> fresh {return_type}"));
        } else {
            contract.push_str(&format!(" -> {return_type}"));
        }
    }
    if !function.effects.is_empty() {
        contract.push_str(&format!(
            " effects({})",
            effects_contract(&function.effects)
        ));
    }
    contract
}

fn function_kind_contract(function: &FunctionSig) -> String {
    if function.is_async {
        "async fn".to_string()
    } else {
        "fn".to_string()
    }
}

fn params_contract(params: &[ParamSig]) -> String {
    params
        .iter()
        .map(|param| match param.effect {
            Some(effect) => format!("{}: {} {}", param.name, effect, param.type_name),
            None => format!("{}: {}", param.name, param.type_name),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn return_contract(function: &FunctionSig) -> String {
    match (&function.return_type, function.returns_fresh) {
        (Some(return_type), true) => format!("fresh {return_type}"),
        (Some(return_type), false) => return_type.clone(),
        (None, _) => "<missing>".to_string(),
    }
}

fn effects_contract(effects: &BTreeSet<String>) -> String {
    if effects.is_empty() {
        return "<none>".to_string();
    }
    effects.iter().cloned().collect::<Vec<_>>().join(", ")
}

fn has_effect(effects: &BTreeSet<String>, effect: &str) -> bool {
    effects.contains(effect)
}

fn guarantee_effects(effects: &BTreeSet<String>) -> BTreeSet<String> {
    effects
        .iter()
        .filter(|effect| {
            matches!(
                effect.as_str(),
                "no_panic" | "noalloc" | "no_block" | "pure"
            )
        })
        .cloned()
        .collect()
}

fn type_contract(ty: &TypeSig) -> String {
    format!(
        "{} {} {{ {} }}",
        type_kind_label(ty.kind),
        ty.name,
        fields_contract(&ty.fields)
    )
}

fn protocol_impl_contract(protocol_impl: &ProtocolImplSig) -> String {
    let mappings = protocol_impl
        .mappings
        .iter()
        .map(|(method, target)| format!("{method} = {target}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "impl {} for {} {{ {} }}",
        protocol_impl.protocol, protocol_impl.type_name, mappings
    )
}

fn fields_contract(fields: &BTreeMap<String, FieldSig>) -> String {
    if fields.is_empty() {
        return "<none>".to_string();
    }
    fields
        .iter()
        .map(|(name, field)| {
            if field.is_weak {
                format!("{name}: weak {}", field.type_name)
            } else if field.is_handle {
                format!("{name}: handle {}", field.type_name)
            } else {
                format!("{name}: {}", field.type_name)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn boundary_contract(boundary: &BoundarySig) -> String {
    if boundary.events.is_empty() {
        return "<none>".to_string();
    }
    boundary
        .events
        .iter()
        .map(boundary_event_label)
        .collect::<Vec<_>>()
        .join("; ")
}

fn type_name(ty: &TypeRef) -> String {
    let name = if ty.name == "Fn" {
        let params = ty
            .fn_params
            .iter()
            .map(type_name)
            .collect::<Vec<_>>()
            .join(", ");
        let return_ty = ty
            .fn_return
            .as_ref()
            .map(|return_ty| format!(" -> {}", type_name(return_ty)))
            .unwrap_or_default();
        format!("Fn({params}){return_ty}")
    } else if ty.args.is_empty() {
        ty.name.clone()
    } else {
        let args = ty.args.iter().map(type_name).collect::<Vec<_>>().join(", ");
        format!("{}<{args}>", ty.name)
    };
    if ty.is_noescape {
        format!("noescape {name}")
    } else if ty.is_owned {
        format!("owned {name}")
    } else {
        name
    }
}

fn effect_name(effect: &EffectDecl) -> String {
    match effect {
        EffectDecl::Name(name) => name.clone(),
        EffectDecl::Retains(param) => format!("retains({param})"),
    }
}

fn boundary_sig(block: &Block) -> BoundarySig {
    let mut boundary = BoundarySig::default();
    collect_boundary_block(block, "body", &mut boundary);
    boundary
}

fn collect_boundary_block(block: &Block, path: &str, boundary: &mut BoundarySig) {
    for (index, statement) in block.statements.iter().enumerate() {
        collect_boundary_stmt(statement, &format!("{path}[{}]", index + 1), boundary);
    }
}

fn collect_boundary_stmt(statement: &Stmt, path: &str, boundary: &mut BoundarySig) {
    match statement {
        Stmt::Let(stmt) => {
            if stmt.kind == LetKind::Local {
                push_boundary_event(
                    boundary,
                    BoundaryEventKind::LocalBinding,
                    Some(stmt.name.clone()),
                    path,
                );
            }
            if let Some(value) = &stmt.value {
                collect_boundary_expr(value, &format!("{path}.value"), boundary);
            }
        }
        Stmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_boundary_expr(value, &format!("{path}.return"), boundary);
            }
        }
        Stmt::With(stmt) => {
            collect_boundary_expr(&stmt.resource, &format!("{path}.resource"), boundary);
            collect_boundary_block(&stmt.body, &format!("{path}.body"), boundary);
        }
        Stmt::If(stmt) => {
            collect_boundary_expr(&stmt.condition, &format!("{path}.condition"), boundary);
            collect_boundary_block(&stmt.then_body, &format!("{path}.then"), boundary);
            if let Some(else_body) = &stmt.else_body {
                collect_boundary_block(else_body, &format!("{path}.else"), boundary);
            }
        }
        Stmt::Loop(stmt) => {
            if let Some(condition) = &stmt.condition {
                collect_boundary_expr(condition, &format!("{path}.condition"), boundary);
            }
            collect_boundary_block(&stmt.body, &format!("{path}.loop"), boundary);
        }
        Stmt::For(stmt) => {
            collect_boundary_expr(&stmt.iterable, &format!("{path}.iterable"), boundary);
            collect_boundary_block(&stmt.body, &format!("{path}.for"), boundary);
        }
        Stmt::TaskGroup(stmt) => {
            collect_boundary_block(&stmt.body, &format!("{path}.task_group"), boundary);
        }
        Stmt::Select(stmt) => {
            for (index, arm) in stmt.arms.iter().enumerate() {
                collect_boundary_expr(
                    &arm.operation,
                    &format!("{path}.select.arm{}", index + 1),
                    boundary,
                );
                collect_boundary_block(
                    &arm.body,
                    &format!("{path}.select.arm{}", index + 1),
                    boundary,
                );
            }
        }
        Stmt::Match(stmt) => {
            collect_boundary_expr(&stmt.value, &format!("{path}.match"), boundary);
            for (index, arm) in stmt.arms.iter().enumerate() {
                collect_boundary_block(&arm.body, &format!("{path}.arm{}", index + 1), boundary);
            }
        }
        Stmt::LetElse(stmt) => {
            collect_boundary_expr(&stmt.value, &format!("{path}.let_else"), boundary);
            collect_boundary_block(&stmt.else_body, &format!("{path}.else"), boundary);
        }
        Stmt::Assign(stmt) => {
            collect_boundary_expr(&stmt.target, &format!("{path}.target"), boundary);
            collect_boundary_expr(&stmt.value, &format!("{path}.value"), boundary);
        }
        Stmt::Expr(expr) => collect_boundary_expr(expr, path, boundary),
        Stmt::Break(_)
        | Stmt::Continue(_)
        | Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Unknown(_) => {}
    }
}

fn collect_boundary_expr(expr: &Expr, path: &str, boundary: &mut BoundarySig) {
    match expr {
        Expr::Effect {
            effect: DataEffect::Take,
            value,
            ..
        } => {
            push_boundary_event(
                boundary,
                BoundaryEventKind::Take,
                boundary_expr_subject(value),
                path,
            );
            collect_boundary_expr(value, &format!("{path}.take"), boundary);
        }
        Expr::Effect { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => {
            collect_boundary_expr(value, path, boundary);
        }
        Expr::Manage { value, .. } => {
            push_boundary_event(
                boundary,
                BoundaryEventKind::Manage,
                boundary_expr_subject(value),
                path,
            );
            collect_boundary_expr(value, &format!("{path}.manage"), boundary);
        }
        Expr::Call { args, .. } => {
            for (index, CallArg { name, value, .. }) in args.iter().enumerate() {
                let arg_path = name.as_ref().map_or_else(
                    || format!("{path}.arg{}", index + 1),
                    |name| format!("{path}.arg({name})"),
                );
                collect_boundary_expr(value, &arg_path, boundary);
            }
        }
        Expr::Binary { left, right, .. } => {
            collect_boundary_expr(left, &format!("{path}.left"), boundary);
            collect_boundary_expr(right, &format!("{path}.right"), boundary);
        }
        Expr::Field { base, .. } => collect_boundary_expr(base, path, boundary),
        Expr::Index { base, index, .. } => {
            collect_boundary_expr(base, path, boundary);
            collect_boundary_expr(index, path, boundary);
        }
        Expr::Closure { body, .. } => {
            collect_boundary_block(body, &format!("{path}.closure"), boundary)
        }
        Expr::Match { value, arms, .. } => {
            collect_boundary_expr(value, &format!("{path}.match"), boundary);
            for (index, arm) in arms.iter().enumerate() {
                collect_boundary_block(&arm.body, &format!("{path}.arm{}", index + 1), boundary);
            }
        }
        Expr::MapLiteral { entries, .. } => {
            for (index, entry) in entries.iter().enumerate() {
                let entry_path = format!("{path}.entry{}", index + 1);
                collect_boundary_expr(&entry.key, &format!("{entry_path}.key"), boundary);
                collect_boundary_expr(&entry.value, &format!("{entry_path}.value"), boundary);
            }
        }
        Expr::ObjectLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Ident(_, _)
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => {}
    }
}

fn push_boundary_event(
    boundary: &mut BoundarySig,
    kind: BoundaryEventKind,
    subject: Option<String>,
    path: &str,
) {
    boundary.events.insert(BoundaryEvent {
        kind,
        subject,
        path: path.to_string(),
    });
}

fn boundary_expr_subject(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(name, _) => Some(name.clone()),
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => boundary_expr_subject(value),
        Expr::Field { name, .. } => Some(format!(".{name}")),
        Expr::Index { .. } => None,
        Expr::ObjectLiteral { .. } | Expr::MapLiteral { .. } | Expr::ArrayLiteral { .. } => None,
        Expr::Call { .. }
        | Expr::Binary { .. }
        | Expr::Closure { .. }
        | Expr::Match { .. }
        | Expr::Number(_, _)
        | Expr::String(_, _)
        | Expr::MultilineString(_, _)
        | Expr::Unknown(_) => None,
    }
}

fn boundary_change_summary(old: &BoundarySig, new: &BoundarySig) -> String {
    let added = new
        .events
        .difference(&old.events)
        .map(|event| format!("added {}", boundary_event_label(event)))
        .collect::<Vec<_>>();
    let removed = old
        .events
        .difference(&new.events)
        .map(|event| format!("removed {}", boundary_event_label(event)))
        .collect::<Vec<_>>();

    let mut parts = added;
    parts.extend(removed);
    if parts.is_empty() {
        "boundary event paths changed".to_string()
    } else {
        parts.join("; ")
    }
}

fn boundary_event_label(event: &BoundaryEvent) -> String {
    let subject = event
        .subject
        .as_ref()
        .map_or(String::new(), |subject| format!(" `{subject}`"));
    format!(
        "{}{} at {}",
        boundary_event_kind_label(event.kind),
        subject,
        event.path
    )
}

fn boundary_event_kind_label(kind: BoundaryEventKind) -> &'static str {
    match kind {
        BoundaryEventKind::LocalBinding => "local binding",
        BoundaryEventKind::Manage => "manage",
        BoundaryEventKind::Take => "take",
    }
}
