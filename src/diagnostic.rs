use serde::{Deserialize, Serialize};

pub mod code {
    pub const RESERVED_DIAGNOSTIC_RS0001: &str = "RS0001";
    pub const MISSING_RETURN_TYPE: &str = "RS0002";
    pub const MISSING_PARAMETER_TYPE: &str = "RS0003";
    pub const UNKNOWN_EFFECT: &str = "RS0004";
    pub const DUPLICATE_DECLARATION: &str = "RS0005";
    pub const DUPLICATE_FEATURE_DECLARATION: &str = "RS0006";
    pub const UNKNOWN_RETAINED_PARAMETER: &str = "RS0007";
    pub const MISSING_PARAMETER_EFFECT: &str = "RS0008";
    pub const INVALID_PURE_EFFECT: &str = "RS0009";
    pub const REMOVED_PROFILE_DECLARATION: &str = "RS0010";
    pub const REMOVED_SHARE_EFFECT: &str = "RS0011";
    pub const REMOVED_RUNTIME_EFFECT: &str = "RS0012";
    pub const INVALID_TRY_OPERATOR: &str = "RS0013";
    pub const INVALID_NOALLOC_ALLOCATION: &str = "RS0014";
    pub const UNSUPPORTED_SYNTAX: &str = "RS0015";
    pub const UNKNOWN_FILE_FEATURE: &str = "RS0016";
    pub const DUPLICATE_FILE_FEATURE: &str = "RS0017";
    pub const INVALID_NO_BLOCK_CALL: &str = "RS0018";
    pub const INVALID_NO_PANIC_CALL: &str = "RS0019";
    pub const INVALID_NOALLOC_CALL: &str = "RS0020";
    pub const NON_EXHAUSTIVE_MATCH: &str = "RS0021";
    pub const ASYNC_CALL_NOT_CONSUMED: &str = "RS0022";
    pub const FD_OUTSIDE_INTERNAL_BOUNDARY: &str = "RS0023";
    pub const UNKNOWN_TYPE: &str = "RS0024";
    pub const UNKNOWN_FIELD: &str = "RS0025";
    pub const UNKNOWN_BINDING: &str = "RS0026";
    pub const UNKNOWN_PROTOCOL: &str = "RS0027";
    pub const INVALID_SELF_PARAMETER: &str = "RS0028";
    pub const AWAIT_OUTSIDE_ASYNC: &str = "RS0029";
    pub const AWAIT_NON_ASYNC: &str = "RS0030";
    pub const AWAIT_LIVE_LOCAL: &str = "RS0031";
    pub const PROTOCOL_NOT_SATISFIED: &str = "RS0032";
    pub const FEATURE_VIOLATION: &str = "RS0101";
    pub const UNNAMED_ARGUMENT: &str = "RS0201";
    pub const MISSING_DATA_EFFECT: &str = "RS0202";
    pub const UNKNOWN_ARGUMENT: &str = "RS0203";
    pub const MISSING_ARGUMENT: &str = "RS0204";
    pub const DUPLICATE_ARGUMENT: &str = "RS0205";
    pub const UNKNOWN_CALLEE: &str = "RS0206";
    pub const ARGUMENT_TYPE_MISMATCH: &str = "RS0207";
    pub const RETURN_TYPE_MISMATCH: &str = "RS0208";
    pub const CONTROL_FLOW_TYPE_MISMATCH: &str = "RS0209";
    pub const OPERATOR_TYPE_MISMATCH: &str = "RS0210";
    pub const MANAGED_TO_LOCAL: &str = "RS0301";
    pub const FIELD_PARTIAL_ACCESS_CONFLICT: &str = "RS0302";
    pub const FIELD_PREFIX_CONFLICT: &str = "RS0303";
    pub const INDEXED_PARTIAL_ACCESS_CONFLICT: &str = "RS0304";
    pub const MOVE_BASE_FIELD_CONFLICT: &str = "RS0305";
    pub const LOCAL_CLASS_BINDING: &str = "RS0306";
    pub const INVALID_MANAGE_OPERAND: &str = "RS0307";
    pub const INVALID_TAKE_OPERAND: &str = "RS0308";
    pub const MANAGED_FIELD_SPLIT_CONFLICT: &str = "RS0309";
    pub const READ_VIEW_MUTATION: &str = "RS0310";
    pub const USE_AFTER_MANAGE: &str = "RS0401";
    pub const LOCAL_VALUE_RETAINED: &str = "RS0501";
    pub const FRESH_RETURN_NOT_CLEAN: &str = "RS0601";
    pub const FRESHNESS_UNKNOWN: &str = "RS0602";
    pub const INVALID_FRESH_RETURN_TYPE: &str = "RS0603";
    pub const FRESH_REQUIRES_LOCAL_BINDING: &str = "RS0604";
    pub const RESOURCE_FIELD: &str = "RS0701";
    pub const RESOURCE_ESCAPE: &str = "RS0702";
    pub const INVALID_RESOURCE_POOL_TYPE: &str = "RS0703";
    pub const RESOURCE_GENERIC_ARGUMENT: &str = "RS0704";
    pub const RESOURCE_POOL_NOT_LOCAL: &str = "RS0705";
    pub const RESOURCE_PRODUCER_MISSING_TRY: &str = "RS0706";
    pub const RESOURCE_POOL_FALLIBLE_FACTORY: &str = "RS0707";
    pub const RESOURCE_POOL_INVALID_MAX_SIZE: &str = "RS0708";
    pub const RESOURCE_POOL_ACTIVE_LEASE_CONFLICT: &str = "RS0709";
    pub const LOCAL_CAPTURED_BY_MANAGED_CLOSURE: &str = "RS0801";
    pub const NOESCAPE_CALLBACK_ESCAPE: &str = "RS0802";
    pub const LOCAL_CLOSURE_ESCAPE: &str = "RS0803";
    pub const NOESCAPE_CONSUMES_CAPTURE: &str = "RS0804";
    pub const TAKE_HANDLE_FIELD: &str = "RS0901";
    pub const INVALID_WEAK_FIELD: &str = "RS0902";
    pub const WEAK_FIELD_REQUIRES_UPGRADE: &str = "RS0903";
    pub const WEAK_FIELD_REQUIRES_WEAK_HANDLE: &str = "RS0904";
    pub const OPERATOR_OVERLOAD_ATTEMPT: &str = "RS1001";
    pub const IMPLICIT_CONVERSION_ATTEMPT: &str = "RS1002";
    pub const OWN_STRUCT_ATTEMPT: &str = "RS1003";
    pub const SURFACE_REFERENCE_ATTEMPT: &str = "RS1004";
    pub const RUSTC_DIAGNOSTIC_MAPPED: &str = "RS1101";
    pub const RUSTC_DIAGNOSTIC_UNMAPPABLE: &str = "RS1102";
    pub const RUNTIME_DIAGNOSTIC: &str = "RS1201";
    pub const PACKAGE_INTERFACE_MISMATCH: &str = "RS1301";
    pub const LINT_SIGNATURE_COMPLEXITY: &str = "RSL001";
    pub const LINT_DUPLICATE_EFFECT: &str = "RSL002";

