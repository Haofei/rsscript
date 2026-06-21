//! JIT arithmetic-edge differential matrix.
//!
//! The generative `backend_differential` suite deliberately *skips* the
//! arithmetic edges (it `prop_assume!(oracle.is_some())`, dropping overflow, and
//! its float strategy avoids division and NaN). Those edges are exactly where a
//! JIT tier is most likely to diverge from the interpreter: native code that
//! must trap on i64 overflow / divide-by-zero / `i64::MIN / -1`, mask an
//! out-of-range shift, and reproduce IEEE-754 NaN / signed-zero semantics — or
//! correctly bail to the interpreter at the guard.
//!
//! Each case runs through the default parity backends (`vm-interpreter`,
//! `vm-jit`, and the native tier plus its force-deopt twin when built). Set
//! `RSSCRIPT_FULL_BACKEND_PARITY=1` to add `rust-compiled` to every case. The
//! assertions cover one of two contracts:
//!
//!   * **trap edges** — every backend fails with a clean RSScript runtime error
//!     (no host panic, no garbage), via [`assert_backends_all_fail`]; and
//!   * **value edges** — every backend returns the identical observable value,
//!     via [`assert_backends_agree`].
//!
//! Each arithmetic operation lives in a small pure helper called with `read`
//! arguments so the value is unknown at compile time and the JIT actually
//! compiles (and guards) the operation rather than constant-folding it.

mod common;

use common::differential::{assert_backends_agree, assert_backends_all_fail};

/// `i64::MAX` is expressible as a literal; `i64::MIN` is built as
/// `(0 - i64::MAX) - 1` because the bare literal `9223372036854775808` would
/// overflow the `Int` lexer. Shared prelude installed in `main` before each call.
const MIN_PRELUDE: &str = "    let max = 9223372036854775807\n    \
                           let min = 0 - max - 1\n    let neg_one = 0 - 1\n";

// --- Trap edges: every backend must fail cleanly -------------------------

#[test]
fn jit_edge_integer_addition_overflow_traps_on_all_backends() {
    let source = "\
fn add(a: Int, b: Int) -> Int {
    return a + b
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: add(a: read 9223372036854775807, b: read 1)))
    return Unit
}
";
    assert_backends_all_fail("jit-edge-add-overflow.rss", source, &[]);
}

#[test]
fn jit_edge_integer_multiplication_overflow_traps_on_all_backends() {
    let source = "\
fn mul(a: Int, b: Int) -> Int {
    return a * b
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: mul(a: read 3037000500, b: read 3037000500)))
    return Unit
}
";
    assert_backends_all_fail("jit-edge-mul-overflow.rss", source, &[]);
}

#[test]
fn jit_edge_integer_subtraction_overflow_traps_on_all_backends() {
    // `i64::MIN - 1` underflows the signed range. Native code must trap (or bail to
    // the interpreter, which traps) exactly as `checked_sub` would — never wrap.
    let source = format!(
        "\
fn sub(a: Int, b: Int) -> Int {{
    return a - b
}}

fn main() -> Unit {{
{MIN_PRELUDE}    Log.write(message: read String.from_int(value: sub(a: read min, b: read 1)))
    return Unit
}}
"
    );
    assert_backends_all_fail("jit-edge-sub-overflow.rss", &source, &[]);
}

#[test]
fn jit_edge_integer_negation_overflow_traps_on_all_backends() {
    // `0 - i64::MIN` has no representable positive counterpart; negating `i64::MIN`
    // must trap on every backend rather than wrap back to `i64::MIN`.
    let source = format!(
        "\
fn negate(a: Int) -> Int {{
    return 0 - a
}}

fn main() -> Unit {{
{MIN_PRELUDE}    Log.write(message: read String.from_int(value: negate(a: read min)))
    return Unit
}}
"
    );
    assert_backends_all_fail("jit-edge-negation-overflow.rss", &source, &[]);
}

#[test]
fn jit_edge_integer_division_by_zero_traps_on_all_backends() {
    let source = "\
fn div(a: Int, b: Int) -> Int {
    return a / b
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: div(a: read 10, b: read 0)))
    return Unit
}
";
    assert_backends_all_fail("jit-edge-div-zero.rss", source, &[]);
}

#[test]
fn jit_edge_integer_modulo_by_zero_traps_on_all_backends() {
    let source = "\
fn rem(a: Int, b: Int) -> Int {
    return a % b
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: rem(a: read 10, b: read 0)))
    return Unit
}
";
    assert_backends_all_fail("jit-edge-mod-zero.rss", source, &[]);
}

#[test]
fn jit_edge_int_min_divided_by_neg_one_traps_on_all_backends() {
    let source = format!(
        "\
fn div(a: Int, b: Int) -> Int {{
    return a / b
}}

fn main() -> Unit {{
{MIN_PRELUDE}    Log.write(message: read String.from_int(value: div(a: read min, b: read neg_one)))
    return Unit
}}
"
    );
    assert_backends_all_fail("jit-edge-min-div-neg1.rss", &source, &[]);
}

