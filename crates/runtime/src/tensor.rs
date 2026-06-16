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

// reductions+math (ops C)

/// Product over one `axis`, removing it. Result is rank `rank-1`, dtype F32.
/// Errors on an out-of-range axis. An empty axis (dim 0) yields the empty
/// product `1.0`.
pub fn tensor_prod_axis(t: &RssTensor, axis: i64) -> Result<RssTensor, TensorError> {
    let axis = check_axis(&t.shape, axis, "prod_axis")?;
    Ok(tensor_reduce_axis(
        t,
        axis,
        DType::F32,
        1.0,
        |acc, x| acc * x,
        |acc, _| acc,
    ))
}

/// Min over one `axis`, removing it. Seeded with `+inf` so any real value wins;
/// an empty axis (dim 0) yields `+inf`. Output dtype = input dtype. Errors on an
/// out-of-range axis.
pub fn tensor_min_axis(t: &RssTensor, axis: i64) -> Result<RssTensor, TensorError> {
    let axis = check_axis(&t.shape, axis, "min_axis")?;
    Ok(tensor_reduce_axis(
        t,
        axis,
        t.dtype,
        f32::INFINITY,
        |acc, x| acc.min(x),
        |acc, _| acc,
    ))
}

/// Validate a list of reduction `axes` against `shape`: each must be in range and
/// distinct. Returns the axes as `usize` sorted DESCENDING, so a caller can reduce
/// the highest axis first (keeping the lower indices valid as the rank shrinks).
/// An empty `axes` list is allowed and yields an empty vector (identity reduction).
fn check_axes(shape: &[usize], axes: &[i64], op_name: &str) -> Result<Vec<usize>, TensorError> {
    let rank = shape.len();
    let mut seen = vec![false; rank];
    let mut out = Vec::with_capacity(axes.len());
    for &axis in axes {
        if axis < 0 || (axis as usize) >= rank {
            return Err(TensorError::new(format!(
                "{op_name} axis {axis} out of range for tensor of rank {rank} (shape {shape:?})"
            )));
        }
        let a = axis as usize;
        if seen[a] {
            return Err(TensorError::new(format!(
                "{op_name} axes {axes:?} must be distinct (duplicate axis {a})"
            )));
        }
        seen[a] = true;
        out.push(a);
    }
    // Reduce highest-index axis first so the remaining indices stay valid.
    out.sort_unstable_by(|a, b| b.cmp(a));
    Ok(out)
}

/// Reduce ALL listed `axes` by repeated single-axis reduction (highest axis
/// first). Empty `axes` returns a copy (identity). `init`/`combine`/`finalize` are
/// applied per single-axis pass; `out_dtype` stamps every intermediate result so
/// the dtype is consistent. Errors on out-of-range or duplicate axes.
fn tensor_reduce_axes(
    t: &RssTensor,
    axes: &[i64],
    op_name: &str,
    out_dtype: DType,
    init: f32,
    combine: impl Fn(f32, f32) -> f32 + Copy,
    finalize: impl Fn(f32, usize) -> f32 + Copy,
) -> Result<RssTensor, TensorError> {
    let ordered = check_axes(&t.shape, axes, op_name)?;
    if ordered.is_empty() {
        // Identity: a copy with the requested output dtype tag.
        return Ok(RssTensor {
            data: Rc::clone(&t.data),
            shape: t.shape.clone(),
            dtype: out_dtype,
        });
    }
    let mut acc = t.clone();
    for axis in ordered {
        acc = tensor_reduce_axis(&acc, axis, out_dtype, init, combine, finalize);
    }
    Ok(acc)
}

/// Sum over all listed `axes`, removing them. Dtype F32. Empty `axes` is identity.
pub fn tensor_sum_axes(t: &RssTensor, axes: &[i64]) -> Result<RssTensor, TensorError> {
    tensor_reduce_axes(
        t,
        axes,
        "sum_axes",
        DType::F32,
        0.0,
        |acc, x| acc + x,
        |acc, _| acc,
    )
}

/// Product over all listed `axes`, removing them. Dtype F32. Empty `axes` is identity.
pub fn tensor_prod_axes(t: &RssTensor, axes: &[i64]) -> Result<RssTensor, TensorError> {
    tensor_reduce_axes(
        t,
        axes,
        "prod_axes",
        DType::F32,
        1.0,
        |acc, x| acc * x,
        |acc, _| acc,
    )
}

/// Max over all listed `axes`, removing them. Output dtype = input dtype. Empty
/// `axes` is identity.
pub fn tensor_max_axes(t: &RssTensor, axes: &[i64]) -> Result<RssTensor, TensorError> {
    tensor_reduce_axes(
        t,
        axes,
        "max_axes",
        t.dtype,
        f32::NEG_INFINITY,
        |acc, x| acc.max(x),
        |acc, _| acc,
    )
}

/// Min over all listed `axes`, removing them. Output dtype = input dtype. Empty
/// `axes` is identity.
pub fn tensor_min_axes(t: &RssTensor, axes: &[i64]) -> Result<RssTensor, TensorError> {
    tensor_reduce_axes(
        t,
        axes,
        "min_axes",
        t.dtype,
        f32::INFINITY,
        |acc, x| acc.min(x),
        |acc, _| acc,
    )
}

/// Mean over all listed `axes`, removing them (divides by each axis length as it
/// goes, which equals dividing by the product of the removed dims). Dtype F32.
/// Empty `axes` is identity.
pub fn tensor_mean_axes(t: &RssTensor, axes: &[i64]) -> Result<RssTensor, TensorError> {
    tensor_reduce_axes(
        t,
        axes,
        "mean_axes",
        DType::F32,
        0.0,
        |acc, x| acc + x,
        |acc, len| if len == 0 { acc } else { acc / len as f32 },
    )
}

/// Elementwise reciprocal `1/x`. Output dtype F32.
pub fn tensor_reciprocal(t: &RssTensor) -> RssTensor {
    tensor_unary_elementwise(t, DType::F32, |x| 1.0 / x)
}

/// Elementwise base-2 exponential `2^x`. Output dtype F32.
pub fn tensor_exp2(t: &RssTensor) -> RssTensor {
    tensor_unary_elementwise(t, DType::F32, f32::exp2)
}

/// Elementwise base-2 logarithm `log2(x)`. Output dtype F32.
pub fn tensor_log2(t: &RssTensor) -> RssTensor {
    tensor_unary_elementwise(t, DType::F32, f32::log2)
}

/// Elementwise reciprocal square root `1/sqrt(x)`. Output dtype F32.
pub fn tensor_rsqrt(t: &RssTensor) -> RssTensor {
    tensor_unary_elementwise(t, DType::F32, |x| 1.0 / x.sqrt())
}

/// Elementwise sine (radians, `f32::sin`). Output dtype F32. This is tinygrad's
/// `Ops.SIN` primitive — the last unary in the ALU set the PYTHON-backend
/// evaluator (`eval_tensor`) handles, so adding it lets the native forward path
/// cover every elementwise unary op.
pub fn tensor_sin(t: &RssTensor) -> RssTensor {
    tensor_unary_elementwise(t, DType::F32, f32::sin)
}

/// Elementwise truncation toward zero (`f32::trunc`): drops the fractional part,
/// keeping the sign (`2.7 -> 2.0`, `-2.7 -> -2.0`). This is tinygrad's `Ops.TRUNC`
/// (a float-result op, distinct from `cast_i32` which retags to I32) — the dtype is
/// preserved so a truncated F32 stays F32.
pub fn tensor_trunc(t: &RssTensor) -> RssTensor {
    tensor_unary_elementwise(t, t.dtype, f32::trunc)
}

