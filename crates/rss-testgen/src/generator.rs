//! Type-directed program generator.
//!
//! Drives a [`SeedReader`] to build a well-typed, **terminating**, deterministic
//! RSScript program: helper functions form a DAG (a function may only call
//! earlier ones — no recursion), loops are the bounded counting form, and `main`
//! prints a fixed set of observations so its stdout is comparable across
//! backends. Arithmetic edges (overflow, divide-by-zero) are *not* avoided: they
//! trap identically on every backend, so "all fail" is still agreement.
//!
//! Generated values of un-printable types (`Float`) are observed only through
//! comparisons reduced to `Bool`, so backend float-formatting differences never
//! reach stdout.

use crate::seed::SeedReader;
use crate::ty::{Binding, FnSig, Scope, Ty};

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

const ALL_TYPES: &[Ty] = &[Ty::Int, Ty::Bool, Ty::Float, Ty::String];

struct Generator<'a> {
    seed: SeedReader<'a>,
    var_counter: usize,
}

impl<'a> Generator<'a> {
    fn new(seed: &'a [u8]) -> Self {
        Generator {
            seed: SeedReader::new(seed),
            var_counter: 0,
        }
    }

    fn fresh_var(&mut self) -> String {
        let name = format!("v{}", self.var_counter);
        self.var_counter += 1;
        name
    }

    fn pick_type(&mut self) -> Ty {
        ALL_TYPES[self.seed.choice(ALL_TYPES.len())].clone()
    }

    // -- top level -------------------------------------------------------

