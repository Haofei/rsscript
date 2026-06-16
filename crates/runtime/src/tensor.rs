//! Native tensor kernels — the foundation of the ML-perf work (slice 1).
//!
//! A `RssTensor` is a packed, row-major `f32` buffer plus a shape. The buffer is
//! shared via `Rc` so cloning a tensor handle is cheap and movement ops (later
//! slices) can share storage without copying. RSScript `Float` is `f64`; tensors
//! store `f32`, so every value is narrowed on the way in and widened on the way
//! out at the host boundary (`from_f32_slice` / `to_f32_slice`). Both backends
//! call these exact functions, so the f64<->f32 rounding is identical and the
//! reg-VM and AOT-compiled results are bit-for-bit equal.
//!
//! `matmul` is a hand-written `ikj`-order f32 kernel (cache-friendly: the inner
//! loop walks contiguous rows of `b` and the output). No external crate
//! dependency — optimized/parallel matmul (SIMD/rayon) is a separate later task.

use std::rc::Rc;

/// Opaque tensor error surfaced to RSScript. Mirrors the `ChannelError` shape: a
/// single message string, `?`-propagated, with `TensorError.message` to read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TensorError {
    message: String,
}

impl TensorError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

pub fn tensor_error_message(error: &TensorError) -> String {
    error.message.clone()
}

/// The element dtype of a tensor. The storage is ALWAYS `Vec<f32>` regardless of
/// dtype; the tag only records how the values should be interpreted by callers and
/// drives output-dtype promotion in the kernels:
/// - `F32`: ordinary floats.
/// - `I32`: integer values held exactly in `f32` (the caller guarantees
///   `|x| <= 2^24`, where every integer is representable exactly).
/// - `Bool`: exactly `0.0` or `1.0`.
///
/// This is the f32-backed + dtype-tag model: no separate integer/bool buffers, so
/// every existing kernel keeps operating on `f32` and just stamps the right output
/// tag. The numeric code (F32=0, I32=1, Bool=2) is exposed to RSScript via
/// `tensor_dtype_code` for tests/introspection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DType {
    F32,
    I32,
    Bool,
}

impl DType {
    fn code(self) -> i64 {
        match self {
            DType::F32 => 0,
            DType::I32 => 1,
            DType::Bool => 2,
        }
    }

    /// Arithmetic promotion for binary elementwise ops (add/sub/mul/div, min/max,
    /// select): the result is `F32` if either operand is `F32`, otherwise `I32`
    /// (Bool is treated as an integer for arithmetic).
    fn promote_arith(a: DType, b: DType) -> DType {
        if a == DType::F32 || b == DType::F32 {
            DType::F32
        } else {
            DType::I32
        }
    }
}

/// A packed, row-major tensor: `data.len() == shape.iter().product()`. The buffer
/// is `Rc`-shared so handles clone cheaply and later slices can alias storage.
/// Single-isolate (no `Send`/`Sync` bound), matching `RssChannel`. `dtype` is a
/// tag over the same `f32` storage (see `DType`).
#[derive(Debug, Clone, PartialEq)]
pub struct RssTensor {
    data: Rc<Vec<f32>>,
    shape: Vec<usize>,
    dtype: DType,
}

impl RssTensor {
    /// The element count implied by `shape` (the product of the dims; `1` for an
    /// empty/scalar shape, since the empty product is `1`).
    fn shape_len(shape: &[usize]) -> usize {
        shape.iter().product()
    }

    pub fn data(&self) -> &[f32] {
        &self.data
    }

    pub fn shape(&self) -> &[usize] {
        &self.shape
    }

    pub fn dtype(&self) -> DType {
        self.dtype
    }
}

/// The tensor's dtype as a numeric code: F32=0, I32=1, Bool=2. Infallible.
pub fn tensor_dtype_code(t: &RssTensor) -> i64 {
    t.dtype.code()
}

/// Build a tensor from an `f64` slice (narrowing to `f32`) and a shape given as
/// `i64`. Validates that the shape is non-negative and that the element count
/// matches `data.len()`; otherwise returns a `TensorError`.
pub fn tensor_from_f32_slice(data: &[f64], shape: &[i64]) -> Result<RssTensor, TensorError> {
    let mut dims = Vec::with_capacity(shape.len());
    for &dim in shape {
        if dim < 0 {
            return Err(TensorError::new(format!(
                "tensor shape dimensions must be non-negative, got {dim}"
            )));
        }
        dims.push(dim as usize);
    }
    let expected = RssTensor::shape_len(&dims);
    if data.len() != expected {
        return Err(TensorError::new(format!(
            "tensor data length {} does not match shape {:?} (expected {expected} elements)",
            data.len(),
            dims
        )));
    }
    let packed = data.iter().map(|&value| value as f32).collect::<Vec<f32>>();
    Ok(RssTensor {
        data: Rc::new(packed),
        shape: dims,
        dtype: DType::F32,
    })
}

/// Read the tensor buffer back out as `f64` (widening from the stored `f32`),
/// row-major.
pub fn tensor_to_f32_slice(tensor: &RssTensor) -> Result<Vec<f64>, TensorError> {
    Ok(tensor.data.iter().map(|&value| value as f64).collect())
}

/// The tensor's shape as `Int`s.
pub fn tensor_shape(tensor: &RssTensor) -> Result<Vec<i64>, TensorError> {
    Ok(tensor.shape.iter().map(|&dim| dim as i64).collect())
}

/// The number of dimensions.
pub fn tensor_rank(tensor: &RssTensor) -> i64 {
    tensor.shape.len() as i64
}

/// 2-D × 2-D matrix multiply, contracting the inner dimensions:
/// `(m, k) × (k, n) -> (m, n)`. Returns a `TensorError` if either operand is not
/// rank-2 or the inner dimensions disagree.
///
/// The kernel uses `ikj` loop order: for each output row `i`, scan each `k` once
/// (loading the scalar `a[i,k]`), then stream the contiguous row `b[k, ..]` into
/// the contiguous output row `c[i, ..]`. This keeps both inner-loop accesses
/// sequential, which autovectorizes well and is far friendlier to the cache than
/// the naive `ijk` order's strided `b` column walk.
pub fn tensor_matmul(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    if a.shape.len() != 2 || b.shape.len() != 2 {
        return Err(TensorError::new(format!(
            "matmul requires two rank-2 tensors, got shapes {:?} and {:?}",
            a.shape, b.shape
        )));
    }
    let (m, ka) = (a.shape[0], a.shape[1]);
    let (kb, n) = (b.shape[0], b.shape[1]);
    if ka != kb {
        return Err(TensorError::new(format!(
            "matmul inner dimensions disagree: {:?} × {:?} (contraction {ka} != {kb})",
            a.shape, b.shape
        )));
    }
    let k = ka;
    let lhs = a.data.as_ref();
    let rhs = b.data.as_ref();
    let mut out = vec![0.0f32; m * n];
    for i in 0..m {
        let lhs_row = &lhs[i * k..i * k + k];
        let out_row = &mut out[i * n..i * n + n];
        for p in 0..k {
            let a_ip = lhs_row[p];
            if a_ip == 0.0 {
                continue;
            }
            let rhs_row = &rhs[p * n..p * n + n];
            for j in 0..n {
                out_row[j] += a_ip * rhs_row[j];
            }
        }
    }
    Ok(RssTensor {
        data: Rc::new(out),
        shape: vec![m, n],
        dtype: DType::F32,
    })
}

/// Compute the NumPy-broadcast output shape of two shapes: right-aligned, each dim
/// pair must be equal or one of them `1`; missing leading dims are treated as `1`.
/// Returns the broadcast shape or a `TensorError` describing the incompatibility.
fn broadcast_shapes(a: &[usize], b: &[usize], op_name: &str) -> Result<Vec<usize>, TensorError> {
    let rank = a.len().max(b.len());
    let mut out = vec![0usize; rank];
    for i in 0..rank {
        // Right-aligned: index from the back; absent leading dims are size 1.
        let da = if i < a.len() { a[a.len() - 1 - i] } else { 1 };
        let db = if i < b.len() { b[b.len() - 1 - i] } else { 1 };
        let dim = if da == db {
            da
        } else if da == 1 {
            db
        } else if db == 1 {
            da
        } else {
            return Err(TensorError::new(format!(
                "{op_name}: shapes {a:?} and {b:?} are not broadcast-compatible (dim {da} vs {db})"
            )));
        };
        out[rank - 1 - i] = dim;
    }
    Ok(out)
}

