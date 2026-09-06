//! A deliberately small, syntax-owned oracle for editor prefixes.
//!
//! This is not an incremental parser. The ordinary parser remains the source
//! of truth for complete programs; this module only preserves enough lexical
//! and delimiter state to distinguish a recoverable prefix from a proven dead
//! end and to expose safe completion terminals.

use std::ops::Range;

use crate::ast::{
    Block, Callee, ConstDecl, Expr, FieldDecl, FunctionDecl, Item, LetStmt, MatchArm, MatchStmt,
    Stmt, SumTypeDecl, TypeAliasDecl, TypeDecl, TypeRef,
};
use crate::lexer::{Token, TokenKind, lex};
use crate::parse_source_raw;
use crate::parser::{
    ParserExpectationSite, ParserExpectationTerminal, ParserIdentifierRole, TOP_LEVEL_STARTERS,
    parse_prefix_oracle,
};

/// Whether a prefix can still be extended into a source program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixParseState {
    Complete,
    Incomplete,
    /// No suffix can repair the prefix without editing text already present.
    Dead,
}

/// Whether the terminal already present in the source is whole or still being
/// typed. Consumers should not treat `Partial` as a parser-validated token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalCompleteness {
    Complete,
    Partial,
}

/// The syntactic region containing the cursor.
///
/// `Unknown` deliberately means that the prefix oracle cannot establish a
/// more specific region. It is not an invitation for a consumer to infer one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxSite {
    TopLevel,
    FunctionHeader,
    FunctionBody,
    CallArguments,
    Unknown,
}

/// Syntax-only details of the enclosing function, when one is unambiguous.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionContext {
    pub name: Option<String>,
}

/// Syntax-only details of the enclosing call, when one is unambiguous.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallContext {
    /// The written callee path, if it is a simple name or dotted path.
    pub callee: Option<String>,
    /// Zero-based argument slot. `None` means the cursor is nested in a call
    /// but the incomplete surface does not establish a slot safely.
    pub argument_index: Option<usize>,
}

/// Context at the end of the source passed to [`parse_source_prefix`].
///
/// This is intentionally lexical and conservative: semantic consumers must
/// treat absent fields and [`SyntaxSite::Unknown`] as unavailable information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorContext {
    /// UTF-8 byte offset of the cursor (always `source.len()`).
    pub byte_offset: usize,
    pub site: SyntaxSite,
    pub function: Option<FunctionContext>,
    pub call: Option<CallContext>,
}

/// The syntactic role required of an identifier completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifierRole {
    ItemName,
    FunctionName,
    ParameterName,
    TypeName,
    Expression,
    FieldName,
}

/// Literal families which can be completed without semantic information.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiteralKind {
    Number,
    String,
    Char,
    InterpolatedString,
    MultilineString,
}

/// A terminal expected at the cursor. Fixed spellings, identifier roles, and
/// literal families are intentionally distinct so clients never need to infer
/// a role from a display string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedTerminal {
    Fixed {
        text: &'static str,
        completeness: TerminalCompleteness,
    },
    Identifier {
        role: IdentifierRole,
        completeness: TerminalCompleteness,
    },
    Literal {
        kind: LiteralKind,
        completeness: TerminalCompleteness,
    },
}

/// Prefix information safe to use by an editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefixParseResult {
    pub state: PrefixParseState,
    /// Completeness of the terminal immediately before the cursor.
    pub current_terminal_completeness: TerminalCompleteness,
    /// Whether `expected_terminals` exhaustively represents the syntactic
    /// possibilities at this cursor. `Partial` means callers may supplement
    /// it, but must not treat a missing terminal as forbidden.
    pub expected_terminals_completeness: TerminalCompleteness,
    pub expected_terminals: Vec<ExpectedTerminal>,
    /// The byte range of the current replaceable terminal. Both endpoints are
    /// guaranteed UTF-8 boundaries in `source`.
    pub replace_range: Range<usize>,
    pub cursor: CursorContext,
    source_identity: SourceIdentity,
    recovery_suffix: Option<String>,
}

impl PrefixParseResult {
    /// Returns whether this oracle result was produced for exactly `source`.
    /// The length plus stable content hash prevents reusing a completion result
    /// merely because a later edit has the same byte length.
    pub fn matches_source(&self, source: &str) -> bool {
        self.source_identity == SourceIdentity::of(source)
    }

