//! The generated-program type model and generation scope.
//!
//! Kept deliberately small and `rsscript`-independent: it mirrors the *surface*
//! types a generated program uses, not the compiler's internal type
//! representation. The generator only ever emits well-typed references, calls,
//! constructions, and matches by consulting the scope and the declared user
//! types.

/// A surface type the generator can produce values of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    Int,
    Bool,
    Float,
    String,
    Option(Box<Ty>),
    /// `Result<T, E>`. The generator fixes `E = String` so `?` chains compose.
    Result(Box<Ty>, Box<Ty>),
    /// A declared `struct` by name.
    Struct(String),
    /// A declared (nullary-variant) `sum` by name.
    Sum(String),
    List(Box<Ty>),
    Map(Box<Ty>, Box<Ty>),
    Set(Box<Ty>),
    Deque(Box<Ty>),
}

impl Ty {
    /// The RSScript spelling of the type.
    pub fn render(&self) -> String {
        match self {
            Ty::Int => "Int".to_string(),
            Ty::Bool => "Bool".to_string(),
            Ty::Float => "Float".to_string(),
            Ty::String => "String".to_string(),
            Ty::Option(inner) => format!("Option<{}>", inner.render()),
            Ty::Result(ok, err) => format!("Result<{}, {}>", ok.render(), err.render()),
            Ty::Struct(name) | Ty::Sum(name) => name.clone(),
            Ty::List(inner) => format!("List<{}>", inner.render()),
            Ty::Map(key, value) => format!("Map<{}, {}>", key.render(), value.render()),
            Ty::Set(inner) => format!("Set<{}>", inner.render()),
            Ty::Deque(inner) => format!("Deque<{}>", inner.render()),
        }
    }

    /// A builtin scalar usable as a `Map` key / `Set` element (`Hashable`):
    /// `Int`/`Bool`/`String` (not `Float`).
    pub fn is_hashable_scalar(&self) -> bool {
        matches!(self, Ty::Int | Ty::Bool | Ty::String)
    }

    /// Whether a *parameter* of this type is declared with a `read` effect prefix.
    /// Copy scalars (Int/Bool/Float) are declared bare (`a: Int`); every reference
    /// type (`String`, `Option`, `Result`, structs, sums) is declared `x: read T`.
    pub fn param_is_read(&self) -> bool {
        !matches!(self, Ty::Int | Ty::Bool | Ty::Float)
    }

    /// Whether a value of this type can be printed directly and deterministically
    /// (via `String.from_int` / `String.from_bool` / itself). `Float` is excluded
    /// (formatting isn't backend-stable; it is observed through comparisons), as
    /// are all compound types (observed by destructuring).
    pub fn is_printable_scalar(&self) -> bool {
        matches!(self, Ty::Int | Ty::Bool | Ty::String)
    }

    /// A scalar/string type (no further structure to destructure).
    pub fn is_scalar(&self) -> bool {
        matches!(self, Ty::Int | Ty::Bool | Ty::Float | Ty::String)
    }
}

/// An in-scope binding.
#[derive(Debug, Clone)]
pub struct Binding {
    pub name: String,
    pub ty: Ty,
    pub mutable: bool,
}

/// A declared function the generator may call.
#[derive(Debug, Clone)]
pub struct FnSig {
    pub name: String,
    pub params: Vec<(String, Ty)>,
    pub ret: Ty,
}

/// A declared `struct` with named, scalar/string-typed fields.
#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<(String, Ty)>,
}

/// A declared `sum` with nullary variants.
#[derive(Debug, Clone)]
pub struct SumDef {
    pub name: String,
    pub variants: Vec<String>,
}

/// Lexical scope during generation: visible bindings plus the functions declared
/// so far. Function calls are restricted to *earlier* declarations (a DAG), which
/// guarantees generated programs terminate (no mutual or self recursion).
#[derive(Debug, Default, Clone)]
pub struct Scope {
    pub bindings: Vec<Binding>,
    pub functions: Vec<FnSig>,
    /// The enclosing function's return type, when it is a `Result<_, String>` —
    /// gates whether the `?` operator may be emitted.
    pub result_error: Option<Ty>,
}

impl Scope {
    pub fn bindings_of(&self, ty: &Ty) -> Vec<&Binding> {
        self.bindings.iter().filter(|b| &b.ty == ty).collect()
    }

    pub fn mutable_bindings_of(&self, ty: &Ty) -> Vec<&Binding> {
        self.bindings
            .iter()
            .filter(|b| b.mutable && &b.ty == ty)
            .collect()
    }

    pub fn functions_returning(&self, ty: &Ty) -> Vec<&FnSig> {
        self.functions.iter().filter(|f| &f.ret == ty).collect()
    }
}