    fn program(&mut self) -> GeneratedProgram {
        let mut source = String::new();
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

        // Signature line.
        let param_src = params
            .iter()
            .map(|(n, t)| {
                if t.param_is_read() {
                    format!("{n}: read {}", t.render())
                } else {
                    format!("{n}: {}", t.render())
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("fn {name}({param_src}) -> {} {{\n", ret.render()));

        // A couple of optional let bindings, then `return <ret expr>`.
        let stmt_count = self.seed.choice(3); // 0..=2
        for _ in 0..stmt_count {
            let stmt = self.gen_let(&mut scope);
            out.push_str(&format!("    {stmt}\n"));
        }
        let ret_expr = self.gen_expr(&ret, &scope, 3);
        out.push_str(&format!("    return {ret_expr}\n}}\n\n"));

        FnSig { name, params, ret }
    }

    // -- main ------------------------------------------------------------

    fn gen_main(&mut self, functions: &[FnSig], out: &mut String) {
        let mut scope = Scope {
            functions: functions.to_vec(),
            ..Scope::default()
        };
        out.push_str("fn main() -> Unit {\n");

        let setup = 2 + self.seed.choice(4); // 2..=5 statements
        for _ in 0..setup {
            let stmt = self.gen_stmt(&mut scope);
            out.push_str(&format!("    {stmt}\n"));
        }

        // Optional control blocks (bounded; always terminating).
        let control = self.seed.choice(3); // 0..=2
        for _ in 0..control {
            let block = self.gen_control(&mut scope);
            out.push_str(&block);
        }

        // At least one observation so stdout is non-empty and deterministic.
        let observations = 1 + self.seed.choice(4); // 1..=4
        for _ in 0..observations {
            let line = self.gen_observation(&scope);
            out.push_str(&format!("    {line}\n"));
        }

        out.push_str("    return Unit\n}\n");
    }

    /// A `Log.write(...)` line printing an Int/Bool/String observation.
    fn gen_observation(&mut self, scope: &Scope) -> String {
        // Pick a printable type.
        let printable = [Ty::Int, Ty::Bool, Ty::String];
        let ty = printable[self.seed.choice(printable.len())].clone();
        let expr = self.gen_expr(&ty, scope, 3);
        let message = match ty {
            Ty::Int => format!("String.from_int(value: {expr})"),
            Ty::Bool => format!("String.from_bool(value: {expr})"),
            Ty::String => expr,
            Ty::Float => unreachable!("float is not printable"),
        };
        format!("Log.write(message: read {message})")
    }

    // -- statements ------------------------------------------------------

    fn gen_stmt(&mut self, scope: &mut Scope) -> String {
        // Bias toward lets so later statements have material to reference.
        match self.seed.weighted(&[5, 2]) {
            0 => self.gen_let(scope),
            _ => self
                .gen_assign(scope)
                .unwrap_or_else(|| self.gen_let(scope)),
        }
    }

    fn gen_let(&mut self, scope: &mut Scope) -> String {
        let ty = self.pick_type();
        let expr = self.gen_expr(&ty, scope, 3);
        let name = self.fresh_var();
        let mutable = self.seed.bool();
        scope.bindings.push(Binding {
            name: name.clone(),
            ty,
            mutable,
        });
        if mutable {
            format!("let mut {name} = {expr}")
        } else {
            format!("let {name} = {expr}")
        }
    }

    /// `name = <expr>` to an existing mutable binding (controlled assignment to a
    /// local). Returns `None` if there is no mutable binding to assign to.
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

    /// An `if` or a bounded counting `while`. Bodies see a cloned scope so any
    /// bindings they introduce don't leak (and can't be observed un-initialized).
    fn gen_control(&mut self, scope: &mut Scope) -> String {
        if self.seed.bool() {
            // if [else]
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
            // Bounded counting loop, guaranteed to terminate.
            let counter = self.fresh_var();
            let limit = 1 + self.seed.range_i64(0, 5); // 1..=6
            let mut body_scope = scope.clone();
            // Mutate an existing mutable Int accumulator if one exists, else just
            // do a harmless statement; either way the counter bounds the loop.
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
        // Weighted choice between an atom, a call, and a type-specific compound.
        match self.seed.weighted(&[3, 2, 4]) {
            0 => self.gen_atom(ty, scope),
            1 => self
                .gen_call(ty, scope, fuel)
                .unwrap_or_else(|| self.gen_atom(ty, scope)),
            _ => self.gen_compound(ty, scope, fuel),
        }
    }

    /// A leaf expression: a literal or an in-scope variable reference.
    fn gen_atom(&mut self, ty: &Ty, scope: &Scope) -> String {
        let vars = scope.bindings_of(ty);
        if !vars.is_empty() && self.seed.bool() {
            return vars[self.seed.choice(vars.len())].name.clone();
        }
        self.gen_literal(ty)
    }

    fn gen_literal(&mut self, ty: &Ty) -> String {
        match ty {
            // Non-negative only: RSScript has no negative-literal / unary-minus
            // parse surface (`-3` is RS0015; negatives are written `0 - 3`), so a
            // bare negative literal would be rejected.
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
        }
    }

    /// A call to an earlier-declared function returning `ty`.
    fn gen_call(&mut self, ty: &Ty, scope: &Scope, fuel: u32) -> Option<String> {
        let candidates = scope.functions_returning(ty);
        if candidates.is_empty() {
            return None;
        }
        let sig = candidates[self.seed.choice(candidates.len())].clone();
        let args = sig
            .params
            .iter()
            .map(|(pname, pty)| {
                let arg = self.gen_expr(pty, scope, fuel - 1);
                if pty.param_is_read() {
                    format!("{pname}: read {arg}")
                } else {
                    format!("{pname}: {arg}")
                }
            })
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!("{}({args})", sig.name))
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
                // No `/` for floats: avoids inf/NaN, which only matter for
                // formatting and are never printed anyway, but keeps results finite.
                let ops = ["+", "-", "*"];
                let op = ops[self.seed.choice(ops.len())];
                let lhs = self.gen_expr(&Ty::Float, scope, fuel - 1);
                let rhs = self.gen_expr(&Ty::Float, scope, fuel - 1);
                format!("({lhs} {op} {rhs})")
            }
            Ty::Bool => self.gen_bool_compound(scope, fuel),
            Ty::String => {
                // String.concat, or a bridge from a scalar.
                match self.seed.choice(3) {
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
                }
            }
        }
    }

    fn gen_bool_compound(&mut self, scope: &Scope, fuel: u32) -> String {
        match self.seed.choice(4) {
            0 | 1 => {
                // Numeric comparison (Int or Float operands).
                let operand = if self.seed.bool() { Ty::Int } else { Ty::Float };
                let cmps = ["<", "<=", ">", ">=", "==", "!="];
                let cmp = cmps[self.seed.choice(cmps.len())];
                let lhs = self.gen_expr(&operand, scope, fuel - 1);
                let rhs = self.gen_expr(&operand, scope, fuel - 1);
                format!("({lhs} {cmp} {rhs})")
            }
            2 => {
                // Boolean equality.
                let cmp = if self.seed.bool() { "==" } else { "!=" };
                let lhs = self.gen_expr(&Ty::Bool, scope, fuel - 1);
                let rhs = self.gen_expr(&Ty::Bool, scope, fuel - 1);
                format!("({lhs} {cmp} {rhs})")
            }
            _ => {
                // Logical connective.
                let op = if self.seed.bool() { "&&" } else { "||" };
                let lhs = self.gen_expr(&Ty::Bool, scope, fuel - 1);
                let rhs = self.gen_expr(&Ty::Bool, scope, fuel - 1);
                format!("({lhs} {op} {rhs})")
            }
        }
    }
}
