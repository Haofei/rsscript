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

/// A packed, row-major tensor: `data.len() == shape.iter().product()`. The buffer
/// is `Rc`-shared so handles clone cheaply and later slices can alias storage.
/// Single-isolate (no `Send`/`Sync` bound), matching `RssChannel`.
#[derive(Debug, Clone, PartialEq)]
pub struct RssTensor {
    data: Rc<Vec<f32>>,
    shape: Vec<usize>,
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
}