#[test]
fn jit_edge_int_min_modulo_neg_one_traps_on_all_backends() {
    let source = format!(
        "\
fn rem(a: Int, b: Int) -> Int {{
    return a % b
}}

fn main() -> Unit {{
{MIN_PRELUDE}    Log.write(message: read String.from_int(value: rem(a: read min, b: read neg_one)))
    return Unit
}}
"
    );
    assert_backends_all_fail("jit-edge-min-mod-neg1.rss", &source, &[]);
}

// --- Value edges: every backend must agree -------------------------------

#[test]
fn jit_edge_shift_amount_wraps_consistently() {
    // `wrapping_shl`/`wrapping_shr` mask the count to 0..63, so a shift of 64 is
    // a shift of 0. Every backend must agree on this defined wrap rather than
    // panic (debug `<<`) or produce a target-dependent value.
    let source = "\
fn shl(value: Int, bits: Int) -> Int {
    return Int.shift_left(value: read value, bits: read bits)
}

fn shr(value: Int, bits: Int) -> Int {
    return Int.shift_right(value: read value, bits: read bits)
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: shl(value: read 1, bits: read 64)))
    Log.write(message: read String.from_int(value: shl(value: read 1, bits: read 63)))
    Log.write(message: read String.from_int(value: shr(value: read 256, bits: read 64)))
    Log.write(message: read String.from_int(value: shr(value: read 256, bits: read 4)))
    return Unit
}
";
    assert_backends_agree("jit-edge-shift-wrap.rss", source, &[]);
}

#[test]
fn jit_edge_float_nan_is_not_equal_to_itself() {
    // `0.0 / 0.0` is NaN; `NaN == NaN` is false on every backend. Reduced to a
    // Bool so float *formatting* (which is not under test here) can't diverge.
    let source = "\
fn nan(a: Float, b: Float) -> Float {
    return a / b
}

fn main() -> Unit {
    let value = nan(a: read 0.0, b: read 0.0)
    Log.write(message: read String.from_bool(value: value == value))
    return Unit
}
";
    assert_backends_agree("jit-edge-float-nan.rss", source, &[]);
}

#[test]
fn jit_edge_float_signed_zero_compares_equal() {
    // `-0.0 == 0.0` is true under IEEE-754; every backend must agree.
    let source = "\
fn neg(value: Float) -> Float {
    return 0.0 - value
}

fn main() -> Unit {
    let negative_zero = neg(value: read 0.0)
    Log.write(message: read String.from_bool(value: negative_zero == 0.0))
    return Unit
}
";
    assert_backends_agree("jit-edge-float-signed-zero.rss", source, &[]);
}

#[test]
fn jit_edge_float_division_by_zero_yields_infinity() {
    // `1.0 / 0.0` is +inf (no trap, unlike Int): every backend must agree it is
    // greater than any finite value. Reduced to a Bool to avoid inf formatting.
    let source = "\
fn div(a: Float, b: Float) -> Float {
    return a / b
}

fn main() -> Unit {
    let infinity = div(a: read 1.0, b: read 0.0)
    Log.write(message: read String.from_bool(value: infinity > 1000000.0))
    return Unit
}
";
    assert_backends_agree("jit-edge-float-inf.rss", source, &[]);
}

// --- J4.3: range-proof check-elision parity -------------------------------

#[test]
fn jit_edge_proven_nonoverflowing_constants_agree_at_extremes() {
    // A region where the native range analysis can PROVE the additions cannot
    // overflow (the operands are compile-time constants whose sums fit i64,
    // including a sum landing exactly on i64::MAX). The native tier may emit
    // unchecked `iadd` here; every backend must still produce the identical value.
    // If the proof were unsound, the unchecked op would wrap and diverge from the
    // checked interpreter — this asserts byte-identical results at the boundary.
    let source = "\
fn main() -> Unit {
    let max = 9223372036854775807
    let near = max - 10
    let exact = near + 10
    Log.write(message: read String.from_int(value: exact))
    let small = 5 + 3
    Log.write(message: read String.from_int(value: small))
    return Unit
}
";
    assert_backends_agree("jit-edge-proven-noflow.rss", source, &[]);
}

#[test]
fn jit_edge_unknown_addition_at_max_still_traps() {
    // The mirror of the proven case: when an addend is `read` (runtime-unknown ⇒
    // TOP in the range analysis), the native tier must KEEP the overflow check.
    // `i64::MAX + 1` over unknown operands must therefore trap on every backend,
    // proving the proof did not over-eagerly strip the check.
    let source = "\
fn add(a: Int, b: Int) -> Int {
    return a + b
}

fn main() -> Unit {
    Log.write(message: read String.from_int(value: add(a: read 9223372036854775807, b: read 1)))
    return Unit
}
";
    assert_backends_all_fail("jit-edge-unknown-add-overflow.rss", source, &[]);
}