/// Elementwise power `a^b` with broadcasting (`f32::powf`). Output dtype F32.
pub fn tensor_pow(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    tensor_broadcast_binary(a, b, "pow", DType::F32, f32::powf)
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

/// Accumulating inverse of `tensor_gather` (its vector-Jacobian product). Scatters
/// the rows of `updates` along `axis` into a zero-initialised output whose `axis`
/// dim is `dim_size`, ACCUMULATING on duplicate indices:
/// `out[.., indices[k], ..] += updates[.., k, ..]`.
///
/// `indices` is a rank-1 integer-valued tensor with `indices.len() ==
/// updates.shape[axis]`. The output shape equals `updates.shape` with `axis`
/// replaced by `dim_size`, and the output dtype equals `updates`' dtype. Errors if
/// `indices` is not rank-1, its length differs from `updates.shape[axis]`, `axis`
/// is out of range, `dim_size <= 0`, or any index falls outside `[0, dim_size)`.
pub fn tensor_scatter_add(
    updates: &RssTensor,
    axis: i64,
    indices: &RssTensor,
    dim_size: i64,
) -> Result<RssTensor, TensorError> {
    let axis = check_axis(&updates.shape, axis, "scatter_add")?;
    if indices.shape.len() != 1 {
        return Err(TensorError::new(format!(
            "scatter_add requires a rank-1 indices tensor, got shape {:?}",
            indices.shape
        )));
    }
    if dim_size <= 0 {
        return Err(TensorError::new(format!(
            "scatter_add dim_size must be positive, got {dim_size}"
        )));
    }
    let out_axis = dim_size as usize;
    let upd_axis = updates.shape[axis];
    let idx_len = indices.shape[0];
    if idx_len != upd_axis {
        return Err(TensorError::new(format!(
            "scatter_add indices length {idx_len} must equal updates.shape[{axis}] = {upd_axis}"
        )));
    }
    // Resolve and bounds-check each index against the OUTPUT axis length.
    let mut resolved = Vec::with_capacity(idx_len);
    for (j, &raw) in indices.data().iter().enumerate() {
        let i = raw.round() as i64;
        if i < 0 || i >= dim_size {
            return Err(TensorError::new(format!(
                "scatter_add index {i} (at position {j}) out of range for axis {axis} of length {dim_size}"
            )));
        }
        resolved.push(i as usize);
    }
    let mut out_shape = updates.shape.clone();
    out_shape[axis] = out_axis;
    let outer: usize = updates.shape[..axis].iter().product();
    let inner: usize = updates.shape[axis + 1..].iter().product();
    let src = updates.data.as_ref();
    let mut out = vec![0.0f32; outer * out_axis * inner];
    for o in 0..outer {
        for (j, &dst_a) in resolved.iter().enumerate() {
            let src_base = (o * upd_axis + j) * inner;
            let dst_base = (o * out_axis + dst_a) * inner;
            for i in 0..inner {
                out[dst_base + i] += src[src_base + i];
            }
        }
    }
    Ok(RssTensor {
        data: Rc::new(out),
        shape: out_shape,
        dtype: updates.dtype,
    })
}

// ---------------------------------------------------------------------------
// bmm+int/bit (ops D).
//
// Batched matmul broadcasts the leading (batch) dims of two rank>=2 operands and
// does a rank-2 matmul over the trailing (m,k)x(k,n) matrices.
//
// The integer/bit ops below operate on the INTEGER VALUE of I32-dtype tensors.
// Storage is always `Vec<f32>`; an I32 tensor holds integers exactly only while
// `|value| <= 2^24`. Each op converts every f32 operand to `i64` (`x as i64`),
// applies the integer/bit operation, and stores the result back as `f32`
// (`result as f32`). Therefore EXACTNESS HOLDS ONLY FOR `|value| <= 2^24`; bit
// ops on larger magnitudes are not guaranteed to round-trip. This is acceptable
// because the port's heavy RNG runs host-side; these kernels exist for index
// arithmetic where the magnitudes are small. All binary ops broadcast (reusing
// `tensor_broadcast_binary`) and output dtype I32. They are TOTAL over any input:
// divide/modulo by zero is DEFINED to return 0 (rather than trapping), and shift
// counts are masked to `0..63` to avoid Rust's shift-overflow UB.
// ---------------------------------------------------------------------------

/// Batched matrix multiply. Both operands must be rank >= 2; the last two dims are
/// the matrix `(m, k) × (k, n) -> (m, n)` and the leading dims are batch dims that
/// BROADCAST against each other (NumPy semantics, reusing `broadcast_shapes`). For
/// each broadcasted batch index the rank-2 matmul kernel is applied to the
/// corresponding `(m,k)` and `(k,n)` slices. Output dtype is always F32. Errors if
/// either operand is rank < 2, the inner dims disagree, or the batch dims are not
/// broadcast-compatible.
pub fn tensor_bmm(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    if a.shape.len() < 2 || b.shape.len() < 2 {
        return Err(TensorError::new(format!(
            "bmm requires two tensors of rank >= 2, got shapes {:?} and {:?}",
            a.shape, b.shape
        )));
    }
    let ar = a.shape.len();
    let br = b.shape.len();
    let (m, ka) = (a.shape[ar - 2], a.shape[ar - 1]);
    let (kb, n) = (b.shape[br - 2], b.shape[br - 1]);
    if ka != kb {
        return Err(TensorError::new(format!(
            "bmm inner dimensions disagree: {:?} × {:?} (contraction {ka} != {kb})",
            a.shape, b.shape
        )));
    }
    let k = ka;
    // Broadcast the batch-dim prefixes (everything except the trailing two dims).
    let a_batch = &a.shape[..ar - 2];
    let b_batch = &b.shape[..br - 2];
    let batch_shape = broadcast_shapes(a_batch, b_batch, "bmm")?;
    let batch_count = RssTensor::shape_len(&batch_shape);
    // Broadcast strides over the batch prefix only (in units of WHOLE matrices).
    let a_batch_strides = broadcast_strides(a_batch, &batch_shape);
    let b_batch_strides = broadcast_strides(b_batch, &batch_shape);
    let batch_rank = batch_shape.len();
    let a_data = a.data.as_ref();
    let b_data = b.data.as_ref();
    let a_mat = m * k;
    let b_mat = k * n;
    let c_mat = m * n;
    let mut out = vec![0.0f32; batch_count * c_mat];
    let mut index = vec![0usize; batch_rank];
    for batch in 0..batch_count {
        // Map the broadcasted batch multi-index to matrix offsets in each operand.
        let mut a_b = 0usize;
        let mut b_b = 0usize;
        for d in 0..batch_rank {
            a_b += index[d] * a_batch_strides[d];
            b_b += index[d] * b_batch_strides[d];
        }
        let lhs = &a_data[a_b * a_mat..a_b * a_mat + a_mat];
        let rhs = &b_data[b_b * b_mat..b_b * b_mat + b_mat];
        let out_mat = &mut out[batch * c_mat..batch * c_mat + c_mat];
        // Same ikj kernel as tensor_matmul, applied to this batch slice.
        for i in 0..m {
            let lhs_row = &lhs[i * k..i * k + k];
            let out_row = &mut out_mat[i * n..i * n + n];
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
        for d in (0..batch_rank).rev() {
            index[d] += 1;
            if index[d] < batch_shape[d] {
                break;
            }
            index[d] = 0;
        }
    }
    let mut out_shape = batch_shape;
    out_shape.push(m);
    out_shape.push(n);
    Ok(RssTensor {
        data: Rc::new(out),
        shape: out_shape,
        dtype: DType::F32,
    })
}

/// Apply an integer binary op on the i64 VALUES of two tensors, broadcasting and
/// stamping the output dtype I32. `op` receives the two operands as `i64` and
/// returns the i64 result, which is stored back as `f32`. Exact only for
/// `|value| <= 2^24` (see the section header).
fn tensor_int_binary(
    a: &RssTensor,
    b: &RssTensor,
    op_name: &str,
    op: impl Fn(i64, i64) -> i64,
) -> Result<RssTensor, TensorError> {
    tensor_broadcast_binary(a, b, op_name, DType::I32, |x, y| {
        op(x as i64, y as i64) as f32
    })
}

/// Truncated integer division toward zero (`a / b` in i64). Divide-by-zero is
/// DEFINED to return 0 (keeping the op total). Broadcasts; output dtype I32.
pub fn tensor_idiv(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    tensor_int_binary(a, b, "idiv", |x, y| if y == 0 { 0 } else { x / y })
}

/// Integer remainder (`a % b` in i64, sign follows the dividend). Modulo-by-zero
/// is DEFINED to return 0. Broadcasts; output dtype I32.
pub fn tensor_mod(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    tensor_int_binary(a, b, "mod", |x, y| if y == 0 { 0 } else { x % y })
}

/// Floor of `x as i64 / y` (Python `//`): rounds toward NEGATIVE infinity, so the
/// quotient sign follows the divisor (`-7 // 2 == -4`, not `-3`). This is distinct
/// from `idiv` (truncate toward zero == tinygrad `Ops.CDIV`); this is tinygrad
/// `Ops.FLOORDIV`. Division-by-zero is DEFINED to return 0. Broadcasts; dtype I32.
pub fn tensor_floordiv(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    tensor_int_binary(a, b, "floordiv", |x, y| {
        if y == 0 {
            return 0;
        }
        let q = x / y;
        let r = x % y;
        // Truncated division rounds toward zero; nudge down by one when the exact
        // result is negative and there is a remainder, recovering floor semantics.
        if r != 0 && ((r < 0) != (y < 0)) {
            q - 1
        } else {
            q
        }
    })
}

/// Floor modulo `x - floordiv(x, y) * y` (Python `%`): the remainder takes the sign
/// of the DIVISOR (`-7 % 2 == 1`), unlike `mod`/`Ops.CMOD` whose sign follows the
/// dividend. This is tinygrad `Ops.FLOORMOD`. Modulo-by-zero is DEFINED to return 0.
/// Broadcasts; dtype I32.
pub fn tensor_floormod(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    tensor_int_binary(a, b, "floormod", |x, y| {
        if y == 0 {
            return 0;
        }
        let r = x % y;
        if r != 0 && ((r < 0) != (y < 0)) {
            r + y
        } else {
            r
        }
    })
}

/// Left shift `a << b` on i64 values, the shift count masked to `0..63` to avoid
/// shift-overflow UB. Broadcasts; output dtype I32.
pub fn tensor_shl(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    tensor_int_binary(a, b, "shl", |x, y| x << (y & 63))
}

/// Right shift `a >> b` (arithmetic, on signed i64) with the shift count masked to
/// `0..63`. Broadcasts; output dtype I32.
pub fn tensor_shr(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    tensor_int_binary(a, b, "shr", |x, y| x >> (y & 63))
}

/// Bitwise AND on i64 values. Broadcasts; output dtype I32.
pub fn tensor_and(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    tensor_int_binary(a, b, "and", |x, y| x & y)
}

/// Bitwise OR on i64 values. Broadcasts; output dtype I32.
pub fn tensor_or(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    tensor_int_binary(a, b, "or", |x, y| x | y)
}

/// Bitwise XOR on i64 values. Broadcasts; output dtype I32.
pub fn tensor_xor(a: &RssTensor, b: &RssTensor) -> Result<RssTensor, TensorError> {
    tensor_int_binary(a, b, "xor", |x, y| x ^ y)
}

/// Reinterpret each f32's BITS as an i32 (`f32::to_bits as i32`), stored back as
/// f32 with dtype I32. Best-effort/for completeness: the resulting integer is only
/// exactly representable while `|value| <= 2^24`, so large bit patterns may not
/// round-trip. Infallible.
pub fn tensor_bitcast_f32_to_i32(t: &RssTensor) -> RssTensor {
    tensor_unary_elementwise(t, DType::I32, |x| (x.to_bits() as i32) as f32)
}

/// Inverse of `tensor_bitcast_f32_to_i32`: take each value's integer (as i32),
/// reinterpret its bits as an f32 (`f32::from_bits`), dtype F32. Same exactness
/// caveat (`|value| <= 2^24`). Infallible.
pub fn tensor_bitcast_i32_to_f32(t: &RssTensor) -> RssTensor {
    tensor_unary_elementwise(t, DType::F32, |x| f32::from_bits((x as i32) as u32))
}

// ---------------------------------------------------------------------------
// Native seeded RNG: threefry2x32-20 (Salmon et al., Random123).
//
// This is a NATIVE u32 kernel — real wrapping arithmetic and 32-bit rotations.
// It is deliberately NOT composed from the f32-backed Tensor int/bit ops, which
// are only exact for |x| <= 2^24 and cannot represent full uint32
// wraparound/rotation.
//
// This is BIT-EXACT with tinygrad's `Tensor.rand` (and the verified host-RNG
// oracle `tinygrad-rsmc/oracle/rng_parity.py`, maxdiff 0.0 vs tinygrad for
// seeds 0/42/123/1337). See `tensor_rand_bits` below for the exact scheme.
//
//   * 2-word state, 20 rounds (5 key injections, after every 4th round).
//   * Rotation constants per round (period 8): [13, 15, 26, 6, 17, 29, 16, 24].
//   * Key schedule parity word: ks[2] = key0 ^ key1 ^ 0x1BD11BDA.
//   * `threefry2x32(counter, key)`: x0 = counter low 32, x1 = counter high 32;
//     k0 = key low 32, k1 = key high 32. (Matches the oracle's
//     `_threefry2x32(x0, x1, k0, k1)` bit-for-bit; verified by the published
//     known-answer vectors in the tests below.)
//
// tinygrad/oracle composition (`tensor_rand_bits`):
//   1. Device key:  key0 = 0x14B5F2D9 (= sha256(b"\0\0\0\0") low-32, tinygrad's
//      CPU device key, decimal 347607321), key1 = seed (low 32 bits).
//   2. Key derivation from the device counter:
//        (nk0, nk1) = threefry2x32(counter, key = key0 | (key1 << 32))
//      where the counter's low 32 bits are x0 and high 32 bits are x1. With
//      counter = 0 this is threefry2x32(0, ...) i.e. x0 = x1 = 0, reproducing
//      the oracle exactly.
//   3. Per-draw element bits: with `half = (n + 1) / 2`, for j in 0..half:
//        (lo, hi) = threefry2x32(j | ((j + half) << 32), nk0 | (nk1 << 32))
//      then `bits = lows ++ highs` (all lows, then all highs).
//   4. Uniform f32: `(bits[k] >> 9) as f32 / 8388608.0` (8388608 == 2^23).
//      This equals tinygrad's `bitcast((bits>>9)|0x3F800000) - 1` exactly.
//
// The per-draw element counts always restart at `arange` (the `j` loop), while
// the device counter only feeds the key derivation. Callers advance `counter`
// by the number of uniforms consumed per draw so successive draws are distinct
// and reproduce tinygrad's multi-draw stream. `randint` draws `n` uniforms;
// `randn` draws `2 * n` uniforms (one `rand` over a doubled leading axis).

const THREEFRY_ROTATIONS: [u32; 8] = [13, 15, 26, 6, 17, 29, 16, 24];
const THREEFRY_PARITY: u32 = 0x1BD11BDA;

/// tinygrad's CPU device key: low 32 bits of sha256(b"\x00\x00\x00\x00").
const THREEFRY_DEVICE_KEY0: u32 = 347_607_321;

/// One block of threefry2x32-20. Returns the two output words for the given
/// 64-bit counter under the 64-bit key. Pure native u32 arithmetic.
#[inline]
fn threefry2x32(counter: u64, key: u64) -> (u32, u32) {
    let mut x0 = counter as u32;
    let mut x1 = (counter >> 32) as u32;
    let k0 = key as u32;
    let k1 = (key >> 32) as u32;
    let ks = [k0, k1, k0 ^ k1 ^ THREEFRY_PARITY];

    // Initial key injection.
    x0 = x0.wrapping_add(ks[0]);
    x1 = x1.wrapping_add(ks[1]);

    // 20 rounds; inject the rotating key schedule after every 4 rounds.
    for round in 0..20usize {
        let rot = THREEFRY_ROTATIONS[round % 8];
        x0 = x0.wrapping_add(x1);
        x1 = x1.rotate_left(rot) ^ x0;

        if round % 4 == 3 {
            let inject = round / 4 + 1;
            x0 = x0.wrapping_add(ks[inject % 3]);
            x1 = x1.wrapping_add(ks[(inject + 1) % 3]);
            x1 = x1.wrapping_add(inject as u32);
        }
    }
    (x0, x1)
}

/// Uniform f32 in [0, 1) from a 32-bit threefry output word, matching
/// tinygrad/the oracle exactly: `(bits >> 9) / 2^23`.
#[inline]
fn threefry_uniform(bits: u32) -> f32 {
    (bits >> 9) as f32 / 8_388_608.0
}

/// Produce `n` uniform threefry bit-words for one `rand` draw, bit-exact with
/// tinygrad's `Tensor.rand` (and the verified oracle `port_rand`). The device
/// counter feeds the key derivation; the per-draw element indices restart at 0.
fn tensor_rand_bits(n: usize, seed: i64, counter: u64) -> Vec<u32> {
    // Device key0 | (seed << 32), then derive the per-draw key from the counter.
    let key1 = seed as u32;
    let device_key = (THREEFRY_DEVICE_KEY0 as u64) | ((key1 as u64) << 32);
    let (nk0, nk1) = threefry2x32(counter, device_key);
    let derived_key = (nk0 as u64) | ((nk1 as u64) << 32);

    let half = n.div_ceil(2);
    let mut lows = Vec::with_capacity(half);
    let mut highs = Vec::with_capacity(half);
    for j in 0..half {
        let block_counter = (j as u64) | (((j + half) as u64) << 32);
        let (lo, hi) = threefry2x32(block_counter, derived_key);
        lows.push(lo);
        highs.push(hi);
    }
    // bits = lows ++ highs, truncated to n.
    lows.extend(highs);
    lows.truncate(n);
    lows
}

/// `n` uniform f32 values in [0, 1) for one `rand` draw (bit-exact with tinygrad).
fn tensor_rand_uniforms(n: usize, seed: i64, counter: u64) -> Vec<f32> {
    tensor_rand_bits(n, seed, counter)
        .into_iter()
        .map(threefry_uniform)
        .collect()
}

/// Validate a shape (`i64` dims, non-negative) and return its dims + element
/// count. Shared by the RNG ops.
fn rng_shape_dims(shape: &[i64]) -> Result<(Vec<usize>, usize), TensorError> {
    let mut dims = Vec::with_capacity(shape.len());
    for &dim in shape {
        if dim < 0 {
            return Err(TensorError::new(format!(
                "tensor shape dimensions must be non-negative, got {dim}"
            )));
        }
        dims.push(dim as usize);
    }
    let count = RssTensor::shape_len(&dims);
    Ok((dims, count))
}

/// Uniform f32 in [0, 1), dtype F32. Bit-exact with tinygrad's `Tensor.rand`:
/// the device counter feeds the key derivation, then `count` element bits are
/// drawn and converted via `(bits >> 9) / 2^23`. `counter = 0` reproduces the
/// verified oracle `port_rand(seed, count)` exactly.
pub fn tensor_rand(shape: &[i64], seed: i64, counter: i64) -> Result<RssTensor, TensorError> {
    let (dims, count) = rng_shape_dims(shape)?;
    let data = tensor_rand_uniforms(count, seed, counter as u64);
    Ok(RssTensor {
        data: Rc::new(data),
        shape: dims,
        dtype: DType::F32,
    })
}

/// Uniform integers in [low, high), dtype I32. Errors if `high <= low`. Values
/// are exact as long as they fit in 2^24 (the I32 exactness contract).
pub fn tensor_randint(
    shape: &[i64],
    low: i64,
    high: i64,
    seed: i64,
    counter: i64,
) -> Result<RssTensor, TensorError> {
    if high <= low {
        return Err(TensorError::new(format!(
            "randint requires low < high, got low={low} high={high}"
        )));
    }
    let (dims, count) = rng_shape_dims(shape)?;
    // tinygrad: randint(low, high) == uniform(low, high, int)
    //         == ((high - low) * rand).cast(int) + low.
    // The cast to int truncates toward zero; rand is in [0, 1) so the result is
    // floor(rand * span) and stays within [low, high). Bit-exact with the
    // corrected `rand` bits.
    let span = (high - low) as f32;
    let data = tensor_rand_uniforms(count, seed, counter as u64)
        .into_iter()
        .map(|u| ((span * u).trunc() as i64 + low) as f32)
        .collect();
    Ok(RssTensor {
        data: Rc::new(data),
        shape: dims,
        dtype: DType::I32,
    })
}

/// Standard normal f32 via the Box–Muller transform, dtype F32, bit-exact with
/// tinygrad's `Tensor.randn`.
///
/// tinygrad's `randn_like` does `src = stack(self).rand_like()`, i.e. a SINGLE
/// `rand` draw over the shape `(2, *shape)` (2 * count uniforms). It then forms
///   `z = cos(2*pi * src[0]) * sqrt(-2 * log(1 - src[1]))`,
/// where `src[0]` is the first `count` uniforms and `src[1]` the next `count`.
/// We replicate exactly that single doubled draw and formula. Because it is a
/// single `rand` over `2*count` elements, the device counter feeds one key
/// derivation and the element-bit loop covers all `2*count` values (so callers
/// advance the counter by `2 * count`).
pub fn tensor_randn(shape: &[i64], seed: i64, counter: i64) -> Result<RssTensor, TensorError> {
    let (dims, count) = rng_shape_dims(shape)?;
    let uniforms = tensor_rand_uniforms(2 * count, seed, counter as u64);
    let mut data = Vec::with_capacity(count);
    for i in 0..count {
        let u0 = uniforms[i]; // src[0][i]
        let u1 = uniforms[count + i]; // src[1][i]
        let z = (std::f32::consts::TAU * u0).cos() * (-2.0 * (1.0 - u1).ln()).sqrt();
        data.push(z);
    }
    Ok(RssTensor {
        data: Rc::new(data),
        shape: dims,
        dtype: DType::F32,
    })
}


// ---------------------------------------------------------------------------
// nn primitives (slice F).
//
// Forward-only NN building blocks: `iota`/`one_hot` build index/label tensors,
// and `softmax`/`log_softmax`/`cross_entropy` are written as DIRECT native loops
// over the reduced axis (not chains of exp/sum/div intrinsics) for numerical
// stability and clarity. softmax/log_softmax subtract the per-slice max before
// exponentiating; log_softmax uses the log-sum-exp form `x - max - log(sum(exp(x
// - max)))` (no separate exp/divide). cross_entropy is the mean over the batch of
// the negative log-softmax at each target class index. The math matches what
// chaining the stable ops would produce; both backends call these exact
// functions so the reg-VM and AOT results agree bit-for-bit. All output F32.
// ---------------------------------------------------------------------------

/// 1-D ramp `[0, 1, ..., n-1]`, dtype I32. Errors if `n < 0`.
pub fn tensor_iota(n: i64) -> Result<RssTensor, TensorError> {
    if n < 0 {
        return Err(TensorError::new(format!(
            "iota length must be non-negative, got {n}"
        )));
    }
    let len = n as usize;
    let mut out = vec![0.0f32; len];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = i as f32;
    }
    Ok(RssTensor {
        data: Rc::new(out),
        shape: vec![len],
        dtype: DType::I32,
    })
}

/// One-hot encode an integer-valued tensor: for input shape `S`, the output has
/// shape `S ++ [num_classes]`, dtype F32, with `1.0` at the class index and `0.0`
/// elsewhere. Index values are read from the f32 storage and ROUNDED to the
/// nearest integer. Errors if `num_classes <= 0` or any index falls outside
/// `[0, num_classes)`.
pub fn tensor_one_hot(indices: &RssTensor, num_classes: i64) -> Result<RssTensor, TensorError> {
    if num_classes <= 0 {
        return Err(TensorError::new(format!(
            "one_hot num_classes must be positive, got {num_classes}"
        )));
    }
    let classes = num_classes as usize;
    let n = indices.data.len();
    let mut out = vec![0.0f32; n * classes];
    for (pos, &raw) in indices.data.iter().enumerate() {
        let c = raw.round() as i64;
        if c < 0 || c >= num_classes {
            return Err(TensorError::new(format!(
                "one_hot index {c} (at position {pos}) out of range for num_classes {num_classes}"
            )));
        }
        out[pos * classes + c as usize] = 1.0;
    }
    let mut out_shape = indices.shape.clone();
    out_shape.push(classes);
    Ok(RssTensor {
        data: Rc::new(out),
        shape: out_shape,
        dtype: DType::F32,
    })
}

/// Numerically-stable softmax along `axis`: subtract the per-slice max, exponentiate,
/// and divide by the per-slice sum. Same shape as the input, dtype F32. Errors on an
/// out-of-range axis. Written as a direct loop over the contiguous axis layout.
pub fn tensor_softmax(t: &RssTensor, axis: i64) -> Result<RssTensor, TensorError> {
    let axis = check_axis(&t.shape, axis, "softmax")?;
    let axis_len = t.shape[axis];
    let outer: usize = t.shape[..axis].iter().product();
    let inner: usize = t.shape[axis + 1..].iter().product();
    let data = t.data.as_ref();
    let mut out = vec![0.0f32; data.len()];
    for o in 0..outer {
        for i in 0..inner {
            // One softmax slice walks `axis_len` elements at stride `inner`.
            let base = o * axis_len * inner + i;
            let mut max = f32::NEG_INFINITY;
            for a in 0..axis_len {
                max = max.max(data[base + a * inner]);
            }
            let mut sum = 0.0f32;
            for a in 0..axis_len {
                let e = (data[base + a * inner] - max).exp();
                out[base + a * inner] = e;
                sum += e;
            }
            for a in 0..axis_len {
                out[base + a * inner] /= sum;
            }
        }
    }
    Ok(RssTensor {
        data: Rc::new(out),
        shape: t.shape.clone(),
        dtype: DType::F32,
    })
}

/// Numerically-stable log-softmax along `axis` in log-sum-exp form:
/// `x - max - log(sum(exp(x - max)))` (no separate exp/divide). Same shape as the
/// input, dtype F32. Errors on an out-of-range axis.
pub fn tensor_log_softmax(t: &RssTensor, axis: i64) -> Result<RssTensor, TensorError> {
    let axis = check_axis(&t.shape, axis, "log_softmax")?;
    let axis_len = t.shape[axis];
    let outer: usize = t.shape[..axis].iter().product();
    let inner: usize = t.shape[axis + 1..].iter().product();
    let data = t.data.as_ref();
    let mut out = vec![0.0f32; data.len()];
    for o in 0..outer {
        for i in 0..inner {
            let base = o * axis_len * inner + i;
            let mut max = f32::NEG_INFINITY;
            for a in 0..axis_len {
                max = max.max(data[base + a * inner]);
            }
            let mut sum = 0.0f32;
            for a in 0..axis_len {
                sum += (data[base + a * inner] - max).exp();
            }
            let log_sum = sum.ln();
            for a in 0..axis_len {
                out[base + a * inner] = data[base + a * inner] - max - log_sum;
            }
        }
    }
    Ok(RssTensor {
        data: Rc::new(out),
        shape: t.shape.clone(),
        dtype: DType::F32,
    })
}

/// Cross-entropy loss for a rank-2 `logits` tensor `[N, C]` (where `axis` is the
/// class axis, normally `1`) against a rank-1 integer `targets` tensor `[N]` of
/// CLASS INDICES (not one-hot). Computes the MEAN over the `N` rows of
/// `-log_softmax(logits)[i, targets[i]]`, returning a SCALAR tensor (shape `[]`),
/// dtype F32. The log-softmax is computed inline in the stable log-sum-exp form.
/// Errors if `logits` is not rank-2, `axis` is not the class axis (must be `1`, the
/// trailing axis of a `[N, C]` tensor), `targets` is not rank-1 of length `N`, or
/// any target index is outside `[0, C)`. An empty batch (`N == 0`) yields `0.0`.
pub fn tensor_cross_entropy(
    logits: &RssTensor,
    targets: &RssTensor,
    axis: i64,
) -> Result<RssTensor, TensorError> {
    if logits.shape.len() != 2 {
        return Err(TensorError::new(format!(
            "cross_entropy requires rank-2 logits [N, C], got shape {:?}",
            logits.shape
        )));
    }
    // The class axis must be the trailing axis (axis 1) of the [N, C] tensor.
    if axis != 1 {
        return Err(TensorError::new(format!(
            "cross_entropy class axis must be 1 for rank-2 logits [N, C], got {axis}"
        )));
    }
    if targets.shape.len() != 1 {
        return Err(TensorError::new(format!(
            "cross_entropy requires a rank-1 targets tensor [N], got shape {:?}",
            targets.shape
        )));
    }
    let n = logits.shape[0];
    let classes = logits.shape[1];
    if targets.shape[0] != n {
        return Err(TensorError::new(format!(
            "cross_entropy targets length {} does not match logits batch {n}",
            targets.shape[0]
        )));
    }
    let data = logits.data.as_ref();
    let tgt = targets.data.as_ref();
    let mut total = 0.0f32;
    for (row, &target_raw) in tgt.iter().enumerate().take(n) {
        let raw = target_raw.round() as i64;
        if raw < 0 || raw as usize >= classes {
            return Err(TensorError::new(format!(
                "cross_entropy target {raw} (at row {row}) out of range for {classes} classes"
            )));
        }
        let base = row * classes;
        // Stable log-sum-exp over this row, then -log_softmax at the target class.
        let mut max = f32::NEG_INFINITY;
        for c in 0..classes {
            max = max.max(data[base + c]);
        }
        let mut sum = 0.0f32;
        for c in 0..classes {
            sum += (data[base + c] - max).exp();
        }
        let target_logit = data[base + raw as usize];
        // -log_softmax[target] = -(x_t - max - log(sum)) = (max + log(sum)) - x_t.
        total += max + sum.ln() - target_logit;
    }
    let mean = if n == 0 { 0.0 } else { total / n as f32 };
    Ok(RssTensor {
        data: Rc::new(vec![mean]),
        shape: vec![],
        dtype: DType::F32,
    })
}

// ---------------------------------------------------------------------------
// conv (slice G).
//
// Forward-only NCHW 2-D convolution + pooling, all output dtype F32. These are
// direct native loops (clear and correct over clever); a fused/blocked/img2col
// path is future work. `conv2d` is the ML "convolution" — i.e. a
// cross-correlation with NO kernel flip (each output is the dot product of the
// weight window with the input window at the same orientation), matching
// PyTorch/tinygrad `Conv2d`. stride/padding/kernel are scalar `Int` (square);
// rectangular strides/kernels, dilation, groups and bias are all future work
// (callers add a broadcast bias separately).
// ---------------------------------------------------------------------------

/// Compute a conv/pool output dim by the floor formula, erroring if non-positive.
/// `in_dim` is the (already-padded, for conv) input length, `window` the kernel
/// extent and `stride` the step. Output = `(in_dim - window) / stride + 1`.
fn conv_out_dim(
    in_dim: i64,
    window: i64,
    stride: i64,
    label: &str,
) -> Result<usize, TensorError> {
    if stride <= 0 {
        return Err(TensorError::new(format!(
            "{label}: stride must be positive, got {stride}"
        )));
    }
    if window <= 0 {
        return Err(TensorError::new(format!(
            "{label}: kernel/window size must be positive, got {window}"
        )));
    }
    let out = (in_dim - window) / stride + 1;
    if out <= 0 {
        return Err(TensorError::new(format!(
            "{label}: non-positive output dimension {out} (in={in_dim}, window={window}, stride={stride})"
        )));
    }
    Ok(out as usize)
}

/// 2-D cross-correlation ("convolution" in the ML sense, NO kernel flip).
///
/// `input` is NCHW `[N, Cin, H, W]`, `weight` is OIHW `[Cout, Cin, KH, KW]`. The
/// input is zero-padded by `padding` on both sides of H and W, then each output
/// `[n, o, oh, ow]` is the sum over `(c, kh, kw)` of
/// `input[n, c, oh*stride+kh-pad, ow*stride+kw-pad] * weight[o, c, kh, kw]`
/// (padded positions read as 0). Output `[N, Cout, Hout, Wout]`, dtype F32, no
/// bias. Errors on rank mismatch, `Cin` mismatch, or non-positive output dims.
/// No dilation/groups (future work).
pub fn tensor_conv2d(
    input: &RssTensor,
    weight: &RssTensor,
    stride: i64,
    padding: i64,
) -> Result<RssTensor, TensorError> {
    if input.shape.len() != 4 {
        return Err(TensorError::new(format!(
            "conv2d: input must be rank-4 NCHW [N, Cin, H, W], got shape {:?}",
            input.shape
        )));
    }
    if weight.shape.len() != 4 {
        return Err(TensorError::new(format!(
            "conv2d: weight must be rank-4 OIHW [Cout, Cin, KH, KW], got shape {:?}",
            weight.shape
        )));
    }
    if padding < 0 {
        return Err(TensorError::new(format!(
            "conv2d: padding must be non-negative, got {padding}"
        )));
    }
    let (n, cin, h, w) = (
        input.shape[0],
        input.shape[1],
        input.shape[2],
        input.shape[3],
    );
    let (cout, wcin, kh, kw) = (
        weight.shape[0],
        weight.shape[1],
        weight.shape[2],
        weight.shape[3],
    );
    if wcin != cin {
        return Err(TensorError::new(format!(
            "conv2d: input channels {cin} != weight in-channels {wcin} (input {:?}, weight {:?})",
            input.shape, weight.shape
        )));
    }
    let pad = padding as usize;
    let padded_h = h as i64 + 2 * padding;
    let padded_w = w as i64 + 2 * padding;
    let hout = conv_out_dim(padded_h, kh as i64, stride, "conv2d (H)")?;
    let wout = conv_out_dim(padded_w, kw as i64, stride, "conv2d (W)")?;
    let stride = stride as usize;

    let in_data = input.data.as_ref();
    let wt_data = weight.data.as_ref();
    // Row-major strides for input [N, Cin, H, W] and weight [Cout, Cin, KH, KW].
    let in_c_stride = h * w;
    let in_n_stride = cin * in_c_stride;
    let wt_kh_stride = kw;
    let wt_c_stride = kh * kw;
    let wt_o_stride = cin * wt_c_stride;

    let mut out = vec![0.0f32; n * cout * hout * wout];
    let mut out_idx = 0usize;
    for ni in 0..n {
        let in_n_base = ni * in_n_stride;
        for oc in 0..cout {
            let wt_o_base = oc * wt_o_stride;
            for oh in 0..hout {
                // Top input row this output row reads from (may be negative → pad).
                let ih0 = oh * stride;
                for ow in 0..wout {
                    let iw0 = ow * stride;
                    let mut acc = 0.0f32;
                    for c in 0..cin {
                        let in_c_base = in_n_base + c * in_c_stride;
                        let wt_c_base = wt_o_base + c * wt_c_stride;
                        for r in 0..kh {
                            // Input row = oh*stride + r - pad; skip if out of bounds.
                            let ih = ih0 + r;
                            if ih < pad || ih >= pad + h {
                                continue;
                            }
                            let in_row = in_c_base + (ih - pad) * w;
                            let wt_row = wt_c_base + r * wt_kh_stride;
                            for s in 0..kw {
                                let iw = iw0 + s;
                                if iw < pad || iw >= pad + w {
                                    continue;
                                }
                                acc += in_data[in_row + (iw - pad)] * wt_data[wt_row + s];
                            }
                        }
                    }
                    out[out_idx] = acc;
                    out_idx += 1;
                }
            }
        }
    }
    Ok(RssTensor {
        data: Rc::new(out),
        shape: vec![n, cout, hout, wout],
        dtype: DType::F32,
    })
}

/// Shared NCHW pooling walk over a square `kernel`×`kernel` window with `stride`
/// and NO padding. `init` seeds each window and `combine` folds each element;
/// `finalize(acc, count)` post-processes (identity for max, divide-by-count for
/// avg). Output `[N, C, (H-kernel)/stride+1, (W-kernel)/stride+1]`, dtype F32.
fn tensor_pool2d(
    input: &RssTensor,
    kernel: i64,
    stride: i64,
    label: &str,
    init: f32,
    combine: impl Fn(f32, f32) -> f32,
    finalize: impl Fn(f32, usize) -> f32,
) -> Result<RssTensor, TensorError> {
    if input.shape.len() != 4 {
        return Err(TensorError::new(format!(
            "{label}: input must be rank-4 NCHW [N, C, H, W], got shape {:?}",
            input.shape
        )));
    }
    let (n, c, h, w) = (
        input.shape[0],
        input.shape[1],
        input.shape[2],
        input.shape[3],
    );
    let hout = conv_out_dim(h as i64, kernel, stride, label)?;
    let wout = conv_out_dim(w as i64, kernel, stride, label)?;
    let k = kernel as usize;
    let stride = stride as usize;
    let count = k * k;

    let in_data = input.data.as_ref();
    let in_c_stride = h * w;
    let in_n_stride = c * in_c_stride;

    let mut out = vec![0.0f32; n * c * hout * wout];
    let mut out_idx = 0usize;
    for ni in 0..n {
        let in_n_base = ni * in_n_stride;
        for ci in 0..c {
            let in_c_base = in_n_base + ci * in_c_stride;
            for oh in 0..hout {
                let ih0 = oh * stride;
                for ow in 0..wout {
                    let iw0 = ow * stride;
                    let mut acc = init;
                    for r in 0..k {
                        let in_row = in_c_base + (ih0 + r) * w + iw0;
                        for s in 0..k {
                            acc = combine(acc, in_data[in_row + s]);
                        }
                    }
                    out[out_idx] = finalize(acc, count);
                    out_idx += 1;
                }
            }
        }
    }
    Ok(RssTensor {
        data: Rc::new(out),
        shape: vec![n, c, hout, wout],
        dtype: DType::F32,
    })
}

/// Max pooling over a square `kernel`×`kernel` window with `stride`, NO padding.
/// NCHW in, output `[N, C, (H-kernel)/stride+1, (W-kernel)/stride+1]`, dtype F32.
/// Errors on non-rank-4 input or non-positive output dims. Rectangular
/// kernels/strides are future work.
pub fn tensor_max_pool2d(
    input: &RssTensor,
    kernel: i64,
    stride: i64,
) -> Result<RssTensor, TensorError> {
    tensor_pool2d(
        input,
        kernel,
        stride,
        "max_pool2d",
        f32::NEG_INFINITY,
        |acc, x| acc.max(x),
        |acc, _| acc,
    )
}

/// Average pooling over a square `kernel`×`kernel` window with `stride`, NO
/// padding (divides by `kernel*kernel`). NCHW in, output
/// `[N, C, (H-kernel)/stride+1, (W-kernel)/stride+1]`, dtype F32. Errors on
/// non-rank-4 input or non-positive output dims. Rectangular kernels/strides are
/// future work.
pub fn tensor_avg_pool2d(
    input: &RssTensor,
    kernel: i64,
    stride: i64,
) -> Result<RssTensor, TensorError> {
    tensor_pool2d(
        input,
        kernel,
        stride,
        "avg_pool2d",
        0.0,
        |acc, x| acc + x,
        |acc, count| acc / count as f32,
    )
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
    fn sin_matches_libm() {
        let t = tensor_from_f32_slice(&[0.0, 1.5707964, 3.1415927], &[3]).unwrap();
        let out = tensor_to_f32_slice(&tensor_sin(&t)).unwrap();
        assert!((out[0] - 0.0).abs() < 1e-6);
        assert!((out[1] - 1.0).abs() < 1e-6);
        assert!(out[2].abs() < 1e-6);
        assert_eq!(tensor_sin(&t).dtype(), DType::F32);
    }

    #[test]
    fn trunc_drops_fraction_toward_zero() {
        let t = tensor_from_f32_slice(&[2.7, -2.7, 2.2, -2.2, 5.0], &[5]).unwrap();
        assert_eq!(
            tensor_to_f32_slice(&tensor_trunc(&t)).unwrap(),
            vec![2.0, -2.0, 2.0, -2.0, 5.0]
        );
        // TRUNC preserves dtype (unlike cast_i32 which retags to I32).
        assert_eq!(tensor_trunc(&t).dtype(), DType::F32);
    }

    #[test]
    fn floordiv_rounds_toward_negative_infinity() {
        let a = tensor_from_f32_slice(&[7.0, -7.0, 7.0, -7.0, 6.0], &[5]).unwrap();
        let b = tensor_from_f32_slice(&[2.0, 2.0, -2.0, -2.0, 3.0], &[5]).unwrap();
        // floor: 7//2=3, -7//2=-4, 7//-2=-4, -7//-2=3, 6//3=2.
        assert_eq!(
            tensor_to_f32_slice(&tensor_floordiv(&a, &b).unwrap()).unwrap(),
            vec![3.0, -4.0, -4.0, 3.0, 2.0]
        );
        // Contrast with idiv (truncate toward zero): -7/2 == -3.
        assert_eq!(
            tensor_to_f32_slice(&tensor_idiv(&a, &b).unwrap()).unwrap(),
            vec![3.0, -3.0, -3.0, 3.0, 2.0]
        );
        assert_eq!(tensor_floordiv(&a, &b).unwrap().dtype(), DType::I32);
    }

    #[test]
    fn floormod_sign_follows_divisor() {
        let a = tensor_from_f32_slice(&[7.0, -7.0, 7.0, -7.0], &[4]).unwrap();
        let b = tensor_from_f32_slice(&[2.0, 2.0, -2.0, -2.0], &[4]).unwrap();
        // floormod: 7%2=1, -7%2=1, 7%-2=-1, -7%-2=-1 (sign of divisor).
        assert_eq!(
            tensor_to_f32_slice(&tensor_floormod(&a, &b).unwrap()).unwrap(),
            vec![1.0, 1.0, -1.0, -1.0]
        );
        // Contrast with mod (sign of dividend): -7%2 == -1.
        assert_eq!(
            tensor_to_f32_slice(&tensor_mod(&a, &b).unwrap()).unwrap(),
            vec![1.0, -1.0, 1.0, -1.0]
        );
    }

    #[test]
    fn floordiv_floormod_divide_by_zero_is_zero() {
        let a = tensor_from_f32_slice(&[5.0, -5.0], &[2]).unwrap();
        let z = tensor_from_f32_slice(&[0.0, 0.0], &[2]).unwrap();
        assert_eq!(
            tensor_to_f32_slice(&tensor_floordiv(&a, &z).unwrap()).unwrap(),
            vec![0.0, 0.0]
        );
        assert_eq!(
            tensor_to_f32_slice(&tensor_floormod(&a, &z).unwrap()).unwrap(),
            vec![0.0, 0.0]
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

    #[test]
    fn scatter_add_roundtrips_gather_counts() {
        // For gather(data, axis, idx), scatter_add(ones_like(gathered), axis, idx,
        // data.shape[axis]) yields the per-element gather count along axis.
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]).unwrap();
        let idx = tensor_cast_i32(&tensor_from_f32_slice(&[1.0, 0.0, 1.0], &[3]).unwrap());
        let gathered = tensor_gather(&t, 1, &idx).unwrap(); // shape [2,3]
        let ones = tensor_unary_elementwise(&gathered, gathered.dtype, |_| 1.0);
        let counts = tensor_scatter_add(&ones, 1, &idx, 3).unwrap();
        // axis-1 length 3 restored; column 1 used twice, column 0 once, column 2 zero.
        assert_eq!(tensor_shape(&counts).unwrap(), vec![2, 3]);
        assert_eq!(
            tensor_to_f32_slice(&counts).unwrap(),
            vec![1.0, 2.0, 0.0, 1.0, 2.0, 0.0]
        );
        // Output dtype = updates dtype.
        assert_eq!(tensor_dtype_code(&counts), tensor_dtype_code(&ones));

        // Scatter-add of the gathered VALUES accumulates per destination index.
        // gathered = [[2,1,2],[5,4,5]] (cols [1,0,1] of t). idx=[1,0,1]:
        //   out col0 <- gathered pos j=1 (idx 0): values (1,4)
        //   out col1 <- gathered pos j=0,2 (idx 1): (2+2, 5+5) = (4,10)
        let scattered = tensor_scatter_add(&gathered, 1, &idx, 3).unwrap();
        assert_eq!(
            tensor_to_f32_slice(&scattered).unwrap(),
            vec![1.0, 4.0, 0.0, 4.0, 10.0, 0.0]
        );
    }

    #[test]
    fn scatter_add_axis0_and_duplicate_accumulation() {
        // axis 0, all updates target the same row -> they accumulate.
        let upd = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        let idx = tensor_cast_i32(&tensor_from_f32_slice(&[0.0, 0.0], &[2]).unwrap());
        let out = tensor_scatter_add(&upd, 0, &idx, 3).unwrap();
        assert_eq!(tensor_shape(&out).unwrap(), vec![3, 2]);
        // row0 = [1+3, 2+4] = [4,6]; rows 1,2 zero.
        assert_eq!(
            tensor_to_f32_slice(&out).unwrap(),
            vec![4.0, 6.0, 0.0, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn scatter_add_rejects_bad_args() {
        let upd = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        let idx = tensor_cast_i32(&tensor_from_f32_slice(&[0.0, 1.0], &[2]).unwrap());
        // axis out of range.
        assert!(tensor_error_message(&tensor_scatter_add(&upd, 5, &idx, 3).unwrap_err())
            .contains("out of range"));
        // dim_size non-positive.
        assert!(tensor_error_message(&tensor_scatter_add(&upd, 0, &idx, 0).unwrap_err())
            .contains("positive"));
        // index out of [0, dim_size).
        let big = tensor_cast_i32(&tensor_from_f32_slice(&[0.0, 5.0], &[2]).unwrap());
        assert!(tensor_error_message(&tensor_scatter_add(&upd, 0, &big, 3).unwrap_err())
            .contains("out of range"));
        // wrong indices length (updates.shape[axis] == 2, give 3).
        let wrong = tensor_cast_i32(&tensor_from_f32_slice(&[0.0, 1.0, 0.0], &[3]).unwrap());
        assert!(tensor_error_message(&tensor_scatter_add(&upd, 0, &wrong, 3).unwrap_err())
            .contains("must equal"));
        // rank-2 indices rejected.
        let idx2 = tensor_cast_i32(&tensor_from_f32_slice(&[0.0, 1.0], &[1, 2]).unwrap());
        assert!(tensor_error_message(&tensor_scatter_add(&upd, 0, &idx2, 3).unwrap_err())
            .contains("rank-1"));
    }

    // reductions+math (ops C)

    #[test]
    fn prod_min_axis_2x3() {
        // [[1,2,3],[4,5,6]]
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]).unwrap();
        // prod axis 0 -> [4,10,18], dtype F32
        let p0 = tensor_prod_axis(&t, 0).unwrap();
        assert_eq!(tensor_shape(&p0).unwrap(), vec![3]);
        assert_eq!(tensor_to_f32_slice(&p0).unwrap(), vec![4.0, 10.0, 18.0]);
        assert_eq!(tensor_dtype_code(&p0), 0);
        // prod axis 1 -> [6,120]
        assert_eq!(
            tensor_to_f32_slice(&tensor_prod_axis(&t, 1).unwrap()).unwrap(),
            vec![6.0, 120.0]
        );
        // min axis 0 -> [1,2,3]; min axis 1 -> [1,4]
        assert_eq!(
            tensor_to_f32_slice(&tensor_min_axis(&t, 0).unwrap()).unwrap(),
            vec![1.0, 2.0, 3.0]
        );
        assert_eq!(
            tensor_to_f32_slice(&tensor_min_axis(&t, 1).unwrap()).unwrap(),
            vec![1.0, 4.0]
        );
        // min_axis preserves input dtype.
        assert_eq!(
            tensor_dtype_code(&tensor_min_axis(&tensor_cast_i32(&t), 0).unwrap()),
            1
        );
        // out-of-range axis errors.
        assert!(
            tensor_error_message(&tensor_prod_axis(&t, 2).unwrap_err()).contains("out of range")
        );
        assert!(
            tensor_error_message(&tensor_min_axis(&t, -1).unwrap_err()).contains("out of range")
        );
    }

    #[test]
    fn multi_axis_reductions() {
        // shape [2,3], reduce both axes -> scalar shape [].
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], &[2, 3]).unwrap();
        let s = tensor_sum_axes(&t, &[0, 1]).unwrap();
        assert_eq!(tensor_shape(&s).unwrap(), Vec::<i64>::new());
        assert_eq!(tensor_to_f32_slice(&s).unwrap(), vec![21.0]);
        // order independence: [1,0] same as [0,1].
        assert_eq!(
            tensor_to_f32_slice(&tensor_sum_axes(&t, &[1, 0]).unwrap()).unwrap(),
            vec![21.0]
        );
        // prod over both axes -> 720.
        assert_eq!(
            tensor_to_f32_slice(&tensor_prod_axes(&t, &[0, 1]).unwrap()).unwrap(),
            vec![720.0]
        );
        // max/min over both -> 6 / 1.
        assert_eq!(
            tensor_to_f32_slice(&tensor_max_axes(&t, &[0, 1]).unwrap()).unwrap(),
            vec![6.0]
        );
        assert_eq!(
            tensor_to_f32_slice(&tensor_min_axes(&t, &[0, 1]).unwrap()).unwrap(),
            vec![1.0]
        );
        // mean over both -> 3.5.
        assert_eq!(
            tensor_to_f32_slice(&tensor_mean_axes(&t, &[0, 1]).unwrap()).unwrap(),
            vec![3.5]
        );
        // rank-3 [2,3,4]: sum over axes [0,2] -> shape [3].
        let data: Vec<f64> = (0..24).map(|x| x as f64).collect();
        let r = tensor_from_f32_slice(&data, &[2, 3, 4]).unwrap();
        let red = tensor_sum_axes(&r, &[0, 2]).unwrap();
        assert_eq!(tensor_shape(&red).unwrap(), vec![3]);
        let mut expected = vec![0.0f64; 3];
        for (a, cell) in expected.iter_mut().enumerate() {
            for o in 0..2 {
                for i in 0..4 {
                    *cell += (o * 12 + a * 4 + i) as f64;
                }
            }
        }
        assert_eq!(tensor_to_f32_slice(&red).unwrap(), expected);
        // single-axis via the _axes form matches the dedicated single-axis op.
        assert_eq!(
            tensor_to_f32_slice(&tensor_sum_axes(&t, &[1]).unwrap()).unwrap(),
            tensor_to_f32_slice(&tensor_sum_axis(&t, 1).unwrap()).unwrap()
        );
        // dtype: sum/prod/mean -> F32; max/min preserve input dtype.
        assert_eq!(tensor_dtype_code(&tensor_prod_axes(&t, &[0]).unwrap()), 0);
        assert_eq!(
            tensor_dtype_code(&tensor_min_axes(&tensor_cast_i32(&t), &[0]).unwrap()),
            1
        );
    }

    #[test]
    fn multi_axis_empty_is_identity() {
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        let id = tensor_sum_axes(&t, &[]).unwrap();
        assert_eq!(tensor_shape(&id).unwrap(), vec![2, 2]);
        assert_eq!(
            tensor_to_f32_slice(&id).unwrap(),
            vec![1.0, 2.0, 3.0, 4.0]
        );
        // max_axes identity preserves input dtype.
        let ii = tensor_cast_i32(&t);
        assert_eq!(tensor_dtype_code(&tensor_max_axes(&ii, &[]).unwrap()), 1);
    }

    #[test]
    fn multi_axis_rejects_bad_axes() {
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        assert!(
            tensor_error_message(&tensor_sum_axes(&t, &[0, 0]).unwrap_err()).contains("distinct")
        );
        assert!(
            tensor_error_message(&tensor_sum_axes(&t, &[0, 5]).unwrap_err())
                .contains("out of range")
        );
    }

    #[test]
    fn math_unary_ops() {
        let t = tensor_from_f32_slice(&[1.0, 2.0, 4.0], &[3]).unwrap();
        assert_eq!(
            tensor_to_f32_slice(&tensor_reciprocal(&t)).unwrap(),
            vec![1.0, 0.5, 0.25]
        );
        assert_eq!(tensor_dtype_code(&tensor_reciprocal(&t)), 0);
        // exp2 / log2 round-trip and exact powers.
        assert_eq!(
            tensor_to_f32_slice(&tensor_exp2(&t)).unwrap(),
            vec![2.0, 4.0, 16.0]
        );
        let l = tensor_from_f32_slice(&[1.0, 2.0, 8.0], &[3]).unwrap();
        assert_eq!(
            tensor_to_f32_slice(&tensor_log2(&l)).unwrap(),
            vec![0.0, 1.0, 3.0]
        );
        // rsqrt: 1/sqrt(4)=0.5, 1/sqrt(16)=0.25.
        let s = tensor_from_f32_slice(&[4.0, 16.0], &[2]).unwrap();
        assert_eq!(
            tensor_to_f32_slice(&tensor_rsqrt(&s)).unwrap(),
            vec![0.5, 0.25]
        );
    }

    #[test]
    fn math_pow_broadcast() {
        let a = tensor_from_f32_slice(&[2.0, 3.0, 4.0], &[3]).unwrap();
        let b = tensor_from_f32_slice(&[2.0], &[1]).unwrap();
        // a^2 broadcast.
        let p = tensor_pow(&a, &b).unwrap();
        assert_eq!(tensor_shape(&p).unwrap(), vec![3]);
        assert_eq!(tensor_to_f32_slice(&p).unwrap(), vec![4.0, 9.0, 16.0]);
        assert_eq!(tensor_dtype_code(&p), 0);
        // equal-shape powf.
        let e = tensor_from_f32_slice(&[3.0, 2.0, 0.0], &[3]).unwrap();
        assert_eq!(
            tensor_to_f32_slice(&tensor_pow(&a, &e).unwrap()).unwrap(),
            vec![8.0, 9.0, 1.0]
        );
        // incompatible shapes error.
        let bad = tensor_from_f32_slice(&[1.0, 2.0], &[2]).unwrap();
        assert!(tensor_error_message(&tensor_pow(&a, &bad).unwrap_err())
            .contains("not broadcast-compatible"));
    }

    // --- bmm+int/bit (ops D) ---

    #[test]
    fn bmm_two_batches_match_two_matmuls() {
        // shape [2,2,2]: two stacked 2x2 matrices.
        let a = tensor_from_f32_slice(
            &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0],
            &[2, 2, 2],
        )
        .unwrap();
        let b = tensor_from_f32_slice(
            &[1.0, 0.0, 0.0, 1.0, 2.0, 0.0, 0.0, 2.0],
            &[2, 2, 2],
        )
        .unwrap();
        let c = tensor_bmm(&a, &b).unwrap();
        assert_eq!(tensor_shape(&c).unwrap(), vec![2, 2, 2]);
        // Verify each batch equals the rank-2 matmul of its slices.
        let a0 = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        let b0 = tensor_from_f32_slice(&[1.0, 0.0, 0.0, 1.0], &[2, 2]).unwrap();
        let a1 = tensor_from_f32_slice(&[5.0, 6.0, 7.0, 8.0], &[2, 2]).unwrap();
        let b1 = tensor_from_f32_slice(&[2.0, 0.0, 0.0, 2.0], &[2, 2]).unwrap();
        let mut expected = tensor_to_f32_slice(&tensor_matmul(&a0, &b0).unwrap()).unwrap();
        expected.extend(tensor_to_f32_slice(&tensor_matmul(&a1, &b1).unwrap()).unwrap());
        assert_eq!(tensor_to_f32_slice(&c).unwrap(), expected);
        assert_eq!(tensor_dtype_code(&c), 0); // F32
    }

    #[test]
    fn bmm_broadcasts_batch_dim() {
        // a: [2,2,3] (two matrices), b: [1,3,2] (one matrix, broadcast over batch).
        let a = tensor_from_f32_slice(
            &(0..12).map(|x| x as f64).collect::<Vec<_>>(),
            &[2, 2, 3],
        )
        .unwrap();
        let b = tensor_from_f32_slice(
            &[1.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            &[1, 3, 2],
        )
        .unwrap();
        let c = tensor_bmm(&a, &b).unwrap();
        assert_eq!(tensor_shape(&c).unwrap(), vec![2, 2, 2]);
        let bm = tensor_reshape(&b, &[3, 2]).unwrap();
        let a0 = tensor_reshape(
            &tensor_from_f32_slice(&[0.0, 1.0, 2.0, 3.0, 4.0, 5.0], &[6]).unwrap(),
            &[2, 3],
        )
        .unwrap();
        let a1 = tensor_reshape(
            &tensor_from_f32_slice(&[6.0, 7.0, 8.0, 9.0, 10.0, 11.0], &[6]).unwrap(),
            &[2, 3],
        )
        .unwrap();
        let mut expected = tensor_to_f32_slice(&tensor_matmul(&a0, &bm).unwrap()).unwrap();
        expected.extend(tensor_to_f32_slice(&tensor_matmul(&a1, &bm).unwrap()).unwrap());
        assert_eq!(tensor_to_f32_slice(&c).unwrap(), expected);
    }

    #[test]
    fn bmm_rejects_bad_shapes() {
        let v = tensor_from_f32_slice(&[1.0, 2.0], &[2]).unwrap();
        let m = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[2, 2]).unwrap();
        assert!(tensor_error_message(&tensor_bmm(&v, &m).unwrap_err()).contains("rank >= 2"));
        let a = tensor_from_f32_slice(&(0..8).map(|x| x as f64).collect::<Vec<_>>(), &[2, 2, 2])
            .unwrap();
        let bad = tensor_from_f32_slice(&(0..18).map(|x| x as f64).collect::<Vec<_>>(), &[2, 3, 3])
            .unwrap();
        assert!(tensor_error_message(&tensor_bmm(&a, &bad).unwrap_err())
            .contains("inner dimensions disagree"));
        // Non-broadcastable batch dims (2 vs 3).
        let a3 = tensor_from_f32_slice(&(0..12).map(|x| x as f64).collect::<Vec<_>>(), &[3, 2, 2])
            .unwrap();
        assert!(tensor_error_message(&tensor_bmm(&a, &a3).unwrap_err())
            .contains("not broadcast-compatible"));
    }

    #[test]
    fn int_ops_on_small_ints() {
        let a = tensor_cast_i32(&tensor_from_f32_slice(&[7.0, -7.0, 8.0, 5.0], &[4]).unwrap());
        let b = tensor_cast_i32(&tensor_from_f32_slice(&[2.0, 2.0, 3.0, 0.0], &[4]).unwrap());
        // idiv: trunc toward zero; div-by-zero -> 0.
        let q = tensor_idiv(&a, &b).unwrap();
        assert_eq!(tensor_dtype_code(&q), 1);
        assert_eq!(tensor_to_f32_slice(&q).unwrap(), vec![3.0, -3.0, 2.0, 0.0]);
        // mod: remainder; mod-by-zero -> 0.
        let r = tensor_mod(&a, &b).unwrap();
        assert_eq!(tensor_to_f32_slice(&r).unwrap(), vec![1.0, -1.0, 2.0, 0.0]);
    }

    #[test]
    fn shift_and_bitwise_ops() {
        let a = tensor_cast_i32(&tensor_from_f32_slice(&[1.0, 6.0, 12.0], &[3]).unwrap());
        let s = tensor_cast_i32(&tensor_from_f32_slice(&[3.0, 1.0, 2.0], &[3]).unwrap());
        assert_eq!(
            tensor_to_f32_slice(&tensor_shl(&a, &s).unwrap()).unwrap(),
            vec![8.0, 12.0, 48.0]
        );
        assert_eq!(
            tensor_to_f32_slice(&tensor_shr(&a, &s).unwrap()).unwrap(),
            vec![0.0, 3.0, 3.0]
        );
        let x = tensor_cast_i32(&tensor_from_f32_slice(&[6.0, 6.0, 6.0], &[3]).unwrap());
        let y = tensor_cast_i32(&tensor_from_f32_slice(&[3.0, 3.0, 3.0], &[3]).unwrap());
        assert_eq!(
            tensor_to_f32_slice(&tensor_and(&x, &y).unwrap()).unwrap(),
            vec![2.0, 2.0, 2.0]
        );
        assert_eq!(
            tensor_to_f32_slice(&tensor_or(&x, &y).unwrap()).unwrap(),
            vec![7.0, 7.0, 7.0]
        );
        assert_eq!(
            tensor_to_f32_slice(&tensor_xor(&x, &y).unwrap()).unwrap(),
            vec![5.0, 5.0, 5.0]
        );
    }

    #[test]
    fn int_ops_broadcast_and_dtype() {
        // [2,2] op [2] broadcasts the row; output dtype I32.
        let a = tensor_cast_i32(
            &tensor_from_f32_slice(&[10.0, 20.0, 30.0, 40.0], &[2, 2]).unwrap(),
        );
        let row = tensor_cast_i32(&tensor_from_f32_slice(&[3.0, 7.0], &[2]).unwrap());
        let m = tensor_mod(&a, &row).unwrap();
        assert_eq!(tensor_shape(&m).unwrap(), vec![2, 2]);
        assert_eq!(tensor_dtype_code(&m), 1);
        assert_eq!(tensor_to_f32_slice(&m).unwrap(), vec![1.0, 6.0, 0.0, 5.0]);
    }

    #[test]
    fn bitcast_round_trips_small_values() {
        let t = tensor_from_f32_slice(&[1.5, -2.0, 0.0], &[3]).unwrap();
        let i = tensor_bitcast_f32_to_i32(&t);
        assert_eq!(tensor_dtype_code(&i), 1);
        let back = tensor_bitcast_i32_to_f32(&i);
        assert_eq!(tensor_dtype_code(&back), 0);
        assert_eq!(
            tensor_to_f32_slice(&back).unwrap(),
            tensor_to_f32_slice(&t).unwrap()
        );
    }

    #[test]
    fn threefry_known_vector_zero() {
        // threefry2x32-20 with counter=0, key=0 is a published test vector:
        // (0x6b200159, 0x99ba4efe).
        let (w0, w1) = threefry2x32(0, 0);
        assert_eq!(w0, 0x6b200159);
        assert_eq!(w1, 0x99ba4efe);
    }

    #[test]
    fn threefry_known_vector_max() {
        // counter = 0xffffffffffffffff, key = 0xffffffffffffffff:
        // (0x1cb996fc, 0xbb002be7).
        let (w0, w1) = threefry2x32(u64::MAX, u64::MAX);
        assert_eq!(w0, 0x1cb996fc);
        assert_eq!(w1, 0xbb002be7);
    }

    #[test]
    #[allow(clippy::excessive_precision)] // golden values copied verbatim from the oracle
    fn rand_matches_tinygrad_golden() {
        // Golden values from the verified oracle `port_rand(seed, 8)`
        // (tinygrad-rsmc/oracle/rng_parity.py, maxdiff 0.0 vs tinygrad's
        // Tensor.rand for seeds 0/42/123/1337). counter = 0 must reproduce them
        // bit-for-bit (to f32 precision). This proves bit-exactness against the
        // verified reference WITHOUT importing tinygrad.
        let golden: [(i64, [f32; 8]); 4] = [
            (
                0,
                [
                    0.141_474_96,
                    0.299_082_28,
                    0.712_325_45,
                    0.913_471_34,
                    0.523_076_3,
                    0.329_058_53,
                    0.210_222_48,
                    0.245_554_21,
                ],
            ),
            (
                42,
                [
                    0.191_925_88,
                    0.822_174_2,
                    0.146_426_92,
                    0.196_875_33,
                    0.781_715_5,
                    0.072_996_974,
                    0.608_558_77,
                    0.354_853_87,
                ],
            ),
            (
                123,
                [
                    0.642_869_83,
                    0.493_291_74,
                    0.910_367_97,
                    0.854_320_64,
                    0.217_574_36,
                    0.943_871_4,
                    0.061_119_795,
                    0.274_717_33,
                ],
            ),
            (
                1337,
                [
                    0.488_659_86,
                    0.347_988_0,
                    0.659_324_53,
                    0.636_474_5,
                    0.471_165_3,
                    0.141_468_76,
                    0.278_090_83,
                    0.049_955_13,
                ],
            ),
        ];
        for (seed, expected) in golden {
            let t = tensor_rand(&[8], seed, 0).unwrap();
            let got = tensor_to_f32_slice(&t).unwrap();
            for (k, (&g, e)) in got.iter().zip(expected).enumerate() {
                assert!(
                    (g as f32 - e).abs() < 1e-6,
                    "seed={seed} idx={k}: got {g} expected {e}"
                );
            }
        }
    }

    #[test]
    fn rand_is_deterministic_and_in_range() {
        let a = tensor_rand(&[2, 3], 42, 0).unwrap();
        let b = tensor_rand(&[2, 3], 42, 0).unwrap();
        assert_eq!(tensor_to_f32_slice(&a).unwrap(), tensor_to_f32_slice(&b).unwrap());
        assert_eq!(tensor_shape(&a).unwrap(), vec![2, 3]);
        assert_eq!(tensor_dtype_code(&a), 0);
        for v in tensor_to_f32_slice(&a).unwrap() {
            assert!((0.0..1.0).contains(&v), "value {v} out of [0,1)");
        }
        // A different counter gives different output.
        let c = tensor_rand(&[2, 3], 42, 1).unwrap();
        assert_ne!(tensor_to_f32_slice(&a).unwrap(), tensor_to_f32_slice(&c).unwrap());
    }

    #[test]
    fn randint_is_deterministic_and_in_range() {
        let a = tensor_randint(&[100], 5, 10, 7, 0).unwrap();
        let b = tensor_randint(&[100], 5, 10, 7, 0).unwrap();
        assert_eq!(tensor_to_f32_slice(&a).unwrap(), tensor_to_f32_slice(&b).unwrap());
        assert_eq!(tensor_dtype_code(&a), 1);
        for v in tensor_to_f32_slice(&a).unwrap() {
            assert!((5.0..10.0).contains(&v), "value {v} out of [5,10)");
            assert_eq!(v, v.trunc(), "value {v} not integral");
        }
    }

    #[test]
    fn randint_rejects_empty_range() {
        assert!(tensor_randint(&[4], 5, 5, 1, 0).is_err());
        assert!(tensor_randint(&[4], 5, 4, 1, 0).is_err());
    }

    #[test]
    fn randn_is_deterministic_with_rough_moments() {
        let a = tensor_randn(&[5000], 123, 0).unwrap();
        let b = tensor_randn(&[5000], 123, 0).unwrap();
        assert_eq!(tensor_to_f32_slice(&a).unwrap(), tensor_to_f32_slice(&b).unwrap());
        assert_eq!(tensor_dtype_code(&a), 0);
        let data = tensor_to_f32_slice(&a).unwrap();
        let n = data.len() as f64;
        let mean = data.iter().sum::<f64>() / n;
        let var = data.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / n;
        assert!(mean.abs() < 0.1, "mean {mean} not ~0");
        assert!((var - 1.0).abs() < 0.15, "variance {var} not ~1");
    }

    #[test]
    fn randn_matches_tinygrad_golden() {
        // Golden values from the verified oracle `port_randn(seed, 8)`
        // (tinygrad-rsmc/oracle/randn_parity.py, maxdiff 0.0 vs tinygrad's
        // Tensor.randn(8) for seeds 0/42/123/1337). counter = 0 reproduces them.
        // randn = single rand over (2,8), Box-Muller: cos(2*pi*src[0]) *
        // sqrt(-2*log(1 - src[1])). Self-contained (no tinygrad import).
        let golden: [(i64, [f32; 8]); 4] = [
            (
                0,
                [
                    0.584_557, 0.177_917, 0.379_769, -1.321_681, 1.691_838, 0.381_049,
                    -0.403_509, 1.159_926,
                ],
            ),
            (
                42,
                [
                    0.991_906, 1.591_51, 0.879_765, 0.444_388, 1.455_374, -1.535_073,
                    0.489_513, 0.618_576,
                ],
            ),
            (
                123,
                [
                    -1.200_55, 2.083_955, -0.281_694, -0.272_154, 0.439_678, -0.784_856,
                    1.007_717, -1.064_269,
                ],
            ),
            (
                1337,
                [
                    -1.395_279, 0.317_524, -0.684_533, 0.516_59, -1.281_658, 0.168_111,
                    -1.159_221, -0.858_989,
                ],
            ),
        ];
        for (seed, expected) in golden {
            let t = tensor_randn(&[8], seed, 0).unwrap();
            let got = tensor_to_f32_slice(&t).unwrap();
            for (k, (&g, e)) in got.iter().zip(expected).enumerate() {
                assert!(
                    (g as f32 - e).abs() < 1e-5,
                    "seed={seed} idx={k}: got {g} expected {e}"
                );
            }
        }
    }

    #[test]
    fn randint_matches_tinygrad_golden() {
        // Golden values from the verified oracle `port_randint(seed, 8, 0, 10)`
        // (tinygrad-rsmc/oracle/randint_parity.py, maxdiff 0 vs tinygrad's
        // Tensor.randint(8, low=0, high=10) for seeds 0/42/123/1337). counter = 0.
        // randint = ((high-low)*rand).trunc() + low. Exact match. Self-contained.
        let golden: [(i64, [f32; 8]); 4] = [
            (0, [1.0, 2.0, 7.0, 9.0, 5.0, 3.0, 2.0, 2.0]),
            (42, [1.0, 8.0, 1.0, 1.0, 7.0, 0.0, 6.0, 3.0]),
            (123, [6.0, 4.0, 9.0, 8.0, 2.0, 9.0, 0.0, 2.0]),
            (1337, [4.0, 3.0, 6.0, 6.0, 4.0, 1.0, 2.0, 0.0]),
        ];
        for (seed, expected) in golden {
            let t = tensor_randint(&[8], 0, 10, seed, 0).unwrap();
            let got = tensor_to_f32_slice(&t).unwrap();
            for (k, (&g, e)) in got.iter().zip(expected).enumerate() {
                assert_eq!(g as f32, e, "seed={seed} idx={k}: got {g} expected {e}");
            }
        }
    }

    // --- nn primitives (slice F) ---

    #[test]
    fn iota_basic_and_rejects_negative() {
        let r = tensor_iota(4).unwrap();
        assert_eq!(tensor_shape(&r).unwrap(), vec![4]);
        assert_eq!(tensor_dtype_code(&r), 1); // I32
        assert_eq!(tensor_to_f32_slice(&r).unwrap(), vec![0.0, 1.0, 2.0, 3.0]);
        assert_eq!(tensor_to_f32_slice(&tensor_iota(0).unwrap()).unwrap(), Vec::<f64>::new());
        assert!(tensor_error_message(&tensor_iota(-1).unwrap_err()).contains("non-negative"));
    }

    #[test]
    fn one_hot_correctness_and_out_of_range() {
        // indices [1, 0, 2], 3 classes -> 3x3 identity-ish.
        let idx = tensor_cast_i32(&tensor_from_f32_slice(&[1.0, 0.0, 2.0], &[3]).unwrap());
        let oh = tensor_one_hot(&idx, 3).unwrap();
        assert_eq!(tensor_shape(&oh).unwrap(), vec![3, 3]);
        assert_eq!(tensor_dtype_code(&oh), 0); // F32
        assert_eq!(
            tensor_to_f32_slice(&oh).unwrap(),
            vec![0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0]
        );
        // Higher-rank indices: shape [2,2] -> [2,2,2].
        let idx2 = tensor_cast_i32(&tensor_from_f32_slice(&[0.0, 1.0, 1.0, 0.0], &[2, 2]).unwrap());
        let oh2 = tensor_one_hot(&idx2, 2).unwrap();
        assert_eq!(tensor_shape(&oh2).unwrap(), vec![2, 2, 2]);
        assert_eq!(
            tensor_to_f32_slice(&oh2).unwrap(),
            vec![1.0, 0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0]
        );
        // Out-of-range index errors.
        let bad = tensor_cast_i32(&tensor_from_f32_slice(&[0.0, 3.0], &[2]).unwrap());
        assert!(tensor_error_message(&tensor_one_hot(&bad, 3).unwrap_err()).contains("out of range"));
        // num_classes <= 0 errors.
        assert!(tensor_error_message(&tensor_one_hot(&idx, 0).unwrap_err()).contains("positive"));
    }

    #[test]
    fn softmax_sums_to_one() {
        // [[1,2,3],[1,1,1]] softmax along axis 1.
        let t = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 1.0, 1.0, 1.0], &[2, 3]).unwrap();
        let s = tensor_softmax(&t, 1).unwrap();
        assert_eq!(tensor_shape(&s).unwrap(), vec![2, 3]);
        assert_eq!(tensor_dtype_code(&s), 0);
        let got = tensor_to_f32_slice(&s).unwrap();
        // Each row sums to 1.
        for row in 0..2 {
            let sum: f64 = got[row * 3..row * 3 + 3].iter().sum();
            assert!((sum - 1.0).abs() < 1e-6, "row {row} sum {sum}");
        }
        // Uniform row -> all 1/3.
        for v in &got[3..6] {
            assert!((v - 1.0 / 3.0).abs() < 1e-6);
        }
        // softmax along axis 0 also sums to 1 per column.
        let s0 = tensor_softmax(&t, 0).unwrap();
        let g0 = tensor_to_f32_slice(&s0).unwrap();
        for col in 0..3 {
            let sum = g0[col] + g0[3 + col];
            assert!((sum - 1.0).abs() < 1e-6);
        }
        assert!(tensor_error_message(&tensor_softmax(&t, 2).unwrap_err()).contains("out of range"));
    }

    #[test]
    fn log_softmax_matches_log_of_softmax() {
        let t = tensor_from_f32_slice(&[0.5, -1.0, 2.0, 3.0, 0.0, -2.0], &[2, 3]).unwrap();
        let ls = tensor_to_f32_slice(&tensor_log_softmax(&t, 1).unwrap()).unwrap();
        let sm = tensor_to_f32_slice(&tensor_softmax(&t, 1).unwrap()).unwrap();
        for (l, s) in ls.iter().zip(sm.iter()) {
            assert!((l - s.ln()).abs() < 1e-5, "{l} vs ln({s})");
        }
    }

    #[test]
    fn cross_entropy_known_case() {
        // logits [[1,2,3]], target class 2.
        // log-sum-exp: max=3, sum=exp(-2)+exp(-1)+exp(0)=0.135335+0.367879+1=1.503214
        // log(sum)=0.407606 ; loss = max + log(sum) - x_target = 3 + 0.407606 - 3 = 0.407606
        let logits = tensor_from_f32_slice(&[1.0, 2.0, 3.0], &[1, 3]).unwrap();
        let targets = tensor_cast_i32(&tensor_from_f32_slice(&[2.0], &[1]).unwrap());
        let ce = tensor_cross_entropy(&logits, &targets, 1).unwrap();
        assert_eq!(tensor_shape(&ce).unwrap(), Vec::<i64>::new()); // scalar
        assert_eq!(tensor_dtype_code(&ce), 0);
        let v = tensor_to_f32_slice(&ce).unwrap()[0];
        assert!((v - 0.407606).abs() < 1e-4, "got {v}");
        // Two rows: mean of the per-row losses.
        let l2 = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 3.0, 2.0, 1.0], &[2, 3]).unwrap();
        let t2 = tensor_cast_i32(&tensor_from_f32_slice(&[2.0, 0.0], &[2]).unwrap());
        let ce2 = tensor_to_f32_slice(&tensor_cross_entropy(&l2, &t2, 1).unwrap()).unwrap()[0];
        // Both rows symmetric -> each loss 0.407606 -> mean same.
        assert!((ce2 - 0.407606).abs() < 1e-4, "got {ce2}");
        // Matches -log_softmax[target] from the log_softmax kernel.
        let ls = tensor_to_f32_slice(&tensor_log_softmax(&logits, 1).unwrap()).unwrap();
        assert!((v - (-ls[2])).abs() < 1e-5);
    }

    #[test]
    fn cross_entropy_rejects_bad_args() {
        let logits = tensor_from_f32_slice(&[1.0, 2.0, 3.0], &[1, 3]).unwrap();
        let t = tensor_cast_i32(&tensor_from_f32_slice(&[0.0], &[1]).unwrap());
        // wrong axis.
        assert!(tensor_error_message(&tensor_cross_entropy(&logits, &t, 0).unwrap_err())
            .contains("class axis must be 1"));
        // non-rank-2 logits.
        let v = tensor_from_f32_slice(&[1.0, 2.0, 3.0], &[3]).unwrap();
        assert!(tensor_error_message(&tensor_cross_entropy(&v, &t, 1).unwrap_err())
            .contains("rank-2"));
        // targets length mismatch.
        let t2 = tensor_cast_i32(&tensor_from_f32_slice(&[0.0, 1.0], &[2]).unwrap());
        assert!(tensor_error_message(&tensor_cross_entropy(&logits, &t2, 1).unwrap_err())
            .contains("does not match logits batch"));
        // out-of-range target.
        let bad = tensor_cast_i32(&tensor_from_f32_slice(&[5.0], &[1]).unwrap());
        assert!(tensor_error_message(&tensor_cross_entropy(&logits, &bad, 1).unwrap_err())
            .contains("out of range"));
        // rank-2 targets rejected.
        let t3 = tensor_cast_i32(&tensor_from_f32_slice(&[0.0], &[1, 1]).unwrap());
        assert!(tensor_error_message(&tensor_cross_entropy(&logits, &t3, 1).unwrap_err())
            .contains("rank-1"));
    }

    // conv (slice G)

    #[test]
    fn conv2d_3x3_2x2_stride1_pad0() {
        // 1x1x3x3 input, 1x1x2x2 weight, stride 1, pad 0 → 1x1x2x2.
        let input = tensor_from_f32_slice(
            &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0],
            &[1, 1, 3, 3],
        )
        .unwrap();
        let weight = tensor_from_f32_slice(&[1.0, 0.0, 0.0, 1.0], &[1, 1, 2, 2]).unwrap();
        let out = tensor_conv2d(&input, &weight, 1, 0).unwrap();
        assert_eq!(tensor_shape(&out).unwrap(), vec![1, 1, 2, 2]);
        // out[i,j] = in[i,j]*1 + in[i+1,j+1]*1.
        // [1+5, 2+6, 4+8, 5+9] = [6, 8, 12, 14].
        assert_eq!(tensor_to_f32_slice(&out).unwrap(), vec![6.0, 8.0, 12.0, 14.0]);
        assert_eq!(tensor_dtype_code(&out), 0);
    }

    #[test]
    fn conv2d_stride2() {
        // 1x1x4x4 input, 1x1x2x2 ones weight, stride 2, pad 0 → 1x1x2x2.
        let input = tensor_from_f32_slice(
            &[
                1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0,
                16.0,
            ],
            &[1, 1, 4, 4],
        )
        .unwrap();
        let weight = tensor_from_f32_slice(&[1.0, 1.0, 1.0, 1.0], &[1, 1, 2, 2]).unwrap();
        let out = tensor_conv2d(&input, &weight, 2, 0).unwrap();
        assert_eq!(tensor_shape(&out).unwrap(), vec![1, 1, 2, 2]);
        // Sum of each non-overlapping 2x2 block:
        // [1+2+5+6, 3+4+7+8, 9+10+13+14, 11+12+15+16] = [14, 22, 46, 54].
        assert_eq!(tensor_to_f32_slice(&out).unwrap(), vec![14.0, 22.0, 46.0, 54.0]);
    }

    #[test]
    fn conv2d_pad1() {
        // 1x1x2x2 input, 1x1x2x2 ones weight, stride 1, pad 1 → 1x1x3x3.
        let input = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[1, 1, 2, 2]).unwrap();
        let weight = tensor_from_f32_slice(&[1.0, 1.0, 1.0, 1.0], &[1, 1, 2, 2]).unwrap();
        let out = tensor_conv2d(&input, &weight, 1, 1).unwrap();
        assert_eq!(tensor_shape(&out).unwrap(), vec![1, 1, 3, 3]);
        // Padded input is a 4x4 with a 1-wide zero border around [[1,2],[3,4]].
        // Each output is the 2x2 sum of the padded window:
        // corners 1,2,3,4; edges 1+2,3+4 (top/bottom), 1+3,2+4 (left/right);
        // center 1+2+3+4.
        assert_eq!(
            tensor_to_f32_slice(&out).unwrap(),
            vec![1.0, 3.0, 2.0, 4.0, 10.0, 6.0, 3.0, 7.0, 4.0]
        );
    }

    #[test]
    fn conv2d_rejects_channel_mismatch() {
        let input = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[1, 1, 2, 2]).unwrap();
        let weight = tensor_from_f32_slice(&[1.0; 8], &[1, 2, 2, 2]).unwrap();
        let err = tensor_conv2d(&input, &weight, 1, 0).unwrap_err();
        assert!(tensor_error_message(&err).contains("channels"));
    }

    #[test]
    fn max_pool2d_4x4_2x2() {
        // 1x1x4x4 input, kernel 2, stride 2 → 1x1x2x2 of block maxes.
        let input = tensor_from_f32_slice(
            &[
                1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0,
                16.0,
            ],
            &[1, 1, 4, 4],
        )
        .unwrap();
        let out = tensor_max_pool2d(&input, 2, 2).unwrap();
        assert_eq!(tensor_shape(&out).unwrap(), vec![1, 1, 2, 2]);
        // Max of each non-overlapping 2x2 block: [6, 8, 14, 16].
        assert_eq!(tensor_to_f32_slice(&out).unwrap(), vec![6.0, 8.0, 14.0, 16.0]);
    }

    #[test]
    fn avg_pool2d_4x4_2x2() {
        let input = tensor_from_f32_slice(
            &[
                1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0,
                16.0,
            ],
            &[1, 1, 4, 4],
        )
        .unwrap();
        let out = tensor_avg_pool2d(&input, 2, 2).unwrap();
        assert_eq!(tensor_shape(&out).unwrap(), vec![1, 1, 2, 2]);
        // Mean of each block: [(1+2+5+6)/4, (3+4+7+8)/4, ...] = [3.5, 5.5, 11.5, 13.5].
        assert_eq!(tensor_to_f32_slice(&out).unwrap(), vec![3.5, 5.5, 11.5, 13.5]);
    }

    #[test]
    fn pool2d_rejects_non_positive_output() {
        let input = tensor_from_f32_slice(&[1.0, 2.0, 3.0, 4.0], &[1, 1, 2, 2]).unwrap();
        let err = tensor_max_pool2d(&input, 3, 1).unwrap_err();
        assert!(tensor_error_message(&err).contains("non-positive output"));
    }
}
