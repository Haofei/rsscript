use crate::text_util::{split_top_level_type_args, type_arg_names, type_root_name};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::checks;
use crate::checks::budget::{
    AnalysisDiagnostics, FrontendBudget, FrontendBudgetLimits, budget_completion,
};
use crate::diagnostic::{Diagnostic, Span, code};
use crate::hir::{
    CallResolution, DuplicateSymbolKind, FieldInfo, FunctionSig, Hir, HirBlock, HirExpr,
    HirMatchArm, HirStmt, HirTypeKind, ParamSig,
};
use crate::interfaces::CORE_INTERFACES;
use crate::lexer::{Token, lex_with_budget};
use crate::semantic::{
    AnalysisResult, ResolvedType, SemanticDatabase, SourceSnapshot, ValidatedProgram,
};
use crate::syntax::ast::{
    AssignStmt, Block, Callee, DataEffect, Expr, FieldDecl, FunctionDecl, GenericBound,
    GenericParam, Item, MatchPattern, Stmt, TypeKind, TypeRef,
};
use crate::syntax::ast::{Program, merge_programs};

mod assign;
mod derives;
mod diagnostics;
mod exhaustiveness;
mod resource_types;
mod syntax_support;
mod task_group;
mod unknowns;
use assign::AssignChecker;
use task_group::{
    collect_direct_task_group_awaited_handles, collect_nested_task_group_async_lets,
    collect_task_group_async_lets, collect_task_group_awaited_handles,
    direct_task_group_awaited_handles_in_stmt, find_nested_task_group_await_span,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResourceGenericContext {
    Ordinary,
    Return,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PatternWitness {
    Any,
    Bool(bool),
    Constructor {
        name: String,
        fields: Vec<(String, PatternWitness)>,
    },
}

fn resource_result_return_arg_allowed(
    ty: &TypeRef,
    index: usize,
    context: ResourceGenericContext,
) -> bool {
    context == ResourceGenericContext::Return && ty.name == "Result" && index == 0
}

/// The source shape is explicit because a single source preserves its historic
/// parser entrypoint, while package analysis merges separately parsed files.
enum AnalysisSources<'a> {
    Single { file: &'a str, source: &'a str },
    Many(&'a [(&'a str, &'a str)]),
}

impl AnalysisInput<'_> {
    fn start_span(&self) -> Span {
        let (file, source) = match self.sources {
            AnalysisSources::Single { file, source } => (file, source),
            AnalysisSources::Many(sources) => {
                sources.first().copied().unwrap_or(("unknown.rss", ""))
            }
        };
        Span {
            file: file.to_string(),
            line: 1,
            column: 1,
            length: source.len(),
        }
    }
}

#[derive(Clone, Copy)]
enum AnalysisFlavor {
    FullWithStandardPackages,
    FullWithBuiltinInterfaces,
    FullWithoutBuiltinInterfaces,
    SyntaxOnly,
}

struct AnalysisInput<'a> {
    sources: AnalysisSources<'a>,
    interfaces: &'a [(&'a str, &'a str)],
    flavor: AnalysisFlavor,
}