/// Broadcast strides for `src_shape` against `target`: right-aligned, stride 0 on
/// any stretched (size-1) or absent leading dim. The caller guarantees `src_shape`
/// is broadcast-compatible with `target` (validated by `broadcast_shapes`), so this
/// never fails. Mirrors the stride logic in `tensor_broadcast_to`.
fn broadcast_strides(src_shape: &[usize], target: &[usize]) -> Vec<usize> {
    let src_rank = src_shape.len();
    let tgt_rank = target.len();
    let offset = tgt_rank - src_rank;
    // Row-major strides of the (contiguous) source.
    let mut row_strides = vec![0usize; src_rank];
    let mut acc = 1usize;
    for axis in (0..src_rank).rev() {
        row_strides[axis] = acc;
        acc *= src_shape[axis];
    }
    let mut strides = vec![0usize; tgt_rank];
    for (d, slot) in strides.iter_mut().enumerate() {
        if d < offset {
            *slot = 0; // absent leading dim: size 1, stride 0.
        } else {
            let s = src_shape[d - offset];
            *slot = if s == 1 { 0 } else { row_strides[d - offset] };
        }
    }
    strides
}

/// Apply a binary elementwise op with NumPy broadcasting, producing a fresh tensor
/// whose shape is the broadcast of `a.shape` and `b.shape` and whose dtype is
/// `out_dtype`. For the equal-shape case this is a straight zipped map (bit-for-bit
/// identical to the old non-broadcasting path); otherwise each operand is read via
/// broadcast strides (stride 0 on stretched/absent dims). Errors if the shapes are
/// not broadcast-compatible.
fn tensor_broadcast_binary(
    a: &RssTensor,
    b: &RssTensor,
    op_name: &str,
    out_dtype: DType,
    op: impl Fn(f32, f32) -> f32,
) -> Result<RssTensor, TensorError> {
    // Fast path: identical shapes need no stride arithmetic and stay bit-identical
    // to the original equal-shape kernel.
    if a.shape == b.shape {
        let out = a
            .data
            .iter()
            .zip(b.data.iter())
            .map(|(&x, &y)| op(x, y))
            .collect::<Vec<f32>>();
        return Ok(RssTensor {
            data: Rc::new(out),
            shape: a.shape.clone(),
            dtype: out_dtype,
        });
    }

    let target = broadcast_shapes(&a.shape, &b.shape, op_name)?;
    let rank = target.len();
    let a_strides = broadcast_strides(&a.shape, &target);
    let b_strides = broadcast_strides(&b.shape, &target);
    let total = RssTensor::shape_len(&target);
    let a_data = a.data.as_ref();
    let b_data = b.data.as_ref();
    let mut out = vec![0.0f32; total];
    let mut index = vec![0usize; rank];
    for slot in out.iter_mut() {
        let mut a_off = 0usize;
        let mut b_off = 0usize;
        for d in 0..rank {
            a_off += index[d] * a_strides[d];
            b_off += index[d] * b_strides[d];
        }
        *slot = op(a_data[a_off], b_data[b_off]);
        for d in (0..rank).rev() {
            index[d] += 1;
            if index[d] < target[d] {
                break;
            }
            index[d] = 0;
        }
    }
    Ok(RssTensor {
        data: Rc::new(out),
        shape: target,
        dtype: out_dtype,
    })
}

/// Apply a unary elementwise op, producing a fresh tensor with the same shape and
/// the given output dtype. Infallible.
fn tensor_unary_elementwise(t: &RssTensor, out_dtype: DType, op: impl Fn(f32) -> f32) -> RssTensor {
    let out = t.data.iter().map(|&x| op(x)).collect::<Vec<f32>>();
    RssTensor {
        data: Rc::new(out),
        shape: t.shape.clone(),
        dtype: out_dtype,
    }
}

/// Elementwise addition with broadcasting. Output dtype: F32 if either operand is
/// F32, else I32 (Bool treated as int).
pub fn tensor_add(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    tensor_broadcast_binary(a, b, "add", DType::promote_arith(a.dtype, b.dtype), |x, y| {
        x + y
    })
}

/// Elementwise subtraction (`a - b`) with broadcasting. Output dtype per arith
/// promotion.
pub fn tensor_sub(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    tensor_broadcast_binary(a, b, "sub", DType::promote_arith(a.dtype, b.dtype), |x, y| {
        x - y
    })
}

/// Elementwise multiplication with broadcasting. Output dtype per arith promotion.
pub fn tensor_mul(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    tensor_broadcast_binary(a, b, "mul", DType::promote_arith(a.dtype, b.dtype), |x, y| {
        x * y
    })
}

/// Elementwise division (`a / b`) with broadcasting. Output dtype per arith
/// promotion.
pub fn tensor_div(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    tensor_broadcast_binary(a, b, "div", DType::promote_arith(a.dtype, b.dtype), |x, y| {
        x / y
    })
}

/// Elementwise maximum with broadcasting. Output dtype per arith promotion.
pub fn tensor_maximum(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    tensor_broadcast_binary(
        a,
        b,
        "maximum",
        DType::promote_arith(a.dtype, b.dtype),
        |x, y| x.max(y),
    )
}

/// Elementwise minimum with broadcasting. Output dtype per arith promotion.
pub fn tensor_minimum(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    tensor_broadcast_binary(
        a,
        b,
        "minimum",
        DType::promote_arith(a.dtype, b.dtype),
        |x, y| x.min(y),
    )
}

/// Elementwise `a < b` with broadcasting; output dtype Bool (1.0/0.0).
pub fn tensor_cmplt(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    tensor_broadcast_binary(a, b, "cmplt", DType::Bool, |x, y| {
        if x < y {
            1.0
        } else {
            0.0
        }
    })
}

/// Elementwise `a != b` with broadcasting; output dtype Bool (1.0/0.0).
pub fn tensor_cmpne(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    tensor_broadcast_binary(a, b, "cmpne", DType::Bool, |x, y| {
        if x != y {
            1.0
        } else {
            0.0
        }
    })
}

/// Elementwise `a == b` with broadcasting; output dtype Bool (1.0/0.0).
pub fn tensor_cmpeq(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    tensor_broadcast_binary(a, b, "cmpeq", DType::Bool, |x, y| {
        if x == y {
            1.0
        } else {
            0.0
        }
    })
}

/// Elementwise select (tinygrad's `where`): pick `a` where `cond` is nonzero, else
/// `b`. All three broadcast together. Output dtype: F32 if either `a` or `b` is
/// F32, else I32. Named `select` (not `where`) for RSScript naming clarity. Errors
/// if the three shapes are not mutually broadcast-compatible.
pub fn tensor_select(
    cond: &RssTensor,
    a: &RssTensor,
    b: &RssTensor,
) -> Result<RssTensor, TensorError> {
    // Mutually broadcast all three: first cond+a, then that result against b.
    let ca = broadcast_shapes(&cond.shape, &a.shape, "select")?;
    let target = broadcast_shapes(&ca, &b.shape, "select")?;
    let rank = target.len();
    let cond_strides = broadcast_strides(&cond.shape, &target);
    let a_strides = broadcast_strides(&a.shape, &target);
    let b_strides = broadcast_strides(&b.shape, &target);
    let total = RssTensor::shape_len(&target);
    let cond_data = cond.data.as_ref();
    let a_data = a.data.as_ref();
    let b_data = b.data.as_ref();
    let mut out = vec![0.0f32; total];
    let mut index = vec![0usize; rank];
    for slot in out.iter_mut() {
        let mut c_off = 0usize;
        let mut a_off = 0usize;
        let mut b_off = 0usize;
        for d in 0..rank {
            c_off += index[d] * cond_strides[d];
            a_off += index[d] * a_strides[d];
            b_off += index[d] * b_strides[d];
        }
        *slot = if cond_data[c_off] != 0.0 {
            a_data[a_off]
        } else {
            b_data[b_off]
        };
        for d in (0..rank).rev() {
            index[d] += 1;
            if index[d] < target[d] {
                break;
            }
            index[d] = 0;
        }
    }
    Ok(RssTensor {
        data: Rc::new(out),
        shape: target,
        dtype: DType::promote_arith(a.dtype, b.dtype),
    })
}