    /// A syntax-owned, mechanically safe suffix when the oracle can prove one.
    /// `None` means the parser found no single recovery suffix; consumers must
    /// not rescan delimiters or literal text to invent one.
    pub fn recovery_suffix(&self) -> Option<&str> {
        self.recovery_suffix.as_deref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceIdentity {
    len: usize,
    hash: u64,
}

impl SourceIdentity {
    fn of(source: &str) -> Self {
        // FNV-1a is stable across processes (unlike a randomized HashMap
        // hasher) and length protects the common same-prefix edit case.
        let hash = source
            .as_bytes()
            .iter()
            .fold(0xcbf29ce484222325, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
            });
        Self {
            len: source.len(),
            hash,
        }
    }
}

/// Parse a source prefix for syntax completion.
///
/// The result is intentionally conservative. In particular, `Incomplete`
/// means a known suffix may repair the prefix; it does not promise all parser
/// productions have been modeled here. `Dead` is reserved for lexical errors,
/// crossed delimiters, retired source spellings, and parser-dispatch failures
/// for which appending cannot change the already-read terminal.
pub fn parse_source_prefix(file: &str, source: &str) -> PrefixParseResult {
    let replace_range = trailing_replace_range(source);
    let tokens = lex(file, source);

    let surface = scan_surface(source);
    let current_terminal_completeness = terminal_completeness(&surface, &replace_range);
    let significant: Vec<_> = tokens
        .iter()
        .filter(|token| !matches!(token.kind, TokenKind::Eof))
        .collect();
    let cursor = if surface.dead_range.is_some() {
        unknown_cursor_context(source.len())
    } else {
        cursor_context(source.len(), &significant)
    };
    if let Some(range) = surface.dead_range {
        return dead(source, range, current_terminal_completeness, cursor);
    }
    if let Some((kind, range)) = surface.partial_literal {
        return incomplete_with_context(
            source,
            range,
            vec![ExpectedTerminal::Literal {
                kind,
                completeness: TerminalCompleteness::Partial,
            }],
            current_terminal_completeness,
            TerminalCompleteness::Partial,
            cursor,
            None,
        );
    }
    if has_retired_top_level_declaration(&significant) {
        // Retired spellings are only dead when their placement and following
        // terminals establish an old top-level declaration. They remain valid
        // identifiers in bodies and named call arguments.
        return dead(source, replace_range, current_terminal_completeness, cursor);
    }

    let parsed = parse_source_raw(file, source);

    // At top level the parser dispatch is closed. If the first terminal is not
    // a prefix of a legal starter, no appended text can make it legal.
    if surface.unclosed.is_empty()
        && starts_at_top_level(&significant)
        && significant
            .first()
            .and_then(|token| ident_text(token))
            .is_some_and(|text| {
                !TOP_LEVEL_STARTERS
                    .iter()
                    .any(|starter| starter.text().starts_with(text))
            })
    {
        return dead(source, replace_range, current_terminal_completeness, cursor);
    }

    let oracle = parse_prefix_oracle(file, source);
    if let Some(close) = surface.unclosed.last().copied()
        && (!matches!(close, Delimiter::Paren)
            || matches!(
                oracle.as_ref().map(|oracle| oracle.site),
                Some(ParserExpectationSite::Unknown | ParserExpectationSite::TopLevel)
            ))
    {
        return incomplete_with_context(
            source,
            source.len()..source.len(),
            vec![fixed(close.expected_close())],
            current_terminal_completeness,
            TerminalCompleteness::Partial,
            cursor,
            Some(surface_recovery_suffix(&surface)),
        );
    }

    if let Some(oracle) = oracle
        && (program_has_recovery_marker(&parsed)
            || source.is_empty()
            || !matches!(oracle.site, ParserExpectationSite::TopLevel))
    {
        let recovery_suffix = recovery_suffix_from_parser_terminals(&oracle.expected);
        return incomplete_with_context(
            source,
            replace_range,
            oracle.expected.into_iter().map(parser_terminal).collect(),
            current_terminal_completeness,
            // Parser failpoints are exact only for the productions explicitly
            // instrumented above. The top-level catalog and delimiter recovery
            // remain intentionally open-ended for editor callers.
            if oracle.instrumented && !matches!(oracle.site, ParserExpectationSite::TopLevel) {
                TerminalCompleteness::Complete
            } else {
                TerminalCompleteness::Partial
            },
            cursor,
            recovery_suffix,
        );
    }

    if let Some(close) = surface.unclosed.last().copied() {
        return incomplete_with_context(
            source,
            source.len()..source.len(),
            vec![fixed(close.expected_close())],
            current_terminal_completeness,
            TerminalCompleteness::Partial,
            cursor,
            Some(surface_recovery_suffix(&surface)),
        );
    }

    if !program_has_recovery_marker(&parsed) {
        return PrefixParseResult {
            state: PrefixParseState::Complete,
            current_terminal_completeness,
            expected_terminals_completeness: TerminalCompleteness::Complete,
            expected_terminals: Vec::new(),
            replace_range,
            cursor,
            source_identity: SourceIdentity::of(source),
            recovery_suffix: None,
        };
    }

    // A malformed declaration may still be completed (the ordinary parser is
    // intentionally recovery-oriented), so do not overstate it as dead.
    incomplete_with_context(
        source,
        replace_range,
        Vec::new(),
        current_terminal_completeness,
        TerminalCompleteness::Partial,
        cursor,
        None,
    )
}

fn parser_terminal(terminal: ParserExpectationTerminal) -> ExpectedTerminal {
    match terminal {
        ParserExpectationTerminal::Fixed(text) => fixed(text),
        ParserExpectationTerminal::Identifier(role) => identifier(match role {
            ParserIdentifierRole::Function => IdentifierRole::FunctionName,
            ParserIdentifierRole::Parameter => IdentifierRole::ParameterName,
            ParserIdentifierRole::Type => IdentifierRole::TypeName,
        }),
    }
}

fn recovery_suffix_from_parser_terminals(
    terminals: &[ParserExpectationTerminal],
) -> Option<String> {
    let fixed: Vec<_> = terminals
        .iter()
        .filter_map(|terminal| match terminal {
            ParserExpectationTerminal::Fixed(text) => Some(*text),
            ParserExpectationTerminal::Identifier(_) => None,
        })
        .collect();
    (fixed.len() == 1).then(|| fixed[0].to_owned())
}

fn surface_recovery_suffix(surface: &SurfaceScan) -> String {
    surface
        .unclosed
        .iter()
        .rev()
        .map(|delimiter| delimiter.expected_close())
        .collect()
}

/// The parser deliberately preserves malformed syntax in a mostly usable AST
/// so diagnostics and editor features can still inspect its valid neighbours.
/// A prefix oracle must not confuse that recovery with a complete program.
///
/// Keep this structural rather than source-text based: the parser owns the
/// recovery markers, and this traversal follows every AST edge on which one
/// can be nested. New recovery fields should be added here with the owning AST
/// variant, making the conservative `Complete` contract easy to audit.
fn program_has_recovery_marker(program: &crate::ast::Program) -> bool {
    !program.unknown_top_level_spans.is_empty()
        || !program.malformed_declaration_spans.is_empty()
        || program.items.iter().any(item_has_recovery_marker)
}

fn item_has_recovery_marker(item: &Item) -> bool {
    match item {
        Item::Module(_) | Item::Use(_) => false,
        Item::Type(decl) => type_decl_has_recovery_marker(decl),
        Item::SumType(decl) => sum_type_decl_has_recovery_marker(decl),
        Item::TypeAlias(decl) => type_alias_has_recovery_marker(decl),
        Item::Const(decl) => const_decl_has_recovery_marker(decl),
        Item::Function(decl) => function_decl_has_recovery_marker(decl),
    }
}

fn type_decl_has_recovery_marker(decl: &TypeDecl) -> bool {
    !decl.malformed_generic_param_spans.is_empty()
        || !decl.malformed_field_spans.is_empty()
        || decl.fields.iter().any(field_has_recovery_marker)
        || decl
            .drop_body
            .as_ref()
            .is_some_and(block_has_recovery_marker)
}

fn sum_type_decl_has_recovery_marker(decl: &SumTypeDecl) -> bool {
    decl.variants
        .iter()
        .any(|variant| variant.fields.iter().any(field_has_recovery_marker))
}

fn type_alias_has_recovery_marker(decl: &TypeAliasDecl) -> bool {
    type_ref_has_recovery_marker(&decl.target)
}

fn const_decl_has_recovery_marker(decl: &ConstDecl) -> bool {
    decl.type_annotation
        .as_ref()
        .is_some_and(type_ref_has_recovery_marker)
        || expr_has_recovery_marker(&decl.value)
}

fn function_decl_has_recovery_marker(decl: &FunctionDecl) -> bool {
    !decl.malformed_generic_param_spans.is_empty()
        || !decl.malformed_param_spans.is_empty()
        || decl.params.iter().any(|param| {
            type_ref_has_recovery_marker(&param.ty)
                || param.default.as_ref().is_some_and(expr_has_recovery_marker)
        })
        || decl
            .return_ty
            .as_ref()
            .is_some_and(type_ref_has_recovery_marker)
        || block_has_recovery_marker(&decl.body)
}

fn field_has_recovery_marker(field: &FieldDecl) -> bool {
    type_ref_has_recovery_marker(&field.ty)
        || field.default.as_ref().is_some_and(expr_has_recovery_marker)
}

fn type_ref_has_recovery_marker(ty: &TypeRef) -> bool {
    !ty.malformed_arg_spans.is_empty()
        || ty.args.iter().any(type_ref_has_recovery_marker)
        || ty.fn_params.iter().any(type_ref_has_recovery_marker)
        || ty
            .fn_return
            .as_deref()
            .is_some_and(type_ref_has_recovery_marker)
}

fn block_has_recovery_marker(block: &Block) -> bool {
    block.statements.iter().any(stmt_has_recovery_marker)
}

fn stmt_has_recovery_marker(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::MalformedWith(_)
        | Stmt::MalformedIf(_)
        | Stmt::MalformedLoop(_)
        | Stmt::MalformedFor(_)
        | Stmt::MalformedMatch(_)
        | Stmt::Unknown(_) => true,
        Stmt::Let(let_stmt) => let_stmt_has_recovery_marker(let_stmt),
        Stmt::Return(return_stmt) => return_stmt
            .value
            .as_ref()
            .is_some_and(expr_has_recovery_marker),
        Stmt::With(with_stmt) => {
            expr_has_recovery_marker(&with_stmt.resource)
                || block_has_recovery_marker(&with_stmt.body)
        }
        Stmt::If(if_stmt) => {
            expr_has_recovery_marker(&if_stmt.condition)
                || block_has_recovery_marker(&if_stmt.then_body)
                || if_stmt
                    .else_body
                    .as_ref()
                    .is_some_and(block_has_recovery_marker)
        }
        Stmt::Loop(loop_stmt) => {
            loop_stmt
                .condition
                .as_ref()
                .is_some_and(expr_has_recovery_marker)
                || block_has_recovery_marker(&loop_stmt.body)
        }
        Stmt::For(for_stmt) => {
            expr_has_recovery_marker(&for_stmt.iterable)
                || block_has_recovery_marker(&for_stmt.body)
        }
        Stmt::Match(match_stmt) => match_stmt_has_recovery_marker(match_stmt),
        Stmt::TaskGroup(task_group) => block_has_recovery_marker(&task_group.body),
        Stmt::Select(select) => select.arms.iter().any(|arm| {
            expr_has_recovery_marker(&arm.operation) || block_has_recovery_marker(&arm.body)
        }),
        Stmt::LetElse(let_else) => {
            expr_has_recovery_marker(&let_else.value)
                || block_has_recovery_marker(&let_else.else_body)
        }
        Stmt::Assign(assign) => {
            expr_has_recovery_marker(&assign.target) || expr_has_recovery_marker(&assign.value)
        }
        Stmt::Expr(expr) => expr_has_recovery_marker(expr),
        Stmt::Break(_) | Stmt::Continue(_) => false,
    }
}