struct PreparedAnalysis {
    tokens: Vec<Token>,
    source_snapshot: SourceSnapshot,
    interface_snapshot: SourceSnapshot,
    source_programs: Vec<crate::syntax::ast::Program>,
    syntax_program: crate::syntax::ast::Program,
    interface_programs: Vec<crate::syntax::ast::Program>,
    hir: Hir,
    type_aliases: BTreeMap<String, AliasDefinition>,
    budget: Rc<FrontendBudget>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AliasDefinition {
    pub(crate) parameters: Vec<String>,
    pub(crate) target: TypeRef,
}

/// Own the analyzer front-end protocol in one place. Public entrypoints select
/// only their historical input shape and HIR interface policy.
fn prepare_analysis(input: AnalysisInput<'_>, budget: Rc<FrontendBudget>) -> PreparedAnalysis {
    let source_snapshot = match input.sources {
        AnalysisSources::Single { file, source } => SourceSnapshot::single(file, source),
        AnalysisSources::Many(sources) => SourceSnapshot::from_sources(sources.iter().copied()),
    };
    let (tokens, source_programs) = match input.sources {
        AnalysisSources::Single { file, source } => {
            let tokens = lex_with_budget(file, source, budget.clone());
            let program = crate::syntax::parse_source_tokens(file, &tokens, budget.clone());
            (tokens, vec![program])
        }
        AnalysisSources::Many(sources) => {
            let mut tokens = Vec::new();
            let mut programs = Vec::new();
            for (file, source) in sources {
                let source_tokens = lex_with_budget(file, source, budget.clone());
                programs.push(crate::syntax::parse_source_tokens(
                    file,
                    &source_tokens,
                    budget.clone(),
                ));
                tokens.extend(source_tokens);
            }
            (tokens, programs)
        }
    };
    let mut syntax_program = merge_programs(source_programs.iter().cloned());

    if matches!(input.flavor, AnalysisFlavor::SyntaxOnly) {
        crate::syntax::isolate_module_namespaces(&mut syntax_program);
        let hir = Hir::from_syntax(&syntax_program);
        return PreparedAnalysis {
            tokens,
            source_snapshot,
            interface_snapshot: SourceSnapshot::from_sources(std::iter::empty()),
            source_programs,
            syntax_program,
            interface_programs: Vec::new(),
            hir,
            type_aliases: BTreeMap::new(),
            budget,
        };
    }

    // Alias lookup historically includes core and standard-package aliases for
    // every full analysis flavor, even when HIR builtin interfaces are disabled.
    let default_interface_programs = crate::interfaces::default_interfaces()
        .map(|(file, source)| {
            let tokens = lex_with_budget(file, source, budget.clone());
            crate::syntax::parse_source_tokens(file, &tokens, budget.clone())
        })
        .collect::<Vec<_>>();
    let mut supplied_interface_programs = input
        .interfaces
        .iter()
        .map(|(file, source)| {
            let tokens = lex_with_budget(file, source, budget.clone());
            crate::syntax::parse_source_tokens(file, &tokens, budget.clone())
        })
        .collect::<Vec<_>>();
    isolate_sources_with_interfaces(&mut syntax_program, &mut supplied_interface_programs);
    let hir = match input.flavor {
        AnalysisFlavor::FullWithStandardPackages => Hir::from_syntax_with_prepared_interfaces(
            &syntax_program,
            &default_interface_programs,
            &[],
        ),
        AnalysisFlavor::FullWithBuiltinInterfaces => Hir::from_syntax_with_prepared_interfaces(
            &syntax_program,
            &default_interface_programs[..CORE_INTERFACES.len()],
            &supplied_interface_programs,
        ),
        AnalysisFlavor::FullWithoutBuiltinInterfaces => Hir::from_syntax_with_prepared_interfaces(
            &syntax_program,
            &[],
            &supplied_interface_programs,
        ),
        AnalysisFlavor::SyntaxOnly => unreachable!("syntax-only analysis returned above"),
    };
    let type_aliases = match input.flavor {
        AnalysisFlavor::FullWithStandardPackages => collect_type_alias_metadata(
            default_interface_programs
                .iter()
                .chain(std::iter::once(&syntax_program)),
        ),
        AnalysisFlavor::FullWithBuiltinInterfaces
        | AnalysisFlavor::FullWithoutBuiltinInterfaces => collect_type_alias_metadata(
            default_interface_programs
                .iter()
                .chain(supplied_interface_programs.iter())
                .chain(std::iter::once(&syntax_program)),
        ),
        AnalysisFlavor::SyntaxOnly => unreachable!("syntax-only analysis returned above"),
    };
    let interface_programs = match input.flavor {
        AnalysisFlavor::FullWithStandardPackages => default_interface_programs,
        AnalysisFlavor::FullWithBuiltinInterfaces
        | AnalysisFlavor::FullWithoutBuiltinInterfaces => supplied_interface_programs,
        AnalysisFlavor::SyntaxOnly => unreachable!("syntax-only analysis returned above"),
    };
    let interface_snapshot = match input.flavor {
        AnalysisFlavor::FullWithStandardPackages => {
            SourceSnapshot::from_sources(crate::interfaces::default_interfaces())
        }
        AnalysisFlavor::FullWithBuiltinInterfaces
        | AnalysisFlavor::FullWithoutBuiltinInterfaces => {
            SourceSnapshot::from_sources(input.interfaces.iter().copied())
        }
        AnalysisFlavor::SyntaxOnly => unreachable!("syntax-only analysis returned above"),
    };
    PreparedAnalysis {
        tokens,
        source_snapshot,
        interface_snapshot,
        source_programs,
        syntax_program,
        interface_programs,
        hir,
        type_aliases,
        budget,
    }
}

/// Module isolation must see source and supplied interfaces as one symbol
/// graph, then restore their semantic roles. Otherwise `use host.fs.*` resolves
/// only as a bare function and the stable external symbol loses `host.fs`.
fn isolate_sources_with_interfaces(sources: &mut Program, interfaces: &mut [Program]) {
    let source_files = program_files(sources);
    let interface_files = interfaces.iter().map(program_files).collect::<Vec<_>>();
    let mut combined =
        merge_programs(std::iter::once(sources.clone()).chain(interfaces.iter().cloned()));
    crate::syntax::isolate_module_namespaces(&mut combined);
    *sources = program_for_files(&combined, &source_files);
    for (interface, files) in interfaces.iter_mut().zip(interface_files) {
        *interface = program_for_files(&combined, &files);
    }
}

fn program_files(program: &Program) -> HashSet<String> {
    let mut files = program
        .items
        .iter()
        .map(|item| match item {
            Item::Module(value) => &value.span.file,
            Item::Use(value) => &value.span.file,
            Item::Type(value) => &value.span.file,
            Item::SumType(value) => &value.span.file,
            Item::TypeAlias(value) => &value.span.file,
            Item::Const(value) => &value.span.file,
            Item::Function(value) => &value.span.file,
        })
        .cloned()
        .collect::<HashSet<_>>();
    files.extend(
        program
            .unknown_top_level_spans
            .iter()
            .chain(&program.malformed_declaration_spans)
            .map(|span| span.file.clone()),
    );
    files.extend(program.protocols.iter().map(|decl| decl.span.file.clone()));
    files.extend(
        program
            .protocol_impls
            .iter()
            .map(|implementation| implementation.span.file.clone()),
    );
    files
}

fn program_for_files(program: &Program, files: &HashSet<String>) -> Program {
    let contains = |file: &str| files.contains(file);
    Program {
        unknown_top_level_spans: program
            .unknown_top_level_spans
            .iter()
            .filter(|span| contains(&span.file))
            .cloned()
            .collect(),
        malformed_declaration_spans: program
            .malformed_declaration_spans
            .iter()
            .filter(|span| contains(&span.file))
            .cloned()
            .collect(),
        protocols: program
            .protocols
            .iter()
            .filter(|decl| contains(&decl.span.file))
            .cloned()
            .collect(),
        protocol_impls: program
            .protocol_impls
            .iter()
            .filter(|implementation| contains(&implementation.span.file))
            .cloned()
            .collect(),
        items: program
            .items
            .iter()
            .filter(|item| match item {
                Item::Module(value) => contains(&value.span.file),
                Item::Use(value) => contains(&value.span.file),
                Item::Type(value) => contains(&value.span.file),
                Item::SumType(value) => contains(&value.span.file),
                Item::TypeAlias(value) => contains(&value.span.file),
                Item::Const(value) => contains(&value.span.file),
                Item::Function(value) => contains(&value.span.file),
            })
            .cloned()
            .collect(),
    }
}

fn analyze_input(input: AnalysisInput<'_>) -> Vec<Diagnostic> {
    analyze_input_result(input, FrontendBudgetLimits::default(), None).into_diagnostics()
}

#[cfg(test)]
fn analyze_input_with_budget(
    input: AnalysisInput<'_>,
    limits: FrontendBudgetLimits,
    cancel: Option<Arc<AtomicBool>>,
) -> Vec<Diagnostic> {
    analyze_input_result(input, limits, cancel).into_diagnostics()
}

fn analyze_input_result(
    input: AnalysisInput<'_>,
    limits: FrontendBudgetLimits,
    cancel: Option<Arc<AtomicBool>>,
) -> AnalysisResult {
    let flavor = input.flavor;
    let budget = FrontendBudget::with_cancellation(limits, input.start_span(), cancel);
    let prepared = prepare_analysis(input, budget);
    match flavor {
        AnalysisFlavor::SyntaxOnly => analyze_syntax_program(prepared),
        AnalysisFlavor::FullWithStandardPackages
        | AnalysisFlavor::FullWithBuiltinInterfaces
        | AnalysisFlavor::FullWithoutBuiltinInterfaces => analyze_program(prepared),
    }
}

pub fn analyze_source(file: &str, source: &str) -> Vec<Diagnostic> {
    analyze_input(AnalysisInput {
        sources: AnalysisSources::Single { file, source },
        interfaces: &[],
        flavor: AnalysisFlavor::FullWithStandardPackages,
    })
}

pub fn analyze_source_result(file: &str, source: &str) -> AnalysisResult {
    analyze_input_result(
        AnalysisInput {
            sources: AnalysisSources::Single { file, source },
            interfaces: &[],
            flavor: AnalysisFlavor::FullWithStandardPackages,
        },
        FrontendBudgetLimits::default(),
        None,
    )
}

pub fn validate_source(file: &str, source: &str) -> Result<ValidatedProgram, Vec<Diagnostic>> {
    analyze_source_result(file, source).into_validated()
}

pub fn analyze_syntax_source(file: &str, source: &str) -> Vec<Diagnostic> {
    analyze_input(AnalysisInput {
        sources: AnalysisSources::Single { file, source },
        interfaces: &[],
        flavor: AnalysisFlavor::SyntaxOnly,
    })
}

pub fn analyze_source_without_core(file: &str, source: &str) -> Vec<Diagnostic> {
    analyze_input(AnalysisInput {
        sources: AnalysisSources::Single { file, source },
        interfaces: &[],
        flavor: AnalysisFlavor::FullWithoutBuiltinInterfaces,
    })
}

pub fn core_interfaces() -> &'static [(&'static str, &'static str)] {
    CORE_INTERFACES
}

pub fn standard_package_interfaces() -> &'static [(&'static str, &'static str)] {
    crate::interfaces::STANDARD_PACKAGE_INTERFACES
}

pub fn analyze_source_with_core(file: &str, source: &str) -> Vec<Diagnostic> {
    analyze_source(file, source)
}

pub fn analyze_source_with_interfaces(
    file: &str,
    source: &str,
    interfaces: &[(&str, &str)],
) -> Vec<Diagnostic> {
    analyze_input(AnalysisInput {
        sources: AnalysisSources::Single { file, source },
        interfaces,
        flavor: AnalysisFlavor::FullWithBuiltinInterfaces,
    })
}

pub fn analyze_source_with_interfaces_result(
    file: &str,
    source: &str,
    interfaces: &[(&str, &str)],
) -> AnalysisResult {
    analyze_input_result(
        AnalysisInput {
            sources: AnalysisSources::Single { file, source },
            interfaces,
            flavor: AnalysisFlavor::FullWithBuiltinInterfaces,
        },
        FrontendBudgetLimits::default(),
        None,
    )
}

pub fn analyze_source_with_interfaces_without_core(
    file: &str,
    source: &str,
    interfaces: &[(&str, &str)],
) -> Vec<Diagnostic> {
    analyze_input(AnalysisInput {
        sources: AnalysisSources::Single { file, source },
        interfaces,
        flavor: AnalysisFlavor::FullWithoutBuiltinInterfaces,
    })
}

pub fn analyze_sources_with_interfaces(
    sources: &[(&str, &str)],
    interfaces: &[(&str, &str)],
) -> Vec<Diagnostic> {
    analyze_input(AnalysisInput {
        sources: AnalysisSources::Many(sources),
        interfaces,
        flavor: AnalysisFlavor::FullWithBuiltinInterfaces,
    })
}

pub fn analyze_sources_with_interfaces_result(
    sources: &[(&str, &str)],
    interfaces: &[(&str, &str)],
) -> AnalysisResult {
    analyze_input_result(
        AnalysisInput {
            sources: AnalysisSources::Many(sources),
            interfaces,
            flavor: AnalysisFlavor::FullWithBuiltinInterfaces,
        },
        FrontendBudgetLimits::default(),
        None,
    )
}

pub fn validate_sources_with_interfaces(
    sources: &[(&str, &str)],
    interfaces: &[(&str, &str)],
) -> Result<ValidatedProgram, Vec<Diagnostic>> {
    analyze_sources_with_interfaces_result(sources, interfaces).into_validated()
}

pub fn analyze_sources_with_interfaces_without_core(
    sources: &[(&str, &str)],
    interfaces: &[(&str, &str)],
) -> Vec<Diagnostic> {
    analyze_input(AnalysisInput {
        sources: AnalysisSources::Many(sources),
        interfaces,
        flavor: AnalysisFlavor::FullWithoutBuiltinInterfaces,
    })
}