/// Cast to F32: values unchanged, only the dtype tag changes.
pub fn tensor_cast_f32(t: &RssTensor) -> RssTensor {
    tensor_unary_elementwise(t, DType::F32, |x| x)
}

/// Cast to I32: each value truncated toward zero (`x.trunc()`), dtype I32.
pub fn tensor_cast_i32(t: &RssTensor) -> RssTensor {
    tensor_unary_elementwise(t, DType::I32, |x| x.trunc())
}

/// Cast to Bool: value `1.0` where `x != 0.0` else `0.0`, dtype Bool.
pub fn tensor_cast_bool(t: &RssTensor) -> RssTensor {
    tensor_unary_elementwise(t, DType::Bool, |x| if x != 0.0 { 1.0 } else { 0.0 })
}

/// Elementwise negation. Preserves the input dtype.
pub fn tensor_neg(t: &RssTensor) -> RssTensor {
    tensor_unary_elementwise(t, t.dtype, |x| -x)
}

/// Elementwise natural exponential. Output dtype F32.
pub fn tensor_exp(t: &RssTensor) -> RssTensor {
    tensor_unary_elementwise(t, DType::F32, f32::exp)
}

/// Elementwise natural logarithm. Output dtype F32.
pub fn tensor_log(t: &RssTensor) -> RssTensor {
    tensor_unary_elementwise(t, DType::F32, f32::ln)
}

/// Elementwise square root. Output dtype F32.
pub fn tensor_sqrt(t: &RssTensor) -> RssTensor {
    tensor_unary_elementwise(t, DType::F32, f32::sqrt)
}

/// Elementwise ReLU: `max(0, x)`. Output dtype F32.
pub fn tensor_relu(t: &RssTensor) -> RssTensor {
    tensor_unary_elementwise(t, DType::F32, |x| x.max(0.0))
}

/// Sum of every element (a global reduction to a scalar `Float`). The accumulation
/// is done in `f32` and then widened to `f64` at the boundary, matching how the
/// tensor stores `f32` and the rest of the kernels round; both backends call this
/// exact function so the reg-VM and AOT results agree bit-for-bit.
pub fn tensor_sum_all(t: &RssTensor) -> f64 {
    let mut acc = 0.0f32;
    for &x in t.data.iter() {
        acc += x;
    }
    acc as f64
}

/// Validate `axis` against `shape` and return it as a `usize`. Errors if negative
/// or `>= rank`.
fn check_axis(shape: &[usize], axis: i64, op_name: &str) -> Result<usize, TensorError> {
    let rank = shape.len();
    if axis < 0 || (axis as usize) >= rank {
        return Err(TensorError::new(format!(
            "{op_name} axis {axis} out of range for tensor of rank {rank} (shape {shape:?})"
        )));
    }
    Ok(axis as usize)
}

/// Reduce one `axis` of a row-major contiguous tensor, removing it. The result has
/// `shape` with index `axis` dropped (rank-1). `init` seeds each output cell and
/// `combine(acc, x)` folds successive elements along the reduced axis; `finalize`
/// post-processes each cell (e.g. divide by the axis length for a mean). The walk
/// uses the contiguous strides: an element at multi-index `i` contributes to the
/// output cell formed by deleting coordinate `axis` from `i`.
fn tensor_reduce_axis(
    t: &RssTensor,
    axis: usize,
    out_dtype: DType,
    init: f32,
    combine: impl Fn(f32, f32) -> f32,
    finalize: impl Fn(f32, usize) -> f32,
) -> RssTensor {
    let shape = &t.shape;
    let axis_len = shape[axis];
    // `outer` = product of dims before `axis`, `inner` = product of dims after it.
    // Row-major layout makes the buffer a sequence of `outer` blocks, each of
    // `axis_len` rows of `inner` contiguous elements. The reduced output is the
    // `outer × inner` grid (the axis collapsed away).
    let outer: usize = shape[..axis].iter().product();
    let inner: usize = shape[axis + 1..].iter().product();
    let out_len = outer * inner;
    let mut out = vec![init; out_len];
    let data = t.data.as_ref();
    for o in 0..outer {
        for a in 0..axis_len {
            let row = (o * axis_len + a) * inner;
            let out_base = o * inner;
            for i in 0..inner {
                out[out_base + i] = combine(out[out_base + i], data[row + i]);
            }
        }
    }
    for cell in out.iter_mut() {
        *cell = finalize(*cell, axis_len);
    }
    let mut out_shape = shape.clone();
    out_shape.remove(axis);
    RssTensor {
        data: Rc::new(out),
        shape: out_shape,
        dtype: out_dtype,
    }
}

/// Sum over one `axis`, removing it. Result is rank `rank-1`, dtype F32. Errors on
/// an out-of-range axis.
pub fn tensor_sum_axis(t: &RssTensor, axis: i64) -> Result<RssTensor, TensorError> {
    let axis = check_axis(&t.shape, axis, "sum_axis")?;
    Ok(tensor_reduce_axis(
        t,
        axis,
        DType::F32,
        0.0,
        |acc, x| acc + x,
        |acc, _| acc,
    ))
}

/// Max over one `axis`, removing it. Seeded with `-inf` so any real value wins;
/// note that an empty axis (dim 0) yields `-inf`. Output dtype = input dtype.
/// Errors on an out-of-range axis.
pub fn tensor_max_axis(t: &RssTensor, axis: i64) -> Result<RssTensor, TensorError> {
    let axis = check_axis(&t.shape, axis, "max_axis")?;
    Ok(tensor_reduce_axis(
        t,
        axis,
        t.dtype,
        f32::NEG_INFINITY,
        |acc, x| acc.max(x),
        |acc, _| acc,
    ))
}

/// Mean over one `axis`, removing it (sum / axis length), dtype F32. Errors on an
/// out-of-range axis.
pub fn tensor_mean_axis(t: &RssTensor, axis: i64) -> Result<RssTensor, TensorError> {
    let axis = check_axis(&t.shape, axis, "mean_axis")?;
    Ok(tensor_reduce_axis(
        t,
        axis,
        DType::F32,
        0.0,
        |acc, x| acc + x,
        |acc, len| if len == 0 { acc } else { acc / len as f32 },
    ))
}

/// Index of the maximum along one `axis`, removing it. The result tensor holds the
/// integer indices stored as `f32` (so it is a regular `Tensor`; callers read them
/// back via `to_f32_slice` and round). Ties resolve to the lowest index. Errors on
/// an out-of-range axis; an empty axis yields index `0`.
///
/// This cannot reuse `tensor_reduce_axis` because it tracks two pieces of state
/// per cell (best value seen + its index), so it has its own walk.
pub fn tensor_argmax_axis(t: &RssTensor, axis: i64) -> Result<RssTensor, TensorError> {
    let axis = check_axis(&t.shape, axis, "argmax_axis")?;
    let shape = &t.shape;
    let axis_len = shape[axis];
    let outer: usize = shape[..axis].iter().product();
    let inner: usize = shape[axis + 1..].iter().product();
    let out_len = outer * inner;
    let mut best_val = vec![f32::NEG_INFINITY; out_len];
    let mut best_idx = vec![0.0f32; out_len];
    let data = t.data.as_ref();
    for o in 0..outer {
        for a in 0..axis_len {
            let row = (o * axis_len + a) * inner;
            let out_base = o * inner;
            for i in 0..inner {
                let v = data[row + i];
                let cell = out_base + i;
                if v > best_val[cell] {
                    best_val[cell] = v;
                    best_idx[cell] = a as f32;
                }
            }
        }
    }
    let mut out_shape = shape.clone();
    out_shape.remove(axis);
    Ok(RssTensor {
        data: Rc::new(best_idx),
        shape: out_shape,
        dtype: DType::I32,
    })
}