fn let_stmt_has_recovery_marker(stmt: &LetStmt) -> bool {
    stmt.malformed
        || stmt
            .type_annotation
            .as_ref()
            .is_some_and(type_ref_has_recovery_marker)
        || stmt.value.as_ref().is_some_and(expr_has_recovery_marker)
}

fn match_stmt_has_recovery_marker(stmt: &MatchStmt) -> bool {
    !stmt.malformed_arm_spans.is_empty()
        || expr_has_recovery_marker(&stmt.value)
        || stmt.arms.iter().any(match_arm_has_recovery_marker)
}

fn match_arm_has_recovery_marker(arm: &MatchArm) -> bool {
    arm.guard.as_ref().is_some_and(expr_has_recovery_marker) || block_has_recovery_marker(&arm.body)
}

fn expr_has_recovery_marker(expr: &Expr) -> bool {
    match expr {
        Expr::Unknown(_) => true,
        Expr::Ident(..)
        | Expr::Number(..)
        | Expr::String(..)
        | Expr::CharLiteral(..)
        | Expr::MultilineString(..) => false,
        Expr::ObjectLiteral { fields, .. } => fields
            .iter()
            .any(|field| expr_has_recovery_marker(&field.value)),
        Expr::MapLiteral { entries, .. } => entries.iter().any(|entry| {
            expr_has_recovery_marker(&entry.key) || expr_has_recovery_marker(&entry.value)
        }),
        Expr::ArrayLiteral { items, .. } => items.iter().any(expr_has_recovery_marker),
        Expr::Binary { left, right, .. } => {
            expr_has_recovery_marker(left) || expr_has_recovery_marker(right)
        }
        Expr::Field { base, .. }
        | Expr::Effect { value: base, .. }
        | Expr::Manage { value: base, .. }
        | Expr::Spawn { value: base, .. }
        | Expr::Await { value: base, .. }
        | Expr::Try { value: base, .. } => expr_has_recovery_marker(base),
        Expr::Index { base, index, .. } => {
            expr_has_recovery_marker(base) || expr_has_recovery_marker(index)
        }
        Expr::Call { callee, args, .. } => {
            callee_has_recovery_marker(callee)
                || args
                    .iter()
                    .any(|arg| arg.malformed || expr_has_recovery_marker(&arg.value))
        }
        Expr::Closure { body, .. } => block_has_recovery_marker(body),
        Expr::Match {
            value,
            arms,
            malformed_arm_spans,
            ..
        } => {
            !malformed_arm_spans.is_empty()
                || expr_has_recovery_marker(value)
                || arms.iter().any(match_arm_has_recovery_marker)
        }
    }
}