pub fn analyze_sources_with_interfaces_without_core_result(
    sources: &[(&str, &str)],
    interfaces: &[(&str, &str)],
) -> AnalysisResult {
    analyze_input_result(
        AnalysisInput {
            sources: AnalysisSources::Many(sources),
            interfaces,
            flavor: AnalysisFlavor::FullWithoutBuiltinInterfaces,
        },
        FrontendBudgetLimits::default(),
        None,
    )
}

pub fn validate_sources_with_interfaces_without_core(
    sources: &[(&str, &str)],
    interfaces: &[(&str, &str)],
) -> Result<ValidatedProgram, Vec<Diagnostic>> {
    analyze_sources_with_interfaces_without_core_result(sources, interfaces).into_validated()
}

#[cfg(test)]
mod entrypoint_tests {
    use super::{
        AnalysisFlavor, AnalysisInput, AnalysisSources, PreparedAnalysis, analyze_input_result,
        analyze_input_with_budget, analyze_source_with_interfaces,
        analyze_source_with_interfaces_without_core, analyze_sources_with_interfaces,
        analyze_sources_with_interfaces_without_core, prepare_analysis, render_type_ref,
    };
    use crate::checks::budget::{FrontendBudget, FrontendBudgetLimits};
    use crate::diagnostic::code;
    use crate::semantic::{FrontendCompletion, FrontendStopReason};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    const SOURCE: &str = "fn helper(value: read Int) -> Int { return value }\n\
        fn main() -> Int { return helper(value: 1) }\n";
    const ALIAS_SOURCE: &str = "type SourceAlias = CallerAlias<Int>\n\
        type Callback = owned Fn(Int) -> String\n\
        fn main() -> Unit { return Unit }\n";
    const CALLER_INTERFACE: &str = "type CallerAlias<T> = Result<T, String>\n\
        pub fn Caller.make<T>(value: T) -> CallerAlias<T>\n";

    fn prepare(
        flavor: AnalysisFlavor,
        source: &'static str,
        interfaces: &'static [(&'static str, &'static str)],
    ) -> PreparedAnalysis {
        let input = AnalysisInput {
            sources: AnalysisSources::Single {
                file: "main.rss",
                source,
            },
            interfaces,
            flavor,
        };
        let budget = FrontendBudget::new(FrontendBudgetLimits::default(), input.start_span());
        prepare_analysis(input, budget)
    }

    #[test]
    fn single_and_merged_entrypoints_agree_for_each_interface_policy() {
        let sources = [("main.rss", SOURCE)];
        let interfaces = [];

        assert_eq!(
            analyze_source_with_interfaces("main.rss", SOURCE, &interfaces),
            analyze_sources_with_interfaces(&sources, &interfaces),
        );
        assert_eq!(
            analyze_source_with_interfaces_without_core("main.rss", SOURCE, &interfaces),
            analyze_sources_with_interfaces_without_core(&sources, &interfaces),
        );
    }

    #[test]
    fn cyclic_generic_aliases_in_interfaces_report_rs0039() {
        let interfaces = [(
            "cycle.rssi",
            "type A<T> = B<List<T>>\ntype B<T> = A<List<T>>\n",
        )];
        let diagnostics =
            analyze_source_with_interfaces_without_core("main.rss", SOURCE, &interfaces);

        assert_eq!(
            diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.code == "RS0039")
                .count(),
            2,
            "{diagnostics:?}"
        );
    }

    #[test]
    fn preparation_preserves_analyzer_interface_program_policy() {
        static CALLER_INTERFACES: &[(&str, &str)] = &[("caller.rssi", CALLER_INTERFACE)];

        let standard = prepare(AnalysisFlavor::FullWithStandardPackages, SOURCE, &[]);
        assert_eq!(
            standard.interface_programs.len(),
            crate::interfaces::default_interfaces().count(),
        );
        assert!(
            standard
                .hir
                .resolve_function(Some("List"), "new")
                .is_some_and(|signature| signature.is_builtin),
        );

        let with_builtins = prepare(
            AnalysisFlavor::FullWithBuiltinInterfaces,
            SOURCE,
            CALLER_INTERFACES,
        );
        assert_eq!(with_builtins.interface_programs.len(), 1);
        assert!(
            with_builtins
                .hir
                .resolve_function(Some("List"), "new")
                .is_some_and(|signature| signature.is_builtin),
        );
        assert!(
            with_builtins
                .hir
                .resolve_function(Some("Caller"), "make")
                .is_some_and(|signature| !signature.is_builtin),
        );

        let without_builtins = prepare(
            AnalysisFlavor::FullWithoutBuiltinInterfaces,
            SOURCE,
            CALLER_INTERFACES,
        );
        assert_eq!(without_builtins.interface_programs.len(), 1);
        assert!(
            without_builtins
                .hir
                .resolve_function(Some("List"), "new")
                .is_none(),
        );
        assert!(
            without_builtins
                .hir
                .resolve_function(Some("Caller"), "make")
                .is_some_and(|signature| !signature.is_builtin),
        );

        let syntax_only = prepare(AnalysisFlavor::SyntaxOnly, SOURCE, &[]);
        assert!(syntax_only.interface_programs.is_empty());
        assert!(syntax_only.type_aliases.is_empty());
    }

    #[test]
    fn preparation_builds_alias_metadata_from_prepared_programs() {
        static CALLER_INTERFACES: &[(&str, &str)] = &[("caller.rssi", CALLER_INTERFACE)];
        let prepared = prepare(
            AnalysisFlavor::FullWithoutBuiltinInterfaces,
            ALIAS_SOURCE,
            CALLER_INTERFACES,
        );

        assert_eq!(
            render_type_ref(&prepared.type_aliases["CallerAlias"].target),
            "Result<T, String>"
        );
        assert_eq!(prepared.type_aliases["CallerAlias"].parameters, vec!["T"]);
        assert_eq!(
            render_type_ref(&prepared.type_aliases["SourceAlias"].target),
            "CallerAlias<Int>"
        );
        assert_eq!(
            render_type_ref(&prepared.type_aliases["Callback"].target),
            "owned Fn(Int) -> String"
        );
    }

    #[test]
    fn wide_program_reports_incomplete_analysis_when_node_budget_is_exhausted() {
        let source = (0..200)
            .map(|index| format!("fn f{index}() -> Unit {{ return Unit }}\n"))
            .collect::<String>();
        let input = AnalysisInput {
            sources: AnalysisSources::Single {
                file: "wide.rss",
                source: &source,
            },
            interfaces: &[],
            flavor: AnalysisFlavor::FullWithoutBuiltinInterfaces,
        };
        let token_count = crate::lexer::lex("wide.rss", &source).len();

        let diagnostics = analyze_input_with_budget(
            input,
            FrontendBudgetLimits {
                nodes: token_count * 2,
                ..FrontendBudgetLimits::default()
            },
            None,
        );

        let incomplete = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == code::ANALYSIS_INCOMPLETE)
            .expect("wide analysis should stop at the shared node budget");
        assert!(
            incomplete
                .causes
                .iter()
                .any(|cause| cause.contains("nodes"))
        );
    }

    fn assert_incomplete_cause(diagnostics: &[crate::Diagnostic], expected: &str) {
        let incomplete = diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == code::ANALYSIS_INCOMPLETE)
            .expect("frontend should report incomplete analysis");
        assert!(
            incomplete
                .causes
                .iter()
                .any(|cause| cause.contains(expected)),
            "expected {expected:?} exhaustion, got {incomplete:?}",
        );
    }

    #[test]
    fn deeply_nested_expression_stops_at_parse_depth_budget() {
        let source = format!(
            "fn main() -> Unit {{ return {}Unit }}\n",
            "await ".repeat(2_000),
        );
        let diagnostics = analyze_input_with_budget(
            AnalysisInput {
                sources: AnalysisSources::Single {
                    file: "deep.rss",
                    source: &source,
                },
                interfaces: &[],
                flavor: AnalysisFlavor::SyntaxOnly,
            },
            FrontendBudgetLimits {
                parse_depth: 32,
                ..FrontendBudgetLimits::default()
            },
            None,
        );

        assert_incomplete_cause(&diagnostics, "parse depth");
    }

    #[test]
    fn token_storm_stops_during_lexing() {
        let source = "? ".repeat(10_000);
        let diagnostics = analyze_input_with_budget(
            AnalysisInput {
                sources: AnalysisSources::Single {
                    file: "tokens.rss",
                    source: &source,
                },
                interfaces: &[],
                flavor: AnalysisFlavor::SyntaxOnly,
            },
            FrontendBudgetLimits {
                tokens: 64,
                ..FrontendBudgetLimits::default()
            },
            None,
        );

        assert_incomplete_cause(&diagnostics, "tokens");
    }

    #[test]
    fn wide_syntax_tree_stops_at_ast_node_budget() {
        let source = (0..100)
            .map(|index| format!("fn f{index}() -> Unit {{ return Unit }}\n"))
            .collect::<String>();
        let diagnostics = analyze_input_with_budget(
            AnalysisInput {
                sources: AnalysisSources::Single {
                    file: "ast-nodes.rss",
                    source: &source,
                },
                interfaces: &[],
                flavor: AnalysisFlavor::SyntaxOnly,
            },
            FrontendBudgetLimits {
                ast_nodes: 16,
                ..FrontendBudgetLimits::default()
            },
            None,
        );

        assert_incomplete_cause(&diagnostics, "AST nodes");
    }

    #[test]
    fn oversized_source_stops_before_lexing() {
        let source = " ".repeat(4_096);
        let diagnostics = analyze_input_with_budget(
            AnalysisInput {
                sources: AnalysisSources::Single {
                    file: "large.rss",
                    source: &source,
                },
                interfaces: &[],
                flavor: AnalysisFlavor::SyntaxOnly,
            },
            FrontendBudgetLimits {
                source_bytes: 128,
                ..FrontendBudgetLimits::default()
            },
            None,
        );

        assert_incomplete_cause(&diagnostics, "source bytes");
    }

    #[test]
    fn cancellation_stops_frontend_work() {
        let cancel = Arc::new(AtomicBool::new(true));
        let result = analyze_input_result(
            AnalysisInput {
                sources: AnalysisSources::Single {
                    file: "cancelled.rss",
                    source: SOURCE,
                },
                interfaces: &[],
                flavor: AnalysisFlavor::SyntaxOnly,
            },
            FrontendBudgetLimits::default(),
            Some(cancel),
        );

        assert_eq!(
            result.completion(),
            FrontendCompletion::Incomplete(FrontendStopReason::Cancelled)
        );
        assert_incomplete_cause(result.diagnostics(), "cancellation");
        assert!(result.into_validated().is_err());
    }

    #[test]
    fn parser_consumes_the_tokens_lexed_by_preparation() {
        let token_count = crate::lexer::lex("single-lex.rss", SOURCE).len() - 1;
        let diagnostics = analyze_input_with_budget(
            AnalysisInput {
                sources: AnalysisSources::Single {
                    file: "single-lex.rss",
                    source: SOURCE,
                },
                interfaces: &[],
                flavor: AnalysisFlavor::SyntaxOnly,
            },
            FrontendBudgetLimits {
                tokens: token_count,
                ..FrontendBudgetLimits::default()
            },
            None,
        );

        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.code != code::ANALYSIS_INCOMPLETE),
            "{diagnostics:?}",
        );
    }
}