// ---------------------------------------------------------------------------
// Movement ops (slice 4).
//
// DESIGN: `RssTensor` is kept ROW-MAJOR CONTIGUOUS — there is deliberately no
// `strides` field, because matmul/elementwise/reduce all assume packed data.
// So `reshape` is genuinely zero-copy (it shares the same `Rc<Vec<f32>>` buffer
// and only swaps the shape), while `transpose`/`permute`/`broadcast_to`
// MATERIALIZE a fresh contiguous buffer in the new logical order. True
// zero-copy strided views (sharing storage for transpose/broadcast) are a
// future optimization and intentionally out of scope here.
// ---------------------------------------------------------------------------

/// Reshape to `new_shape` WITHOUT copying: the returned tensor aliases the same
/// `Rc<Vec<f32>>` buffer (cheap `Rc::clone`), only the shape changes. Errors if
/// any new dim is negative or the new element count differs from the current
/// one (reshape must preserve the total number of elements).
pub fn tensor_reshape(t: &RssTensor, new_shape: &[i64]) -> Result<RssTensor, TensorError> {
    let mut dims = Vec::with_capacity(new_shape.len());
    for &dim in new_shape {
        if dim < 0 {
            return Err(TensorError::new(format!(
                "reshape dimensions must be non-negative, got {dim}"
            )));
        }
        dims.push(dim as usize);
    }
    let expected = RssTensor::shape_len(&dims);
    let current = t.data.len();
    if expected != current {
        return Err(TensorError::new(format!(
            "reshape element count mismatch: tensor has {current} elements but shape {dims:?} implies {expected}"
        )));
    }
    Ok(RssTensor {
        // Zero-copy: share the existing buffer, only the shape differs.
        data: Rc::clone(&t.data),
        shape: dims,
        dtype: t.dtype,
    })
}

/// 2-D transpose: `(r, c) -> (c, r)`, materializing a fresh contiguous buffer in
/// transposed order. Errors if the tensor is not rank-2.
pub fn tensor_transpose(t: &RssTensor) -> Result<RssTensor, TensorError> {
    if t.shape.len() != 2 {
        return Err(TensorError::new(format!(
            "transpose requires a rank-2 tensor, got shape {:?}",
            t.shape
        )));
    }
    let (rows, cols) = (t.shape[0], t.shape[1]);
    let src = t.data.as_ref();
    let mut out = vec![0.0f32; rows * cols];
    for i in 0..rows {
        for j in 0..cols {
            // out[j, i] = src[i, j]
            out[j * rows + i] = src[i * cols + j];
        }
    }
    Ok(RssTensor {
        data: Rc::new(out),
        shape: vec![cols, rows],
        dtype: t.dtype,
    })
}

/// General axis permutation: `axes` must be a permutation of `0..rank`. The
/// output dim `d` is the source dim `axes[d]`. Materializes a fresh contiguous
/// buffer in the permuted order. Errors if `axes` is not a valid permutation.
pub fn tensor_permute(t: &RssTensor, axes: &[i64]) -> Result<RssTensor, TensorError> {
    let rank = t.shape.len();
    if axes.len() != rank {
        return Err(TensorError::new(format!(
            "permute axes length {} does not match tensor rank {rank}",
            axes.len()
        )));
    }
    let mut perm = Vec::with_capacity(rank);
    let mut seen = vec![false; rank];
    for &axis in axes {
        if axis < 0 || axis as usize >= rank {
            return Err(TensorError::new(format!(
                "permute axis {axis} out of range for rank {rank}"
            )));
        }
        let a = axis as usize;
        if seen[a] {
            return Err(TensorError::new(format!(
                "permute axes {axes:?} must be a permutation of 0..{rank} (duplicate axis {a})"
            )));
        }
        seen[a] = true;
        perm.push(a);
    }

    // Output shape: out_shape[d] = src_shape[perm[d]].
    let out_shape: Vec<usize> = perm.iter().map(|&a| t.shape[a]).collect();
    let total = RssTensor::shape_len(&out_shape);
    let src = t.data.as_ref();

    // Row-major strides for the source, so a multi-index maps to a flat offset.
    let mut src_strides = vec![0usize; rank];
    let mut acc = 1usize;
    for axis in (0..rank).rev() {
        src_strides[axis] = acc;
        acc *= t.shape[axis];
    }

    let mut out = vec![0.0f32; total];
    // Walk the output in row-major order, decoding each flat index into the
    // output multi-index, then mapping back to the source offset via perm.
    let mut out_index = vec![0usize; rank];
    for slot in out.iter_mut() {
        let mut src_offset = 0usize;
        for d in 0..rank {
            // out dim d corresponds to source axis perm[d].
            src_offset += out_index[d] * src_strides[perm[d]];
        }
        *slot = src[src_offset];
        // Increment the mixed-radix output index (last axis fastest).
        for d in (0..rank).rev() {
            out_index[d] += 1;
            if out_index[d] < out_shape[d] {
                break;
            }
            out_index[d] = 0;
        }
    }
    Ok(RssTensor {
        data: Rc::new(out),
        shape: out_shape,
        dtype: t.dtype,
    })
}

/// NumPy-style broadcast to `target_shape`, materializing the expanded buffer.
/// The source shape is right-aligned against the target; each source dim must be
/// either 1 (stretched) or equal to the target dim. The target rank must be at
/// least the source rank. Errors if the shapes are not broadcast-compatible.
pub fn tensor_broadcast_to(t: &RssTensor, target_shape: &[i64]) -> Result<RssTensor, TensorError> {
    let mut target = Vec::with_capacity(target_shape.len());
    for &dim in target_shape {
        if dim < 0 {
            return Err(TensorError::new(format!(
                "broadcast_to dimensions must be non-negative, got {dim}"
            )));
        }
        target.push(dim as usize);
    }
    let src_rank = t.shape.len();
    let tgt_rank = target.len();
    if tgt_rank < src_rank {
        return Err(TensorError::new(format!(
            "broadcast_to cannot reduce rank: source shape {:?} into {target:?}",
            t.shape
        )));
    }

    // Right-align the source shape against the target; missing leading dims are
    // treated as 1. `src_strides[d]` is the source stride for target axis d (0
    // for a broadcast/stretched axis).
    let offset = tgt_rank - src_rank;
    let mut row_strides = vec![0usize; src_rank];
    let mut acc = 1usize;
    for axis in (0..src_rank).rev() {
        row_strides[axis] = acc;
        acc *= t.shape[axis];
    }
    let mut src_strides = vec![0usize; tgt_rank];
    for d in 0..tgt_rank {
        if d < offset {
            // Leading target axis with no source dim: implicitly size 1, stride 0.
            src_strides[d] = 0;
        } else {
            let s = t.shape[d - offset];
            if s == target[d] {
                src_strides[d] = row_strides[d - offset];
            } else if s == 1 {
                // Stretched axis: stride 0 so the single source element repeats.
                src_strides[d] = 0;
            } else {
                return Err(TensorError::new(format!(
                    "broadcast_to: source shape {:?} is not broadcastable to {target:?} (dim {d}: {s} vs {})",
                    t.shape, target[d]
                )));
            }
        }
    }

    let total = RssTensor::shape_len(&target);
    let src = t.data.as_ref();
    let mut out = vec![0.0f32; total];
    let mut out_index = vec![0usize; tgt_rank];
    for slot in out.iter_mut() {
        let mut src_offset = 0usize;
        for d in 0..tgt_rank {
            src_offset += out_index[d] * src_strides[d];
        }
        *slot = src[src_offset];
        for d in (0..tgt_rank).rev() {
            out_index[d] += 1;
            if out_index[d] < target[d] {
                break;
            }
            out_index[d] = 0;
        }
    }
    Ok(RssTensor {
        data: Rc::new(out),
        shape: target,
        dtype: t.dtype,
    })
}

// ---------------------------------------------------------------------------
// movement+gather (ops B).
//
// `pad`/`shrink`/`flip` keep the row-major-contiguous model: each materializes a
// fresh packed buffer in the new logical order, preserving the input dtype.
// `gather` selects rows along one axis using a rank-1 integer index tensor.
// ---------------------------------------------------------------------------

