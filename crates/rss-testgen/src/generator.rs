//! Type-directed program generator.
//!
//! Drives a [`SeedReader`] to build a well-typed, **terminating**, deterministic
//! RSScript program: helper functions form a DAG (a function may only call
//! earlier ones — no recursion), loops are the bounded counting form, and `main`
//! prints a fixed set of observations so its stdout is comparable across
//! backends. Arithmetic edges (overflow, divide-by-zero) are *not* avoided: they
//! trap identically on every backend, so "all fail" is still agreement.
//!
//! Coverage: scalars (Int/Bool/Float/String), control flow, functions, plus
//! `struct`s, nullary `sum`s, `Option`, and `Result` (with the `?` operator).
//! Values of un-printable shapes (`Float`, compounds) are observed only through
//! comparisons / destructuring reduced to a printable scalar, so backend
//! float-formatting and hash-iteration-order differences never reach stdout.

use crate::seed::SeedReader;
use crate::ty::{Binding, FnSig, Scope, StructDef, SumDef, Ty};

/// A generated program plus the metadata a harness needs.
#[derive(Debug, Clone)]
pub struct GeneratedProgram {
    pub source: String,
    /// `true` once async generation lands (T5); always `false` for now.
    pub is_async: bool,
}

/// Generate a program from a raw seed. Total over any `&[u8]`.
pub fn generate(seed: &[u8]) -> GeneratedProgram {
    Generator::new(seed).program()
}

const SCALARS: &[Ty] = &[Ty::Int, Ty::Bool, Ty::Float, Ty::String];

struct Generator<'a> {
    seed: SeedReader<'a>,
    var_counter: usize,
    structs: Vec<StructDef>,
    sums: Vec<SumDef>,
    /// Names of declared unbounded generic helper functions `fn gN<T>(p: read T)
    /// -> Int`. Called with varied concrete type args to exercise monomorphization.
    generic_fns: Vec<String>,
}

impl<'a> Generator<'a> {
    fn new(seed: &'a [u8]) -> Self {
        Generator {
            seed: SeedReader::new(seed),
            var_counter: 0,
            structs: Vec::new(),
            sums: Vec::new(),
            generic_fns: Vec::new(),
        }
    }

    fn fresh_var(&mut self) -> String {
        let name = format!("v{}", self.var_counter);
        self.var_counter += 1;
        name
    }

    fn pick_scalar(&mut self) -> Ty {
        SCALARS[self.seed.choice(SCALARS.len())].clone()
    }

    /// A scalar usable as a `Map` key / `Set` element (`Hashable`): no `Float`.
    fn pick_hashable_scalar(&mut self) -> Ty {
        const HASHABLE: &[Ty] = &[Ty::Int, Ty::Bool, Ty::String];
        HASHABLE[self.seed.choice(HASHABLE.len())].clone()
    }

    /// A type for a binding/param/return: scalars dominate; compounds appear when
    /// available. Compound payloads are restricted to scalars to keep the surface
    /// finite and generation simple.
    fn pick_type(&mut self) -> Ty {
        match self.seed.weighted(&[10, 2, 2, 3, 3, 2, 2, 2, 1]) {
            0 => self.pick_scalar(),
            1 => Ty::Option(Box::new(self.pick_scalar())),
            2 => Ty::Result(Box::new(self.pick_scalar()), Box::new(Ty::String)),
            3 if !self.structs.is_empty() => Ty::Struct(
                self.structs[self.seed.choice(self.structs.len())]
                    .name
                    .clone(),
            ),
            4 if !self.sums.is_empty() => {
                Ty::Sum(self.sums[self.seed.choice(self.sums.len())].name.clone())
            }
            5 => Ty::List(Box::new(self.pick_scalar())),
            6 => Ty::Map(
                Box::new(self.pick_hashable_scalar()),
                Box::new(self.pick_scalar()),
            ),
            7 => Ty::Set(Box::new(self.pick_hashable_scalar())),
            8 => Ty::Deque(Box::new(self.pick_scalar())),
            _ => self.pick_scalar(),
        }
    }

    // -- top level -------------------------------------------------------