fn callee_has_recovery_marker(callee: &Callee) -> bool {
    match callee {
        Callee::Name(_) | Callee::Qualified { .. } => false,
        Callee::ReceiverCall { receiver, .. } => expr_has_recovery_marker(receiver),
    }
}

fn starts_at_top_level(tokens: &[&Token]) -> bool {
    !tokens.is_empty()
}

fn fixed(text: &'static str) -> ExpectedTerminal {
    ExpectedTerminal::Fixed {
        text,
        completeness: TerminalCompleteness::Complete,
    }
}

fn identifier(role: IdentifierRole) -> ExpectedTerminal {
    ExpectedTerminal::Identifier {
        role,
        completeness: TerminalCompleteness::Complete,
    }
}

fn incomplete_with_context(
    source: &str,
    range: Range<usize>,
    expected_terminals: Vec<ExpectedTerminal>,
    current_terminal_completeness: TerminalCompleteness,
    expected_terminals_completeness: TerminalCompleteness,
    cursor: CursorContext,
    recovery_suffix: Option<String>,
) -> PrefixParseResult {
    PrefixParseResult {
        state: PrefixParseState::Incomplete,
        current_terminal_completeness,
        expected_terminals_completeness,
        expected_terminals,
        replace_range: range,
        cursor,
        source_identity: SourceIdentity::of(source),
        recovery_suffix,
    }
}

