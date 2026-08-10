use super::*;
use crate::syntax::ast::GenericParam;

impl Analyzer<'_> {
    /// A compiler-owned derive expands to generated Rust, so every field must
    /// support it. This reports an RSScript diagnostic before lowering instead
    /// of letting a `Float: Eq` style trait-bound error leak from rustc.
    pub(super) fn check_derive_field_requirements(&mut self) {
        use crate::syntax::ast::Item;
        let local_types = self.collect_local_value_types();
        let mut violations: Vec<(crate::diagnostic::Span, String, String, &'static str)> =
            Vec::new();

        for item in &self.syntax_program.items {
            let (derives, type_params, field_sets): (
                &[String],
                &[GenericParam],
                Vec<&[FieldDecl]>,
            ) = match item {
                // Resources default to `Debug` only and hold special internal
                // fields, so they are not part of structural-derive checking.
                Item::Type(type_decl) if type_decl.kind != TypeKind::Resource => (
                    &type_decl.derives,
                    &type_decl.type_params,
                    vec![type_decl.fields.as_slice()],
                ),
                Item::SumType(sum) => (
                    &sum.derives,
                    &sum.type_params,
                    sum.variants
                        .iter()
                        .map(|variant| variant.fields.as_slice())
                        .collect(),
                ),
                _ => continue,
            };

            let required = required_field_derives(derives);
            if required.is_empty() {
                continue;
            }
            let generic_params: HashSet<&str> = type_params
                .iter()
                .map(|param| param.name.as_str())
                .collect();

            for fields in &field_sets {
                for field in fields.iter() {
                    // `handle`/`weak` fields lower to `Managed<T>`/`WeakManaged<T>`,
                    // which implement only `Clone`/`Debug` — never `Eq`, `Ord`,
                    // `Hash`, or serde — so any required derive is unsupported.
                    let managed = field.is_handle || field.is_weak;
                    for &derive in &required {
                        let supported = if managed {
                            DeriveSupport::No
                        } else {
                            field_supports_derive(&field.ty, derive, &local_types, &generic_params)
                        };
                        if supported == DeriveSupport::No {
                            let type_name = if field.is_handle {
                                format!("handle {}", type_ref_name(&field.ty))
                            } else if field.is_weak {
                                format!("weak {}", type_ref_name(&field.ty))
                            } else {
                                type_ref_name(&field.ty)
                            };
                            violations.push((
                                field.span.clone(),
                                field.name.clone(),
                                type_name,
                                derive,
                            ));
                        }
                    }
                }
            }
        }

        for (span, field_name, type_name, derive) in violations {
            self.diagnostics.push(
                Diagnostic::error(
                    code::DERIVE_FIELD_UNSUPPORTED,
                    format!("`{derive}` derive is not supported by field `{field_name}`."),
                    span,
                    format!("`{type_name}` does not support the `{derive}` derive"),
                )
                .with_cause(derive_requirement_cause(derive))
                .with_fix(
                    "remove_or_change_derive",
                    format!(
                        "Remove `{derive}` from the derive list, or change `{field_name}` to a type that supports it."
                    ),
                    "manual",
                ),
            );
        }
    }

    /// Map locally declared type names to whether they are value types (struct
    /// or sum) and which derives they carry. Only locally declared types get a
    /// definite verdict; interface/runtime types have hand-written impls and are
    /// left to the backend.
    pub(super) fn collect_local_value_types(&self) -> HashMap<String, LocalTypeDerives> {
        use crate::syntax::ast::Item;
        let mut map = HashMap::new();
        for item in &self.syntax_program.items {
            match item {
                Item::Type(type_decl) => {
                    map.insert(
                        type_decl.name.clone(),
                        LocalTypeDerives {
                            is_value: type_decl.kind == TypeKind::Struct,
                            derives: type_decl.derives.iter().cloned().collect(),
                        },
                    );
                }
                Item::SumType(sum) => {
                    map.insert(
                        sum.name.clone(),
                        LocalTypeDerives {
                            is_value: true,
                            derives: sum.derives.iter().cloned().collect(),
                        },
                    );
                }
                _ => {}
            }
        }
        map
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeriveSupport {
    Yes,
    No,
    Unknown,
}

pub(super) struct LocalTypeDerives {
    is_value: bool,
    derives: HashSet<String>,
}

/// The derives that impose a per-field requirement. `Debug`/`Clone` are
/// satisfied by every field type the checker accepts, and `Schema`/`ReviewSchema`
/// are review-only markers, so they impose nothing. `Ord` implies `Eq` in the
/// generated Rust, so a single `Ord` check covers both.
fn required_field_derives(derives: &[String]) -> Vec<&'static str> {
    let has = |name: &str| derives.iter().any(|derive| derive == name);
    let mut required = Vec::new();
    if has("Ord") {
        required.push("Ord");
    } else if has("Eq") {
        required.push("Eq");
    }
    if has("Hash") {
        required.push("Hash");
    }
    if has("JsonEncode") {
        required.push("JsonEncode");
    }
    if has("JsonDecode") {
        required.push("JsonDecode");
    }
    required
}

fn derive_requirement_cause(derive: &str) -> &'static str {
    match derive {
        "Eq" => {
            "`Eq` is total equality, so every field's type must be `Eq` (for example `Float` is not, and a struct or sum field must itself derive `Eq`)."
        }
        "Ord" => {
            "`Ord` is total ordering, so every field's type must be `Ord` (for example `Float` is not, and a struct or sum field must itself derive `Ord`)."
        }
        "Hash" => {
            "`Hash` requires hashable fields, so every field's type must be `Hash` (`Float`, `Map`, and `Set` are not, and a struct or sum field must itself derive `Hash`)."
        }
        "JsonEncode" => {
            "`JsonEncode` requires every struct or sum field's type to also derive `JsonEncode`."
        }
        "JsonDecode" => {
            "`JsonDecode` requires every struct or sum field's type to also derive `JsonDecode`."
        }
        _ => "Every field must support the derived trait.",
    }
}