    fn program(&mut self) -> GeneratedProgram {
        // Always declare `features: local` so generated `local` bindings are
        // allowed; the feature is harmless when unused.
        let mut source = String::from("features: local\n\n");

        let struct_count = self.seed.choice(3); // 0..=2
        for index in 0..struct_count {
            let def = self.gen_struct_decl(index, &mut source);
            self.structs.push(def);
        }
        let sum_count = self.seed.choice(3); // 0..=2
        for index in 0..sum_count {
            let def = self.gen_sum_decl(index, &mut source);
            self.sums.push(def);
        }

        let generic_count = self.seed.choice(3); // 0..=2
        for index in 0..generic_count {
            let name = format!("g{index}");
            let body = self.seed.range_i64(0, 1000);
            // Unbounded `T`: the body can't operate on an opaque value, so it
            // returns a constant; the coverage is in monomorphizing the call sites.
            source.push_str(&format!(
                "fn {name}<T>(p: read T) -> Int {{\n    return {body}\n}}\n\n"
            ));
            self.generic_fns.push(name);
        }

        let mut functions: Vec<FnSig> = Vec::new();
        let fn_count = self.seed.choice(4); // 0..=3 helpers
        for index in 0..fn_count {
            let sig = self.gen_fn(index, &functions, &mut source);
            functions.push(sig);
        }

        self.gen_main(&functions, &mut source);
        GeneratedProgram {
            source,
            is_async: false,
        }
    }

    // -- type declarations -----------------------------------------------

    fn gen_struct_decl(&mut self, index: usize, out: &mut String) -> StructDef {
        let name = format!("S{index}");
        let field_count = 1 + self.seed.choice(3); // 1..=3
        let mut fields = Vec::new();
        for f in 0..field_count {
            fields.push((format!("f{f}"), self.pick_scalar()));
        }
        let body = fields
            .iter()
            .map(|(n, t)| format!("    {n}: {}", t.render()))
            .collect::<Vec<_>>()
            .join("\n");
        // `derives(Clone)` so the struct can be freely read/copied in arguments.
        out.push_str(&format!("struct {name} derives(Clone) {{\n{body}\n}}\n\n"));
        StructDef { name, fields }
    }

    fn gen_sum_decl(&mut self, index: usize, out: &mut String) -> SumDef {
        let name = format!("E{index}");
        let variant_count = 2 + self.seed.choice(3); // 2..=4
        let variants: Vec<String> = (0..variant_count).map(|v| format!("{name}V{v}")).collect();
        let body = variants
            .iter()
            .map(|v| format!("    {v}"))
            .collect::<Vec<_>>()
            .join("\n");
        out.push_str(&format!("sum {name} derives(Clone) {{\n{body}\n}}\n\n"));
        SumDef { name, variants }
    }

    // -- helper functions ------------------------------------------------

    fn gen_fn(&mut self, index: usize, earlier: &[FnSig], out: &mut String) -> FnSig {
        let name = format!("f{index}");
        let param_count = 1 + self.seed.choice(3); // 1..=3
        let mut params = Vec::new();
        let mut scope = Scope {
            functions: earlier.to_vec(),
            ..Scope::default()
        };
        for _ in 0..param_count {
            let ty = self.pick_type();
            let pname = self.fresh_var();
            params.push((pname.clone(), ty.clone()));
            scope.bindings.push(Binding {
                name: pname,
                ty,
                mutable: false,
            });
        }
        let ret = self.pick_type();
        // A `Result<_, String>`-returning function may use `?`.
        if let Ty::Result(_, err) = &ret {
            scope.result_error = Some((**err).clone());
        }

        let param_src = params
            .iter()
            .map(|(n, t)| self.render_param(n, t))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("fn {name}({param_src}) -> {} {{\n", ret.render()));

        let stmt_count = self.seed.choice(3); // 0..=2
        for _ in 0..stmt_count {
            let stmt = self.gen_stmt(&mut scope);
            out.push_str(&format!("    {stmt}\n"));
        }
        let ret_expr = self.gen_expr(&ret, &scope, 3);
        out.push_str(&format!("    return {ret_expr}\n}}\n\n"));

