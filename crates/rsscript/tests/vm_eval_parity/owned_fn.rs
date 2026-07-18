//! eval≡lowered parity: first-class `owned Fn(...)` values (storable closures).
//!
//! `owned Fn` is a first-class value: storable in structs/collections, bound to
//! locals, and called as a value (`let f = r.fxn; f(args)`). The VM dispatches
//! through `CallClosure`; the Rust backend lowers a storable `owned Fn` to
//! `Rc<dyn Fn(..)>` (Clone + callable through a shared `List.get` read). Both
//! backends must print identical stdout.
#![allow(unused_imports, dead_code)]
use super::*;

/// Focused regression: `List<owned Fn(Int)->Int>` + a struct field holding an
/// `owned Fn`, with a Copy capture, fetched from the list and called.
#[test]
fn parity_owned_fn_toy_probe() {
    let source = r#"
features: local

struct Adder derives(Clone) {
    fxn: owned Fn(Int) -> Int
}

fn main() -> Unit {
    let base = 100
    local adders = List.new<Adder>()
    let a0 = Adder(fxn: fn(value) captures(read base) effects(pure) { return value + base })
    let a1 = Adder(fxn: fn(value) captures(read base) effects(pure) { return value * base })
    List.push(list: mut adders, value: read a0)
    List.push(list: mut adders, value: read a1)

    let mut i = 0
    let mut total = 0
    while i < 2 {
        let r = List.get(list: read adders, index: i)
        let f = r.fxn
        total = total + f(3)
        i = i + 1
    }
    // (3 + 100) + (3 * 100) = 103 + 300 = 403
    Log.write(message: read String.from_int(value: total))
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend("owned-fn-toy.rss", "rsscript_owned_fn_toy", source);
}

/// PatternMatcher-shaped: rules-as-data. A `List<RwRule>` where each `RwRule`
/// stores an `owned Fn(read UOp) -> Option<UOp>` rewrite action (one capturing a
/// Copy `Int` and move-capturing a non-`Copy` `String`). A generic driver
/// fetches each stored rule and calls it as a value (`let f = r.fxn; f(read u)`)
/// per node across a per-node fixpoint loop, owning a mutable `Ctx` it bumps on
/// each firing, with a `Map<UOp, UOp>` memo keyed on the `UOp` value. Mirrors
/// tinygrad's PatternMatcher.rewrite + the unified_rewrite fixpoint, with the
/// rule's Callable as a stored closure (replacing the hand-rolled Act-enum
/// dispatch in graph_rewrite.rss).
#[test]
fn parity_owned_fn_pattern_matcher() {
    let source = r#"
features: local

pub sum Ops derives(Clone, Eq, Hash) { Const Var Add Mul }

struct UOp derives(Clone, Eq, Hash) {
    op: Ops
    src: List<UOp>
    arg: Int
}

struct Ctx derives(Clone) {
    fired: Int
}

struct RwRule derives(Clone) {
    fxn: owned Fn(read UOp) -> Option<UOp>
}

fn uconst(v: Int) -> fresh UOp {
    return UOp(op: Const, src: List.new<UOp>(), arg: v)
}
fn uvar(id: Int) -> fresh UOp {
    return UOp(op: Var, src: List.new<UOp>(), arg: id)
}
fn ubin(o: Ops, a: read UOp, b: read UOp) -> fresh UOp {
    local s = List.new<UOp>()
    List.push(list: mut s, value: read a)
    List.push(list: mut s, value: read b)
    return UOp(op: o.clone(), src: take s, arg: 0)
}

fn is_const(u: read UOp, v: Int) -> Bool {
    if u.op == Const {
        if u.arg == v { return true }
    }
    return false
}

// Rebuild a fresh copy of a node from its fields (terminal nodes are leaves
// here), so a `fresh` return is provably newly created and never read-aliased.
fn rebuild(u: read UOp) -> fresh UOp {
    if u.op == Const { return uconst(v: u.arg) }
    if u.op == Var { return uvar(id: u.arg) }
    let c0 = List.get(list: read u.src, index: 0)
    let c1 = List.get(list: read u.src, index: 1)
    if u.op == Mul { return ubin(o: Mul, a: read c0, b: read c1) }
    return ubin(o: Add, a: read c0, b: read c1)
}

// Build the rule set as DATA. Each rule's rewrite is a stored closure, the
// first-class `owned Fn` value. The driver (not the rule) owns the mutable
// `Ctx`, so a rule is a pure `read UOp -> Option<UOp>` rewrite, exactly like
// tinygrad's `(UPat, Callable)` pairs. A rule that fires returns `Some(new)`.
fn build_rules() -> fresh List<RwRule> {
    local rules = List.new<RwRule>()

    // Rule 1: (x * 1) -> x   (rebuild the left child fresh). No captures; uses
    // only its `u` parameter (typed `read UOp` by the Fn slot) and free fns.
    let r_mul1 = RwRule(fxn: fn(u) captures() effects(pure) {
        if u.op == Mul {
            let lhs = List.get(list: read u.src, index: 0)
            let rhs = List.get(list: read u.src, index: 1)
            if is_const(u: read rhs, v: 1) {
                return Some(uvar(id: lhs.arg))
            }
        }
        return None
    })
    List.push(list: mut rules, value: read r_mul1)

    // Rule 2: (x * 0) -> Const 0. MOVE-captures a non-`Copy` `String` tag
    // (`take tag` — proves the owned move-capture path; the stored closure owns
    // its own copy and reads it each call) and Copy-captures `zero` (Int).
    let zero = 0
    let tag = "mul0"
    let r_mul0 = RwRule(fxn: fn(u) captures(read zero, take tag) effects(pure) {
        if u.op == Mul {
            let rhs = List.get(list: read u.src, index: 1)
            if is_const(u: read rhs, v: zero) {
                if String.len(value: read tag) > 0 {
                    return Some(uconst(v: zero))
                }
            }
        }
        return None
    })
    List.push(list: mut rules, value: read r_mul0)

    return take rules
}

// Apply rules to a single node to a local fixpoint (unified_rewrite's
// `while test_n is not None` loop). Each iteration FETCHES a stored rule and
// CALLS it as a value (`let f = r.fxn; f(read cur)`). The driver counts firings
// into the mutable `Ctx` it owns.
fn rewrite_fixed(node: read UOp, rules: read List<RwRule>, ctx: mut Ctx) -> fresh UOp {
    let mut cur = uvar(id: node.arg)
    if node.op == Const { cur = uconst(v: node.arg) }
    if node.op == Mul {
        let c0 = List.get(list: read node.src, index: 0)
        let c1 = List.get(list: read node.src, index: 1)
        cur = ubin(o: Mul, a: read c0, b: read c1)
    }
    if node.op == Add {
        let c0 = List.get(list: read node.src, index: 0)
        let c1 = List.get(list: read node.src, index: 1)
        cur = ubin(o: Add, a: read c0, b: read c1)
    }

    let mut changed = true
    let nrules = List.len(list: read rules)
    while changed {
        changed = false
        let mut i = 0
        while i < nrules {
            let r = List.get(list: read rules, index: i)
            let f = r.fxn
            match f(read cur) {
                Some(next) => {
                    cur = next
                    changed = true
                    ctx.fired = ctx.fired + 1
                }
                None => {}
            }
            i = i + 1
        }
    }
    return rebuild(u: read cur)
}

fn main() -> Unit {
    let rules = build_rules()
    local ctx = Ctx(fired: 0)
    local memo = Map.new<UOp, UOp>()

    // Build (x * 1) * 0  with x = Var 7.
    let x = uvar(id: 7)
    let one = uconst(v: 1)
    let x_mul_1 = ubin(o: Mul, a: read x, b: read one)
    let zero = uconst(v: 0)

    // Bottom-up: rewrite the inner (x*1) first, memoize on the UOp key.
    let inner = rewrite_fixed(node: read x_mul_1, rules: read rules, ctx: mut ctx)
    Map.insert(map: mut memo, key: read x_mul_1, value: read inner)

    // Memo hit: rebuild a FRESH copy from the cached node's fields (never return
    // a value aliased behind the shared map), mirroring graph_rewrite.rss.
    let memo_hit = Map.get(map: read memo, key: read x_mul_1)
    let inner2 = match memo_hit {
        Some(v) => rebuild(u: read v)
        None => rewrite_fixed(node: read x_mul_1, rules: read rules, ctx: mut ctx)
    }
    let outer = ubin(o: Mul, a: read inner2, b: read zero)
    let result = rewrite_fixed(node: read outer, rules: read rules, ctx: mut ctx)

    // (x*1)*0 simplifies to Const 0. Assert the op and arg, and the fire count.
    let is_zero = is_const(u: read result, v: 0)
    Log.write(message: read "result_is_const_0:")
    if is_zero {
        Log.write(message: read "yes")
    } else {
        Log.write(message: read "no")
    }
    Log.write(message: read "result_op_const:")
    if result.op == Const {
        Log.write(message: read "yes")
    } else {
        Log.write(message: read "no")
    }
    Log.write(message: read "fired:")
    Log.write(message: read String.from_int(value: ctx.fired))
    // Concrete correctness: Rule 1 fires once on the inner (x*1)->x, Rule 2 once
    // on the rebuilt (x*1-simplified)*0 -> Const 0, so exactly two firings.
    Log.write(message: read "fired_is_2:")
    if ctx.fired == 2 {
        Log.write(message: read "yes")
    } else {
        Log.write(message: read "no")
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "owned-fn-pattern-matcher.rss",
        "rsscript_owned_fn_pattern_matcher",
        source,
    );
}

/// The feature this commit adds: a stored rule whose type carries a `mut`
/// effect on a `Fn`-type parameter — `owned Fn(read UOp, mut Ctx) -> Option<UOp>`
/// — so the RULE BODY itself mutates the shared `Ctx`. The firing count is
/// produced BY THE RULES (`ctx.fired = ctx.fired + 1` inside each closure), not
/// by the driver. The driver fetches each stored rule and calls it as a value
/// with the matching call-site effects (`f(read u, mut ctx)`) across a fixpoint;
/// the `mut Ctx` argument is an exclusive borrow for the call whose mutations
/// propagate back. Both backends must print identical stdout: the VM writes the
/// mutated argument register back after `CallClosure`; AOT lowers the slot to
/// `Rc<dyn Fn(&UOp, &mut Ctx) -> Option<UOp>>` and passes `&u, &mut ctx`.
#[test]
fn parity_owned_fn_mut_ctx_rule() {
    let source = r#"
features: local

pub sum Ops derives(Clone, Eq, Hash) { Const Var Add Mul }

struct UOp derives(Clone, Eq, Hash) {
    op: Ops
    src: List<UOp>
    arg: Int
}

struct Ctx derives(Clone) {
    fired: Int
}

struct RwRule derives(Clone) {
    fxn: owned Fn(read UOp, mut Ctx) -> Option<UOp>
}

fn uconst(v: Int) -> fresh UOp {
    return UOp(op: Const, src: List.new<UOp>(), arg: v)
}
fn uvar(id: Int) -> fresh UOp {
    return UOp(op: Var, src: List.new<UOp>(), arg: id)
}
fn ubin(o: Ops, a: read UOp, b: read UOp) -> fresh UOp {
    local s = List.new<UOp>()
    List.push(list: mut s, value: read a)
    List.push(list: mut s, value: read b)
    return UOp(op: o.clone(), src: take s, arg: 0)
}

fn is_const(u: read UOp, v: Int) -> Bool {
    if u.op == Const {
        if u.arg == v { return true }
    }
    return false
}

fn rebuild(u: read UOp) -> fresh UOp {
    if u.op == Const { return uconst(v: u.arg) }
    if u.op == Var { return uvar(id: u.arg) }
    let c0 = List.get(list: read u.src, index: 0)
    let c1 = List.get(list: read u.src, index: 1)
    if u.op == Mul { return ubin(o: Mul, a: read c0, b: read c1) }
    return ubin(o: Add, a: read c0, b: read c1)
}

// Each rule is a stored `owned Fn(read UOp, mut Ctx) -> Option<UOp>`. When a
// rule FIRES, the rule body itself records the firing into the shared `Ctx`
// (`ctx.fired = ctx.fired + 1`) — the count is produced by the rules, not the
// driver. The `mut Ctx` parameter is an exclusive borrow for that call.
fn build_rules() -> fresh List<RwRule> {
    local rules = List.new<RwRule>()

    // Rule 1: (x * 1) -> x. No captures; mutates `ctx` on fire.
    let r_mul1 = RwRule(fxn: fn(u, ctx) captures() effects(pure) {
        if u.op == Mul {
            let lhs = List.get(list: read u.src, index: 0)
            let rhs = List.get(list: read u.src, index: 1)
            if is_const(u: read rhs, v: 1) {
                ctx.fired = ctx.fired + 1
                return Some(uvar(id: lhs.arg))
            }
        }
        return None
    })
    List.push(list: mut rules, value: read r_mul1)

    // Rule 2: (x * 0) -> Const 0. Move-captures a `String` tag and Copy-captures
    // `zero`; mutates `ctx` on fire.
    let zero = 0
    let tag = "mul0"
    let r_mul0 = RwRule(fxn: fn(u, ctx) captures(read zero, take tag) effects(pure) {
        if u.op == Mul {
            let rhs = List.get(list: read u.src, index: 1)
            if is_const(u: read rhs, v: zero) {
                if String.len(value: read tag) > 0 {
                    ctx.fired = ctx.fired + 1
                    return Some(uconst(v: zero))
                }
            }
        }
        return None
    })
    List.push(list: mut rules, value: read r_mul0)

    return take rules
}

// Drive each node to a fixpoint. Each iteration FETCHES a stored rule and CALLS
// it as a value, threading the shared `mut Ctx` THROUGH the rule
// (`f(read cur, mut ctx)`). The rule — not the driver — increments the count.
fn rewrite_fixed(node: read UOp, rules: read List<RwRule>, ctx: mut Ctx) -> fresh UOp {
    let mut cur = uvar(id: node.arg)
    if node.op == Const { cur = uconst(v: node.arg) }
    if node.op == Mul {
        let c0 = List.get(list: read node.src, index: 0)
        let c1 = List.get(list: read node.src, index: 1)
        cur = ubin(o: Mul, a: read c0, b: read c1)
    }
    if node.op == Add {
        let c0 = List.get(list: read node.src, index: 0)
        let c1 = List.get(list: read node.src, index: 1)
        cur = ubin(o: Add, a: read c0, b: read c1)
    }

    let mut changed = true
    let nrules = List.len(list: read rules)
    while changed {
        changed = false
        let mut i = 0
        while i < nrules {
            let r = List.get(list: read rules, index: i)
            let f = r.fxn
            match f(read cur, mut ctx) {
                Some(next) => {
                    cur = next
                    changed = true
                }
                None => {}
            }
            i = i + 1
        }
    }
    return rebuild(u: read cur)
}

fn main() -> Unit {
    let rules = build_rules()
    local ctx = Ctx(fired: 0)
    local memo = Map.new<UOp, UOp>()

    let x = uvar(id: 7)
    let one = uconst(v: 1)
    let x_mul_1 = ubin(o: Mul, a: read x, b: read one)
    let zero = uconst(v: 0)

    let inner = rewrite_fixed(node: read x_mul_1, rules: read rules, ctx: mut ctx)
    Map.insert(map: mut memo, key: read x_mul_1, value: read inner)

    let memo_hit = Map.get(map: read memo, key: read x_mul_1)
    let inner2 = match memo_hit {
        Some(v) => rebuild(u: read v)
        None => rewrite_fixed(node: read x_mul_1, rules: read rules, ctx: mut ctx)
    }
    let outer = ubin(o: Mul, a: read inner2, b: read zero)
    let result = rewrite_fixed(node: read outer, rules: read rules, ctx: mut ctx)

    let is_zero = is_const(u: read result, v: 0)
    Log.write(message: read "result_is_const_0:")
    if is_zero {
        Log.write(message: read "yes")
    } else {
        Log.write(message: read "no")
    }
    Log.write(message: read "result_op_const:")
    if result.op == Const {
        Log.write(message: read "yes")
    } else {
        Log.write(message: read "no")
    }
    // The firing count was produced BY THE RULES mutating `mut Ctx`, not the
    // driver. Rule 1 fires once on the inner (x*1)->x, Rule 2 once on the rebuilt
    // (x*1-simplified)*0 -> Const 0, so exactly two firings.
    Log.write(message: read "fired:")
    Log.write(message: read String.from_int(value: ctx.fired))
    Log.write(message: read "fired_is_2:")
    if ctx.fired == 2 {
        Log.write(message: read "yes")
    } else {
        Log.write(message: read "no")
    }
    return Unit
}
"#;
    common::assert_vm_eval_matches_backend(
        "owned-fn-mut-ctx-rule.rss",
        "rsscript_owned_fn_mut_ctx_rule",
        source,
    );
}
