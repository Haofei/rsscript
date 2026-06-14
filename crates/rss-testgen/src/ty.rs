//! The generated-program type model and generation scope.
//!
//! Kept deliberately small and `rsscript`-independent: it mirrors the *surface*
//! types a generated program uses, not the compiler's internal type
//! representation. New tiers extend [`Ty`] (structs, sums, collections, …); the
//! scope tracks what is in scope so the generator only ever emits well-typed
//! references and calls.

/// A surface type the generator can produce values of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    Int,
    Bool,
    Float,
    String,
}

impl Ty {
    /// The RSScript spelling of the type.
    pub fn render(&self) -> &'static str {
        match self {
            Ty::Int => "Int",
            Ty::Bool => "Bool",
            Ty::Float => "Float",
            Ty::String => "String",
        }
    }

    /// Whether a *parameter* of this type is declared with a `read` effect prefix.
    /// Copy scalars (Int/Bool/Float) are declared bare (`a: Int`); reference types
    /// like `String` are declared `s: read String` (matches the stdlib `.rssi`
    /// conventions and the example corpus).
    pub fn param_is_read(&self) -> bool {
        matches!(self, Ty::String)
    }

    /// Whether a value of this type can be printed deterministically across every
    /// backend. `Float` is excluded: its textual formatting is not guaranteed
    /// identical between the VM and the compiled backend, so floats are only ever
    /// *observed* through comparisons reduced to `Bool`.
    pub fn is_printable(&self) -> bool {
        matches!(self, Ty::Int | Ty::Bool | Ty::String)
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

/// Lexical scope during generation: visible bindings plus the functions declared
/// so far. Function calls are restricted to *earlier* declarations (a DAG), which
/// guarantees generated programs terminate (no mutual or self recursion).
#[derive(Debug, Default, Clone)]
pub struct Scope {
    pub bindings: Vec<Binding>,
    pub functions: Vec<FnSig>,
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
