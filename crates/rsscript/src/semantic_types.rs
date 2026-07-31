use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use crate::syntax::ast::{DataEffect, TypeRef};
use crate::syntax::ast::{Item, Program};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeId(u32);

impl TypeId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct TypeQualifiers {
    pub fresh: bool,
    pub noescape: bool,
    pub owned: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResolvedParamEffect {
    Read,
    Mut,
    Take,
}

impl From<DataEffect> for ResolvedParamEffect {
    fn from(effect: DataEffect) -> Self {
        match effect {
            DataEffect::Read => Self::Read,
            DataEffect::Mut => Self::Mut,
            DataEffect::Take => Self::Take,
        }
    }
}

impl From<ResolvedParamEffect> for DataEffect {
    fn from(effect: ResolvedParamEffect) -> Self {
        match effect {
            ResolvedParamEffect::Read => Self::Read,
            ResolvedParamEffect::Mut => Self::Mut,
            ResolvedParamEffect::Take => Self::Take,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ResolvedTypeKind {
    Named {
        name: String,
        arguments: Box<[ResolvedType]>,
    },
    Function {
        parameters: Box<[ResolvedType]>,
        parameter_effects: Box<[Option<ResolvedParamEffect>]>,
        return_type: Option<Box<ResolvedType>>,
    },
}

/// Canonical structural type used after parsing.
///
/// Source spans deliberately do not participate in semantic identity. They
/// remain on syntax nodes for diagnostics while this value is shared by HIR,
/// semantic facts, and executable backends.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResolvedType {
    pub qualifiers: TypeQualifiers,
    pub kind: ResolvedTypeKind,
}

impl ResolvedType {
    pub fn named(name: impl Into<String>, arguments: impl IntoIterator<Item = Self>) -> Self {
        Self {
            qualifiers: TypeQualifiers::default(),
            kind: ResolvedTypeKind::Named {
                name: name.into(),
                arguments: arguments.into_iter().collect::<Vec<_>>().into_boxed_slice(),
            },
        }
    }

    pub fn root_name(&self) -> Option<&str> {
        match &self.kind {
            ResolvedTypeKind::Named { name, .. } => Some(name),
            ResolvedTypeKind::Function { .. } => None,
        }
    }

    pub fn arguments(&self) -> &[Self] {
        match &self.kind {
            ResolvedTypeKind::Named { arguments, .. } => arguments,
            ResolvedTypeKind::Function { .. } => &[],
        }
    }

    pub fn named_argument(&self, root: &str, index: usize) -> Option<&Self> {
        (self.root_name() == Some(root))
            .then(|| self.arguments().get(index))
            .flatten()
    }

    pub fn function_return(&self) -> Option<&Self> {
        match &self.kind {
            ResolvedTypeKind::Function { return_type, .. } => return_type.as_deref(),
            ResolvedTypeKind::Named { .. } => None,
        }
    }

    pub fn is_function(&self) -> bool {
        matches!(self.kind, ResolvedTypeKind::Function { .. })
    }

    pub fn without_fresh(mut self) -> Self {
        self.qualifiers.fresh = false;
        self
    }

    pub fn from_type_ref(ty: &TypeRef) -> Self {
        let qualifiers = TypeQualifiers {
            fresh: ty.is_fresh,
            noescape: ty.is_noescape,
            owned: ty.is_owned,
        };
        let kind = if ty.name == "Fn" {
            ResolvedTypeKind::Function {
                parameters: ty
                    .fn_params
                    .iter()
                    .map(Self::from_type_ref)
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
                parameter_effects: ty
                    .fn_params
                    .iter()
                    .enumerate()
                    .map(|(index, _)| {
                        ty.effective_fn_param_effect(index)
                            .map(ResolvedParamEffect::from)
                    })
                    .collect(),
                return_type: ty
                    .fn_return
                    .as_deref()
                    .map(Self::from_type_ref)
                    .map(Box::new),
            }
        } else {
            ResolvedTypeKind::Named {
                name: ty.name.clone(),
                arguments: ty
                    .args
                    .iter()
                    .map(Self::from_type_ref)
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            }
        };
        Self { qualifiers, kind }
    }

    /// Compatibility conversion for legacy HIR fields that still expose a
    /// rendered type. New semantic facts are built with `from_type_ref`.
    pub(crate) fn from_display(type_name: &str) -> Self {
        let mut rest = type_name.trim();
        let mut qualifiers = TypeQualifiers::default();
        loop {
            if let Some(value) = rest.strip_prefix("fresh ").map(str::trim) {
                qualifiers.fresh = true;
                rest = value;
            } else if let Some(value) = rest.strip_prefix("noescape ").map(str::trim) {
                qualifiers.noescape = true;
                rest = value;
            } else if let Some(value) = rest.strip_prefix("owned ").map(str::trim) {
                qualifiers.owned = true;
                rest = value;
            } else {
                break;
            }
        }
        let kind = if let Some(parameters) = rest.strip_prefix("Fn(") {
            let close = matching_function_close(parameters);
            if let Some(close) = close {
                let parameter_text = &parameters[..close];
                let mut effects = Vec::new();
                let parameters = crate::text_util::split_top_level_type_args(parameter_text)
                    .into_iter()
                    .filter(|parameter| !parameter.is_empty())
                    .map(|parameter| {
                        let (effect, parameter) =
                            if let Some(parameter) = parameter.strip_prefix("read ") {
                                (Some(ResolvedParamEffect::Read), parameter)
                            } else if let Some(parameter) = parameter.strip_prefix("mut ") {
                                (Some(ResolvedParamEffect::Mut), parameter)
                            } else if let Some(parameter) = parameter.strip_prefix("take ") {
                                (Some(ResolvedParamEffect::Take), parameter)
                            } else {
                                (None, parameter)
                            };
                        effects.push(effect);
                        Self::from_display(parameter)
                    })
                    .collect::<Vec<_>>()
                    .into_boxed_slice();
                let return_type = parameters_after_close(parameter_text, rest)
                    .strip_prefix("->")
                    .map(str::trim)
                    .filter(|return_type| !return_type.is_empty())
                    .map(Self::from_display)
                    .map(Box::new);
                ResolvedTypeKind::Function {
                    parameters,
                    parameter_effects: effects.into_boxed_slice(),
                    return_type,
                }
            } else {
                ResolvedTypeKind::Named {
                    name: rest.to_string(),
                    arguments: Box::new([]),
                }
            }
        } else {
            ResolvedTypeKind::Named {
                name: crate::text_util::type_root_name(rest).to_string(),
                arguments: crate::text_util::type_arg_names(rest)
                    .unwrap_or_default()
                    .into_iter()
                    .map(Self::from_display)
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            }
        };
        Self { qualifiers, kind }
    }

    pub fn to_type_ref(&self, span: &crate::diagnostic::Span) -> TypeRef {
        let (name, args, fn_params, fn_param_effects, fn_return) = match &self.kind {
            ResolvedTypeKind::Named { name, arguments } => (
                name.clone(),
                arguments
                    .iter()
                    .map(|argument| argument.to_type_ref(span))
                    .collect(),
                Vec::new(),
                Vec::new(),
                None,
            ),
            ResolvedTypeKind::Function {
                parameters,
                parameter_effects,
                return_type,
            } => (
                "Fn".to_string(),
                Vec::new(),
                parameters
                    .iter()
                    .map(|parameter| parameter.to_type_ref(span))
                    .collect(),
                parameter_effects
                    .iter()
                    .map(|effect| effect.map(DataEffect::from))
                    .collect(),
                return_type
                    .as_deref()
                    .map(|return_type| Box::new(return_type.to_type_ref(span))),
            ),
        };
        TypeRef {
            name,
            args,
            malformed_arg_spans: Vec::new(),
            is_fresh: self.qualifiers.fresh,
            is_noescape: self.qualifiers.noescape,
            is_owned: self.qualifiers.owned,
            fn_params,
            fn_param_effects,
            fn_return,
            span: span.clone(),
        }
    }

    pub fn substitute(&self, substitutions: &BTreeMap<String, ResolvedType>) -> Self {
        if let ResolvedTypeKind::Named { name, arguments } = &self.kind
            && arguments.is_empty()
            && let Some(replacement) = substitutions.get(name)
        {
            let mut replacement = replacement.clone();
            replacement.qualifiers.fresh |= self.qualifiers.fresh;
            replacement.qualifiers.noescape |= self.qualifiers.noescape;
            replacement.qualifiers.owned |= self.qualifiers.owned;
            return replacement;
        }

        let kind = match &self.kind {
            ResolvedTypeKind::Named { name, arguments } => ResolvedTypeKind::Named {
                name: name.clone(),
                arguments: arguments
                    .iter()
                    .map(|argument| argument.substitute(substitutions))
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
            },
            ResolvedTypeKind::Function {
                parameters,
                parameter_effects,
                return_type,
            } => ResolvedTypeKind::Function {
                parameters: parameters
                    .iter()
                    .map(|parameter| parameter.substitute(substitutions))
                    .collect::<Vec<_>>()
                    .into_boxed_slice(),
                parameter_effects: parameter_effects.clone(),
                return_type: return_type
                    .as_deref()
                    .map(|return_type| return_type.substitute(substitutions))
                    .map(Box::new),
            },
        };
        Self {
            qualifiers: self.qualifiers,
            kind,
        }
    }

    pub fn collect_substitutions(
        &self,
        actual: &ResolvedType,
        parameters: &HashSet<&str>,
        substitutions: &mut BTreeMap<String, ResolvedType>,
    ) {
        if let ResolvedTypeKind::Named { name, arguments } = &self.kind
            && arguments.is_empty()
            && parameters.contains(name.as_str())
        {
            substitutions
                .entry(name.clone())
                .or_insert_with(|| actual.clone());
            return;
        }

        match (&self.kind, &actual.kind) {
            (
                ResolvedTypeKind::Named { name, arguments },
                ResolvedTypeKind::Named {
                    name: actual_name,
                    arguments: actual_arguments,
                },
            ) if name == actual_name && arguments.len() == actual_arguments.len() => {
                for (pattern, actual) in arguments.iter().zip(actual_arguments) {
                    pattern.collect_substitutions(actual, parameters, substitutions);
                }
            }
            (
                ResolvedTypeKind::Function {
                    parameters: pattern_parameters,
                    return_type: pattern_return,
                    ..
                },
                ResolvedTypeKind::Function {
                    parameters: actual_parameters,
                    return_type: actual_return,
                    ..
                },
            ) if pattern_parameters.len() == actual_parameters.len() => {
                for (pattern, actual) in pattern_parameters.iter().zip(actual_parameters) {
                    pattern.collect_substitutions(actual, parameters, substitutions);
                }
                if let (Some(pattern), Some(actual)) =
                    (pattern_return.as_deref(), actual_return.as_deref())
                {
                    pattern.collect_substitutions(actual, parameters, substitutions);
                }
            }
            _ => {}
        }
    }
}

fn matching_function_close(parameters: &str) -> Option<usize> {
    let mut nested = 0usize;
    for (index, character) in parameters.char_indices() {
        match character {
            '<' | '(' => nested = nested.saturating_add(1),
            '>' => nested = nested.saturating_sub(1),
            ')' if nested == 0 => return Some(index),
            ')' => nested = nested.saturating_sub(1),
            _ => {}
        }
    }
    None
}

fn parameters_after_close<'a>(parameter_text: &str, full_type: &'a str) -> &'a str {
    let offset = "Fn(".len() + parameter_text.len() + 1;
    full_type.get(offset..).unwrap_or_default().trim()
}

impl fmt::Display for ResolvedType {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.qualifiers.fresh {
            formatter.write_str("fresh ")?;
        }
        if self.qualifiers.noescape {
            formatter.write_str("noescape ")?;
        } else if self.qualifiers.owned {
            formatter.write_str("owned ")?;
        }
        match &self.kind {
            ResolvedTypeKind::Named { name, arguments } => {
                formatter.write_str(name)?;
                if !arguments.is_empty() {
                    formatter.write_str("<")?;
                    for (index, argument) in arguments.iter().enumerate() {
                        if index != 0 {
                            formatter.write_str(", ")?;
                        }
                        argument.fmt(formatter)?;
                    }
                    formatter.write_str(">")?;
                }
                Ok(())
            }
            ResolvedTypeKind::Function {
                parameters,
                parameter_effects,
                return_type,
            } => {
                formatter.write_str("Fn(")?;
                for (index, parameter) in parameters.iter().enumerate() {
                    if index != 0 {
                        formatter.write_str(", ")?;
                    }
                    if let Some(effect) = parameter_effects.get(index).copied().flatten() {
                        let effect = DataEffect::from(effect);
                        write!(formatter, "{} ", effect.as_str())?;
                    }
                    parameter.fmt(formatter)?;
                }
                formatter.write_str(")")?;
                if let Some(return_type) = return_type {
                    formatter.write_str(" -> ")?;
                    return_type.fmt(formatter)?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TypeArena {
    types: Vec<ResolvedType>,
    ids: HashMap<ResolvedType, TypeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionTypeFacts {
    pub type_parameters: Box<[String]>,
    pub parameters: Box<[(String, TypeId)]>,
    pub return_type: Option<TypeId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedTypeFacts {
    pub type_parameters: Box<[String]>,
    pub fields: Box<[(String, TypeId)]>,
}

/// Interned type facts shared by checked consumers.
///
/// These facts are built from the same namespace-isolated syntax snapshot that
/// produced HIR. Backends consume IDs from here instead of rebuilding semantic
/// signatures by parsing rendered type names.
#[derive(Debug, Clone, Default)]
pub struct SemanticTypeFacts {
    arena: TypeArena,
    functions: BTreeMap<String, FunctionTypeFacts>,
    named_types: BTreeMap<String, NamedTypeFacts>,
}

impl SemanticTypeFacts {
    pub(crate) fn from_programs<'a>(
        program: &Program,
        interfaces: impl IntoIterator<Item = &'a Program>,
    ) -> Self {
        let mut facts = Self::default();
        for item in interfaces
            .into_iter()
            .flat_map(|interface| interface.items.iter())
            .chain(program.items.iter())
        {
            facts.record_item(item);
        }
        facts
    }

    fn record_item(&mut self, item: &Item) {
        match item {
            Item::Function(function) => {
                let parameters = function
                    .params
                    .iter()
                    .map(|parameter| {
                        (
                            parameter.name.clone(),
                            self.arena
                                .intern(ResolvedType::from_type_ref(&parameter.ty)),
                        )
                    })
                    .collect::<Vec<_>>()
                    .into_boxed_slice();
                let return_type = function
                    .return_ty
                    .as_ref()
                    .map(ResolvedType::from_type_ref)
                    .map(|ty| self.arena.intern(ty));
                self.functions.insert(
                    function.name.clone(),
                    FunctionTypeFacts {
                        type_parameters: function
                            .type_params
                            .iter()
                            .map(|parameter| parameter.name.clone())
                            .collect::<Vec<_>>()
                            .into_boxed_slice(),
                        parameters,
                        return_type,
                    },
                );
            }
            Item::Type(declaration) => {
                let fields = declaration
                    .fields
                    .iter()
                    .map(|field| {
                        (
                            field.name.clone(),
                            self.arena.intern(ResolvedType::from_type_ref(&field.ty)),
                        )
                    })
                    .collect::<Vec<_>>()
                    .into_boxed_slice();
                self.named_types.insert(
                    declaration.name.clone(),
                    NamedTypeFacts {
                        type_parameters: declaration
                            .type_params
                            .iter()
                            .map(|parameter| parameter.name.clone())
                            .collect::<Vec<_>>()
                            .into_boxed_slice(),
                        fields,
                    },
                );
            }
            Item::SumType(declaration) => {
                self.named_types.insert(
                    declaration.name.clone(),
                    NamedTypeFacts {
                        type_parameters: declaration
                            .type_params
                            .iter()
                            .map(|parameter| parameter.name.clone())
                            .collect::<Vec<_>>()
                            .into_boxed_slice(),
                        fields: Box::new([]),
                    },
                );
            }
            Item::TypeAlias(_) | Item::Const(_) | Item::Module(_) | Item::Use(_) => {}
        }
    }

    pub fn arena(&self) -> &TypeArena {
        &self.arena
    }

    pub fn functions(&self) -> impl Iterator<Item = (&str, &FunctionTypeFacts)> {
        self.functions
            .iter()
            .map(|(name, facts)| (name.as_str(), facts))
    }

    pub fn named_type(&self, name: &str) -> Option<&NamedTypeFacts> {
        self.named_types.get(name)
    }
}

impl TypeArena {
    pub fn intern(&mut self, ty: ResolvedType) -> TypeId {
        if let Some(id) = self.ids.get(&ty) {
            return *id;
        }
        let id = TypeId(u32::try_from(self.types.len()).expect("type arena exceeds u32"));
        self.types.push(ty.clone());
        self.ids.insert(ty, id);
        id
    }

    pub fn get(&self, id: TypeId) -> &ResolvedType {
        &self.types[id.index()]
    }

    pub fn len(&self) -> usize {
        self.types.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structurally_equal_types_share_an_id() {
        let mut arena = TypeArena::default();
        let first = arena.intern(ResolvedType::from_display("Map<String, Int>"));
        let second = arena.intern(ResolvedType::from_display("Map<String, Int>"));
        assert_eq!(first, second);
        assert_eq!(arena.len(), 1);
    }

    #[test]
    fn substitution_uses_declared_names_at_any_depth() {
        let pattern = ResolvedType::from_display("Pair<U, List<W>>");
        let actual = ResolvedType::from_display("Pair<Int, List<String>>");
        let parameters = HashSet::from(["U", "W"]);
        let mut substitutions = BTreeMap::new();
        pattern.collect_substitutions(&actual, &parameters, &mut substitutions);

        assert_eq!(substitutions["U"].to_string(), "Int");
        assert_eq!(substitutions["W"].to_string(), "String");
        assert_eq!(
            pattern.substitute(&substitutions).to_string(),
            "Pair<Int, List<String>>"
        );
    }

    #[test]
    fn function_type_round_trip_preserves_effects_and_qualifiers() {
        let ty =
            ResolvedType::from_display("fresh noescape Fn(read List<U>, take W) -> Result<W, U>");
        assert_eq!(
            ty.to_string(),
            "fresh noescape Fn(read List<U>, take W) -> Result<W, U>"
        );
    }

    #[test]
    fn structural_queries_replace_display_string_decomposition() {
        let result = ResolvedType::from_display("fresh Result<List<Int>, String>");
        assert_eq!(result.root_name(), Some("Result"));
        assert_eq!(
            result
                .named_argument("Result", 0)
                .and_then(|ok| ok.named_argument("List", 0))
                .and_then(ResolvedType::root_name),
            Some("Int")
        );
        assert_eq!(
            result
                .named_argument("Result", 1)
                .and_then(ResolvedType::root_name),
            Some("String")
        );

        let function = ResolvedType::from_display("noescape Fn(Int) -> Task<Bool>");
        assert!(function.is_function());
        assert_eq!(
            function
                .function_return()
                .and_then(|ty| ty.named_argument("Task", 0))
                .and_then(ResolvedType::root_name),
            Some("Bool")
        );

        let implicit_unit_function = ResolvedType::from_display("noescape Fn()");
        assert!(implicit_unit_function.is_function());
        assert!(implicit_unit_function.function_return().is_none());
    }
}