        FnSig { name, params, ret }
    }

    fn render_param(&self, name: &str, ty: &Ty) -> String {
        if ty.param_is_read() {
            format!("{name}: read {}", ty.render())
        } else {
            format!("{name}: {}", ty.render())
        }
    }

    // -- main ------------------------------------------------------------

    fn gen_main(&mut self, functions: &[FnSig], out: &mut String) {
        let mut scope = Scope {
            functions: functions.to_vec(),
            ..Scope::default()
        };
        // main may return Result<Unit, String> to host `?`.
        let main_is_result = self.seed.bool() && self.any_fn_returns_result(functions);
        if main_is_result {
            scope.result_error = Some(Ty::String);
            out.push_str("fn main() -> Result<Unit, String> {\n");
        } else {
            out.push_str("fn main() -> Unit {\n");
        }

        let setup = 2 + self.seed.choice(4); // 2..=5
        for _ in 0..setup {
            let stmt = self.gen_stmt(&mut scope);
            out.push_str(&format!("    {stmt}\n"));
        }

        let control = self.seed.choice(3); // 0..=2
        for _ in 0..control {
            let block = self.gen_control(&mut scope);
            out.push_str(&block);
        }

        let observations = 1 + self.seed.choice(4); // 1..=4
        for _ in 0..observations {
            let line = self.gen_observation(&mut scope);
            out.push_str(&line);
        }

        if main_is_result {
            out.push_str("    return Ok(Unit)\n}\n");
        } else {
            out.push_str("    return Unit\n}\n");
        }
    }

    fn any_fn_returns_result(&self, functions: &[FnSig]) -> bool {
        functions.iter().any(|f| matches!(f.ret, Ty::Result(_, _)))
    }

    /// One or more `Log.write` lines observing a freshly generated value. Compound
    /// values are destructured down to a printable scalar.
    fn gen_observation(&mut self, scope: &mut Scope) -> String {
        let ty = self.pick_type();
        let expr = self.gen_expr(&ty, scope, 3);
        let binding = self.fresh_var();
        // Annotate the binding with its full type. For `Result<T, E>` this pins
        // `E`, which a bare `let v = Ok(x)` leaves unconstrained — accepted by the
        // checker but un-lowerable to Rust (E0282). Harmless for other types.
        let mut out = format!("    let {binding}: {} = {expr}\n", ty.render());
        out.push_str(&self.observe(&binding, &ty, 1));
        out
    }

    /// Emit statements that print a scalar derived from the value named `access`
    /// of type `ty`. `indent` is the current block depth (1 = main body).
    fn observe(&mut self, access: &str, ty: &Ty, indent: usize) -> String {
        let pad = "    ".repeat(indent);
        match ty {
            Ty::Int | Ty::Bool | Ty::String | Ty::Float => {
                format!("{pad}{}\n", self.log_scalar(access, ty))
            }
            Ty::Option(inner) => {
                let some = self.observe("x", inner, indent + 1);
                let inner_pad = "    ".repeat(indent + 1);
                format!(
                    "{pad}match {access} {{\n\
                     {pad}    Some(x) => {{\n{some}{inner_pad}}}\n\
                     {pad}    None => {{ Log.write(message: read \"none\") }}\n\
                     {pad}}}\n"
                )
            }
            Ty::Result(ok, _) => {
                let ok_obs = self.observe("x", ok, indent + 1);
                let inner_pad = "    ".repeat(indent + 1);
                format!(
                    "{pad}match {access} {{\n\
                     {pad}    Ok(x) => {{\n{ok_obs}{inner_pad}}}\n\
                     {pad}    Err(e) => {{ Log.write(message: read e) }}\n\
                     {pad}}}\n"
                )
            }
            Ty::Struct(name) => {
                let def = self.struct_def(name);
                let (fname, fty) = def.fields[0].clone();
                self.observe(&format!("{access}.{fname}"), &fty, indent)
            }
            Ty::Sum(name) => {
                let def = self.sum_def(name);
                let arms = def
                    .variants
                    .iter()
                    .map(|v| format!("{pad}    {v} => {{ Log.write(message: read \"{v}\") }}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                format!("{pad}match {access} {{\n{arms}\n{pad}}}\n")
            }
            // Collections are observed by their length (a deterministic Int);
            // element iteration order isn't guaranteed identical across backends.
            Ty::List(_) => {
                let len = format!("List.len(list: read {access})");
                format!("{pad}{}\n", self.log_scalar(&len, &Ty::Int))
            }
            Ty::Map(_, _) => {
                let len = format!("Map.len(map: read {access})");
                format!("{pad}{}\n", self.log_scalar(&len, &Ty::Int))
            }
            Ty::Set(_) => {
                let len = format!("Set.len(set: read {access})");
                format!("{pad}{}\n", self.log_scalar(&len, &Ty::Int))
            }
            Ty::Deque(_) => {
                let len = format!("Deque.len(deque: read {access})");
                format!("{pad}{}\n", self.log_scalar(&len, &Ty::Int))
            }
        }
    }

    /// A `Log.write(...)` line printing a *scalar* access expression.
    fn log_scalar(&self, access: &str, ty: &Ty) -> String {
        let message = match ty {
            Ty::Int => format!("String.from_int(value: {access})"),
            Ty::Bool => format!("String.from_bool(value: {access})"),
            Ty::String => access.to_string(),
            Ty::Float => format!("String.from_bool(value: ({access} > 0.0))"),
            _ => unreachable!("log_scalar called on non-scalar"),
        };
        format!("Log.write(message: read {message})")
    }

    fn struct_def(&self, name: &str) -> StructDef {
        self.structs
            .iter()
            .find(|d| d.name == name)
            .expect("declared struct")
            .clone()
    }

    fn sum_def(&self, name: &str) -> SumDef {
        self.sums
            .iter()
            .find(|d| d.name == name)
            .expect("declared sum")
            .clone()
    }

    // -- statements ------------------------------------------------------

    fn gen_stmt(&mut self, scope: &mut Scope) -> String {
        // Bias toward lets; sometimes assign, a `?`-unwrap, or a populated
        // collection.
        match self.seed.weighted(&[6, 2, 2, 3, 2, 1, 2]) {
            0 => self.gen_let(scope),
            1 => self
                .gen_assign(scope)
                .unwrap_or_else(|| self.gen_let(scope)),
            2 => self
                .gen_try(scope)
                .or_else(|| self.gen_assign(scope))
                .unwrap_or_else(|| self.gen_let(scope)),
            3 => self.gen_collection_stmt(scope),
            4 => self.gen_closure_stmt(scope),
            5 => self.gen_cancellation_stmt(),
            _ => self.gen_counter_stmt(),
        }
    }

    /// A `Counter` resource: create, add a few times, observe the value. A
    /// runtime-backed mutable resource whose final value is deterministic, so it
    /// exercises resource construction / `mut` receiver calls across backends.
    fn gen_counter_stmt(&mut self) -> String {
        let counter = self.fresh_var();
        let start = self.seed.range_i64(0, 1000);
        let mut lines = vec![format!("let {counter} = Counter.new(value: {start})")];
        let adds = self.seed.choice(3); // 0..=2
        for _ in 0..adds {
            let amount = self.seed.range_i64(0, 1000);
            lines.push(format!(
                "Counter.add(counter: mut {counter}, amount: {amount})"
            ));
        }
        lines.push(format!(
            "Log.write(message: read String.from_int(value: Counter.value(counter: read {counter})))"
        ));
        lines.join("\n    ")
    }

    /// Deterministic concurrency primitive: create a `CancellationSource`, observe
    /// its token is not cancelled, cancel it, observe it is. Both observations are
    /// backend-deterministic (no scheduler/timing), exercising the cancellation
    /// runtime objects' lowering across backends.
    fn gen_cancellation_stmt(&mut self) -> String {
        let source = self.fresh_var();
        let token = self.fresh_var();
        let observe = |token: &str| {
            format!(
                "Log.write(message: read String.from_bool(value: \
                 CancellationToken.is_cancelled(token: read {token})))"
            )
        };
        [
            format!("let mut {source} = CancellationSource.new()"),
            format!("let {token} = CancellationSource.token(source: read {source})"),
            observe(&token),
            format!("CancellationSource.cancel(source: mut {source})"),
            observe(&token),
        ]
        .join("\n    ")
    }

    /// Build a populated `List<T>` and `List.filter` it with a capture-free
    /// closure predicate, observing the filtered length. Exercises closure
    /// lowering (parameter, body, `noescape Fn(T) -> Bool`) without capturing
    /// outer state, and the length is deterministic across backends (insertion
    /// order is preserved, unlike `Set`/`Map` iteration).
    fn gen_closure_stmt(&mut self, scope: &mut Scope) -> String {
        let elem = self.pick_scalar();
        let list = self.fresh_var();
        let count = 1 + self.seed.choice(3); // 1..=3
        let mut lines = vec![format!(
            "let mut {list}: List<{e}> = List.new<{e}>()",
            e = elem.render()
        )];
        for _ in 0..count {
            let value = self.gen_literal(&elem);
            lines.push(format!("List.push(list: mut {list}, value: read {value})"));
        }
        let predicate = match elem {
            Ty::Int => "(x > 0)",
            Ty::Bool => "x",
            Ty::Float => "(x > 0.0)",
            Ty::String => "(String.len(value: read x) > 0)",
            _ => "true",
        };
        let result = self.fresh_var();
        lines.push(format!(
            "let {result}: List<{e}> = List.filter(list: read {list}, predicate: |x| {{ return {predicate} }})",
            e = elem.render()
        ));
        lines.push(format!(
            "Log.write(message: read String.from_int(value: List.len(list: read {result})))"
        ));
        let list_ty = Ty::List(Box::new(elem));
        scope.bindings.push(Binding {
            name: list,
            ty: list_ty.clone(),
            mutable: true,
        });
        scope.bindings.push(Binding {
            name: result,
            ty: list_ty,
            mutable: false,
        });
        lines.join("\n    ")
    }

    /// Build a populated collection: `let mut v: C = C.new<..>()` followed by a
    /// few inserts. Returned as a single (possibly multi-line) statement whose
    /// continuation lines are pre-indented to match the caller's block.
    fn gen_collection_stmt(&mut self, scope: &mut Scope) -> String {
        let name = self.fresh_var();
        // Empty collections are fine now that typed `let` bindings carry their
        // annotation through lowering (the Deque/SortedMap/SortedSet gap that made
        // an empty `C.new<T>()` un-inferable — E0282 — is fixed in the lowerer).
        let count = self.seed.choice(4); // 0..=3 elements
        let (ty, new_expr, mut inserts, len_expr): (Ty, String, Vec<String>, String) =
            match self.seed.choice(4) {
                0 => {
                    let elem = self.pick_scalar();
                    let ty = Ty::List(Box::new(elem.clone()));
                    let inserts = (0..count)
                        .map(|_| {
                            let v = self.gen_literal(&elem);
                            format!("List.push(list: mut {name}, value: read {v})")
                        })
                        .collect();
                    (
                        ty,
                        format!("List.new<{}>()", elem.render()),
                        inserts,
                        format!("List.len(list: read {name})"),
                    )
                }
                1 => {
                    let elem = self.pick_scalar();
                    let ty = Ty::Deque(Box::new(elem.clone()));
                    let inserts = (0..count)
                        .map(|_| {
                            let v = self.gen_literal(&elem);
                            format!("Deque.push_back(deque: mut {name}, value: read {v})")
                        })
                        .collect();
                    (
                        ty,
                        format!("Deque.new<{}>()", elem.render()),
                        inserts,
                        format!("Deque.len(deque: read {name})"),
                    )
                }
                2 => {
                    let elem = self.pick_hashable_scalar();
                    let ty = Ty::Set(Box::new(elem.clone()));
                    let inserts = (0..count)
                        .map(|_| {
                            let v = self.gen_literal(&elem);
                            format!("Set.insert(set: mut {name}, value: read {v})")
                        })
                        .collect();
                    (
                        ty,
                        format!("Set.new<{}>()", elem.render()),
                        inserts,
                        format!("Set.len(set: read {name})"),
                    )
                }
                _ => {
                    let key = self.pick_hashable_scalar();
                    let value = self.pick_scalar();
                    let ty = Ty::Map(Box::new(key.clone()), Box::new(value.clone()));
                    let inserts = (0..count)
                        .map(|_| {
                            let k = self.gen_literal(&key);
                            let v = self.gen_literal(&value);
                            format!("Map.insert(map: mut {name}, key: read {k}, value: read {v})")
                        })
                        .collect();
                    (
                        ty,
                        format!("Map.new<{}, {}>()", key.render(), value.render()),
                        inserts,
                        format!("Map.len(map: read {name})"),
                    )
                }
            };
        let ty_render = ty.render();
        scope.bindings.push(Binding {
            name: name.clone(),
            ty,
            mutable: true,
        });
        let mut lines = vec![format!("let mut {name}: {ty_render} = {new_expr}")];
        lines.append(&mut inserts);
        // Observe the length so the collection contributes to stdout (deterministic
        // across backends, unlike element iteration order).
        lines.push(format!(
            "Log.write(message: read String.from_int(value: {len_expr}))"
        ));
        // Caller prefixes the first line with one indent; pre-indent the rest.
        lines.join("\n    ")
    }

    fn gen_let(&mut self, scope: &mut Scope) -> String {
        let ty = self.pick_type();
        // Occasionally exercise the None/Err constructors (they need a known type,
        // unlike the inferable Some/Ok).
        let (expr, force_immutable) = match &ty {
            Ty::Option(_) if self.seed.choice(4) == 0 => ("None".to_string(), true),
            Ty::Result(_, _) if self.seed.choice(4) == 0 => {
                let msg = self.gen_literal(&Ty::String);
                (format!("Err({msg})"), true)
            }
            _ => (self.gen_expr(&ty, scope, 3), false),
        };
        let name = self.fresh_var();
        let mutable = !force_immutable && self.seed.bool();
        // Compound types are annotated. Critically, this pins `Result<T, E>`'s
        // error type: a bare `let v = Ok(x)` leaves `E` unconstrained, which the
        // checker accepts but the Rust backend cannot infer (E0282).
        let annotate = !ty.is_scalar();
        scope.bindings.push(Binding {
            name: name.clone(),
            ty: ty.clone(),
            mutable,
        });
        let mut_kw = if mutable { "mut " } else { "" };
        if annotate {
            format!("let {mut_kw}{name}: {} = {expr}", ty.render())
        } else if !mutable && ty.is_scalar() && self.seed.choice(3) == 0 {
            // Exercise the `local` binding kind for an immutable scalar (copyable,
            // so it can still be read/returned without escaping a resource).
            format!("local {name} = {expr}")
        } else {
            format!("let {mut_kw}{name} = {expr}")
        }
    }

    /// `name = <expr>` to an existing mutable binding (controlled assignment).
    fn gen_assign(&mut self, scope: &Scope) -> Option<String> {
        let ty = self.pick_type();
        let candidates = scope.mutable_bindings_of(&ty);
        if candidates.is_empty() {
            return None;
        }
        let name = candidates[self.seed.choice(candidates.len())].name.clone();
        let expr = self.gen_expr(&ty, scope, 3);
        Some(format!("{name} = {expr}"))
    }

    /// `let v = <call returning Result<T, String>>?` — only inside a function that
    /// itself returns `Result<_, String>`.
    fn gen_try(&mut self, scope: &mut Scope) -> Option<String> {
        scope.result_error.as_ref()?;
        // Find an earlier function returning Result<T, String> for some T.
        let candidates: Vec<FnSig> = scope
            .functions
            .iter()
            .filter(|f| matches!(&f.ret, Ty::Result(_, err) if **err == Ty::String))
            .cloned()
            .collect();
        if candidates.is_empty() {
            return None;
        }
        let sig = candidates[self.seed.choice(candidates.len())].clone();
        let Ty::Result(ok, _) = &sig.ret else {
            return None;
        };
        let ok_ty = (**ok).clone();
        let call = self.render_call(&sig, scope);
        let name = self.fresh_var();
        scope.bindings.push(Binding {
            name: name.clone(),
            ty: ok_ty,
            mutable: false,
        });
        Some(format!("let {name} = {call}?"))
    }

    /// An `if` or a bounded counting `while`. Bodies see a cloned scope so any
    /// bindings they introduce don't leak.
    fn gen_control(&mut self, scope: &mut Scope) -> String {
        if self.seed.bool() {
            let cond = self.gen_expr(&Ty::Bool, scope, 2);
            let mut body_scope = scope.clone();
            let then_stmt = self.gen_stmt(&mut body_scope);
            let mut out = format!("    if {cond} {{\n        {then_stmt}\n");
            if self.seed.bool() {
                let mut else_scope = scope.clone();
                let else_stmt = self.gen_stmt(&mut else_scope);
                out.push_str(&format!("    }} else {{\n        {else_stmt}\n    }}\n"));
            } else {
                out.push_str("    }\n");
            }
            out
        } else {
            let counter = self.fresh_var();
            let limit = 1 + self.seed.range_i64(0, 5); // 1..=6
            let mut body_scope = scope.clone();
            let body_stmt = self
                .gen_assign(&body_scope)
                .unwrap_or_else(|| self.gen_let(&mut body_scope));
            format!(
                "    let mut {counter} = 0\n    while {counter} < {limit} {{\n        \
                 {body_stmt}\n        {counter} = {counter} + 1\n    }}\n"
            )
        }
    }

    // -- expressions -----------------------------------------------------

    fn gen_expr(&mut self, ty: &Ty, scope: &Scope, fuel: u32) -> String {
        if fuel == 0 {
            return self.gen_atom(ty, scope);
        }
        // Occasionally satisfy an `Int` target with a monomorphized generic call.
        if *ty == Ty::Int && !self.generic_fns.is_empty() && fuel > 0 && self.seed.choice(4) == 0 {
            return self.gen_generic_call(scope, fuel);
        }
        match self.seed.weighted(&[3, 2, 4]) {
            0 => self.gen_atom(ty, scope),
            1 => self
                .gen_call(ty, scope, fuel)
                .unwrap_or_else(|| self.gen_atom(ty, scope)),
            _ => self.gen_compound(ty, scope, fuel),
        }
    }

    /// A leaf: an in-scope variable, or a freshly constructed value.
    fn gen_atom(&mut self, ty: &Ty, scope: &Scope) -> String {
        let vars = scope.bindings_of(ty);
        if !vars.is_empty() && self.seed.bool() {
            return vars[self.seed.choice(vars.len())].name.clone();
        }
        self.gen_construct(ty, scope)
    }

    /// Construct a value of `ty` (literal for scalars; constructor for compounds).
    fn gen_construct(&mut self, ty: &Ty, scope: &Scope) -> String {
        match ty {
            Ty::Int | Ty::Bool | Ty::Float | Ty::String => self.gen_literal(ty),
            Ty::Option(inner) => {
                let value = self.gen_expr(inner, scope, 1);
                format!("Some({value})")
            }
            Ty::Result(ok, _) => {
                let value = self.gen_expr(ok, scope, 1);
                format!("Ok({value})")
            }
            Ty::Struct(name) => {
                let def = self.struct_def(name);
                let args = def
                    .fields
                    .iter()
                    .map(|(fname, fty)| {
                        let value = self.gen_literal(fty);
                        format!("{fname}: {value}")
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{name}({args})")
            }
            Ty::Sum(name) => {
                let def = self.sum_def(name);
                def.variants[self.seed.choice(def.variants.len())].clone()
            }
            // Collections are constructed empty here; populated instances are
            // built by `gen_collection_stmt` (new + a few inserts).
            Ty::List(inner) => format!("List.new<{}>()", inner.render()),
            Ty::Map(key, value) => {
                format!("Map.new<{}, {}>()", key.render(), value.render())
            }
            Ty::Set(inner) => format!("Set.new<{}>()", inner.render()),
            Ty::Deque(inner) => format!("Deque.new<{}>()", inner.render()),
        }
    }

    fn gen_literal(&mut self, ty: &Ty) -> String {
        match ty {
            // Non-negative only: RSScript has no negative-literal / unary-minus
            // parse surface (`-3` is RS0015; negatives are written `0 - 3`).
            Ty::Int => self.seed.range_i64(0, 1000).to_string(),
            Ty::Bool => if self.seed.bool() { "true" } else { "false" }.to_string(),
            Ty::Float => {
                let whole = self.seed.range_i64(0, 1000);
                let frac = self.seed.range_i64(0, 999);
                format!("{whole}.{frac}")
            }
            Ty::String => {
                let len = self.seed.choice(7); // 0..=6
                const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789 ";
                let mut s = String::with_capacity(len);
                for _ in 0..len {
                    let idx = self.seed.choice(ALPHABET.len());
                    s.push(ALPHABET[idx] as char);
                }
                format!("\"{s}\"")
            }
            // Compounds are not "literals"; construct them via gen_construct.
            other => self.gen_construct_scalar_fallback(other),
        }
    }

    /// Fallback when `gen_literal` is asked for a compound (only happens for struct
    /// fields, which are restricted to scalars, so this is effectively unreachable
    /// — but stay total rather than panic).
    fn gen_construct_scalar_fallback(&mut self, _ty: &Ty) -> String {
        "0".to_string()
    }

    /// A call to an earlier-declared function returning `ty`.
    fn gen_call(&mut self, ty: &Ty, scope: &Scope, fuel: u32) -> Option<String> {
        let candidates = scope.functions_returning(ty);
        if candidates.is_empty() {
            return None;
        }
        let sig = candidates[self.seed.choice(candidates.len())].clone();
        Some(self.render_call_with_fuel(&sig, scope, fuel))
    }

    fn render_call(&mut self, sig: &FnSig, scope: &Scope) -> String {
        self.render_call_with_fuel(sig, scope, 2)
    }

    fn render_call_with_fuel(&mut self, sig: &FnSig, scope: &Scope, fuel: u32) -> String {
        let args = sig
            .params
            .iter()
            .map(|(pname, pty)| {
                let arg = self.gen_expr(pty, scope, fuel.saturating_sub(1));
                if pty.param_is_read() {
                    format!("{pname}: read {arg}")
                } else {
                    format!("{pname}: {arg}")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("{}({args})", sig.name)
    }

    /// `gN<C>(p: read <arg>)` — a generic helper instantiated at a concrete scalar
    /// type `C`. Returns `Int`. Different call sites monomorphize to distinct Rust
    /// instantiations while the VM erases the type — both must agree.
    fn gen_generic_call(&mut self, scope: &Scope, fuel: u32) -> String {
        let name = self.generic_fns[self.seed.choice(self.generic_fns.len())].clone();
        let concrete = self.pick_scalar();
        let arg = self.gen_expr(&concrete, scope, fuel.saturating_sub(1));
        format!("{name}<{}>(p: read {arg})", concrete.render())
    }

    /// A type-specific compound expression.
    fn gen_compound(&mut self, ty: &Ty, scope: &Scope, fuel: u32) -> String {
        match ty {
            Ty::Int => {
                let ops = ["+", "-", "*", "/", "%"];
                let op = ops[self.seed.choice(ops.len())];
                let lhs = self.gen_expr(&Ty::Int, scope, fuel - 1);
                let rhs = self.gen_expr(&Ty::Int, scope, fuel - 1);
                format!("({lhs} {op} {rhs})")
            }
            Ty::Float => {
                let ops = ["+", "-", "*"];
                let op = ops[self.seed.choice(ops.len())];
                let lhs = self.gen_expr(&Ty::Float, scope, fuel - 1);
                let rhs = self.gen_expr(&Ty::Float, scope, fuel - 1);
                format!("({lhs} {op} {rhs})")
            }
            Ty::Bool => self.gen_bool_compound(scope, fuel),
            Ty::String => match self.seed.choice(3) {
                0 => {
                    let lhs = self.gen_expr(&Ty::String, scope, fuel - 1);
                    let rhs = self.gen_expr(&Ty::String, scope, fuel - 1);
                    format!("String.concat(left: read {lhs}, right: read {rhs})")
                }
                1 => {
                    let inner = self.gen_expr(&Ty::Int, scope, fuel - 1);
                    format!("String.from_int(value: {inner})")
                }
                _ => {
                    let inner = self.gen_expr(&Ty::Bool, scope, fuel - 1);
                    format!("String.from_bool(value: {inner})")
                }
            },
            // Compounds have no operator forms: just construct.
            Ty::Option(_)
            | Ty::Result(_, _)
            | Ty::Struct(_)
            | Ty::Sum(_)
            | Ty::List(_)
            | Ty::Map(_, _)
            | Ty::Set(_)
            | Ty::Deque(_) => self.gen_construct(ty, scope),
        }
    }

    fn gen_bool_compound(&mut self, scope: &Scope, fuel: u32) -> String {
        match self.seed.choice(4) {
            0 | 1 => {
                let operand = if self.seed.bool() { Ty::Int } else { Ty::Float };
                let cmps = ["<", "<=", ">", ">=", "==", "!="];
                let cmp = cmps[self.seed.choice(cmps.len())];
                let lhs = self.gen_expr(&operand, scope, fuel - 1);
                let rhs = self.gen_expr(&operand, scope, fuel - 1);
                format!("({lhs} {cmp} {rhs})")
            }
            2 => {
                let cmp = if self.seed.bool() { "==" } else { "!=" };
                let lhs = self.gen_expr(&Ty::Bool, scope, fuel - 1);
                let rhs = self.gen_expr(&Ty::Bool, scope, fuel - 1);
                format!("({lhs} {cmp} {rhs})")
            }
            _ => {
                let op = if self.seed.bool() { "&&" } else { "||" };
                let lhs = self.gen_expr(&Ty::Bool, scope, fuel - 1);
                let rhs = self.gen_expr(&Ty::Bool, scope, fuel - 1);
                format!("({lhs} {op} {rhs})")
            }
        }
    }
}
