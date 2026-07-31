//! Behavior Bill of Materials (BBOM)
//!
//! Produces a structured manifest of what a program *does* — not what it depends on.
//! This is the behavioral counterpart to SBOM: instead of listing dependencies,
//! it lists capabilities, mutations, retentions, resource access, native boundaries,
//! and unknown surface area.

use serde::Serialize;

use crate::review::{ReviewMapClassification, review_map_sources_with_interfaces};
use crate::syntax::ast::{
    Block, DataEffect, EffectDecl, Expr, FunctionDecl, Item, Program, Stmt, TypeRef,
};
use crate::syntax::parse_source;

#[derive(Debug, Clone, Serialize)]
pub struct BehaviorBom {
    pub version: &'static str,
    pub files: Vec<String>,
    pub summary: BomSummary,
    pub mutations: Vec<BomMutation>,
    pub retentions: Vec<BomRetention>,
    pub resources: Vec<BomResource>,
    pub native_boundaries: Vec<BomNativeBoundary>,
    pub capabilities: Vec<BomCapability>,
    pub unknown: BomUnknown,
}

#[derive(Debug, Clone, Serialize)]
pub struct BomSummary {
    pub total_functions: usize,
    pub total_lines: usize,
    pub review_required_functions: usize,
    pub foldable_functions: usize,
    pub unknown_functions: usize,
    pub unknown_ratio: f64,
    pub review_ratio: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct BomMutation {
    pub target: String,
    pub function: String,
    pub kind: BomMutationKind,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BomMutationKind {
    MutParameter,
    TakeParameter,
    HandleFieldWrite,
    ManagedStateWrite,
    LocalReassignment,
}

#[derive(Debug, Clone, Serialize)]
pub struct BomRetention {
    pub parameter: String,
    pub function: String,
    pub kind: BomRetentionKind,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BomRetentionKind {
    Retains,
    ManagedClosureCapture,
    SpawnCapture,
}

#[derive(Debug, Clone, Serialize)]
pub struct BomResource {
    pub kind: String,
    pub function: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BomNativeBoundary {
    pub call: String,
    pub function: String,
    pub kind: BomBoundaryKind,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BomBoundaryKind {
    Native,
    Unsafe,
    Parallel,
}

#[derive(Debug, Clone, Serialize)]
pub struct BomCapability {
    pub name: String,
    pub function: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BomUnknown {
    pub functions: Vec<BomUnknownFunction>,
    pub total_lines: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct BomUnknownFunction {
    pub name: String,
    pub line: usize,
    pub unresolved_calls: Vec<String>,
}

/// Generate a Behavior BOM from source files.
pub fn behavior_bom_sources(sources: Vec<(&str, &str)>) -> BehaviorBom {
    behavior_bom_sources_with_interfaces(sources, &[])
}

/// Generate a Behavior BOM from source files with interface declarations.
pub fn behavior_bom_sources_with_interfaces(
    sources: Vec<(&str, &str)>,
    interfaces: &[(&str, &str)],
) -> BehaviorBom {
    let review_map = review_map_sources_with_interfaces(
        sources.iter().map(|(f, s)| (*f, *s)).collect(),
        interfaces,
    );

    let programs: Vec<(&str, Program)> = sources
        .iter()
        .map(|(file, source)| (*file, parse_source(file, source)))
        .collect();

    let mut mutations = Vec::new();
    let mut retentions = Vec::new();
    let mut resources = Vec::new();
    let mut native_boundaries = Vec::new();
    let mut capabilities = Vec::new();

    for (_file, program) in &programs {
        for item in &program.items {
            if let Item::Function(function) = item {
                collect_function_behaviors(
                    function,
                    &mut mutations,
                    &mut retentions,
                    &mut resources,
                    &mut native_boundaries,
                    &mut capabilities,
                );
            }
        }
    }

    let mut unknown_functions = Vec::new();
    let mut unknown_lines = 0usize;
    for file in &review_map.files {
        for region in &file.regions {
            if region.classification == ReviewMapClassification::Unknown {
                let unresolved: Vec<String> = region
                    .reasons
                    .iter()
                    .filter(|r| r.starts_with("unresolved call"))
                    .map(|r| r.trim_start_matches("unresolved call(s): ").to_string())
                    .collect();
                unknown_functions.push(BomUnknownFunction {
                    name: region.function.clone(),
                    line: region.line,
                    unresolved_calls: unresolved,
                });
                unknown_lines += region.line_count;
            }
        }
    }

    let total_functions = review_map.summary.total_functions;
    let total_lines = review_map.summary.total_lines;

    BehaviorBom {
        version: "0.1",
        files: sources.iter().map(|(f, _)| f.to_string()).collect(),
        summary: BomSummary {
            total_functions,
            total_lines,
            review_required_functions: review_map.summary.review_required.functions,
            foldable_functions: review_map.summary.foldable.functions,
            unknown_functions: review_map.summary.unknown.functions,
            unknown_ratio: if total_lines > 0 {
                unknown_lines as f64 / total_lines as f64
            } else {
                0.0
            },
            review_ratio: if total_lines > 0 {
                review_map.summary.must_review_lines as f64 / total_lines as f64
            } else {
                0.0
            },
        },
        mutations,
        retentions,
        resources,
        native_boundaries,
        capabilities,
        unknown: BomUnknown {
            functions: unknown_functions,
            total_lines: unknown_lines,
        },
    }
}

fn collect_assignment_mutations(block: &Block, function: &str, mutations: &mut Vec<BomMutation>) {
    for statement in &block.statements {
        match statement {
            Stmt::Assign(stmt) => {
                if let Some(root) = assign_target_root(&stmt.target) {
                    mutations.push(BomMutation {
                        target: root.to_string(),
                        function: function.to_string(),
                        kind: BomMutationKind::LocalReassignment,
                    });
                }
            }
            Stmt::With(stmt) => collect_assignment_mutations(&stmt.body, function, mutations),
            Stmt::If(stmt) => {
                collect_assignment_mutations(&stmt.then_body, function, mutations);
                if let Some(else_body) = &stmt.else_body {
                    collect_assignment_mutations(else_body, function, mutations);
                }
            }
            Stmt::Loop(stmt) => collect_assignment_mutations(&stmt.body, function, mutations),
            Stmt::For(stmt) => collect_assignment_mutations(&stmt.body, function, mutations),
            Stmt::Match(stmt) => {
                for arm in &stmt.arms {
                    collect_assignment_mutations(&arm.body, function, mutations);
                }
            }
            Stmt::TaskGroup(stmt) => collect_assignment_mutations(&stmt.body, function, mutations),
            Stmt::LetElse(stmt) => {
                collect_assignment_mutations(&stmt.else_body, function, mutations)
            }
            _ => {}
        }
    }
}

fn assign_target_root(expr: &Expr) -> Option<&str> {
    match expr {
        Expr::Ident(name, _) => Some(name.as_str()),
        Expr::Field { base, .. } | Expr::Index { base, .. } => assign_target_root(base),
        _ => None,
    }
}

fn collect_function_behaviors(
    function: &FunctionDecl,
    mutations: &mut Vec<BomMutation>,
    retentions: &mut Vec<BomRetention>,
    resources: &mut Vec<BomResource>,
    native_boundaries: &mut Vec<BomNativeBoundary>,
    capabilities: &mut Vec<BomCapability>,
) {
    let fname = &function.name;

    // Mutations from parameters
    for param in &function.params {
        match param.effect {
            Some(DataEffect::Mut) => mutations.push(BomMutation {
                target: param.name.clone(),
                function: fname.clone(),
                kind: BomMutationKind::MutParameter,
            }),
            Some(DataEffect::Take) => mutations.push(BomMutation {
                target: param.name.clone(),
                function: fname.clone(),
                kind: BomMutationKind::TakeParameter,
            }),
            _ => {}
        }
    }

    // Body-level controlled assignments (`x = e` on a `let mut` local) are
    // review-visible mutations alongside parameter effects.
    collect_assignment_mutations(&function.body, fname, mutations);

    // Effects declarations
    for effect in &function.effects {
        match effect {
            EffectDecl::Retains(param) => retentions.push(BomRetention {
                parameter: param.clone(),
                function: fname.clone(),
                kind: BomRetentionKind::Retains,
            }),
            EffectDecl::Name(name) => match name.as_str() {
                "native" => native_boundaries.push(BomNativeBoundary {
                    call: fname.clone(),
                    function: fname.clone(),
                    kind: BomBoundaryKind::Native,
                }),
                "unsafe" => native_boundaries.push(BomNativeBoundary {
                    call: fname.clone(),
                    function: fname.clone(),
                    kind: BomBoundaryKind::Unsafe,
                }),
                "parallel" => native_boundaries.push(BomNativeBoundary {
                    call: fname.clone(),
                    function: fname.clone(),
                    kind: BomBoundaryKind::Parallel,
                }),
                other => capabilities.push(BomCapability {
                    name: other.to_string(),
                    function: fname.clone(),
                }),
            },
        }
    }

    // Resource usage from parameter types
    for param in &function.params {
        if type_ref_is_resource_like(&param.ty) {
            resources.push(BomResource {
                kind: type_ref_name_string(&param.ty),
                function: fname.clone(),
            });
        }
    }

    // Async boundary
    if function.is_async {
        capabilities.push(BomCapability {
            name: "async".to_string(),
            function: fname.clone(),
        });
    }
}

fn type_ref_is_resource_like(ty: &TypeRef) -> bool {
    matches!(
        ty.name.as_str(),
        "File" | "Directory" | "HttpClient" | "TcpStream" | "UdpSocket" | "Process"
    )
}

fn type_ref_name_string(ty: &TypeRef) -> String {
    ty.name.clone()
}

/// Format BBOM as human-readable text.
pub fn format_bbom_human(bom: &BehaviorBom) -> String {
    let mut out = String::new();

    out.push_str("╔══════════════════════════════════════╗\n");
    out.push_str("║   BEHAVIOR BILL OF MATERIALS (BBOM) ║\n");
    out.push_str("╚══════════════════════════════════════╝\n\n");

    out.push_str(&format!("files: {}\n", bom.files.join(", ")));
    out.push_str(&format!(
        "functions: {} total ({} review-required, {} foldable, {} unknown)\n",
        bom.summary.total_functions,
        bom.summary.review_required_functions,
        bom.summary.foldable_functions,
        bom.summary.unknown_functions
    ));
    out.push_str(&format!(
        "unknown ratio: {:.1}%\n",
        bom.summary.unknown_ratio * 100.0
    ));
    out.push_str(&format!(
        "review ratio: {:.1}%\n\n",
        bom.summary.review_ratio * 100.0
    ));

    if !bom.mutations.is_empty() {
        out.push_str("mutations:\n");
        for m in &bom.mutations {
            out.push_str(&format!(
                "  {:?} {} (in {})\n",
                m.kind, m.target, m.function
            ));
        }
        out.push('\n');
    }

    if !bom.retentions.is_empty() {
        out.push_str("retentions:\n");
        for r in &bom.retentions {
            out.push_str(&format!(
                "  {:?} {} (in {})\n",
                r.kind, r.parameter, r.function
            ));
        }
        out.push('\n');
    }

    if !bom.resources.is_empty() {
        out.push_str("resources:\n");
        for r in &bom.resources {
            out.push_str(&format!("  {} (in {})\n", r.kind, r.function));
        }
        out.push('\n');
    }

    if !bom.native_boundaries.is_empty() {
        out.push_str("native boundaries:\n");
        for b in &bom.native_boundaries {
            out.push_str(&format!("  {:?} {} (in {})\n", b.kind, b.call, b.function));
        }
        out.push('\n');
    }

    if !bom.capabilities.is_empty() {
        out.push_str("capabilities:\n");
        for c in &bom.capabilities {
            out.push_str(&format!("  {} (in {})\n", c.name, c.function));
        }
        out.push('\n');
    }

    if !bom.unknown.functions.is_empty() {
        out.push_str("unknown:\n");
        out.push_str(&format!(
            "  {} functions, {} lines\n",
            bom.unknown.functions.len(),
            bom.unknown.total_lines
        ));
        for f in &bom.unknown.functions {
            out.push_str(&format!("  {} (line {})", f.name, f.line));
            if !f.unresolved_calls.is_empty() {
                out.push_str(&format!(" — unresolved: {}", f.unresolved_calls.join(", ")));
            }
            out.push('\n');
        }
        out.push('\n');
    }

    out
}

/// Format BBOM as JSON.
pub fn format_bbom_json(bom: &BehaviorBom) -> String {
    serde_json::to_string_pretty(bom).expect("BBOM JSON serialization should not fail")
}