fn analyze_program(prepared: PreparedAnalysis) -> AnalysisResult {
    let PreparedAnalysis {
        tokens,
        source_snapshot,
        interface_snapshot,
        source_programs,
        syntax_program,
        interface_programs,
        hir,
        type_aliases,
        budget,
    } = prepared;
    let mut analyzer = Analyzer {
        tokens: &tokens,
        syntax_program,
        interface_programs,
        hir,
        diagnostics: AnalysisDiagnostics::new(budget.clone()),
        budget,
        type_aliases,
        in_task_group: false,
        async_let_names: Vec::new(),
    };
    analyzer.run();
    analyzer.diagnostics.push_incomplete();
    crate::syntax::demangle_diagnostics(
        &analyzer.syntax_program,
        analyzer.diagnostics.as_mut_slice(),
    );
    let completion = budget_completion(&analyzer.budget);
    let diagnostics = analyzer.diagnostics.into_vec();
    AnalysisResult::new(
        SemanticDatabase::new(
            source_snapshot,
            interface_snapshot,
            source_programs,
            analyzer.syntax_program,
            analyzer.interface_programs,
            analyzer.hir,
        ),
        diagnostics,
        completion,
    )
}

/// Namespaces that the compiler generates and a user declaration must not claim:
/// desugaring temporaries (`__rss_*`) and runtime helpers (`__rsscript_*`). Other
/// `__`-prefixed names (Python-style dunders like `__hash__`, `__eq__`, and the
/// synthetic `__TupleN` structs the tuple desugar injects) are legal — they don't
/// collide with any generated namespace.
fn is_reserved_generated_name(leaf: &str) -> bool {
    leaf.starts_with("__rss_") || leaf.starts_with("__rsscript_")
}

fn collect_type_alias_metadata<'a>(
    programs: impl IntoIterator<Item = &'a crate::syntax::ast::Program>,
) -> BTreeMap<String, AliasDefinition> {
    let mut type_aliases = BTreeMap::new();
    for program in programs {
        for item in &program.items {
            let Item::TypeAlias(alias) = item else {
                continue;
            };
            type_aliases.insert(
                alias.name.clone(),
                AliasDefinition {
                    parameters: alias
                        .type_params
                        .iter()
                        .map(|param| param.name.clone())
                        .collect(),
                    target: alias.target.clone(),
                },
            );
        }
    }
    type_aliases
}

fn analyze_syntax_program(prepared: PreparedAnalysis) -> AnalysisResult {
    let PreparedAnalysis {
        tokens,
        source_snapshot,
        interface_snapshot,
        source_programs,
        syntax_program,
        interface_programs,
        hir,
        type_aliases,
        budget,
    } = prepared;
    let mut analyzer = Analyzer {
        tokens: &tokens,
        syntax_program,
        interface_programs,
        hir,
        diagnostics: AnalysisDiagnostics::new(budget.clone()),
        budget,
        type_aliases,
        in_task_group: false,
        async_let_names: Vec::new(),
    };
    analyzer.run_syntax_only();
    analyzer.diagnostics.push_incomplete();
    crate::syntax::demangle_diagnostics(
        &analyzer.syntax_program,
        analyzer.diagnostics.as_mut_slice(),
    );
    let completion = budget_completion(&analyzer.budget);
    let diagnostics = analyzer.diagnostics.into_vec();
    AnalysisResult::new(
        SemanticDatabase::new(
            source_snapshot,
            interface_snapshot,
            source_programs,
            analyzer.syntax_program,
            analyzer.interface_programs,
            analyzer.hir,
        ),
        diagnostics,
        completion,
    )
}

pub(crate) struct Analyzer<'a> {
    pub(crate) tokens: &'a [Token],
    pub(crate) syntax_program: crate::syntax::ast::Program,
    pub(crate) interface_programs: Vec<crate::syntax::ast::Program>,
    pub(crate) hir: Hir,
    pub(crate) diagnostics: AnalysisDiagnostics,
    pub(crate) budget: Rc<FrontendBudget>,
    pub(crate) type_aliases: std::collections::BTreeMap<String, AliasDefinition>,
    in_task_group: bool,
    pub(crate) async_let_names: Vec<String>,
}