/// Row-major strides for a contiguous shape (stride of the last axis is 1).
fn row_major_strides(shape: &[usize]) -> Vec<usize> {
    let rank = shape.len();
    let mut strides = vec![0usize; rank];
    let mut acc = 1usize;
    for axis in (0..rank).rev() {
        strides[axis] = acc;
        acc *= shape[axis];
    }
    strides
}

/// Pad each axis with `0.0`. `pads` is flat per-axis `[before0, after0, before1,
/// after1, ...]` (length == 2*rank). Output dim `d` = `before[d] + old[d] +
/// after[d]`. Errors if `pads` length != 2*rank or any pad is negative.
pub fn tensor_pad(t: &RssTensor, pads: &[i64]) -> Result<RssTensor, TensorError> {
    let rank = t.shape.len();
    if pads.len() != 2 * rank {
        return Err(TensorError::new(format!(
            "pad expects {} values (2 per axis) for a rank-{rank} tensor, got {}",
            2 * rank,
            pads.len()
        )));
    }
    let mut before = vec![0usize; rank];
    let mut out_shape = vec![0usize; rank];
    for axis in 0..rank {
        let b = pads[2 * axis];
        let a = pads[2 * axis + 1];
        if b < 0 || a < 0 {
            return Err(TensorError::new(format!(
                "pad amounts must be non-negative, got [{b}, {a}] for axis {axis}"
            )));
        }
        before[axis] = b as usize;
        out_shape[axis] = b as usize + t.shape[axis] + a as usize;
    }
    let total = RssTensor::shape_len(&out_shape);
    let src = t.data.as_ref();
    let src_strides = row_major_strides(&t.shape);
    let mut out = vec![0.0f32; total];
    // Walk the output in row-major order; a cell maps back to the source iff every
    // coordinate lands inside the original (un-padded) region.
    let mut out_index = vec![0usize; rank];
    for slot in out.iter_mut() {
        let mut in_bounds = true;
        let mut src_offset = 0usize;
        for d in 0..rank {
            if out_index[d] < before[d] || out_index[d] >= before[d] + t.shape[d] {
                in_bounds = false;
                break;
            }
            src_offset += (out_index[d] - before[d]) * src_strides[d];
        }
        if in_bounds {
            *slot = src[src_offset];
        }
        for d in (0..rank).rev() {
            out_index[d] += 1;
            if out_index[d] < out_shape[d] {
                break;
            }
            out_index[d] = 0;
        }
    }
    Ok(RssTensor {
        data: Rc::new(out),
        shape: out_shape,
        dtype: t.dtype,
    })
}

/// Slice each axis to the half-open range `[start, end)`. `bounds` is flat per-axis
/// `[start0, end0, start1, end1, ...]` (length == 2*rank). Output dim `d` =
/// `end[d] - start[d]`. Errors if `bounds` length != 2*rank or any axis violates
/// `0 <= start <= end <= dim`.
pub fn tensor_shrink(t: &RssTensor, bounds: &[i64]) -> Result<RssTensor, TensorError> {
    let rank = t.shape.len();
    if bounds.len() != 2 * rank {
        return Err(TensorError::new(format!(
            "shrink expects {} values (2 per axis) for a rank-{rank} tensor, got {}",
            2 * rank,
            bounds.len()
        )));
    }
    let mut start = vec![0usize; rank];
    let mut out_shape = vec![0usize; rank];
    for axis in 0..rank {
        let s = bounds[2 * axis];
        let e = bounds[2 * axis + 1];
        let dim = t.shape[axis] as i64;
        if s < 0 || s > e || e > dim {
            return Err(TensorError::new(format!(
                "shrink bounds [{s}, {e}) out of range for axis {axis} of length {dim} (require 0 <= start <= end <= dim)"
            )));
        }
        start[axis] = s as usize;
        out_shape[axis] = (e - s) as usize;
    }
    let total = RssTensor::shape_len(&out_shape);
    let src = t.data.as_ref();
    let src_strides = row_major_strides(&t.shape);
    let mut out = vec![0.0f32; total];
    let mut out_index = vec![0usize; rank];
    for slot in out.iter_mut() {
        let mut src_offset = 0usize;
        for d in 0..rank {
            src_offset += (out_index[d] + start[d]) * src_strides[d];
        }
        *slot = src[src_offset];
        for d in (0..rank).rev() {
            out_index[d] += 1;
            if out_index[d] < out_shape[d] {
                break;
            }
            out_index[d] = 0;
        }
    }
    Ok(RssTensor {
        data: Rc::new(out),
        shape: out_shape,
        dtype: t.dtype,
    })
}

/// Reverse the tensor along each listed axis, materializing a fresh buffer. The
/// shape is unchanged. Errors if any axis is out of range or appears twice.
pub fn tensor_flip(t: &RssTensor, axes: &[i64]) -> Result<RssTensor, TensorError> {
    let rank = t.shape.len();
    let mut flip = vec![false; rank];
    for &axis in axes {
        if axis < 0 || axis as usize >= rank {
            return Err(TensorError::new(format!(
                "flip axis {axis} out of range for tensor of rank {rank}"
            )));
        }
        let a = axis as usize;
        if flip[a] {
            return Err(TensorError::new(format!(
                "flip axes {axes:?} contain duplicate axis {a}"
            )));
        }
        flip[a] = true;
    }
    let total = t.data.len();
    let src = t.data.as_ref();
    let src_strides = row_major_strides(&t.shape);
    let mut out = vec![0.0f32; total];
    let mut out_index = vec![0usize; rank];
    for slot in out.iter_mut() {
        let mut src_offset = 0usize;
        for d in 0..rank {
            let coord = if flip[d] {
                t.shape[d] - 1 - out_index[d]
            } else {
                out_index[d]
            };
            src_offset += coord * src_strides[d];
        }
        *slot = src[src_offset];
        for d in (0..rank).rev() {
            out_index[d] += 1;
            if out_index[d] < t.shape[d] {
                break;
            }
            out_index[d] = 0;
        }
    }
    Ok(RssTensor {
        data: Rc::new(out),
        shape: t.shape.clone(),
        dtype: t.dtype,
    })
}