fn dead(
    source: &str,
    range: Range<usize>,
    current_terminal_completeness: TerminalCompleteness,
    cursor: CursorContext,
) -> PrefixParseResult {
    PrefixParseResult {
        state: PrefixParseState::Dead,
        current_terminal_completeness,
        expected_terminals_completeness: TerminalCompleteness::Complete,
        expected_terminals: Vec::new(),
        replace_range: range,
        cursor,
        source_identity: SourceIdentity::of(source),
        recovery_suffix: None,
    }
}

fn ident_text(token: &Token) -> Option<&str> {
    match &token.kind {
        TokenKind::Ident(text) => Some(text),
        TokenKind::Keyword(text) => Some(text),
        _ => None,
    }
}

fn terminal_completeness(
    surface: &SurfaceScan,
    replace_range: &Range<usize>,
) -> TerminalCompleteness {
    if surface.partial_literal.is_some() || !replace_range.is_empty() {
        // An identifier-like suffix might still be growing. Completion must not
        // reinterpret it as a parser-validated terminal.
        TerminalCompleteness::Partial
    } else {
        TerminalCompleteness::Complete
    }
}

/// Recognize only the historical declaration shapes, and only where a new
/// top-level item can begin. In particular, these spellings remain ordinary
/// field names, bindings, and local identifiers.
fn has_retired_top_level_declaration(tokens: &[&Token]) -> bool {
    let mut delimiter_depth = 0usize;
    let mut item_start = true;

    for (index, token) in tokens.iter().enumerate() {
        if delimiter_depth == 0 && item_start && is_retired_declaration_start(tokens, index) {
            return true;
        }

        if token.symbol("(") || token.symbol("[") || token.symbol("{") {
            delimiter_depth += 1;
            item_start = false;
        } else if token.symbol(")") || token.symbol("]") || token.symbol("}") {
            delimiter_depth = delimiter_depth.saturating_sub(1);
            // A closing top-level item body is a reliable boundary for the
            // next declaration. Other malformed constructs stay conservative.
            item_start = delimiter_depth == 0 && token.symbol("}");
        } else if delimiter_depth == 0 {
            item_start = false;
        }
    }
    false
}

fn is_retired_declaration_start(tokens: &[&Token], index: usize) -> bool {
    let Some(token) = tokens.get(index) else {
        return false;
    };
    let next = tokens.get(index + 1);
    match ident_text(token) {
        // The braces make this specifically the removed file-header form,
        // rather than a name occurring in another grammar production.
        Some("features") => next.is_some_and(|token| token.symbol("{")),
        // `native` used to be a source-level function modifier.
        Some("native") => next.is_some_and(|token| token.is_ident_text("fn")),
        // Profiles were top-level named declarations, e.g. `profile debug`.
        Some("profile") => next.and_then(|token| ident_text(token)).is_some(),
        _ => false,
    }
}

#[derive(Debug, Clone)]
enum CursorDelimiter {
    Paren(Option<CallContext>),
    Other,
}

fn unknown_cursor_context(byte_offset: usize) -> CursorContext {
    CursorContext {
        byte_offset,
        site: SyntaxSite::Unknown,
        function: None,
        call: None,
    }
}

fn cursor_context(byte_offset: usize, tokens: &[&Token]) -> CursorContext {
    let mut delimiters = Vec::new();
    let mut brace_depth = 0usize;
    let mut functions: Vec<(usize, FunctionContext)> = Vec::new();
    let mut pending_function: Option<FunctionContext> = None;
    let mut malformed = false;

    for (index, token) in tokens.iter().enumerate() {
        if token.is_ident_text("fn") {
            pending_function = Some(FunctionContext { name: None });
            continue;
        }
        if let Some(function) = &mut pending_function
            && function.name.is_none()
            && let Some(name) = ident_text(token)
        {
            function.name = Some(name.to_owned());
        }

        if token.symbol(",") {
            if let Some(CursorDelimiter::Paren(Some(call))) = delimiters.last_mut() {
                let Some(argument_index) = call.argument_index.as_mut() else {
                    malformed = true;
                    continue;
                };
                *argument_index += 1;
            }
            continue;
        }

        if token.symbol("(") {
            let call = if pending_function.is_some() {
                None
            } else {
                call_callee(tokens, index).map(|callee| CallContext {
                    callee: Some(callee),
                    argument_index: Some(0),
                })
            };
            delimiters.push(CursorDelimiter::Paren(call));
            continue;
        }
        if token.symbol("[") {
            delimiters.push(CursorDelimiter::Other);
            continue;
        }
        if token.symbol("{") {
            delimiters.push(CursorDelimiter::Other);
            brace_depth += 1;
            if let Some(function) = pending_function.take() {
                if function.name.is_some() {
                    functions.push((brace_depth, function));
                } else {
                    malformed = true;
                }
            }
            continue;
        }
        if token.symbol(")") {
            if matches!(delimiters.pop(), Some(CursorDelimiter::Paren(_))) {
                continue;
            }
            malformed = true;
            continue;
        }
        if token.symbol("]") {
            if matches!(delimiters.pop(), Some(CursorDelimiter::Other)) {
                continue;
            }
            malformed = true;
            continue;
        }
        if token.symbol("}") {
            if !matches!(delimiters.pop(), Some(CursorDelimiter::Other)) || brace_depth == 0 {
                malformed = true;
                continue;
            }
            brace_depth -= 1;
            while functions
                .last()
                .is_some_and(|(function_depth, _)| *function_depth > brace_depth)
            {
                functions.pop();
            }
        }
    }

    if malformed {
        return unknown_cursor_context(byte_offset);
    }

    let in_function_header = pending_function.is_some();
    let function = functions
        .last()
        .map(|(_, function)| function.clone())
        .or(pending_function);
    let call = delimiters
        .iter()
        .rev()
        .find_map(|delimiter| match delimiter {
            CursorDelimiter::Paren(Some(call)) => Some(call.clone()),
            CursorDelimiter::Paren(None) | CursorDelimiter::Other => None,
        });
    let site = if call.is_some() {
        SyntaxSite::CallArguments
    } else if in_function_header {
        SyntaxSite::FunctionHeader
    } else if function.is_some() {
        SyntaxSite::FunctionBody
    } else if delimiters.is_empty() {
        SyntaxSite::TopLevel
    } else {
        SyntaxSite::Unknown
    };

    CursorContext {
        byte_offset,
        site,
        function,
        call,
    }
}