fn render_type_ref(ty: &TypeRef) -> String {
    let mut rendered = if ty.name == "Fn" {
        let params = ty
            .fn_params
            .iter()
            .enumerate()
            .map(|(index, parameter)| {
                let parameter = render_type_ref(parameter);
                match ty.fn_param_effects.get(index).copied().flatten() {
                    Some(effect) => format!("{} {parameter}", effect.as_str()),
                    None => parameter,
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        let return_type = ty
            .fn_return
            .as_deref()
            .map(render_type_ref)
            .unwrap_or_else(|| "Unit".to_string());
        format!("Fn({params}) -> {return_type}")
    } else if ty.args.is_empty() {
        ty.name.clone()
    } else {
        format!(
            "{}<{}>",
            ty.name,
            ty.args
                .iter()
                .map(render_type_ref)
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    if ty.is_owned {
        rendered = format!("owned {rendered}");
    }
    if ty.is_noescape {
        rendered = format!("noescape {rendered}");
    }
    if ty.is_fresh {
        rendered = format!("fresh {rendered}");
    }
    rendered
}

fn substitute_alias_type_ref(ty: &TypeRef, substitutions: &BTreeMap<String, TypeRef>) -> TypeRef {
    if ty.args.is_empty()
        && ty.fn_params.is_empty()
        && ty.fn_return.is_none()
        && let Some(replacement) = substitutions.get(&ty.name)
    {
        let mut replacement = replacement.clone();
        replacement.is_fresh |= ty.is_fresh;
        replacement.is_noescape |= ty.is_noescape;
        replacement.is_owned |= ty.is_owned;
        return replacement;
    }
    let mut substituted = ty.clone();
    substituted.args = substituted
        .args
        .iter()
        .map(|argument| substitute_alias_type_ref(argument, substitutions))
        .collect();
    substituted.fn_params = substituted
        .fn_params
        .iter()
        .map(|parameter| substitute_alias_type_ref(parameter, substitutions))
        .collect();
    substituted.fn_return = substituted
        .fn_return
        .as_deref()
        .map(|return_type| Box::new(substitute_alias_type_ref(return_type, substitutions)));
    substituted
}

impl Analyzer<'_> {
    /// Expand a type-alias reference, including generic aliases, to its target
    /// type. `IntList` -> `List<Int>`; `Pair<Int>` -> `Result<Int, String>` for
    /// `type Pair<T> = Result<T, String>`. Expansion is recursive, so aliases are
    /// transparent below nominal type arguments as well as at the root.
    pub(crate) fn expand_type_alias(&self, type_name: &str) -> String {
        self.expand_type_alias_inner(type_name, &mut std::collections::BTreeSet::new())
    }

    fn expand_type_alias_inner(
        &self,
        type_name: &str,
        visiting: &mut std::collections::BTreeSet<String>,
    ) -> String {
        use crate::text_util::{substitute_type_args, type_arg_names, type_root_name};

        let trimmed = type_name.trim();
        let prefixed = ["fresh ", "noescape ", "owned "]
            .into_iter()
            .find_map(|prefix| trimmed.strip_prefix(prefix).map(|target| (prefix, target)));
        if let Some((prefix, target)) = prefixed {
            format!("{prefix}{}", self.expand_type_alias_inner(target, visiting))
        } else {
            let root = type_root_name(trimmed);
            let expanded_args = type_arg_names(trimmed).map(|args| {
                args.into_iter()
                    .map(|arg| self.expand_type_alias_inner(arg, visiting))
                    .collect::<Vec<_>>()
            });
            let normalized = expanded_args.as_ref().map_or_else(
                || trimmed.to_string(),
                |args| format!("{root}<{}>", args.join(", ")),
            );
            if let Some(definition) = self.type_aliases.get(root) {
                let target = render_type_ref(&definition.target);
                let params = definition.parameters.clone();
                let alias_target = if params.is_empty() {
                    Some(target)
                } else {
                    expanded_args.as_ref().and_then(|args| {
                        if args.len() != params.len() {
                            return None;
                        }
                        let substitutions = params
                            .into_iter()
                            .zip(args.iter().cloned())
                            .collect::<std::collections::HashMap<_, _>>();
                        Some(substitute_type_args(&target, &substitutions))
                    })
                };
                if let Some(alias_target) = alias_target {
                    if !visiting.insert(root.to_string()) {
                        return normalized;
                    }
                    let expanded = self.expand_type_alias_inner(&alias_target, visiting);
                    visiting.remove(root);
                    return expanded;
                }
            }
            normalized
        }
    }

    pub(crate) fn canonical_type_ref(&self, ty: &TypeRef) -> TypeRef {
        self.canonical_type_ref_inner(ty, &mut std::collections::BTreeSet::new())
    }

    fn canonical_type_ref_inner(
        &self,
        ty: &TypeRef,
        visiting: &mut std::collections::BTreeSet<String>,
    ) -> TypeRef {
        let mut normalized = ty.clone();
        normalized.args = normalized
            .args
            .iter()
            .map(|argument| self.canonical_type_ref_inner(argument, visiting))
            .collect();
        normalized.fn_params = normalized
            .fn_params
            .iter()
            .map(|parameter| self.canonical_type_ref_inner(parameter, visiting))
            .collect();
        normalized.fn_return = normalized
            .fn_return
            .as_deref()
            .map(|return_type| Box::new(self.canonical_type_ref_inner(return_type, visiting)));
        if let Some(definition) = self.type_aliases.get(&normalized.name)
            && definition.parameters.len() == normalized.args.len()
        {
            if !visiting.insert(normalized.name.clone()) {
                return normalized;
            }
            let substitutions = definition
                .parameters
                .iter()
                .cloned()
                .zip(normalized.args.iter().cloned())
                .collect::<BTreeMap<_, _>>();
            let substituted = substitute_alias_type_ref(&definition.target, &substitutions);
            let mut canonical = self.canonical_type_ref_inner(&substituted, visiting);
            canonical.is_fresh |= normalized.is_fresh;
            canonical.is_noescape |= normalized.is_noescape;
            canonical.is_owned |= normalized.is_owned;
            canonical.span = normalized.span.clone();
            visiting.remove(&normalized.name);
            return canonical;
        }
        normalized
    }

    fn run(&mut self) {
        macro_rules! run_pass {
            ($pass:expr) => {
                if !self.budget.consume_nodes(self.tokens.len().max(1)) {
                    return;
                }
                $pass;
                if self.budget.is_exhausted() {
                    return;
                }
            };
        }

        run_pass!(self.check_unsupported_syntax());
        run_pass!(self.check_derive_field_requirements());
        run_pass!(self.check_assignments());
        run_pass!(self.check_async_fn_lowerable());
        run_pass!(self.check_match_exhaustiveness());
        run_pass!(checks::declarations::check(self));
        run_pass!(checks::types::check_names(self));
        run_pass!(checks::declarations::check_generic_constraints(self));
        run_pass!(self.check_try_operator_result_returns());
        run_pass!(checks::types::check_resource_shapes(self));
        run_pass!(checks::calls::check(self));
        run_pass!(checks::body::check(self));
        run_pass!(checks::forbidden::check(self));
    }

    fn run_syntax_only(&mut self) {
        if !self.budget.consume_nodes(self.tokens.len().max(1)) {
            return;
        }
        self.check_unsupported_syntax();
    }

    fn check_try_operator_result_returns(&mut self) {}

    /// A user `async fn` lowers to a `Pending` chain. Control-flow statements
    /// with awaits lower as explicit statement boundaries; keep rejecting
    /// awaits embedded in ordinary expression positions where the lowering
    /// would otherwise hide an executor boundary inside an expression.
    fn check_async_fn_lowerable(&mut self) {
        use crate::syntax::ast::Item;
        let mut diagnostics = Vec::new();
        for item in &self.syntax_program.items {
            let Item::Function(function) = item else {
                continue;
            };
            if !function.is_async {
                continue;
            }
            if let Some(span) = async_block_nonlinear_await(&function.body) {
                diagnostics.push(async_not_lowerable_diagnostic(
                    span,
                    "this `await` is inside an expression that needs full async expression lowering",
                    "Move the await to a statement boundary, or put it inside an `if`/`loop`/`match` body where RSScript can create an explicit async boundary.",
                ));
            }
            // Independent of lowerability: a `Task.cancellation_token()` call in
            // A `Task.cancellation_token()` call in an async fn outside an
            // actual task_group still has no lexical cancellation owner, so it
            // would silently lower to a never-cancelled token. Reject it instead
            // of handing out fake structured cancellation.
            if let Some(span) = block_first_cancellation_token(&function.body) {
                diagnostics.push(cancellation_token_outside_task_group_diagnostic(span));
            }
        }
        self.diagnostics.extend(diagnostics);
    }

    /// Controlled ordinary assignment: `x = e` updates a `let mut` local. The
    /// left side must be a place whose root is a reassignable local, and `mut`
    /// must appear in the binding so mutation stays visible to the type system.
    fn check_assignments(&mut self) {
        use crate::syntax::ast::Item;
        let diagnostics = {
            let declared_types = self
                .syntax_program
                .items
                .iter()
                .chain(
                    self.interface_programs
                        .iter()
                        .flat_map(|program| program.items.iter()),
                )
                .filter_map(|item| match item {
                    Item::Type(decl) => Some(decl.name.clone()),
                    Item::SumType(decl) => Some(decl.name.clone()),
                    Item::TypeAlias(decl) => Some(decl.name.clone()),
                    _ => None,
                })
                .collect();
            let mut checker = AssignChecker::new(&self.hir, declared_types);
            for item in &self.syntax_program.items {
                if let Item::Function(function) = item {
                    checker.check_function(function);
                }
            }
            checker.diagnostics
        };
        self.diagnostics.extend(diagnostics);
    }
}

fn generic_namespace_args(namespace: &str) -> Option<(&str, Vec<&str>)> {
    let (root, rest) = namespace.split_once('<')?;
    let args = rest.strip_suffix('>')?;
    Some((root, split_top_level_type_args(args)))
}

/// The top-level parameter slices of a `Fn(...)` type string, e.g.
/// `owned Fn(read UOp, mut Ctx) -> Option<UOp>` → `["read UOp", "mut Ctx"]`.
/// Returns `None` when the string is not a `Fn` type.
fn fn_type_params(type_name: &str) -> Option<Vec<&str>> {
    let trimmed = type_name.trim();
    let after_prefix = trimmed
        .strip_prefix("fresh ")
        .unwrap_or(trimmed)
        .trim_start();
    let after_prefix = after_prefix
        .strip_prefix("owned ")
        .or_else(|| after_prefix.strip_prefix("noescape "))
        .unwrap_or(after_prefix)
        .trim_start();
    let inner = after_prefix.strip_prefix("Fn(")?;
    // Find the matching close paren of the parameter list.
    let mut depth = 1usize;
    let mut end = None;
    for (index, ch) in inner.char_indices() {
        match ch {
            '(' | '<' => depth += 1,
            ')' | '>' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(index);
                    break;
                }
            }
            _ => {}
        }
    }
    let params = &inner[..end?];
    if params.trim().is_empty() {
        return Some(Vec::new());
    }
    Some(split_top_level_type_args(params))
}

/// The declared effect of each parameter in a `Fn(...)` type string, in order.
fn fn_type_param_effects(type_name: &str) -> Vec<Option<DataEffect>> {
    fn_type_params(type_name)
        .unwrap_or_default()
        .into_iter()
        .map(|param| match param.split_whitespace().next() {
            Some("read") => Some(DataEffect::Read),
            Some("mut") => Some(DataEffect::Mut),
            Some("take") => Some(DataEffect::Take),
            _ => None,
        })
        .collect()
}

/// The bare type name of the `index`-th parameter of a `Fn(...)` type string,
/// with any leading effect keyword stripped (`mut Ctx` → `Ctx`).
fn fn_type_param_type_name(type_name: &str, index: usize) -> Option<String> {
    let params = fn_type_params(type_name)?;
    let param = params.get(index)?.trim();
    let bare = param
        .strip_prefix("read ")
        .or_else(|| param.strip_prefix("mut "))
        .or_else(|| param.strip_prefix("take "))
        .unwrap_or(param);
    Some(bare.trim().to_string())
}

pub(crate) fn generic_bounds(params: &[GenericParam]) -> HashMap<String, Option<GenericBound>> {
    params
        .iter()
        .map(|param| (param.name.clone(), param.bound.clone()))
        .collect()
}

fn async_not_lowerable_diagnostic(
    span: crate::diagnostic::Span,
    label: &str,
    cause: &str,
) -> Diagnostic {
    Diagnostic::error(
        code::ASYNC_FN_NOT_LOWERABLE,
        "async function is not lowerable in this version.",
        span,
        label,
    )
    .with_cause(cause)
    .with_fix(
        "restructure_async_fn",
        "Make every `await` a top-level statement, or move a `task_group` into a synchronous function.",
        "manual",
    )
}

fn cancellation_token_outside_task_group_diagnostic(span: crate::diagnostic::Span) -> Diagnostic {
    Diagnostic::error(
        code::CANCELLATION_TOKEN_OUTSIDE_TASK_GROUP,
        "`Task.cancellation_token()` is not allowed inside an `async fn`.",
        span,
        "this would observe a never-cancelled token, not the task_group's",
    )
    .with_cause(
        "An `async fn` has no lexically enclosing `task_group`, so this call cannot inherit the group's cancellation token and would silently never cancel.",
    )
    .with_fix(
        "pass_cancellation_token",
        "Call `Task.cancellation_token()` inside the `task_group` block and pass the token into this function as a `read CancellationToken` parameter.",
        "manual",
    )
}

/// First `Task.cancellation_token()` call span anywhere in a function body.
fn block_first_cancellation_token(block: &Block) -> Option<crate::diagnostic::Span> {
    block
        .statements
        .iter()
        .find_map(stmt_first_cancellation_token)
}

fn stmt_first_cancellation_token(statement: &Stmt) -> Option<crate::diagnostic::Span> {
    match statement {
        Stmt::Let(stmt) => stmt.value.as_ref().and_then(expr_first_cancellation_token),
        Stmt::Return(stmt) => stmt.value.as_ref().and_then(expr_first_cancellation_token),
        Stmt::Expr(expr) => expr_first_cancellation_token(expr),
        Stmt::With(stmt) => expr_first_cancellation_token(&stmt.resource)
            .or_else(|| block_first_cancellation_token(&stmt.body)),
        Stmt::If(stmt) => expr_first_cancellation_token(&stmt.condition)
            .or_else(|| block_first_cancellation_token(&stmt.then_body))
            .or_else(|| {
                stmt.else_body
                    .as_ref()
                    .and_then(block_first_cancellation_token)
            }),
        Stmt::Loop(stmt) => stmt
            .condition
            .as_ref()
            .and_then(expr_first_cancellation_token)
            .or_else(|| block_first_cancellation_token(&stmt.body)),
        Stmt::For(stmt) => expr_first_cancellation_token(&stmt.iterable)
            .or_else(|| block_first_cancellation_token(&stmt.body)),
        Stmt::Match(stmt) => expr_first_cancellation_token(&stmt.value).or_else(|| {
            stmt.arms
                .iter()
                .find_map(|arm| block_first_cancellation_token(&arm.body))
        }),
        Stmt::TaskGroup(_) => None,
        Stmt::LetElse(stmt) => expr_first_cancellation_token(&stmt.value)
            .or_else(|| block_first_cancellation_token(&stmt.else_body)),
        _ => None,
    }
}

fn expr_first_cancellation_token(expr: &Expr) -> Option<crate::diagnostic::Span> {
    match expr {
        Expr::Call { callee, args, span } => {
            if let Callee::Qualified { namespace, name } = callee
                && namespace == "Task"
                && name == "cancellation_token"
            {
                return Some(span.clone());
            }
            args.iter()
                .find_map(|arg| expr_first_cancellation_token(&arg.value))
        }
        Expr::Binary { left, right, .. } => {
            expr_first_cancellation_token(left).or_else(|| expr_first_cancellation_token(right))
        }
        Expr::Field { base, .. } => expr_first_cancellation_token(base),
        Expr::Index { base, index, .. } => {
            expr_first_cancellation_token(base).or_else(|| expr_first_cancellation_token(index))
        }
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Await { value, .. }
        | Expr::Try { value, .. } => expr_first_cancellation_token(value),
        Expr::Closure { body, .. } => block_first_cancellation_token(body),
        Expr::Match { value, arms, .. } => expr_first_cancellation_token(value).or_else(|| {
            arms.iter()
                .find_map(|arm| block_first_cancellation_token(&arm.body))
        }),
        Expr::MapLiteral { entries, .. } => entries
            .iter()
            .find_map(|entry| expr_first_cancellation_token(&entry.key))
            .or_else(|| {
                entries
                    .iter()
                    .find_map(|entry| expr_first_cancellation_token(&entry.value))
            }),
        Expr::ObjectLiteral { .. }
        | Expr::ArrayLiteral { .. }
        | Expr::Ident(..)
        | Expr::Number(..)
        | Expr::String(..)
        | Expr::CharLiteral(..)
        | Expr::MultilineString(..)
        | Expr::Unknown(_) => None,
    }
}

/// AST analogue of [`crate::hir::assign_target_reads`]: the evaluated
/// sub-expressions of an assignment target (a field/index base, an index
/// expression), excluding the write root. So awaits/`?`/calls embedded in an
/// assignment *target* (e.g. `xs[await f()] = v`) are visited like the RHS.
fn assign_target_reads_ast(target: &Expr) -> Vec<&Expr> {
    match target {
        Expr::Ident(_, _) => Vec::new(),
        Expr::Field { base, .. } => vec![base.as_ref()],
        Expr::Index { base, index, .. } => vec![base.as_ref(), index.as_ref()],
        other => vec![other],
    }
}

/// The awaited inner expression of `await x` / `await x?`.
fn async_await_inner_ast(expr: &Expr) -> Option<&Expr> {
    match expr {
        Expr::Try { value, .. } => match value.as_ref() {
            Expr::Await { value, .. } => Some(value),
            _ => None,
        },
        Expr::Await { value, .. } => Some(value),
        _ => None,
    }
}

fn expr_first_await(expr: &Expr) -> Option<crate::diagnostic::Span> {
    match expr {
        Expr::Await { span, .. } => Some(span.clone()),
        Expr::Binary { left, right, .. } => {
            expr_first_await(left).or_else(|| expr_first_await(right))
        }
        Expr::Field { base, .. } => expr_first_await(base),
        Expr::Index { base, index, .. } => {
            expr_first_await(base).or_else(|| expr_first_await(index))
        }
        Expr::Call { args, .. } => args.iter().find_map(|arg| expr_first_await(&arg.value)),
        Expr::Effect { value, .. }
        | Expr::Manage { value, .. }
        | Expr::Spawn { value, .. }
        | Expr::Try { value, .. } => expr_first_await(value),
        Expr::Closure { body, .. } => block_first_await(body),
        Expr::Match { value, arms, .. } => expr_first_await(value)
            .or_else(|| {
                arms.iter()
                    .find_map(|arm| arm.guard.as_ref().and_then(expr_first_await))
            })
            .or_else(|| arms.iter().find_map(|arm| block_first_await(&arm.body))),
        Expr::MapLiteral { entries, .. } => entries
            .iter()
            .find_map(|entry| expr_first_await(&entry.key))
            .or_else(|| {
                entries
                    .iter()
                    .find_map(|entry| expr_first_await(&entry.value))
            }),
        Expr::ObjectLiteral { fields, .. } => fields
            .iter()
            .find_map(|field| expr_first_await(&field.value)),
        Expr::ArrayLiteral { items, .. } => items.iter().find_map(expr_first_await),
        Expr::Ident(..)
        | Expr::Number(..)
        | Expr::String(..)
        | Expr::CharLiteral(..)
        | Expr::MultilineString(..)
        | Expr::Unknown(_) => None,
    }
}

fn block_first_await(block: &Block) -> Option<crate::diagnostic::Span> {
    block.statements.iter().find_map(stmt_first_await_ast)
}

fn stmt_first_await_ast(statement: &Stmt) -> Option<crate::diagnostic::Span> {
    match statement {
        Stmt::Let(stmt) => stmt.value.as_ref().and_then(expr_first_await),
        Stmt::Return(stmt) => stmt.value.as_ref().and_then(expr_first_await),
        Stmt::Expr(expr) => expr_first_await(expr),
        Stmt::With(stmt) => {
            expr_first_await(&stmt.resource).or_else(|| block_first_await(&stmt.body))
        }
        Stmt::If(stmt) => expr_first_await(&stmt.condition)
            .or_else(|| block_first_await(&stmt.then_body))
            .or_else(|| stmt.else_body.as_ref().and_then(block_first_await)),
        Stmt::Loop(stmt) => stmt
            .condition
            .as_ref()
            .and_then(expr_first_await)
            .or_else(|| block_first_await(&stmt.body)),
        Stmt::For(stmt) => {
            if stmt.is_async {
                Some(stmt.span.clone())
            } else {
                expr_first_await(&stmt.iterable).or_else(|| block_first_await(&stmt.body))
            }
        }
        Stmt::Match(stmt) => expr_first_await(&stmt.value).or_else(|| {
            stmt.arms
                .iter()
                .find_map(|arm| block_first_await(&arm.body))
        }),
        Stmt::Select(stmt) => stmt.arms.iter().find_map(|arm| {
            expr_first_await(&arm.operation).or_else(|| block_first_await(&arm.body))
        }),
        Stmt::TaskGroup(stmt) => block_first_await(&stmt.body),
        Stmt::LetElse(stmt) => {
            expr_first_await(&stmt.value).or_else(|| block_first_await(&stmt.else_body))
        }
        Stmt::Assign(stmt) => assign_target_reads_ast(&stmt.target)
            .into_iter()
            .find_map(expr_first_await)
            .or_else(|| expr_first_await(&stmt.value)),
        _ => None,
    }
}

/// The span of the first `await` that is *not* a top-level statement of an async
/// body (i.e. nested in control flow or a non-await expression position).
fn async_block_nonlinear_await(block: &Block) -> Option<crate::diagnostic::Span> {
    for statement in &block.statements {
        let nested = match statement {
            Stmt::Let(stmt) => match stmt.value.as_ref() {
                Some(value) => match async_await_inner_ast(value) {
                    Some(inner) => expr_first_await(inner),
                    None => expr_first_await(value),
                },
                None => None,
            },
            Stmt::Expr(expr) => match async_await_inner_ast(expr) {
                Some(inner) => expr_first_await(inner),
                None => expr_first_await(expr),
            },
            // `return await op` is a top-level await; only nested awaits in the
            // returned expression are non-linear.
            Stmt::Return(stmt) => match stmt.value.as_ref() {
                Some(value) => match async_await_inner_ast(value) {
                    Some(inner) => expr_first_await(inner),
                    None => expr_first_await(value),
                },
                None => None,
            },
            Stmt::Select(_) | Stmt::For(_) | Stmt::TaskGroup(_) => None,
            Stmt::If(_) | Stmt::Loop(_) | Stmt::Match(_) | Stmt::With(_) => None,
            // An assignment's RHS follows the same rule as a `let` initializer (a
            // direct `await` is linear; nested awaits are not). The target is an
            // evaluated place, so any await inside a field/index base or index
            // expression is non-linear (e.g. `xs[await f()] = v`).
            Stmt::Assign(stmt) => assign_target_reads_ast(&stmt.target)
                .into_iter()
                .find_map(expr_first_await)
                .or_else(|| match async_await_inner_ast(&stmt.value) {
                    Some(inner) => expr_first_await(inner),
                    None => expr_first_await(&stmt.value),
                }),
            other => stmt_first_await_ast(other),
        };
        if nested.is_some() {
            return nested;
        }
    }
    None
}

pub(crate) fn protocol_method_names(items: &[Item], protocol: &str) -> HashSet<String> {
    items
        .iter()
        .filter_map(|item| {
            let Item::Function(function) = item else {
                return None;
            };
            let (namespace, method) = split_qualified_name(&function.name);
            (namespace.as_deref() == Some(protocol)).then(|| method.to_string())
        })
        .collect()
}

pub(crate) fn function_body_belongs_to_protocol(
    function: &FunctionDecl,
    protocol_names: &HashSet<String>,
) -> bool {
    !function.body.statements.is_empty() && function_belongs_to_protocol(function, protocol_names)
}

pub(crate) fn function_belongs_to_protocol(
    function: &FunctionDecl,
    protocol_names: &HashSet<String>,
) -> bool {
    split_qualified_name(&function.name)
        .0
        .is_some_and(|namespace| protocol_names.contains(&namespace))
}

pub(crate) fn protocol_signature_mismatch(
    protocol: &FunctionSig,
    target: &FunctionSig,
    concrete_type: &str,
) -> Option<String> {
    if protocol.is_async != target.is_async {
        return Some("async/sync kind must match the protocol method exactly.".to_string());
    }
    if protocol.params.len() != target.params.len() {
        return Some(format!(
            "parameter count mismatch: protocol has {}, implementation has {}.",
            protocol.params.len(),
            target.params.len()
        ));
    }
    for (protocol_param, target_param) in protocol.params.iter().zip(&target.params) {
        if let Some(reason) = protocol_param_mismatch(protocol_param, target_param, concrete_type) {
            return Some(reason);
        }
    }
    let substitutions = BTreeMap::from([(
        "Self".to_string(),
        ResolvedType::from_display(concrete_type),
    )]);
    let protocol_return = protocol
        .return_ty
        .as_ref()
        .map(|return_type| return_type.substitute(&substitutions));
    if protocol_return != target.return_ty {
        return Some(format!(
            "return type mismatch: protocol expects `{}`, implementation returns `{}`.",
            protocol_return
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "Unit".to_string()),
            target
                .return_ty
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "Unit".to_string())
        ));
    }
    if protocol.returns_fresh != target.returns_fresh {
        return Some("fresh return mode must match the protocol method exactly.".to_string());
    }
    if protocol.retained_params != target.retained_params {
        return Some("retains(...) effects must match the protocol method exactly.".to_string());
    }
    None
}