fn combine_support(left: DeriveSupport, right: DeriveSupport) -> DeriveSupport {
    match (left, right) {
        (DeriveSupport::No, _) | (_, DeriveSupport::No) => DeriveSupport::No,
        (DeriveSupport::Unknown, _) | (_, DeriveSupport::Unknown) => DeriveSupport::Unknown,
        _ => DeriveSupport::Yes,
    }
}

/// Whether a field of type `ty` supports `derive`. Returns `Unknown` whenever
/// the verdict is not certain, so the checker only rejects known-bad cases and
/// never a program the backend would accept.
fn field_supports_derive(
    ty: &TypeRef,
    derive: &str,
    local_types: &HashMap<String, LocalTypeDerives>,
    generic_params: &HashSet<&str>,
) -> DeriveSupport {
    let root = type_root_name(&ty.name);
    if generic_params.contains(root) {
        // `#[derive(..)]` adds the matching `T: Trait` bound, so a generic
        // parameter field is valid at the definition site.
        return DeriveSupport::Unknown;
    }
    match derive {
        "Eq" | "Ord" | "Hash" => structural_support(ty, root, derive, local_types, generic_params),
        "JsonEncode" | "JsonDecode" => json_support(ty, root, derive, local_types, generic_params),
        _ => DeriveSupport::Unknown,
    }
}

fn structural_support(
    ty: &TypeRef,
    root: &str,
    derive: &str,
    local_types: &HashMap<String, LocalTypeDerives>,
    generic_params: &HashSet<&str>,
) -> DeriveSupport {
    match root {
        "Float" | "Float32" | "Float64" => DeriveSupport::No,
        "Int" | "Int8" | "Int16" | "Int32" | "Int64" | "UInt" | "UInt8" | "UInt16" | "UInt32"
        | "UInt64" | "Bool" | "Byte" | "Char" | "Unit" | "String" => DeriveSupport::Yes,
        "List" | "Option" => match ty.args.first() {
            Some(arg) => field_supports_derive(arg, derive, local_types, generic_params),
            None => DeriveSupport::Unknown,
        },
        "Result" if !ty.args.is_empty() => ty.args.iter().fold(DeriveSupport::Yes, |acc, arg| {
            combine_support(
                acc,
                field_supports_derive(arg, derive, local_types, generic_params),
            )
        }),
        // `HashSet<T>: Eq` requires `T: Eq + Hash`, so the element must be both.
        "Set" if derive == "Eq" => match ty.args.first() {
            Some(elem) => combine_support(
                field_supports_derive(elem, "Eq", local_types, generic_params),
                hash_key_support(elem, local_types, generic_params),
            ),
            None => DeriveSupport::Unknown,
        },
        // `HashMap<K, V>: Eq` requires `K: Eq + Hash` and `V: Eq`.
        "Map" if derive == "Eq" => match (ty.args.first(), ty.args.get(1)) {
            (Some(key), Some(value)) => combine_support(
                combine_support(
                    field_supports_derive(key, "Eq", local_types, generic_params),
                    hash_key_support(key, local_types, generic_params),
                ),
                field_supports_derive(value, "Eq", local_types, generic_params),
            ),
            _ => DeriveSupport::Unknown,
        },
        // `HashMap`/`HashSet` implement neither `Ord` nor `Hash`.
        "Map" | "Set" => DeriveSupport::No,
        _ => user_type_support(ty, root, derive, local_types, generic_params),
    }
}