fn call_callee(tokens: &[&Token], open_index: usize) -> Option<String> {
    let mut index = open_index.checked_sub(1)?;
    let mut parts = vec![ident_text(tokens.get(index)?)?.to_owned()];
    if matches!(parts[0].as_str(), "fn" | "if" | "for" | "while" | "match") {
        return None;
    }
    while index >= 2 && tokens[index - 1].symbol(".") {
        let namespace = ident_text(tokens[index - 2])?;
        parts.push(namespace.to_owned());
        index -= 2;
    }
    parts.reverse();
    Some(parts.join("."))
}

fn trailing_replace_range(source: &str) -> Range<usize> {
    let end = source.len();
    let mut start = end;
    for (index, ch) in source.char_indices().rev() {
        if ch == '_' || ch.is_ascii_alphanumeric() {
            start = index;
        } else {
            break;
        }
    }
    start..end
}

#[derive(Debug, Clone, Copy)]
enum Delimiter {
    Paren,
    Bracket,
    Brace,
}

impl Delimiter {
    fn from_open(ch: char) -> Option<Self> {
        match ch {
            '(' => Some(Self::Paren),
            '[' => Some(Self::Bracket),
            '{' => Some(Self::Brace),
            _ => None,
        }
    }

    fn matches(self, close: char) -> bool {
        matches!(
            (self, close),
            (Self::Paren, ')') | (Self::Bracket, ']') | (Self::Brace, '}')
        )
    }

    fn expected_close(self) -> &'static str {
        match self {
            Self::Paren => ")",
            Self::Bracket => "]",
            Self::Brace => "}",
        }
    }
}

struct SurfaceScan {
    dead_range: Option<Range<usize>>,
    partial_literal: Option<(LiteralKind, Range<usize>)>,
    unclosed: Vec<Delimiter>,
}

/// Lex only the boundary facts the ordinary lexer intentionally discards: raw
/// byte positions, unfinished literals, and a single nesting stack.
fn scan_surface(source: &str) -> SurfaceScan {
    let mut scan = SurfaceScan {
        dead_range: None,
        partial_literal: None,
        unclosed: Vec::new(),
    };
    let mut index = 0;
    while index < source.len() {
        let rest = &source[index..];
        if rest.starts_with("//") {
            index += rest.find('\n').unwrap_or(rest.len());
            continue;
        }
        if rest.starts_with("\"\"\"") {
            let start = index;
            index += 3;
            if let Some(end) = source[index..].find("\"\"\"") {
                index += end + 3;
                continue;
            }
            scan.partial_literal = Some((LiteralKind::MultilineString, start..source.len()));
            break;
        }
        if rest.starts_with("$\"") {
            let start = index;
            match quoted_end(source, index + 1, '"', true) {
                Some(end) => {
                    index = end;
                    continue;
                }
                None => {
                    scan.partial_literal =
                        Some((LiteralKind::InterpolatedString, start..source.len()));
                    break;
                }
            }
        }
        if rest.starts_with('"') {
            let start = index;
            match quoted_end(source, index, '"', true) {
                Some(end) => {
                    index = end;
                    continue;
                }
                None => {
                    scan.partial_literal = Some((LiteralKind::String, start..source.len()));
                    break;
                }
            }
        }
        if rest.starts_with('\'') {
            let start = index;
            match quoted_end(source, index, '\'', false) {
                Some(end) => {
                    index = end;
                    continue;
                }
                None => {
                    scan.partial_literal = Some((LiteralKind::Char, start..source.len()));
                    break;
                }
            }
        }

        let ch = rest.chars().next().expect("index is in bounds");
        if let Some(open) = Delimiter::from_open(ch) {
            scan.unclosed.push(open);
        } else if matches!(ch, ')' | ']' | '}') {
            if !scan.unclosed.last().is_some_and(|open| open.matches(ch)) {
                scan.dead_range = Some(index..index + ch.len_utf8());
                break;
            }
            scan.unclosed.pop();
        } else if !is_lexical_character(ch) {
            scan.dead_range = Some(index..index + ch.len_utf8());
            break;
        }
        index += ch.len_utf8();
    }
    scan
}