fn protocol_param_mismatch(
    protocol: &ParamSig,
    target: &ParamSig,
    concrete_type: &str,
) -> Option<String> {
    if protocol.name != target.name {
        return Some(format!(
            "parameter name mismatch: protocol expects `{}`, implementation has `{}`.",
            protocol.name, target.name
        ));
    }
    if protocol.effect != target.effect {
        return Some(format!(
            "parameter effect mismatch for `{}`: protocol expects `{}`, implementation has `{}`.",
            protocol.name,
            protocol
                .effect
                .map(|effect| effect.as_str())
                .unwrap_or("none"),
            target
                .effect
                .map(|effect| effect.as_str())
                .unwrap_or("none")
        ));
    }
    let expected_type = protocol.ty.substitute(&BTreeMap::from([(
        "Self".to_string(),
        ResolvedType::from_display(concrete_type),
    )]));
    if expected_type != target.ty {
        return Some(format!(
            "parameter type mismatch for `{}`: protocol expects `{expected_type}`, implementation has `{}`.",
            protocol.name, target.ty
        ));
    }
    None
}

pub(crate) fn split_qualified_name(name: &str) -> (Option<String>, &str) {
    if let Some((namespace, name)) = name.rsplit_once('.') {
        (Some(namespace.to_string()), name)
    } else {
        (None, name)
    }
}