/// Gather rows along `axis` using a rank-1 integer-valued `indices` tensor. The
/// output shape equals `data.shape` with dim `axis` replaced by `indices.len()`:
/// `out[..., j, ...] = data[..., indices[j], ...]` along `axis`. Output dtype =
/// `data`'s dtype. Index values are read from `indices.data()` and rounded to the
/// nearest integer (they are I32-dtype `f32` values). Errors if `axis` is out of
/// range, `indices` is not rank-1, or any index is out of bounds.
///
/// NOTE: only a rank-1 index tensor along a single axis is supported in this slice.
pub fn tensor_gather(
    data: &RssTensor,
    axis: i64,
    indices: &RssTensor,
) -> Result<RssTensor, TensorError> {
    let axis = check_axis(&data.shape, axis, "gather")?;
    if indices.shape.len() != 1 {
        return Err(TensorError::new(format!(
            "gather requires a rank-1 indices tensor, got shape {:?}",
            indices.shape
        )));
    }
    let axis_len = data.shape[axis];
    let idx_len = indices.shape[0];
    // Resolve each index up front (rounding the f32 value) and bounds-check it.
    let mut resolved = Vec::with_capacity(idx_len);
    for (j, &raw) in indices.data().iter().enumerate() {
        let i = raw.round() as i64;
        if i < 0 || i as usize >= axis_len {
            return Err(TensorError::new(format!(
                "gather index {i} (at position {j}) out of range for axis {axis} of length {axis_len}"
            )));
        }
        resolved.push(i as usize);
    }
    let mut out_shape = data.shape.clone();
    out_shape[axis] = idx_len;
    let outer: usize = data.shape[..axis].iter().product();
    let inner: usize = data.shape[axis + 1..].iter().product();
    let src = data.data.as_ref();
    let mut out = vec![0.0f32; outer * idx_len * inner];
    for o in 0..outer {
        for (j, &src_a) in resolved.iter().enumerate() {
            let src_base = (o * axis_len + src_a) * inner;
            let dst_base = (o * idx_len + j) * inner;
            out[dst_base..dst_base + inner].copy_from_slice(&src[src_base..src_base + inner]);
        }
    }
    Ok(RssTensor {
        data: Rc::new(out),
        shape: out_shape,
        dtype: data.dtype,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_to_round_trip() {
        let tensor = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        assert_eq!(tensor_rank(&tensor), 2);
        assert_eq!(tensor_shape(&tensor).unwrap(), vec![2, 2]);
        assert_eq!(tensor_to_f32_slice(&tensor).unwrap(), vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn from_rejects_shape_mismatch() {
        let err = tensor_from_f32_slice(&[1.0, 2.0, 3.0], &[2, 2]).unwrap_err();
        assert!(tensor_error_message(&err).contains("does not match shape"));
    }

    #[test]
    fn matmul_2x2() {
        let a = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        let b = tensor_from_f32_slice(&[5.0, 6.0, 7.0, 8.0], &[2, 2]).unwrap();
        let c = tensor_matmul(&a, &b).unwrap();
        assert_eq!(tensor_shape(&c).unwrap(), vec![2, 2]);
        // [[1*5+2*7, 1*6+2*8],[3*5+4*7, 3*6+4*8]] = [[19,22],[43,50]]
        assert_eq!(tensor_to_f32_slice(&c).unwrap(), vec![19.0, 22.0, 43.0, 50.0]);
    }

    #[test]
    fn matmul_rejects_shape_mismatch() {
        let a = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]).unwrap();
        let b = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        let err = tensor_matmul(&a, &b).unwrap_err();
        assert!(tensor_error_message(&err).contains("inner dimensions disagree"));
    }

    #[test]
    fn matmul_rejects_non_rank2() {
        let a = tensor_from_f32_slice(&[1.0, 2.0, 3.0], &[3]).unwrap();
        let b = tensor_from_f32_slice(&[1.0, 2.0, 3.0], &[3]).unwrap();
        let err = tensor_matmul(&a, &b).unwrap_err();
        assert!(tensor_error_message(&err).contains("rank-2"));
    }

    #[test]
    fn elementwise_binary_ops() {
        let a = tensor_from_f32_slice(&[1.0, 2.0, 4.0, 8.0], &[2, 2]).unwrap();
        let b = tensor_from_f32_slice(&[2.0, 2.0, 2.0, 2.0], &[2, 2]).unwrap();
        assert_eq!(
            tensor_to_f32_slice(&tensor_add(&a, &b).unwrap()).unwrap(),
            vec![3.0, 4.0, 6.0, 10.0]
        );
        assert_eq!(
            tensor_to_f32_slice(&tensor_sub(&a, &b).unwrap()).unwrap(),
            vec![-1.0, 0.0, 2.0, 6.0]
        );
        assert_eq!(
            tensor_to_f32_slice(&tensor_mul(&a, &b).unwrap()).unwrap(),
            vec![2.0, 4.0, 8.0, 16.0]
        );
        assert_eq!(
            tensor_to_f32_slice(&tensor_div(&a, &b).unwrap()).unwrap(),
            vec![0.5, 1.0, 2.0, 4.0]
        );
    }

    #[test]
    fn binary_ops_reject_shape_mismatch() {
        let a = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        let b = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]).unwrap();
        for op in [tensor_add, tensor_sub, tensor_mul, tensor_div] {
            let err = op(&a, &b).unwrap_err();
            assert!(tensor_error_message(&err).contains("not broadcast-compatible"));
        }
    }

    #[test]
    fn binary_ops_broadcast() {
        // [2,3] + [3] -> broadcast the row.
        let a = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]).unwrap();
        let row = tensor_from_f32_slice(&[10.0, 20.0, 30.0], &[3]).unwrap();
        let sum = tensor_add(&a, &row).unwrap();
        assert_eq!(tensor_shape(&sum).unwrap(), vec![2, 3]);
        assert_eq!(
            tensor_to_f32_slice(&sum).unwrap(),
            vec![11.0, 22.0, 33.0, 14.0, 25.0, 36.0]
        );
        // [2,3] + [1,3] same result.
        let row2 = tensor_from_f32_slice(&[10.0, 20.0, 30.0], &[1, 3]).unwrap();
        assert_eq!(
            tensor_to_f32_slice(&tensor_add(&a, &row2).unwrap()).unwrap(),
            tensor_to_f32_slice(&sum).unwrap()
        );
        // [2,1] * [2,3] stretches the column operand.
        let col = tensor_from_f32_slice(&[2.0, 3.0], &[2, 1]).unwrap();
        assert_eq!(
            tensor_to_f32_slice(&tensor_mul(&col, &a).unwrap()).unwrap(),
            vec![2.0, 4.0, 6.0, 12.0, 15.0, 18.0]
        );
    }

    #[test]
    fn dtype_tags_and_promotion() {
        let f = tensor_from_f32_slice(&[1.0, 2.0], &[2]).unwrap();
        assert_eq!(tensor_dtype_code(&f), 0); // F32
        let i = tensor_cast_i32(&tensor_from_f32_slice(&[1.7, -2.9], &[2]).unwrap());
        assert_eq!(tensor_dtype_code(&i), 1); // I32
        assert_eq!(tensor_to_f32_slice(&i).unwrap(), vec![1.0, -2.0]); // trunc toward zero
        let bo = tensor_cast_bool(&tensor_from_f32_slice(&[0.0, 3.0], &[2]).unwrap());
        assert_eq!(tensor_dtype_code(&bo), 2); // Bool
        assert_eq!(tensor_to_f32_slice(&bo).unwrap(), vec![0.0, 1.0]);
        // I32 + I32 -> I32; I32 + F32 -> F32.
        let i2 = tensor_cast_i32(&f);
        assert_eq!(tensor_dtype_code(&tensor_add(&i, &i2).unwrap()), 1);
        assert_eq!(tensor_dtype_code(&tensor_add(&i, &f).unwrap()), 0);
        // Bool treated as int: Bool + Bool -> I32.
        assert_eq!(tensor_dtype_code(&tensor_add(&bo, &bo).unwrap()), 1);
        // neg preserves dtype; exp promotes to F32; argmax -> I32; cast_f32 -> F32.
        assert_eq!(tensor_dtype_code(&tensor_neg(&i)), 1);
        assert_eq!(tensor_dtype_code(&tensor_exp(&i)), 0);
        assert_eq!(tensor_dtype_code(&tensor_cast_f32(&i)), 0);
        let m = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        assert_eq!(tensor_dtype_code(&tensor_argmax_axis(&m, 0).unwrap()), 1);
        assert_eq!(tensor_dtype_code(&tensor_sum_axis(&m, 0).unwrap()), 0);
        // max_axis preserves input dtype.
        assert_eq!(
            tensor_dtype_code(&tensor_max_axis(&tensor_cast_i32(&m), 0).unwrap()),
            1
        );
    }

    #[test]
    fn comparisons_and_select() {
        let a = tensor_from_f32_slice(&[1.0, 5.0, 3.0], &[3]).unwrap();
        let b = tensor_from_f32_slice(&[2.0, 5.0, 1.0], &[3]).unwrap();
        let lt = tensor_cmplt(&a, &b).unwrap();
        assert_eq!(tensor_dtype_code(&lt), 2);
        assert_eq!(tensor_to_f32_slice(&lt).unwrap(), vec![1.0, 0.0, 0.0]);
        assert_eq!(
            tensor_to_f32_slice(&tensor_cmpeq(&a, &b).unwrap()).unwrap(),
            vec![0.0, 1.0, 0.0]
        );
        assert_eq!(
            tensor_to_f32_slice(&tensor_cmpne(&a, &b).unwrap()).unwrap(),
            vec![1.0, 0.0, 1.0]
        );
        // select picks a where cond nonzero. cond = a<b.
        let sel = tensor_select(&lt, &a, &b).unwrap();
        assert_eq!(tensor_to_f32_slice(&sel).unwrap(), vec![1.0, 5.0, 1.0]);
        // maximum/minimum.
        assert_eq!(
            tensor_to_f32_slice(&tensor_maximum(&a, &b).unwrap()).unwrap(),
            vec![2.0, 5.0, 3.0]
        );
        assert_eq!(
            tensor_to_f32_slice(&tensor_minimum(&a, &b).unwrap()).unwrap(),
            vec![1.0, 5.0, 1.0]
        );
        // select broadcasts all three: scalar-ish cond [1] over [3].
        let cond1 = tensor_cast_bool(&tensor_from_f32_slice(&[1.0], &[1]).unwrap());
        assert_eq!(
            tensor_to_f32_slice(&tensor_select(&cond1, &a, &b).unwrap()).unwrap(),
            vec![1.0, 5.0, 3.0]
        );
        // incompatible select shapes error.
        let bad = tensor_from_f32_slice(&[1.0, 2.0], &[2]).unwrap();
        assert!(tensor_error_message(&tensor_select(&lt, &a, &bad).unwrap_err())
            .contains("not broadcast-compatible"));
    }

    #[test]
    fn elementwise_unary_ops() {
        let t = tensor_from_f32_slice(&[1.0, -2.0, 4.0, 0.0], &[2, 2]).unwrap();
        assert_eq!(
            tensor_to_f32_slice(&tensor_neg(&t)).unwrap(),
            vec![-1.0, 2.0, -4.0, 0.0]
        );
        assert_eq!(
            tensor_to_f32_slice(&tensor_relu(&t)).unwrap(),
            vec![1.0, 0.0, 4.0, 0.0]
        );
        let s = tensor_from_f32_slice(&[4.0, 9.0], &[2]).unwrap();
        assert_eq!(
            tensor_to_f32_slice(&tensor_sqrt(&s)).unwrap(),
            vec![2.0, 3.0]
        );
        // exp/log: round-trip log(exp(x)) ≈ x within f32 tolerance.
        let v = tensor_from_f32_slice(&[0.5, 1.5, 2.5], &[3]).unwrap();
        let round = tensor_log(&tensor_exp(&v));
        let got = tensor_to_f32_slice(&round).unwrap();
        for (g, e) in got.iter().zip([0.5_f64, 1.5, 2.5]) {
            assert!((g - e).abs() < 1e-5, "{g} vs {e}");
        }
        assert_eq!(tensor_shape(&tensor_neg(&t)).unwrap(), vec![2, 2]);
    }

    #[test]
    fn sum_all_global() {
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        assert_eq!(tensor_sum_all(&t), 10.0);
    }

    #[test]
    fn reduce_axis_2x3() {
        // [[1,2,3],[4,5,6]]
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]).unwrap();
        // sum axis 0 -> [5,7,9], shape [3]
        let s0 = tensor_sum_axis(&t, 0).unwrap();
        assert_eq!(tensor_shape(&s0).unwrap(), vec![3]);
        assert_eq!(tensor_to_f32_slice(&s0).unwrap(), vec![5.0, 7.0, 9.0]);
        // sum axis 1 -> [6,15], shape [2]
        let s1 = tensor_sum_axis(&t, 1).unwrap();
        assert_eq!(tensor_shape(&s1).unwrap(), vec![2]);
        assert_eq!(tensor_to_f32_slice(&s1).unwrap(), vec![6.0, 15.0]);
        // max axis 0 -> [4,5,6]
        assert_eq!(
            tensor_to_f32_slice(&tensor_max_axis(&t, 0).unwrap()).unwrap(),
            vec![4.0, 5.0, 6.0]
        );
        // mean axis 1 -> [2,5]
        assert_eq!(
            tensor_to_f32_slice(&tensor_mean_axis(&t, 1).unwrap()).unwrap(),
            vec![2.0, 5.0]
        );
        // argmax axis 0 -> [1,1,1] (second row holds the maxima)
        assert_eq!(
            tensor_to_f32_slice(&tensor_argmax_axis(&t, 0).unwrap()).unwrap(),
            vec![1.0, 1.0, 1.0]
        );
        // argmax axis 1 -> [2,2] (last column holds the maxima)
        assert_eq!(
            tensor_to_f32_slice(&tensor_argmax_axis(&t, 1).unwrap()).unwrap(),
            vec![2.0, 2.0]
        );
    }

    #[test]
    fn reduce_axis_rank3() {
        // shape [2,3,4], reduce axis 1 -> [2,4]
        let data: Vec<f64> = (0..24).map(|x| x as f64).collect();
        let t = tensor_from_f32_slice(&data, &[2, 3, 4]).unwrap();
        let s = tensor_sum_axis(&t, 1).unwrap();
        assert_eq!(tensor_shape(&s).unwrap(), vec![2, 4]);
        // out[o,i] = sum_a data[o,a,i]; data[o,a,i] = o*12 + a*4 + i
        // = sum_{a=0..2}(o*12 + a*4 + i) = 3*(o*12+i) + 4*(0+1+2) = 36*... let's just compute
        let mut expected = vec![0.0f64; 8];
        for o in 0..2 {
            for a in 0..3 {
                for i in 0..4 {
                    expected[o * 4 + i] += (o * 12 + a * 4 + i) as f64;
                }
            }
        }
        assert_eq!(tensor_to_f32_slice(&s).unwrap(), expected);
    }

    #[test]
    fn reduce_axis_rejects_out_of_range() {
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        assert!(tensor_error_message(&tensor_sum_axis(&t, 2).unwrap_err()).contains("out of range"));
        assert!(
            tensor_error_message(&tensor_max_axis(&t, -1).unwrap_err()).contains("out of range")
        );
    }

    #[test]
    fn argmax_ties_pick_lowest() {
        // axis 0: column 0 both 5.0 -> index 0; column 1 -> 2 wins at index 1
        let t = tensor_from_f32_slice(&[5.0, 1.0, 5.0, 2.0], &[2, 2]).unwrap();
        assert_eq!(
            tensor_to_f32_slice(&tensor_argmax_axis(&t, 0).unwrap()).unwrap(),
            vec![0.0, 1.0]
        );
    }

    #[test]
    fn reshape_is_zero_copy() {
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]).unwrap();
        let r = tensor_reshape(&t, &[3, 2]).unwrap();
        assert_eq!(tensor_shape(&r).unwrap(), vec![3, 2]);
        assert_eq!(
            tensor_to_f32_slice(&r).unwrap(),
            vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]
        );
        // Same backing buffer (zero-copy): the Rc points at the same allocation.
        assert!(Rc::ptr_eq(&t.data, &r.data));
    }

    #[test]
    fn reshape_rejects_count_mismatch() {
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        let err = tensor_reshape(&t, &[3, 2]).unwrap_err();
        assert!(tensor_error_message(&err).contains("element count mismatch"));
    }

    #[test]
    fn transpose_2d() {
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]).unwrap();
        let tr = tensor_transpose(&t).unwrap();
        assert_eq!(tensor_shape(&tr).unwrap(), vec![3, 2]);
        // [[1,2,3],[4,5,6]]^T = [[1,4],[2,5],[3,6]]
        assert_eq!(
            tensor_to_f32_slice(&tr).unwrap(),
            vec![1.0, 4.0, 2.0, 5.0, 3.0, 6.0]
        );
    }

    #[test]
    fn transpose_rejects_non_rank2() {
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0], &[3]).unwrap();
        let err = tensor_transpose(&t).unwrap_err();
        assert!(tensor_error_message(&err).contains("rank-2"));
    }

    #[test]
    fn permute_matches_transpose_for_2d() {
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]).unwrap();
        let p = tensor_permute(&t, &[1, 0]).unwrap();
        let tr = tensor_transpose(&t).unwrap();
        assert_eq!(tensor_shape(&p).unwrap(), tensor_shape(&tr).unwrap());
        assert_eq!(
            tensor_to_f32_slice(&p).unwrap(),
            tensor_to_f32_slice(&tr).unwrap()
        );
    }

    #[test]
    fn permute_3d() {
        // shape [2,1,3], values 0..6 row-major.
        let t =
            tensor_from_f32_slice(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0], &[2, 1, 3]).unwrap();
        // permute axes [2,0,1] -> shape [3,2,1].
        let p = tensor_permute(&t, &[2, 0, 1]).unwrap();
        assert_eq!(tensor_shape(&p).unwrap(), vec![3, 2, 1]);
        // src[i,0,k] at flat i*3+k. out[k,i,0] = src[i,0,k].
        // out row-major over (k,i): k=0:(i0,i1)->src0,src3; k=1->src1,src4; k=2->src2,src5
        assert_eq!(
            tensor_to_f32_slice(&p).unwrap(),
            vec![0.0, 3.0, 1.0, 4.0, 2.0, 5.0]
        );
    }

    #[test]
    fn permute_rejects_bad_axes() {
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        assert!(tensor_error_message(&tensor_permute(&t, &[0, 0]).unwrap_err())
            .contains("permutation"));
        assert!(tensor_error_message(&tensor_permute(&t, &[0]).unwrap_err())
            .contains("does not match tensor rank"));
        assert!(tensor_error_message(&tensor_permute(&t, &[0, 5]).unwrap_err())
            .contains("out of range"));
    }

    #[test]
    fn broadcast_to_expands() {
        // [3] -> [2,3]: each row is the source.
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0], &[3]).unwrap();
        let b = tensor_broadcast_to(&t, &[2, 3]).unwrap();
        assert_eq!(tensor_shape(&b).unwrap(), vec![2, 3]);
        assert_eq!(
            tensor_to_f32_slice(&b).unwrap(),
            vec![1.0, 2.0, 3.0, 1.0, 2.0, 3.0]
        );
        // [2,1] -> [2,3]: stretch the inner axis.
        let c = tensor_from_f32_slice(&[10.0, 20.0], &[2, 1]).unwrap();
        let bc = tensor_broadcast_to(&c, &[2, 3]).unwrap();
        assert_eq!(
            tensor_to_f32_slice(&bc).unwrap(),
            vec![10.0, 10.0, 10.0, 20.0, 20.0, 20.0]
        );
    }

    #[test]
    fn broadcast_to_rejects_incompatible() {
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0], &[3]).unwrap();
        assert!(tensor_error_message(&tensor_broadcast_to(&t, &[2, 4]).unwrap_err())
            .contains("not broadcastable"));
        let r = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        assert!(tensor_error_message(&tensor_broadcast_to(&r, &[2]).unwrap_err())
            .contains("cannot reduce rank"));
    }

    #[test]
    fn pad_2d() {
        // [[1,2],[3,4]] pad axis0 (before 1, after 0), axis1 (before 0, after 2).
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        let p = tensor_pad(&t, &[1, 0, 0, 2]).unwrap();
        assert_eq!(tensor_shape(&p).unwrap(), vec![3, 4]);
        assert_eq!(
            tensor_to_f32_slice(&p).unwrap(),
            vec![
                0.0, 0.0, 0.0, 0.0, // padded leading row
                1.0, 2.0, 0.0, 0.0, // row 0 + trailing zeros
                3.0, 4.0, 0.0, 0.0, // row 1 + trailing zeros
            ]
        );
        // dtype preserved.
        assert_eq!(tensor_dtype_code(&tensor_pad(&tensor_cast_i32(&t), &[0, 0, 0, 0]).unwrap()), 1);
    }

    #[test]
    fn pad_rejects_bad_args() {
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        assert!(tensor_error_message(&tensor_pad(&t, &[1, 0]).unwrap_err()).contains("2 per axis"));
        assert!(
            tensor_error_message(&tensor_pad(&t, &[1, -1, 0, 0]).unwrap_err())
                .contains("non-negative")
        );
    }

    #[test]
    fn shrink_2d() {
        // [[1,2,3],[4,5,6]] shrink axis0 [0,1), axis1 [1,3) -> [[2,3]]
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]).unwrap();
        let s = tensor_shrink(&t, &[0, 1, 1, 3]).unwrap();
        assert_eq!(tensor_shape(&s).unwrap(), vec![1, 2]);
        assert_eq!(tensor_to_f32_slice(&s).unwrap(), vec![2.0, 3.0]);
    }

    #[test]
    fn shrink_rejects_bad_args() {
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        assert!(
            tensor_error_message(&tensor_shrink(&t, &[0, 2]).unwrap_err()).contains("2 per axis")
        );
        // end > dim.
        assert!(
            tensor_error_message(&tensor_shrink(&t, &[0, 3, 0, 2]).unwrap_err())
                .contains("out of range")
        );
        // start > end.
        assert!(
            tensor_error_message(&tensor_shrink(&t, &[1, 0, 0, 2]).unwrap_err())
                .contains("out of range")
        );
    }

    #[test]
    fn flip_2d() {
        // [[1,2,3],[4,5,6]] flip axis 1 -> [[3,2,1],[6,5,4]]
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]).unwrap();
        let f1 = tensor_flip(&t, &[1]).unwrap();
        assert_eq!(tensor_shape(&f1).unwrap(), vec![2, 3]);
        assert_eq!(tensor_to_f32_slice(&f1).unwrap(), vec![3.0, 2.0, 1.0, 6.0, 5.0, 4.0]);
        // flip both axes -> reverse the whole buffer.
        let fb = tensor_flip(&t, &[0, 1]).unwrap();
        assert_eq!(
            tensor_to_f32_slice(&fb).unwrap(),
            vec![6.0, 5.0, 4.0, 3.0, 2.0, 1.0]
        );
        // empty axes list is a no-op copy.
        assert_eq!(
            tensor_to_f32_slice(&tensor_flip(&t, &[]).unwrap()).unwrap(),
            tensor_to_f32_slice(&t).unwrap()
        );
    }

    #[test]
    fn flip_rejects_bad_axes() {
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        assert!(tensor_error_message(&tensor_flip(&t, &[2]).unwrap_err()).contains("out of range"));
        assert!(
            tensor_error_message(&tensor_flip(&t, &[0, 0]).unwrap_err()).contains("duplicate axis")
        );
    }

    #[test]
    fn gather_axis0_and_axis1() {
        // [[1,2,3],[4,5,6]] gather axis 0 with [1,0,1] -> rows 1,0,1
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]).unwrap();
        let idx = tensor_cast_i32(&tensor_from_f32_slice(&[1.0, 0.0, 1.0], &[3]).unwrap());
        let g0 = tensor_gather(&t, 0, &idx).unwrap();
        assert_eq!(tensor_shape(&g0).unwrap(), vec![3, 3]);
        assert_eq!(
            tensor_to_f32_slice(&g0).unwrap(),
            vec![4.0, 5.0, 6.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0]
        );
        // gather axis 1 with [2,0] -> columns 2,0
        let idx1 = tensor_cast_i32(&tensor_from_f32_slice(&[2.0, 0.0], &[2]).unwrap());
        let g1 = tensor_gather(&t, 1, &idx1).unwrap();
        assert_eq!(tensor_shape(&g1).unwrap(), vec![2, 2]);
        assert_eq!(tensor_to_f32_slice(&g1).unwrap(), vec![3.0, 1.0, 6.0, 4.0]);
        // output dtype = data dtype.
        assert_eq!(tensor_dtype_code(&g0), 0);
        assert_eq!(tensor_dtype_code(&tensor_gather(&tensor_cast_i32(&t), 0, &idx).unwrap()), 1);
    }

    #[test]
    fn gather_rejects_bad_args() {
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        let idx = tensor_cast_i32(&tensor_from_f32_slice(&[0.0], &[1]).unwrap());
        assert!(
            tensor_error_message(&tensor_gather(&t, 5, &idx).unwrap_err()).contains("out of range")
        );
        // out-of-bounds index.
        let bad = tensor_cast_i32(&tensor_from_f32_slice(&[5.0], &[1]).unwrap());
        assert!(
            tensor_error_message(&tensor_gather(&t, 0, &bad).unwrap_err()).contains("out of range")
        );
        // rank-2 index tensor rejected.
        let idx2 = tensor_cast_i32(&tensor_from_f32_slice(&[0.0, 1.0], &[1, 2]).unwrap());
        assert!(
            tensor_error_message(&tensor_gather(&t, 0, &idx2).unwrap_err()).contains("rank-1")
        );
    }
}