fn is_lexical_character(ch: char) -> bool {
    ch.is_whitespace()
        || ch == '_'
        || ch.is_ascii_alphanumeric()
        || matches!(
            ch,
            ':' | ','
                | '.'
                | '<'
                | '>'
                | '?'
                | '|'
                | '&'
                | '~'
                | '^'
                | '+'
                | '-'
                | '*'
                | '/'
                | '%'
                | '='
                | '!'
                | ';'
                | '#'
        )
}

fn quoted_end(source: &str, opening: usize, quote: char, allow_newline: bool) -> Option<usize> {
    let mut index = opening + quote.len_utf8();
    while index < source.len() {
        let ch = source[index..].chars().next()?;
        if !allow_newline && ch == '\n' {
            return None;
        }
        if ch == '\\' {
            index += ch.len_utf8();
            if index < source.len() {
                index += source[index..].chars().next()?.len_utf8();
            }
            continue;
        }
        index += ch.len_utf8();
        if ch == quote {
            return Some(index);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(source: &str) -> PrefixParseState {
        parse_source_prefix("prefix.rss", source).state
    }

    #[test]
    fn empty_prefix_offers_curated_item_starters() {
        let result = parse_source_prefix("prefix.rss", "");
        assert_eq!(result.state, PrefixParseState::Incomplete);
        assert_eq!(
            result.expected_terminals_completeness,
            TerminalCompleteness::Partial
        );
        assert!(result.expected_terminals.contains(&fixed("fn")));
        assert!(!result.expected_terminals.contains(&fixed("native")));
        assert!(!result.expected_terminals.contains(&fixed("features")));
    }

    #[test]
    fn every_curated_item_starter_is_parser_recognized() {
        for starter in TOP_LEVEL_STARTERS {
            let program = parse_source_raw("prefix.rss", starter.text());
            assert!(
                program.unknown_top_level_spans.is_empty(),
                "{starter:?} is offered by prefix completion but not recognized by parser dispatch"
            );
        }
    }

    #[test]
    fn unfinished_literals_are_partial_and_byte_aligned() {
        for (source, kind) in [
            ("\"hello", LiteralKind::String),
            ("'x", LiteralKind::Char),
            ("$\"hello", LiteralKind::InterpolatedString),
            ("\"\"\"hello", LiteralKind::MultilineString),
        ] {
            let result = parse_source_prefix("prefix.rss", source);
            assert_eq!(result.state, PrefixParseState::Incomplete);
            assert_eq!(result.replace_range, 0..source.len());
            assert_eq!(
                result.expected_terminals,
                vec![ExpectedTerminal::Literal {
                    kind,
                    completeness: TerminalCompleteness::Partial,
                }]
            );
        }
        let result = parse_source_prefix("prefix.rss", "fn café");
        assert!("fn café".is_char_boundary(result.replace_range.start));
        assert!("fn café".is_char_boundary(result.replace_range.end));
    }

    #[test]
    fn delimiter_stack_is_nested_and_rejects_crossing_closers() {
        let result = parse_source_prefix("prefix.rss", "fn f([{");
        assert_eq!(result.state, PrefixParseState::Incomplete);
        assert_eq!(result.expected_terminals, vec![fixed("}")]);
        assert_eq!(state("fn f([)]"), PrefixParseState::Dead);
        assert_eq!(state("fn f())"), PrefixParseState::Dead);
    }

    #[test]
    fn unmatched_delimiters_do_not_claim_exhaustive_completions() {
        for source in ["fn main() -> Unit {", "foo("] {
            let result = parse_source_prefix("prefix.rss", source);
            assert_eq!(result.state, PrefixParseState::Incomplete);
            assert_eq!(
                result.expected_terminals_completeness,
                TerminalCompleteness::Partial,
                "{source}"
            );
        }
    }

    #[test]
    fn complete_function_and_representative_valid_prefixes_are_complete() {
        for source in [
            "fn main() -> Int { return 0 }",
            "class Point { x: Int\ny: Int }",
            "protocol Render { fn render() -> Unit }",
        ] {
            assert_eq!(state(source), PrefixParseState::Complete, "{source}");
        }
    }

    #[test]
    fn malformed_function_bodies_do_not_report_complete() {
        for source in [
            "fn main() -> Unit { let = }",
            "fn main() -> Unit { return @ }",
        ] {
            assert_ne!(state(source), PrefixParseState::Complete, "{source}");
        }
    }

    #[test]
    fn parser_recovery_markers_are_incomplete_with_partial_expectations() {
        for source in [
            "fn main() -> Unit { let = }",
            "fn main() -> Unit { let value = outer(|a b| a) }",
            "fn main(,) -> Unit { }",
            "fn main(value: Result<, Int>) -> Unit { }",
            "fn main(value: Int) -> Unit { match value { Ok(item) { } } }",
        ] {
            let result = parse_source_prefix("prefix.rss", source);
            assert_eq!(result.state, PrefixParseState::Incomplete, "{source}");
            assert_eq!(
                result.expected_terminals_completeness,
                TerminalCompleteness::Partial,
                "{source}"
            );
        }
    }

    #[test]
    fn header_expectations_are_structured() {
        let result = parse_source_prefix("prefix.rss", "fn");
        assert_eq!(result.state, PrefixParseState::Incomplete);
        assert_eq!(
            result.expected_terminals,
            vec![identifier(IdentifierRole::FunctionName)]
        );
        let result = parse_source_prefix("prefix.rss", "fn main(");
        assert!(
            result
                .expected_terminals
                .contains(&identifier(IdentifierRole::ParameterName))
        );
        let result = parse_source_prefix("prefix.rss", "fn main");
        assert_eq!(result.state, PrefixParseState::Incomplete);
        assert_eq!(result.expected_terminals, vec![fixed("(")]);
    }

    #[test]
    fn parser_failpoint_fixed_terminals_remain_appendable() {
        for (source, terminal) in [("fn main", "("), ("fn main(", ")")] {
            let result = parse_source_prefix("prefix.rss", source);
            assert!(
                result.expected_terminals.contains(&fixed(terminal)),
                "{source:?} did not expose parser failpoint {terminal:?}"
            );
            let mut completed = source.to_owned();
            completed.push_str(terminal);
            assert_ne!(
                state(&completed),
                PrefixParseState::Dead,
                "{terminal:?} must be accepted or advance {source:?}"
            );
        }
    }

    #[test]
    fn prefix_results_bind_to_the_exact_source_and_expose_owned_recovery() {
        let header = parse_source_prefix("prefix.rss", "fn main(");
        assert!(header.matches_source("fn main("));
        assert!(!header.matches_source("fn other"));
        assert_eq!(header.recovery_suffix(), Some(")"));

        let nested = parse_source_prefix("prefix.rss", "fn f([{");
        assert_eq!(nested.recovery_suffix(), Some("}])"));

        let complete = parse_source_prefix("prefix.rss", "fn main() -> Unit { }");
        assert!(complete.matches_source("fn main() -> Unit { }"));
        assert_eq!(complete.recovery_suffix(), None);
    }

    #[test]
    fn retired_syntax_and_illegal_characters_are_dead() {
        for source in [
            "features { }",
            "native fn main() {}",
            "profile debug",
            "fn f() { @ }",
        ] {
            assert_eq!(state(source), PrefixParseState::Dead, "{source}");
        }
    }

    #[test]
    fn fixed_prefixes_remain_recoverable_but_non_starters_are_dead() {
        assert_eq!(state("f"), PrefixParseState::Incomplete);
        assert_eq!(state("zz"), PrefixParseState::Dead);
    }

    #[test]
    fn every_utf8_prefix_of_a_valid_fixture_remains_appendable() {
        assert_all_char_prefixes_appendable(
            "fn main() -> Unit { let greeting = \"hé\"\nreturn greeting }",
        );
    }

    #[test]
    fn retired_words_remain_appendable_as_valid_names() {
        // This is an SDK pass fixture, not a hand-written approximation: its
        // `features:` argument was the regression that revealed the old global
        // spelling check.
        assert_all_char_prefixes_appendable(include_str!(
            "../../rsscript-sdk/tests/fixtures/pass/features_field_name.rss"
        ));
        assert_all_char_prefixes_appendable(
            "fn main() -> Int {\n    let native = 1\n    return native\n}",
        );
    }

    #[test]
    fn result_exposes_conservative_cursor_context_and_terminal_completeness() {
        let source = "fn load() -> Unit { Settings(first: 1, features";
        let result = parse_source_prefix("prefix.rss", source);
        assert_eq!(
            result.current_terminal_completeness,
            TerminalCompleteness::Partial
        );
        assert_eq!(
            result.expected_terminals_completeness,
            TerminalCompleteness::Partial
        );
        assert_eq!(result.cursor.byte_offset, source.len());
        assert_eq!(result.cursor.site, SyntaxSite::CallArguments);
        assert_eq!(
            result.cursor.function,
            Some(FunctionContext {
                name: Some("load".to_owned()),
            })
        );
        assert_eq!(
            result.cursor.call,
            Some(CallContext {
                callee: Some("Settings".to_owned()),
                argument_index: Some(1),
            })
        );

        let complete = parse_source_prefix("prefix.rss", "fn main()");
        assert_eq!(
            complete.current_terminal_completeness,
            TerminalCompleteness::Complete
        );
        assert_eq!(
            complete.expected_terminals_completeness,
            TerminalCompleteness::Complete
        );
    }

    fn assert_all_char_prefixes_appendable(source: &str) {
        for end in source
            .char_indices()
            .map(|(index, _)| index)
            .chain(std::iter::once(source.len()))
        {
            let prefix = &source[..end];
            let result = parse_source_prefix("prefix.rss", prefix);
            assert_ne!(result.state, PrefixParseState::Dead, "{prefix:?}");
            assert!(prefix.is_char_boundary(result.replace_range.start));
            assert!(prefix.is_char_boundary(result.replace_range.end));
        }
    }
}