fn fresh_return_target_type(return_ty: &TypeRef) -> &TypeRef {
    if matches!(return_ty.name.as_str(), "Result" | "Option")
        && let Some(first_arg) = return_ty.args.first()
    {
        return first_arg;
    }
    return_ty
}

fn builtin_match_is_exhaustive(variants: &HashSet<String>) -> bool {
    (variants.contains("Some") && variants.contains("None"))
        || (variants.contains("Ok") && variants.contains("Err"))
}

fn hir_expr_type_name(expr: &HirExpr) -> Option<&str> {
    match expr {
        HirExpr::Ident {
            name, type_name, ..
        } => type_name
            .as_deref()
            .or_else(|| crate::checks::shared::builtin_value_type_name(name)),
        HirExpr::Call { type_name, .. }
        | HirExpr::Effect { type_name, .. }
        | HirExpr::Manage { type_name, .. }
        | HirExpr::Spawn { type_name, .. }
        | HirExpr::Await { type_name, .. }
        | HirExpr::Try { type_name, .. }
        | HirExpr::Match { type_name, .. }
        | HirExpr::MapLiteral { type_name, .. } => type_name.as_deref(),
        HirExpr::Field { access, .. } => access.type_name.as_deref(),
        HirExpr::Number { value, .. } => Some(crate::hir::number_literal_type_name(value)),
        HirExpr::String { .. } => Some("String"),
        HirExpr::Char { .. } => Some("Char"),
        HirExpr::Binary { .. } | HirExpr::Index { .. } => None,
        HirExpr::ObjectLiteral { .. }
        | HirExpr::ArrayLiteral { .. }
        | HirExpr::Closure { .. }
        | HirExpr::Unknown(_) => None,
    }
}