    pub const PACKAGE_FEATURE_RESOLUTION: &str = "PKG0101";
    pub const PACKAGE_UNSUPPORTED_DEPENDENCY_SOURCE: &str = "PKG0102";
    pub const PACKAGE_REVIEW_POLICY_VIOLATION: &str = "PKG0501";
    pub const PACKAGE_NATIVE_BINDING: &str = "PKG0601";
    pub const PACKAGE_PROVIDER_DECLARATION: &str = "PKG0901";

    pub const REVIEW_FEATURES_CHANGED: &str = "RSR001";
    pub const REVIEW_FUNCTION_REMOVED: &str = "RSR002";
    pub const REVIEW_FUNCTION_ADDED: &str = "RSR003";
    pub const REVIEW_PARAMS_CHANGED: &str = "RSR004";
    pub const REVIEW_RETURN_CHANGED: &str = "RSR005";
    pub const REVIEW_EFFECTS_CHANGED: &str = "RSR006";
    pub const REVIEW_TYPE_REMOVED: &str = "RSR007";
    pub const REVIEW_TYPE_ADDED: &str = "RSR008";
    pub const REVIEW_TYPE_KIND_CHANGED: &str = "RSR009";
    pub const REVIEW_TYPE_FIELDS_CHANGED: &str = "RSR010";
    pub const REVIEW_BOUNDARY_CHANGED: &str = "RSR011";
    pub const REVIEW_UNSAFE_ADDED: &str = "RSR012";
    pub const REVIEW_GUARANTEE_REMOVED: &str = "RSR013";
    pub const REVIEW_FUNCTION_KIND_CHANGED: &str = "RSR014";
    pub const REVIEW_NATIVE_ADDED: &str = "RSR015";
    pub const REVIEW_PROTOCOL_IMPL_CHANGED: &str = "RSR016";
    pub const REVIEW_PARALLEL_ADDED: &str = "RSR017";
    pub const REVIEW_SUM_TYPE_CHANGED: &str = "RSR018";
    pub const REVIEW_CONST_CHANGED: &str = "RSR019";
    pub const REVIEW_TYPE_ALIAS_CHANGED: &str = "RSR020";
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

impl Severity {
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error)
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Span {
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub length: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fix {
    pub kind: String,
    pub title: String,
    pub applicability: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub summary: String,
    pub span: Span,
    pub label: String,
    pub causes: Vec<String>,
    pub fixes: Vec<Fix>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagnosticExplanation {
    pub code: &'static str,
    pub title: &'static str,
    pub explanation: &'static str,
}

impl Diagnostic {
    pub fn error(
        code: &str,
        summary: impl Into<String>,
        span: Span,
        label: impl Into<String>,
    ) -> Self {
        Self {
            code: code.to_string(),
            severity: Severity::Error,
            summary: summary.into(),
            span,
            label: label.into(),
            causes: Vec::new(),
            fixes: Vec::new(),
        }
    }

    pub fn warning(
        code: &str,
        summary: impl Into<String>,
        span: Span,
        label: impl Into<String>,
    ) -> Self {
        Self {
            code: code.to_string(),
            severity: Severity::Warning,
            summary: summary.into(),
            span,
            label: label.into(),
            causes: Vec::new(),
            fixes: Vec::new(),
        }
    }

    pub fn with_cause(mut self, cause: impl Into<String>) -> Self {
        self.causes.push(cause.into());
        self
    }

    pub fn with_fix(
        mut self,
        kind: impl Into<String>,
        title: impl Into<String>,
        applicability: impl Into<String>,
    ) -> Self {
        self.fixes.push(Fix {
            kind: kind.into(),
            title: title.into(),
            applicability: applicability.into(),
        });
        self
    }
}

pub fn explain_diagnostic_code(code: &str) -> Option<&'static DiagnosticExplanation> {
    DIAGNOSTIC_EXPLANATIONS
        .iter()
        .find(|explanation| explanation.code == code)
}

pub fn format_diagnostic_explanation(explanation: &DiagnosticExplanation) -> String {
    format!(
        "{}: {}\n\n{}\n",
        explanation.code, explanation.title, explanation.explanation
    )
}

pub fn format_diagnostics_human(diagnostics: &[Diagnostic]) -> String {
    let mut output = String::new();
    for diagnostic in diagnostics {
        output.push_str(&format!(
            "{}[{}]: {}\n\n",
            diagnostic.severity.as_str(),
            diagnostic.code,
            diagnostic.summary
        ));
        output.push_str(&format!(
            "  {}:{}:{}\n    {}{}\n",
            diagnostic.span.file,
            diagnostic.span.line,
            diagnostic.span.column,
            " ".repeat(diagnostic.span.column.saturating_sub(1)),
            "^".repeat(diagnostic.span.length.max(1))
        ));
        if !diagnostic.label.is_empty() {
            output.push_str(&format!("    {}\n", diagnostic.label));
        }
        for cause in &diagnostic.causes {
            output.push_str(&format!("\n  note: {cause}\n"));
        }
        for fix in &diagnostic.fixes {
            output.push_str(&format!("  help: {}\n", fix.title));
        }
        output.push('\n');
    }
    output
}

static DIAGNOSTIC_EXPLANATIONS: &[DiagnosticExplanation] = &[
    DiagnosticExplanation {
        code: code::RESERVED_DIAGNOSTIC_RS0001,
        title: "reserved diagnostic",
        explanation: "RS0001 is reserved after RSScript migrated to `features:` declarations.",
    },
    DiagnosticExplanation {
        code: code::MISSING_RETURN_TYPE,
        title: "missing return type",
        explanation: "Function signatures must spell out their return type. The checker applies this review rule broadly so API contracts do not rely on inference.",
    },
    DiagnosticExplanation {
        code: code::MISSING_PARAMETER_TYPE,
        title: "missing parameter type",
        explanation: "Parameters must have explicit types so call effects, freshness, and resource rules can be checked against a stable signature.",
    },
    DiagnosticExplanation {
        code: code::UNKNOWN_EFFECT,
        title: "unknown effect",
        explanation: "The effect list contains an effect name outside the currently recognized RSScript v0.5 surface. Effects are review-visible semantic contracts, so unknown effect names are errors.",
    },
    DiagnosticExplanation {
        code: code::DUPLICATE_DECLARATION,
        title: "duplicate declaration",
        explanation: "Top-level type, constructor, and function names must be unique so symbol resolution cannot silently overwrite an earlier declaration.",
    },
    DiagnosticExplanation {
        code: code::DUPLICATE_FEATURE_DECLARATION,
        title: "duplicate features declaration",
        explanation: "RSScript files may declare at most one `features:` header. Multiple declarations make review capability boundaries ambiguous.",
    },
    DiagnosticExplanation {
        code: code::UNKNOWN_RETAINED_PARAMETER,
        title: "invalid retained parameter",
        explanation: "`effects(retains(param))` must name a non-Copy parameter declared by the function signature. Copy values have no managed retention boundary.",
    },
    DiagnosticExplanation {
        code: code::MISSING_PARAMETER_EFFECT,
        title: "missing parameter data effect",
        explanation: "Non-Copy parameters must declare `read`, `mut`, or `take` in the function signature so call effects are review-visible.",
    },
    DiagnosticExplanation {
        code: code::INVALID_PURE_EFFECT,
        title: "invalid pure effect",
        explanation: "`effects(pure)` is call-time observational purity: the function may read current inputs, including non-Copy managed values, but must not mutate, retain, consume local values, use `manage`, or call non-pure functions.",
    },
    DiagnosticExplanation {
        code: code::REMOVED_PROFILE_DECLARATION,
        title: "removed profile declaration",
        explanation: "RSScript v0.5 does not include `profile:` declarations. Use `features:` only for advanced file-level capabilities; omitted features means managed-only.",
    },
    DiagnosticExplanation {
        code: code::REMOVED_SHARE_EFFECT,
        title: "removed share data effect",
        explanation: "RSScript v0.5 does not include `share` as a data effect. Retention must be declared with `effects(retains(param))`.",
    },
    DiagnosticExplanation {
        code: code::REMOVED_RUNTIME_EFFECT,
        title: "removed runtime effect",
        explanation: "RSScript v0.5 uses reductive guarantees. Additive runtime effects such as `io`, `allocates`, `may_panic`, and `may_fail` are not valid effects.",
    },
    DiagnosticExplanation {
        code: code::INVALID_TRY_OPERATOR,
        title: "invalid try operator",
        explanation: "`?` may only be used inside functions that return a compatible `Result<T, E>` type.",
    },
    DiagnosticExplanation {
        code: code::UNSUPPORTED_SYNTAX,
        title: "unsupported syntax",
        explanation: "The frontend parser could not lower this source construct into the supported RSScript AST. The checker reports this before Rust lowering so unsupported source does not become generated Rust `todo!()` code.",
    },
    DiagnosticExplanation {
        code: code::INVALID_NOALLOC_ALLOCATION,
        title: "invalid noalloc allocation",
        explanation: "`effects(noalloc)` forbids obvious allocation sites such as value construction and `manage` migration.",
    },
    DiagnosticExplanation {
        code: code::UNKNOWN_FILE_FEATURE,
        title: "unknown file feature",
        explanation: "A `features:` header may only list review-relevant capabilities known to this compiler version. Some known names are reserved review markers that do not unlock executable syntax. Unknown feature names are rejected so typos do not silently change review risk.",
    },
    DiagnosticExplanation {
        code: code::DUPLICATE_FILE_FEATURE,
        title: "duplicate file feature",
        explanation: "Each review-relevant capability may appear at most once in a `features:` header. Duplicate entries are rejected instead of silently folded away.",
    },
    DiagnosticExplanation {
        code: code::INVALID_NO_BLOCK_CALL,
        title: "invalid no_block call",
        explanation: "`effects(no_block)` is a guarantee that the function does not block the current isolate. Calls inside it must target constructors, enum variants, or functions also declared `effects(no_block)`.",
    },
    DiagnosticExplanation {
        code: code::INVALID_NO_PANIC_CALL,
        title: "invalid no_panic call",
        explanation: "`effects(no_panic)` is a guarantee that the function does not intentionally panic. Calls inside it must target constructors, enum variants, or functions also declared `effects(no_panic)`.",
    },
    DiagnosticExplanation {
        code: code::INVALID_NOALLOC_CALL,
        title: "invalid noalloc call",
        explanation: "`effects(noalloc)` is a guarantee that the function performs no heap allocation. Calls inside it must target enum variants or functions also declared `effects(noalloc)`, while direct constructors and `manage` are allocation diagnostics.",
    },
    DiagnosticExplanation {
        code: code::NON_EXHAUSTIVE_MATCH,
        title: "non-exhaustive match",
        explanation: "`match` must cover every visible variant for the supported enum shapes. In v0.5 that means `Some`/`None`, `Ok`/`Err`, all variants of a declared sum type, or a `_` fallback.",
    },
    DiagnosticExplanation {
        code: code::ASYNC_CALL_NOT_CONSUMED,
        title: "async call not consumed",
        explanation: "Async calls must be consumed by `await` so suspension boundaries remain review-visible. `spawn` is reserved but not executable in v0.5.",
    },
    DiagnosticExplanation {
        code: code::AWAIT_OUTSIDE_ASYNC,
        title: "await outside async function",
        explanation: "`await` is an explicit suspension boundary and is only valid inside an `async fn` body.",
    },
    DiagnosticExplanation {
        code: code::AWAIT_NON_ASYNC,
        title: "await non-async expression",
        explanation: "`await` must directly consume an async call in the executable async MVP; RSScript does not expose Future or Task values as source-level types.",
    },
    DiagnosticExplanation {
        code: code::AWAIT_LIVE_LOCAL,
        title: "local value live across await",
        explanation: "RSScript async frames may suspend managed handles and Copy snapshots, but local values, resources, and runtime guards must not live across an await boundary.",
    },
    DiagnosticExplanation {
        code: code::FD_OUTSIDE_INTERNAL_BOUNDARY,
        title: "Fd outside native/resource internals",
        explanation: "`Fd` is not an ordinary user-facing value type in v0.5. It may appear only in native function signatures or resource internals such as resource fields.",
    },
    DiagnosticExplanation {
        code: code::UNKNOWN_TYPE,
        title: "unknown type",
        explanation: "Every type used in a signature or field must be a built-in type, a core/runtime type known to the frontend, a generic parameter in scope, or a declared source/interface type. Unknown types are rejected before Rust lowering.",
    },
    DiagnosticExplanation {
        code: code::UNKNOWN_FIELD,
        title: "unknown field",
        explanation: "Field accesses must resolve against the RSScript type known for the base expression. Unknown fields are rejected by the frontend instead of being deferred to generated Rust diagnostics.",
    },
    DiagnosticExplanation {
        code: code::UNKNOWN_BINDING,
        title: "unknown binding",
        explanation: "Value identifiers must resolve to a visible parameter, local binding, with-bound resource, or pattern binding before Rust lowering.",
    },
    DiagnosticExplanation {
        code: code::UNKNOWN_PROTOCOL,
        title: "unknown protocol",
        explanation: "Protocol bounds and protocol implementations must name a declared RSScript protocol. Protocols are nominal capability contracts, not structural matches inferred from function names.",
    },
    DiagnosticExplanation {
        code: code::INVALID_SELF_PARAMETER,
        title: "invalid self parameter",
        explanation: "`self` is reserved for explicit method/protocol receiver contracts. It may only appear as the first parameter of a qualified method signature, and protocol methods must declare `self: read|mut|take Self` first.",
    },
    DiagnosticExplanation {
        code: code::PROTOCOL_NOT_SATISFIED,
        title: "protocol not satisfied",
        explanation: "A `Protocol.method(...)` call must prove that the receiver type satisfies the protocol through an explicit generic bound or an explicit `impl Protocol for Type` declaration. Protocols are nominal capability contracts, not structural matches inferred from method names.",
    },
    DiagnosticExplanation {
        code: code::FEATURE_VIOLATION,
        title: "feature violation",
        explanation: "Files must declare review-relevant capabilities before using them. `local`, `manage`, `take`, `ResourcePool<T>`, `native`, `unsafe`, and `async` boundaries require matching `features:` entries, and `native fn` declarations must also spell the boundary as `effects(native)`.",
    },
    DiagnosticExplanation {
        code: code::UNNAMED_ARGUMENT,
        title: "unnamed argument",
        explanation: "RSScript requires named call arguments so signature and review diffs remain readable.",
    },
    DiagnosticExplanation {
        code: code::MISSING_DATA_EFFECT,
        title: "missing call-site data effect",
        explanation: "Arguments for non-Copy parameters must use an explicit `read`, `mut`, or `take` effect matching the callee signature.",
    },
    DiagnosticExplanation {
        code: code::UNKNOWN_ARGUMENT,
        title: "unknown named argument",
        explanation: "Named call arguments must match the resolved callee signature. This catches misspellings and stale call sites after API changes.",
    },
    DiagnosticExplanation {
        code: code::MISSING_ARGUMENT,
        title: "missing required argument",
        explanation: "Every parameter in the resolved callee signature must be supplied by name at the call site.",
    },
    DiagnosticExplanation {
        code: code::DUPLICATE_ARGUMENT,
        title: "duplicate named argument",
        explanation: "A call may bind each named parameter only once. Duplicate names make effect and ownership review ambiguous.",
    },
    DiagnosticExplanation {
        code: code::UNKNOWN_CALLEE,
        title: "unknown callee",
        explanation: "The checker could not resolve the function or method call against user declarations, known constructors, or builtin signatures.",
    },
    DiagnosticExplanation {
        code: code::ARGUMENT_TYPE_MISMATCH,
        title: "argument type mismatch",
        explanation: "When both sides are known, a call argument's expression type must match the resolved parameter type before Rust lowering, and a binding initializer must match its explicit binding type. For `noescape Fn(...) -> T` parameters, the callback arity, callback call arguments, callback body call arguments, and known return expression must match the function type before Rust lowering.",
    },
    DiagnosticExplanation {
        code: code::RETURN_TYPE_MISMATCH,
        title: "return type mismatch",
        explanation: "When both sides are known, a returned expression must match the function's declared return type before Rust lowering. Falling through a non-Unit function is a Unit return mismatch. Result and Option constructors are checked against their success, error, or Some payload types.",
    },
    DiagnosticExplanation {
        code: code::CONTROL_FLOW_TYPE_MISMATCH,
        title: "control-flow type mismatch",
        explanation: "`if` and `while` conditions must be `Bool`, and v0.5 `match` scrutinees must be `Option<T>`, `Result<T, E>`, or a declared sum type with matching arm variants before Rust lowering.",
    },
    DiagnosticExplanation {
        code: code::OPERATOR_TYPE_MISMATCH,
        title: "operator type mismatch",
        explanation: "Built-in comparison and logical operators have fixed operand types. Equality requires matching known operand types, ordering requires numeric operands, and logical operators require Bool operands.",
    },
    DiagnosticExplanation {
        code: code::MANAGED_TO_LOCAL,
        title: "managed-to-local conversion",
        explanation: "Managed values cannot be rebound as local values. Create the value locally at its origin if local ownership is required.",
    },
    DiagnosticExplanation {
        code: code::FIELD_PARTIAL_ACCESS_CONFLICT,
        title: "whole-base field partial access conflict",
        explanation: "A call cannot mix a whole local base or whole local prefix access with a mutable or taking access to one of its fields.",
    },
    DiagnosticExplanation {
        code: code::FIELD_PREFIX_CONFLICT,
        title: "nested field prefix conflict",
        explanation: "Two local field paths conflict when one path is the same as, or a prefix of, the other and either access mutates or takes the path.",
    },
    DiagnosticExplanation {
        code: code::INDEXED_PARTIAL_ACCESS_CONFLICT,
        title: "indexed container partial access conflict",
        explanation: "RSScript v0.5 does not prove indexed element disjointness. Indexed local paths conflict with other accesses to the same local base when mutation or take is involved.",
    },
    DiagnosticExplanation {
        code: code::MOVE_BASE_FIELD_CONFLICT,
        title: "move-base field access conflict",
        explanation: "A call cannot move a local base with `manage` or `take` in the same expression where another argument accesses one of that base's fields.",
    },
    DiagnosticExplanation {
        code: code::LOCAL_CLASS_BINDING,
        title: "local class binding",
        explanation: "Classes are managed identity objects in RSScript v0.5. They are created as managed handles and cannot be bound as local exclusive values.",
    },
    DiagnosticExplanation {
        code: code::INVALID_MANAGE_OPERAND,
        title: "invalid manage operand",
        explanation: "`manage value` moves a local exclusive binding into the managed runtime. The operand must be a local binding that has not already become managed.",
    },
    DiagnosticExplanation {
        code: code::INVALID_TAKE_OPERAND,
        title: "invalid take operand",
        explanation: "`take value` consumes a local value. Managed values cannot be passed to taking parameters because they may have aliases.",
    },
    DiagnosticExplanation {
        code: code::READ_VIEW_MUTATION,
        title: "read view mutation",
        explanation: "`for` loop variables for non-Copy struct elements are read views into the list. They can be passed as `read`, but cannot be used as `mut`, `take`, or `manage` because iteration does not grant exclusive ownership of list elements.",
    },
    DiagnosticExplanation {
        code: code::USE_AFTER_MANAGE,
        title: "use after manage",
        explanation: "`manage value` moves a local value into the managed runtime. The original local binding cannot be used afterwards on any reachable path.",
    },
    DiagnosticExplanation {
        code: code::LOCAL_VALUE_RETAINED,
        title: "local value retained",
        explanation: "APIs marked `effects(retains(param))` may store the argument beyond the call. Passing a clean local value directly would let local ownership escape.",
    },
    DiagnosticExplanation {
        code: code::FRESH_RETURN_NOT_CLEAN,
        title: "fresh return is not clean",
        explanation: "A `fresh` function may only return a newly created value, a known fresh call, a clean local value, or a clean inline field of a local value. Clean locals must not have escaped through manage, take, retain, or capture.",
    },
    DiagnosticExplanation {
        code: code::FRESHNESS_UNKNOWN,
        title: "freshness unknown",
        explanation: "The MVP checker could not prove the returned value is fresh. Current proof support trusts clean locals, clean inline fields of locals, struct constructors, and known fresh calls.",
    },
    DiagnosticExplanation {
        code: code::INVALID_FRESH_RETURN_TYPE,
        title: "invalid fresh return type",
        explanation: "`fresh` may only be used with struct types. Classes and resources are not fresh values.",
    },
    DiagnosticExplanation {
        code: code::FRESH_REQUIRES_LOCAL_BINDING,
        title: "fresh value requires local binding",
        explanation: "A direct `fresh` expression may materialize as a managed temporary for `read`, but `mut` and `take` require an explicit `local` binding first.",
    },
    DiagnosticExplanation {
        code: code::RESOURCE_FIELD,
        title: "resource field",
        explanation: "Resource values cannot be stored directly in ordinary class or struct fields. Use `with` or an approved resource container such as `ResourcePool<T>`.",
    },
    DiagnosticExplanation {
        code: code::RESOURCE_ESCAPE,
        title: "resource escape",
        explanation: "A resource introduced by `with` must not escape the block through return, managed binding, manage, retention, or managed closure capture.",
    },
    DiagnosticExplanation {
        code: code::INVALID_RESOURCE_POOL_TYPE,
        title: "invalid ResourcePool type",
        explanation: "`ResourcePool<T>` is only valid when `T` is a resource type.",
    },
    DiagnosticExplanation {
        code: code::RESOURCE_GENERIC_ARGUMENT,
        title: "resource in ordinary generic type",
        explanation: "Resource values may only be held by explicit resource APIs such as `ResourcePool<T: Resource>`; ordinary generic containers must not be instantiated with resource types.",
    },
    DiagnosticExplanation {
        code: code::RESOURCE_POOL_NOT_LOCAL,
        title: "ResourcePool must be local",
        explanation: "`ResourcePool<T>` must be held in a local binding so its resource lifetime and pool ownership stay explicit.",
    },
    DiagnosticExplanation {
        code: code::RESOURCE_PRODUCER_MISSING_TRY,
        title: "Result resource producer missing try",
        explanation: "A `with` resource context consuming `Result<Resource, E>` must use explicit `?` on the producer expression.",
    },
    DiagnosticExplanation {
        code: code::RESOURCE_POOL_FALLIBLE_FACTORY,
        title: "ResourcePool factory contract violation",
        explanation: "`ResourcePool.new` is the v0.5 eager noescape infallible constructor and requires a factory returning a bare resource. `ResourcePool.try_new` is the matching fallible constructor and requires a factory returning `Result<Resource, E>`.",
    },
    DiagnosticExplanation {
        code: code::RESOURCE_POOL_INVALID_MAX_SIZE,
        title: "ResourcePool max_size not a positive Int literal",
        explanation: "`ResourcePool.new` is eager and infallible in v0.5, so `max_size` must be a positive `Int` literal checked before lowering.",
    },
    DiagnosticExplanation {
        code: code::RESOURCE_POOL_ACTIVE_LEASE_CONFLICT,
        title: "ResourcePool active lease conflict",
        explanation: "While a `ResourcePool.borrow` lease is active, the same pool root cannot be read, mutated, taken, managed, or borrowed again inside the lease body.",
    },
    DiagnosticExplanation {
        code: code::LOCAL_CAPTURED_BY_MANAGED_CLOSURE,
        title: "local captured by managed closure",
        explanation: "A closure bound with `let` is managed and may outlive clean local values. Use a local/noescape callback shape instead.",
    },
    DiagnosticExplanation {
        code: code::NOESCAPE_CALLBACK_ESCAPE,
        title: "noescape callback escape",
        explanation: "`noescape Fn()` parameters are temporary callback capabilities. They may be called directly or forwarded only to another resolved noescape parameter, but they cannot be returned, stored, retained, or passed as ordinary managed values.",
    },
    DiagnosticExplanation {
        code: code::LOCAL_CLOSURE_ESCAPE,
        title: "local closure escape",
        explanation: "A closure bound with `local` is an exclusive local capability. It may be called directly or passed to `noescape Fn()` parameters, but it cannot be returned, stored in managed bindings, or passed as an ordinary managed callback.",
    },
    DiagnosticExplanation {
        code: code::NOESCAPE_CONSUMES_CAPTURE,
        title: "noescape closure consumes captured local",
        explanation: "`noescape Fn()` callbacks are non-consuming in v0.5: the callee may call them multiple times, so a noescape closure may read or mutate captured local values but must not take or manage them.",
    },
    DiagnosticExplanation {
        code: code::TAKE_HANDLE_FIELD,
        title: "take of handle field",
        explanation: "Handle fields are managed references. They cannot be consumed with `take` as if they were inline local fields.",
    },
    DiagnosticExplanation {
        code: code::INVALID_WEAK_FIELD,
        title: "invalid weak field",
        explanation: "`weak` fields break managed class cycles. The MVP only allows weak fields whose type is a class.",
    },
    DiagnosticExplanation {
        code: code::WEAK_FIELD_REQUIRES_UPGRADE,
        title: "weak field requires upgrade",
        explanation: "`weak` fields are non-owning handles. Upgrade them explicitly with `Weak.upgrade(value: read weak_field)` before using the target value.",
    },
    DiagnosticExplanation {
        code: code::WEAK_FIELD_REQUIRES_WEAK_HANDLE,
        title: "weak field requires weak handle",
        explanation: "`weak` fields must be initialized from an explicit weak-handle expression such as `Weak.from(value: read target)`.",
    },
    DiagnosticExplanation {
        code: code::OPERATOR_OVERLOAD_ATTEMPT,
        title: "operator overload attempt",
        explanation: "The MVP language surface rejects likely user-defined operator overloads to keep review semantics explicit.",
    },
    DiagnosticExplanation {
        code: code::IMPLICIT_CONVERSION_ATTEMPT,
        title: "implicit conversion attempt",
        explanation: "RSScript rejects cast-style conversion syntax. Conversions must be visible named APIs such as `Type.from(value: read x)`.",
    },
    DiagnosticExplanation {
        code: code::OWN_STRUCT_ATTEMPT,
        title: "own struct attempt",
        explanation: "RSScript v0.5 has exactly three type declaration kinds: `class`, `struct`, and `resource`. There is no `own struct`.",
    },
    DiagnosticExplanation {
        code: code::SURFACE_REFERENCE_ATTEMPT,
        title: "surface reference attempt",
        explanation: "RSScript does not expose `&T` or `&mut T` syntax. Use explicit parameter effects such as `read`, `mut`, and `take`.",
    },
    DiagnosticExplanation {
        code: code::RUSTC_DIAGNOSTIC_MAPPED,
        title: "mapped rustc diagnostic",
        explanation: "A backend rustc diagnostic was translated through RSScript source-map metadata. Treat this as a compiler, runtime, native binding, or lowering issue unless the mapped RSScript source clearly violates the language rules.",
    },
    DiagnosticExplanation {
        code: code::RUSTC_DIAGNOSTIC_UNMAPPABLE,
        title: "unmappable rustc diagnostic",
        explanation: "rustc reported a backend diagnostic whose generated Rust location could not be mapped back to RSScript source. The compiler should surface the generated Rust reference as secondary internal detail instead of exposing raw rustc output as the primary diagnostic.",
    },
    DiagnosticExplanation {
        code: code::RUNTIME_DIAGNOSTIC,
        title: "runtime diagnostic",
        explanation: "The RSScript runtime reported a managed aliasing or resource conflict with an RSScript source span. Runtime diagnostics should be surfaced as RSScript diagnostics instead of raw Rust panics.",
    },
    DiagnosticExplanation {
        code: code::PACKAGE_INTERFACE_MISMATCH,
        title: "package interface mismatch",
        explanation: "A package `.rssi` public contract must be implemented by the package source with the same type declarations, function signatures, return freshness, and declared effects.",
    },
    DiagnosticExplanation {
        code: code::PACKAGE_FEATURE_RESOLUTION,
        title: "package feature resolution",
        explanation: "Selected package features must be declared by the target package. Declared features may depend on other declared features; feature resolution is additive and deterministic.",
    },
    DiagnosticExplanation {
        code: code::PACKAGE_UNSUPPORTED_DEPENDENCY_SOURCE,
        title: "unsupported package dependency source",
        explanation: "RSScript v0.5 accepts registry and local path package dependencies. Future-looking dependency source forms such as git must be rejected with a stable package-manager diagnostic.",
    },
    DiagnosticExplanation {
        code: code::PACKAGE_REVIEW_POLICY_VIOLATION,
        title: "package review policy violation",
        explanation: "`[review.policy]` turns package review facts such as unknown risk, native boundaries, and unsafe APIs into package-manager errors.",
    },
    DiagnosticExplanation {
        code: code::PACKAGE_NATIVE_BINDING,
        title: "package native binding metadata mismatch",
        explanation: "`native/bindings.rssbind.toml` and `[native.rust]` metadata must point from declared native `.rssi` functions to Rust wrapper functions in the configured native Rust crate. This is a package-manager diagnostic, not an RSScript frontend diagnostic.",
    },
    DiagnosticExplanation {
        code: code::PACKAGE_PROVIDER_DECLARATION,
        title: "package provider declaration mismatch",
        explanation: "`[implements.\"<interface-package>\"]` provider declarations must bind a provider to one interface package version, selected interface feature set, and effective interface hash.",
    },
    DiagnosticExplanation {
        code: code::LINT_SIGNATURE_COMPLEXITY,
        title: "signature complexity lint",
        explanation: "Public signatures should remain reviewable in one screen. The linter warns when public parameters, generic parameters, effect clauses, or nested type shapes exceed the current review budget.",
    },
    DiagnosticExplanation {
        code: code::LINT_DUPLICATE_EFFECT,
        title: "duplicate effect lint",
        explanation: "Effect clauses are review contracts. Repeating the same effect adds noise without changing semantics, so the linter warns and suggests removing the duplicate.",
    },
];

pub fn format_diagnostics_json(diagnostics: &[Diagnostic]) -> String {
    let diagnostics: Vec<JsonDiagnostic<'_>> =
        diagnostics.iter().map(JsonDiagnostic::from).collect();
    serde_json::to_string(&diagnostics).expect("diagnostic JSON serialization should not fail")
}

#[derive(Serialize)]
struct JsonDiagnostic<'a> {
    code: &'a str,
    severity: &'a str,
    summary: &'a str,
    spans: Vec<JsonSpan<'a>>,
    causes: &'a [String],
    fixes: &'a [Fix],
}

#[derive(Serialize)]
struct JsonSpan<'a> {
    file: &'a str,
    line: usize,
    column: usize,
    length: usize,
    label: &'a str,
}

impl<'a> From<&'a Diagnostic> for JsonDiagnostic<'a> {
    fn from(diagnostic: &'a Diagnostic) -> Self {
        Self {
            code: &diagnostic.code,
            severity: diagnostic.severity.as_str(),
            summary: &diagnostic.summary,
            spans: vec![JsonSpan {
                file: &diagnostic.span.file,
                line: diagnostic.span.line,
                column: diagnostic.span.column,
                length: diagnostic.span.length,
                label: &diagnostic.label,
            }],
            causes: &diagnostic.causes,
            fixes: &diagnostic.fixes,
        }
    }
}