/// Whether `ty` can serve as a hashable container key/element. A `Map` key or
/// `Set` element must be `Hash`, but the enclosing `Eq`/`JsonDecode` derive only
/// adds an `Eq`/`Deserialize` bound to a generic parameter — never `Hash` — and
/// RSScript has no `Hash` generic bound to express it. So this mirrors the
/// `Hash` structural rule but treats a generic parameter at *any* nesting depth
/// as `No`, catching nested positions such as `Map<List<T>, V>`.
fn hash_key_support(
    ty: &TypeRef,
    local_types: &HashMap<String, LocalTypeDerives>,
    generic_params: &HashSet<&str>,
) -> DeriveSupport {
    let root = type_root_name(&ty.name);
    if generic_params.contains(root) {
        return DeriveSupport::No;
    }
    match root {
        "Float" | "Float32" | "Float64" => DeriveSupport::No,
        "Int" | "Int8" | "Int16" | "Int32" | "Int64" | "UInt" | "UInt8" | "UInt16" | "UInt32"
        | "UInt64" | "Bool" | "Byte" | "Char" | "Unit" | "String" => DeriveSupport::Yes,
        "List" | "Option" => match ty.args.first() {
            Some(arg) => hash_key_support(arg, local_types, generic_params),
            None => DeriveSupport::Unknown,
        },
        "Result" if !ty.args.is_empty() => ty.args.iter().fold(DeriveSupport::Yes, |acc, arg| {
            combine_support(acc, hash_key_support(arg, local_types, generic_params))
        }),
        // `HashMap`/`HashSet` are not themselves `Hash`.
        "Map" | "Set" => DeriveSupport::No,
        _ => match local_types.get(root) {
            // A local type used as a key must derive `Hash`, and its `#[derive(Hash)]`
            // adds `Arg: Hash` for each generic argument.
            Some(local) if local.is_value => {
                if !local.derives.contains("Hash") {
                    return DeriveSupport::No;
                }
                ty.args.iter().fold(DeriveSupport::Yes, |acc, arg| {
                    combine_support(acc, hash_key_support(arg, local_types, generic_params))
                })
            }
            _ => DeriveSupport::Unknown,
        },
    }
}

/// Whether a field supports `JsonEncode`/`JsonDecode`. Builtin scalars and
/// containers go through serde, but a `Deserialize` impl for `HashMap`/`HashSet`
/// additionally requires the key/element type to be `Eq + Hash`, so a `Float`
/// key is rejected for `JsonDecode`.
fn json_support(
    ty: &TypeRef,
    root: &str,
    derive: &str,
    local_types: &HashMap<String, LocalTypeDerives>,
    generic_params: &HashSet<&str>,
) -> DeriveSupport {
    let decode = derive == "JsonDecode";
    let recurse = |arg: &TypeRef| field_supports_derive(arg, derive, local_types, generic_params);
    // A `Deserialize` impl for `HashMap`/`HashSet` requires the key/element to be
    // `Eq + Hash`; `hash_key_support` rejects generic parameters in that position.
    let hashable_key = |arg: &TypeRef| {
        combine_support(
            field_supports_derive(arg, "Eq", local_types, generic_params),
            hash_key_support(arg, local_types, generic_params),
        )
    };
    match root {
        "Int" | "Int8" | "Int16" | "Int32" | "Int64" | "UInt" | "UInt8" | "UInt16" | "UInt32"
        | "UInt64" | "Float" | "Float32" | "Float64" | "Bool" | "Byte" | "Char" | "Unit"
        | "String" => DeriveSupport::Yes,
        "List" | "Option" => match ty.args.first() {
            Some(arg) => recurse(arg),
            None => DeriveSupport::Unknown,
        },
        "Result" if !ty.args.is_empty() => ty.args.iter().fold(DeriveSupport::Yes, |acc, arg| {
            combine_support(acc, recurse(arg))
        }),
        "Set" => match ty.args.first() {
            Some(arg) if decode => combine_support(recurse(arg), hashable_key(arg)),
            Some(arg) => recurse(arg),
            None => DeriveSupport::Unknown,
        },
        "Map" if ty.args.len() == 2 => {
            let key = &ty.args[0];
            let value = &ty.args[1];
            let elements = combine_support(recurse(key), recurse(value));
            if decode {
                combine_support(elements, hashable_key(key))
            } else {
                elements
            }
        }
        "Map" => DeriveSupport::Unknown,
        _ => user_type_support(ty, root, derive, local_types, generic_params),
    }
}

fn user_type_support(
    ty: &TypeRef,
    root: &str,
    derive: &str,
    local_types: &HashMap<String, LocalTypeDerives>,
    generic_params: &HashSet<&str>,
) -> DeriveSupport {
    match local_types.get(root) {
        Some(local) if local.is_value => {
            let derived = match derive {
                // An `Ord`-deriving value type also gets `Eq` in generated Rust.
                "Eq" => local.derives.contains("Eq") || local.derives.contains("Ord"),
                other => local.derives.contains(other),
            };
            if !derived {
                return DeriveSupport::No;
            }
            // The local type's `#[derive(D)]` adds `Arg: D` for each generic
            // argument, so `Key<Float>` deriving `Eq` still requires `Float: Eq`.
            ty.args.iter().fold(DeriveSupport::Yes, |acc, arg| {
                combine_support(
                    acc,
                    field_supports_derive(arg, derive, local_types, generic_params),
                )
            })
        }
        _ => DeriveSupport::Unknown,
    }
}