fn constructor_pattern_is_irrefutable(pattern: &MatchPattern) -> bool {
    match pattern {
        MatchPattern::Binding { .. } | MatchPattern::Wildcard(_) => true,
        // A variant pattern is irrefutable (over its own constructor) iff every
        // positional sub-pattern is itself irrefutable (a bare binder or `_`).
        // Payload-free (`bindings` empty) is trivially irrefutable.
        MatchPattern::Variant { bindings, .. } => bindings.iter().all(|binding| {
            matches!(
                binding,
                MatchPattern::Binding { .. } | MatchPattern::Wildcard(_)
            )
        }),
        MatchPattern::Struct { fields, .. } => fields.iter().all(|field| {
            field.pattern.is_none()
                || field.pattern.as_deref().is_some_and(|pattern| {
                    matches!(
                        pattern,
                        MatchPattern::Binding { .. } | MatchPattern::Wildcard(_)
                    )
                })
        }),
        // `[..]` / `[..rest]` matches any list; every other list pattern adds a
        // length or element constraint and so is refutable.
        MatchPattern::List {
            prefix,
            rest,
            suffix,
            ..
        } => prefix.is_empty() && suffix.is_empty() && rest.is_some(),
        MatchPattern::Literal { .. } => false,
    }
}

fn constrained_field_patterns(pattern: &MatchPattern) -> Vec<(String, &MatchPattern)> {
    match pattern {
        // Variant patterns are matched positionally against the constructor's
        // declared fields; see `pattern_matches_fields` in the exhaustiveness
        // module, which zips `bindings` with the witness fields by index.
        MatchPattern::Variant { .. } => Vec::new(),
        MatchPattern::Struct { fields, .. } => fields
            .iter()
            .filter_map(|field| {
                let pattern = field.pattern.as_deref()?;
                if matches!(
                    pattern,
                    MatchPattern::Binding { .. } | MatchPattern::Wildcard(_)
                ) {
                    None
                } else {
                    Some((field.name.clone(), pattern))
                }
            })
            .collect(),
        MatchPattern::Binding { .. }
        | MatchPattern::Literal { .. }
        | MatchPattern::List { .. }
        | MatchPattern::Wildcard(_) => Vec::new(),
    }
}

pub(crate) fn duplicate_symbol_label(kind: DuplicateSymbolKind) -> &'static str {
    match kind {
        DuplicateSymbolKind::Function => "function",
        DuplicateSymbolKind::Type => "type",
        DuplicateSymbolKind::Constructor => "callable",
        DuplicateSymbolKind::Field => "field",
    }
}

fn known_type_ref(ty: &TypeRef, generic_params: &HashSet<&str>, hir: &Hir) -> bool {
    if ty.name.is_empty() {
        return true;
    }
    if ty.is_noescape || ty.is_owned {
        return ty.name == "Fn";
    }
    generic_params.contains(ty.name.as_str())
        || is_builtin_type_name(&ty.name)
        || hir.type_info(&ty.name).is_some()
}

pub(crate) const BUILTIN_TYPE_NAMES: &[&str] = &[
    "Unit",
    "Bool",
    "Byte",
    "Char",
    "Int",
    "Int8",
    "Int16",
    "Int32",
    "Int64",
    "UInt",
    "UInt8",
    "UInt16",
    "UInt32",
    "UInt64",
    "Float",
    "Float32",
    "Float64",
    "String",
    "StringView",
    "Url",
    "Fd",
    "Bytes",
    "BytesView",
    "Buffer",
    "BufferView",
    "Path",
    "Result",
    "Option",
    "List",
    "Map",
    "Set",
    "Dyn",
    "Fn",
    "Closure",
    "FileError",
    "IOError",
    "HttpError",
    "JsonError",
    "CsvError",
    "NetworkError",
];

pub(crate) fn is_builtin_type_name(name: &str) -> bool {
    BUILTIN_TYPE_NAMES.contains(&name)
}

fn builtin_value_ident(name: &str) -> bool {
    matches!(name, "true" | "false" | "Unit" | "None" | "null")
}

fn type_ref_contains_name(ty: &TypeRef, name: &str) -> bool {
    ty.name == name
        || ty.args.iter().any(|arg| type_ref_contains_name(arg, name))
        || ty
            .fn_params
            .iter()
            .any(|param| type_ref_contains_name(param, name))
        || ty
            .fn_return
            .as_deref()
            .is_some_and(|return_ty| type_ref_contains_name(return_ty, name))
}

fn type_ref_name(ty: &TypeRef) -> String {
    let base = if ty.name == "Fn" {
        let params = ty
            .fn_params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let prefix = match ty.effective_fn_param_effect(index) {
                    Some(effect) => format!("{} ", effect.as_str()),
                    None => String::new(),
                };
                format!("{prefix}{}", type_ref_name(param))
            })
            .collect::<Vec<_>>()
            .join(", ");
        let return_ty = ty
            .fn_return
            .as_ref()
            .map(|return_ty| format!(" -> {}", type_ref_name(return_ty)))
            .unwrap_or_default();
        format!("Fn({params}){return_ty}")
    } else if ty.args.is_empty() {
        ty.name.clone()
    } else {
        let args = ty
            .args
            .iter()
            .map(type_ref_name)
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}<{args}>", ty.name)
    };
    let name = if ty.is_noescape {
        format!("noescape {base}")
    } else if ty.is_owned {
        format!("owned {base}")
    } else {
        base
    };
    if ty.is_fresh {
        format!("fresh {name}")
    } else {
        name
    }
}

/// The originating source file of a top-level item (from its span).
fn item_span_file(item: &Item) -> String {
    match item {
        Item::Function(decl) => decl.span.file.clone(),
        Item::Const(decl) => decl.span.file.clone(),
        Item::Type(decl) => decl.span.file.clone(),
        Item::SumType(decl) => decl.span.file.clone(),
        Item::TypeAlias(decl) => decl.span.file.clone(),
        Item::Module(decl) => decl.span.file.clone(),
        Item::Use(decl) => decl.span.file.clone(),
    }
}

/// Whether `name` is a plain Rust identifier (used verbatim as a pinned backend
/// symbol): non-empty, starts with a letter or `_`, and otherwise alphanumeric or
/// `_`. Raw identifiers and keywords are intentionally rejected.
pub(crate) fn is_valid_rust_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && !crate::text_util::is_rust_keyword(name)
}

pub(crate) fn type_ref_is_noescape(ty: &TypeRef) -> bool {
    ty.is_noescape || ty.args.iter().any(type_ref_is_noescape)
}

pub(crate) fn type_ref_is_copy(ty: &TypeRef) -> bool {
    !ty.is_fresh
        && !ty.is_noescape
        && ty.args.is_empty()
        && ty.fn_params.is_empty()
        && ty.fn_return.is_none()
        && matches!(
            ty.name.as_str(),
            "Bool"
                | "Byte"
                | "Char"
                | "Float"
                | "Float32"
                | "Float64"
                | "Int"
                | "Int8"
                | "Int16"
                | "Int32"
                | "Int64"
                | "UInt"
                | "UInt8"
                | "UInt16"
                | "UInt32"
                | "UInt64"
                | "Unit"
        )
}
