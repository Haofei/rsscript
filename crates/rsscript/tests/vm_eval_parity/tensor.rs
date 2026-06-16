//! eval≡lowered parity: native Tensor kernels (ML-perf slice 1).
//!
//! Both backends call the identical `rsscript_runtime::tensor_*` functions (the
//! reg-VM stores real `RssTensor` handles and dispatches to them; the AOT backend
//! lowers `Tensor.*` to the same calls), so the f64<->f32 narrowing and the
//! matmul arithmetic are bit-identical. These tests assert the VM and compiled
//! outputs match exactly.
#![allow(unused_imports, dead_code)]
use super::*;

#[test]
fn parity_tensor_shape_and_rank() {
    let source = r#"
features: native, local

fn main() -> Result<Unit, TensorError> {
    let t = Tensor.from_f32_slice(
        data: read [1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        shape: read [2, 3],
    )?
    Log.write(message: read String.from_int(value: Tensor.rank(tensor: read t)))
    let dims = Tensor.shape(tensor: read t)?
    let mut index = 0
    while index < List.len(list: read dims) {
        Log.write(message: read String.from_int(value: List.get(list: read dims, index: index)))
        index = index + 1
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-shape.rss",
        "rsscript_parity_tensor_shape",
        source,
    );
}

#[test]
fn parity_tensor_round_trip() {
    let source = r#"
features: native, local

fn main() -> Result<Unit, TensorError> {
    let t = Tensor.from_f32_slice(
        data: read [10.0, 20.0, 30.0, 40.0],
        shape: read [2, 2],
    )?
    let values = Tensor.to_f32_slice(tensor: read t)?
    let mut index = 0
    while index < List.len(list: read values) {
        Log.write(message: read Float.to_string(value: read List.get(list: read values, index: index)))
        index = index + 1
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-round-trip.rss",
        "rsscript_parity_tensor_round_trip",
        source,
    );
}

#[test]
fn parity_tensor_matmul() {
    let source = r#"
features: native, local

fn main() -> Result<Unit, TensorError> {
    let a = Tensor.from_f32_slice(data: read [1.0, 2.0, 3.0, 4.0], shape: read [2, 2])?
    let b = Tensor.from_f32_slice(data: read [5.0, 6.0, 7.0, 8.0], shape: read [2, 2])?
    let c = Tensor.matmul(a: read a, b: read b)?
    let values = Tensor.to_f32_slice(tensor: read c)?
    let mut index = 0
    while index < List.len(list: read values) {
        Log.write(message: read Float.to_string(value: read List.get(list: read values, index: index)))
        index = index + 1
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-matmul.rss",
        "rsscript_parity_tensor_matmul",
        source,
    );
}

#[test]
fn parity_tensor_matmul_non_square() {
    let source = r#"
features: native, local

fn main() -> Result<Unit, TensorError> {
    // (2x3) x (3x2) -> (2x2)
    let a = Tensor.from_f32_slice(
        data: read [1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        shape: read [2, 3],
    )?
    let b = Tensor.from_f32_slice(
        data: read [7.0, 8.0, 9.0, 10.0, 11.0, 12.0],
        shape: read [3, 2],
    )?
    let c = Tensor.matmul(a: read a, b: read b)?
    let dims = Tensor.shape(tensor: read c)?
    Log.write(message: read String.from_int(value: List.get(list: read dims, index: 0)))
    Log.write(message: read String.from_int(value: List.get(list: read dims, index: 1)))
    let values = Tensor.to_f32_slice(tensor: read c)?
    let mut index = 0
    while index < List.len(list: read values) {
        Log.write(message: read Float.to_string(value: read List.get(list: read values, index: index)))
        index = index + 1
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-matmul-non-square.rss",
        "rsscript_parity_tensor_matmul_non_square",
        source,
    );
}

#[test]
fn parity_tensor_errors() {
    let source = r#"
features: native, local

fn describe(result: take Result<fresh Tensor, TensorError>) -> Unit {
    match result {
        Ok(_) => {
            Log.write(message: read "ok")
        }
        Err(error) => {
            Log.write(message: read TensorError.message(error: read error))
        }
    }
    return Unit
}

fn main() -> Result<Unit, TensorError> {
    // data length does not match shape
    local bad = Tensor.from_f32_slice(data: read [1.0, 2.0, 3.0], shape: read [2, 2])
    describe(result: take bad)

    // matmul on mismatched inner dims
    let a = Tensor.from_f32_slice(data: read [1.0, 2.0, 3.0, 4.0, 5.0, 6.0], shape: read [2, 3])?
    let b = Tensor.from_f32_slice(data: read [1.0, 2.0, 3.0, 4.0], shape: read [2, 2])?
    local mismatch = Tensor.matmul(a: read a, b: read b)
    describe(result: take mismatch)
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-errors.rss",
        "rsscript_parity_tensor_errors",
        source,
    );
}

/// A large matmul (128×128) run on the VM: proves big matmul completes natively
/// (the math is in `rsscript_runtime`, not interpreted bytecode) and the result
/// matches a directly-computed reference. Also exercised in compiled form by the
/// shared parity harness, so VM≡compiled holds at size.
#[test]
fn parity_tensor_large_matmul() {
    // Identity-times-ramp: A = 128x128 identity, B = 128x128 ramp (b[i][j] = i*n+j).
    // A·B == B, so the reference is trivial and the whole 128³ contraction still
    // runs through the native kernel. We print B's diagonal so output is compact.
    let source = r#"
features: native, local

fn build_identity(n: Int) -> Result<fresh Tensor, TensorError> {
    local data = List<Float>.new()
    let mut i = 0
    while i < n {
        let mut j = 0
        while j < n {
            if i == j {
                List.push<Float>(list: mut data, value: read 1.0)
            } else {
                List.push<Float>(list: mut data, value: read 0.0)
            }
            j = j + 1
        }
        i = i + 1
    }
    return Tensor.from_f32_slice(data: read data, shape: read [n, n])
}

fn build_ramp(n: Int) -> Result<fresh Tensor, TensorError> {
    local data = List<Float>.new()
    let mut k = 0
    let total = n * n
    while k < total {
        List.push<Float>(list: mut data, value: read Int.to_float(value: read k))
        k = k + 1
    }
    return Tensor.from_f32_slice(data: read data, shape: read [n, n])
}

fn main() -> Result<Unit, TensorError> {
    let n = 128
    let identity = build_identity(n: n)?
    let ramp = build_ramp(n: n)?
    let product = Tensor.matmul(a: read identity, b: read ramp)?
    let values = Tensor.to_f32_slice(tensor: read product)?
    // Print the diagonal of the product (== diagonal of ramp == i*n + i).
    let mut i = 0
    while i < n {
        let value = List.get(list: read values, index: i * n + i)
        Log.write(message: read Float.to_string(value: read value))
        i = i + 1
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-large-matmul.rss",
        "rsscript_parity_tensor_large_matmul",
        source,
    );
}

#[test]
fn parity_tensor_elementwise_binary() {
    let source = r#"
features: native, local

fn dump(t: read Tensor) -> Result<Unit, TensorError> {
    let values = Tensor.to_f32_slice(tensor: read t)?
    let mut index = 0
    while index < List.len(list: read values) {
        Log.write(message: read Float.to_string(value: read List.get(list: read values, index: index)))
        index = index + 1
    }
    return Ok(Unit)
}

fn main() -> Result<Unit, TensorError> {
    let a = Tensor.from_f32_slice(data: read [1.0, 2.0, 4.0, 8.0], shape: read [2, 2])?
    let b = Tensor.from_f32_slice(data: read [2.0, 2.0, 2.0, 2.0], shape: read [2, 2])?
    let sum = Tensor.add(a: read a, b: read b)?
    let diff = Tensor.sub(a: read a, b: read b)?
    let prod = Tensor.mul(a: read a, b: read b)?
    let quot = Tensor.div(a: read a, b: read b)?
    dump(t: read sum)?
    dump(t: read diff)?
    dump(t: read prod)?
    dump(t: read quot)?
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-elementwise-binary.rss",
        "rsscript_parity_tensor_elementwise_binary",
        source,
    );
}

#[test]
fn parity_tensor_elementwise_unary() {
    let source = r#"
features: native, local

fn dump(t: read Tensor) -> Result<Unit, TensorError> {
    let values = Tensor.to_f32_slice(tensor: read t)?
    let mut index = 0
    while index < List.len(list: read values) {
        Log.write(message: read Float.to_string(value: read List.get(list: read values, index: index)))
        index = index + 1
    }
    return Ok(Unit)
}

fn main() -> Result<Unit, TensorError> {
    let t = Tensor.from_f32_slice(data: read [1.0, 0.0, 4.0, 9.0], shape: read [2, 2])?
    let neg_in = Tensor.from_f32_slice(data: read [1.0, 0.0, 0.0, 0.0], shape: read [2, 2])?
    let neg = Tensor.neg(t: read t)
    dump(t: read neg)?
    dump(t: read Tensor.exp(t: read t))?
    dump(t: read Tensor.log(t: read t))?
    dump(t: read Tensor.sqrt(t: read t))?
    // relu over a tensor with negatives.
    let mixed = Tensor.sub(a: read neg_in, b: read t)?
    dump(t: read Tensor.relu(t: read mixed))?
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-elementwise-unary.rss",
        "rsscript_parity_tensor_elementwise_unary",
        source,
    );
}

#[test]
fn parity_tensor_elementwise_shape_mismatch() {
    let source = r#"
features: native, local

fn main() -> Result<Unit, TensorError> {
    let a = Tensor.from_f32_slice(data: read [1.0, 2.0, 3.0, 4.0], shape: read [2, 2])?
    let b = Tensor.from_f32_slice(data: read [1.0, 2.0, 3.0, 4.0, 5.0, 6.0], shape: read [2, 3])?
    local bad = Tensor.add(a: read a, b: read b)
    match bad {
        Ok(_) => {
            Log.write(message: read "ok")
        }
        Err(error) => {
            Log.write(message: read TensorError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-elementwise-shape-mismatch.rss",
        "rsscript_parity_tensor_elementwise_shape_mismatch",
        source,
    );
}

/// A larger (256×256 = 65 536-element) elementwise add+mul to exercise the bulk
/// path natively on both backends. We print a few sampled elements so output is
/// compact while the whole buffer is still computed.
#[test]
fn parity_tensor_large_elementwise() {
    let source = r#"
features: native, local

fn build_ramp(n: Int) -> Result<fresh Tensor, TensorError> {
    local data = List<Float>.new()
    let mut k = 0
    let total = n * n
    while k < total {
        List.push<Float>(list: mut data, value: read Int.to_float(value: read k))
        k = k + 1
    }
    return Tensor.from_f32_slice(data: read data, shape: read [n, n])
}

fn main() -> Result<Unit, TensorError> {
    let n = 256
    let a = build_ramp(n: n)?
    let b = build_ramp(n: n)?
    let sum = Tensor.add(a: read a, b: read b)?
    let prod = Tensor.mul(a: read a, b: read b)?
    let sum_vals = Tensor.to_f32_slice(tensor: read sum)?
    let prod_vals = Tensor.to_f32_slice(tensor: read prod)?
    let total = n * n
    let mut i = 0
    while i < total {
        Log.write(message: read Float.to_string(value: read List.get(list: read sum_vals, index: i)))
        Log.write(message: read Float.to_string(value: read List.get(list: read prod_vals, index: i)))
        i = i + 12000
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-large-elementwise.rss",
        "rsscript_parity_tensor_large_elementwise",
        source,
    );
}

#[test]
fn parity_tensor_sum_all() {
    let source = r#"
features: native, local

fn main() -> Result<Unit, TensorError> {
    let t = Tensor.from_f32_slice(
        data: read [1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        shape: read [2, 3],
    )?
    let total = Tensor.sum_all(t: read t)
    Log.write(message: read Float.to_string(value: read total))
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-sum-all.rss",
        "rsscript_parity_tensor_sum_all",
        source,
    );
}

#[test]
fn parity_tensor_reduce_axis() {
    let source = r#"
features: native, local

fn dump(t: read Tensor) -> Result<Unit, TensorError> {
    let dims = Tensor.shape(tensor: read t)?
    let mut d = 0
    while d < List.len(list: read dims) {
        Log.write(message: read String.from_int(value: List.get(list: read dims, index: d)))
        d = d + 1
    }
    let values = Tensor.to_f32_slice(tensor: read t)?
    let mut index = 0
    while index < List.len(list: read values) {
        Log.write(message: read Float.to_string(value: read List.get(list: read values, index: index)))
        index = index + 1
    }
    return Ok(Unit)
}

fn main() -> Result<Unit, TensorError> {
    // [[1,2,3],[4,5,6]]
    let t = Tensor.from_f32_slice(
        data: read [1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        shape: read [2, 3],
    )?
    let s0 = Tensor.sum_axis(t: read t, axis: 0)?
    dump(t: read s0)?
    let s1 = Tensor.sum_axis(t: read t, axis: 1)?
    dump(t: read s1)?
    let mx = Tensor.max_axis(t: read t, axis: 0)?
    dump(t: read mx)?
    let mn = Tensor.mean_axis(t: read t, axis: 1)?
    dump(t: read mn)?
    let am0 = Tensor.argmax_axis(t: read t, axis: 0)?
    dump(t: read am0)?
    let am1 = Tensor.argmax_axis(t: read t, axis: 1)?
    dump(t: read am1)?
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-reduce-axis.rss",
        "rsscript_parity_tensor_reduce_axis",
        source,
    );
}

#[test]
fn parity_tensor_reduce_axis_error() {
    let source = r#"
features: native, local

fn main() -> Result<Unit, TensorError> {
    let t = Tensor.from_f32_slice(data: read [1.0, 2.0, 3.0, 4.0], shape: read [2, 2])?
    local bad = Tensor.sum_axis(t: read t, axis: 5)
    match bad {
        Ok(_) => {
            Log.write(message: read "ok")
        }
        Err(error) => {
            Log.write(message: read TensorError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-reduce-axis-error.rss",
        "rsscript_parity_tensor_reduce_axis_error",
        source,
    );
}

/// A larger rank-3 reduction (shape [8,16,32]) reduced over the middle axis,
/// exercising the strided contiguous walk natively on both backends. Prints a few
/// sampled cells so output stays compact while the whole buffer is computed.
#[test]
fn parity_tensor_large_reduce() {
    let source = r#"
features: native, local

fn main() -> Result<Unit, TensorError> {
    local data = List<Float>.new()
    let total = 8 * 16 * 32
    let mut k = 0
    while k < total {
        List.push<Float>(list: mut data, value: read Int.to_float(value: read k))
        k = k + 1
    }
    let t = Tensor.from_f32_slice(data: read data, shape: read [8, 16, 32])?
    // reduce middle axis -> shape [8, 32]
    let summed = Tensor.sum_axis(t: read t, axis: 1)?
    let dims = Tensor.shape(tensor: read summed)?
    Log.write(message: read String.from_int(value: List.get(list: read dims, index: 0)))
    Log.write(message: read String.from_int(value: List.get(list: read dims, index: 1)))
    let values = Tensor.to_f32_slice(tensor: read summed)?
    let mut i = 0
    while i < List.len(list: read values) {
        Log.write(message: read Float.to_string(value: read List.get(list: read values, index: i)))
        i = i + 37
    }
    let means = Tensor.mean_axis(t: read t, axis: 2)?
    let mvals = Tensor.to_f32_slice(tensor: read means)?
    let mut j = 0
    while j < List.len(list: read mvals) {
        Log.write(message: read Float.to_string(value: read List.get(list: read mvals, index: j)))
        j = j + 17
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-large-reduce.rss",
        "rsscript_parity_tensor_large_reduce",
        source,
    );
}

#[test]
fn parity_tensor_reshape() {
    let source = r#"
features: native, local

fn main() -> Result<Unit, TensorError> {
    let t = Tensor.from_f32_slice(
        data: read [1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        shape: read [2, 3],
    )?
    let r = Tensor.reshape(t: read t, shape: read [3, 2])?
    let dims = Tensor.shape(tensor: read r)?
    Log.write(message: read String.from_int(value: List.get(list: read dims, index: 0)))
    Log.write(message: read String.from_int(value: List.get(list: read dims, index: 1)))
    let values = Tensor.to_f32_slice(tensor: read r)?
    let mut index = 0
    while index < List.len(list: read values) {
        Log.write(message: read Float.to_string(value: read List.get(list: read values, index: index)))
        index = index + 1
    }
    // reshape with a wrong element count surfaces an error identically.
    local bad = Tensor.reshape(t: read t, shape: read [4, 2])
    match bad {
        Ok(_) => {
            Log.write(message: read "ok")
        }
        Err(error) => {
            Log.write(message: read TensorError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-reshape.rss",
        "rsscript_parity_tensor_reshape",
        source,
    );
}

#[test]
fn parity_tensor_transpose() {
    let source = r#"
features: native, local

fn main() -> Result<Unit, TensorError> {
    let t = Tensor.from_f32_slice(
        data: read [1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
        shape: read [2, 3],
    )?
    let tr = Tensor.transpose(t: read t)?
    let dims = Tensor.shape(tensor: read tr)?
    Log.write(message: read String.from_int(value: List.get(list: read dims, index: 0)))
    Log.write(message: read String.from_int(value: List.get(list: read dims, index: 1)))
    let values = Tensor.to_f32_slice(tensor: read tr)?
    let mut index = 0
    while index < List.len(list: read values) {
        Log.write(message: read Float.to_string(value: read List.get(list: read values, index: index)))
        index = index + 1
    }
    // transpose of a rank-1 tensor errors identically.
    let v = Tensor.from_f32_slice(data: read [1.0, 2.0, 3.0], shape: read [3])?
    local bad = Tensor.transpose(t: read v)
    match bad {
        Ok(_) => {
            Log.write(message: read "ok")
        }
        Err(error) => {
            Log.write(message: read TensorError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-transpose.rss",
        "rsscript_parity_tensor_transpose",
        source,
    );
}

#[test]
fn parity_tensor_permute() {
    let source = r#"
features: native, local

fn main() -> Result<Unit, TensorError> {
    let t = Tensor.from_f32_slice(
        data: read [0.0, 1.0, 2.0, 3.0, 4.0, 5.0],
        shape: read [2, 1, 3],
    )?
    let p = Tensor.permute(t: read t, axes: read [2, 0, 1])?
    let dims = Tensor.shape(tensor: read p)?
    let mut d = 0
    while d < List.len(list: read dims) {
        Log.write(message: read String.from_int(value: List.get(list: read dims, index: d)))
        d = d + 1
    }
    let values = Tensor.to_f32_slice(tensor: read p)?
    let mut index = 0
    while index < List.len(list: read values) {
        Log.write(message: read Float.to_string(value: read List.get(list: read values, index: index)))
        index = index + 1
    }
    // a non-permutation surfaces an error identically.
    local bad = Tensor.permute(t: read t, axes: read [0, 0, 1])
    match bad {
        Ok(_) => {
            Log.write(message: read "ok")
        }
        Err(error) => {
            Log.write(message: read TensorError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-permute.rss",
        "rsscript_parity_tensor_permute",
        source,
    );
}

#[test]
fn parity_tensor_broadcast_to() {
    let source = r#"
features: native, local

fn main() -> Result<Unit, TensorError> {
    let row = Tensor.from_f32_slice(data: read [1.0, 2.0, 3.0], shape: read [3])?
    let b = Tensor.broadcast_to(t: read row, shape: read [2, 3])?
    let dims = Tensor.shape(tensor: read b)?
    Log.write(message: read String.from_int(value: List.get(list: read dims, index: 0)))
    Log.write(message: read String.from_int(value: List.get(list: read dims, index: 1)))
    let values = Tensor.to_f32_slice(tensor: read b)?
    let mut index = 0
    while index < List.len(list: read values) {
        Log.write(message: read Float.to_string(value: read List.get(list: read values, index: index)))
        index = index + 1
    }
    // a non-broadcastable target surfaces an error identically.
    local bad = Tensor.broadcast_to(t: read row, shape: read [2, 4])
    match bad {
        Ok(_) => {
            Log.write(message: read "ok")
        }
        Err(error) => {
            Log.write(message: read TensorError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-broadcast-to.rss",
        "rsscript_parity_tensor_broadcast_to",
        source,
    );
}

#[test]
fn parity_tensor_broadcast_binary() {
    // [2,3] + [3] (broadcast) and [2,3] + [2,3] (equal-shape parity preserved).
    let source = r#"
features: native, local

fn dump(t: read Tensor) -> Result<Unit, TensorError> {
    let dims = Tensor.shape(tensor: read t)?
    let mut d = 0
    while d < List.len(list: read dims) {
        Log.write(message: read String.from_int(value: List.get(list: read dims, index: d)))
        d = d + 1
    }
    let values = Tensor.to_f32_slice(tensor: read t)?
    let mut index = 0
    while index < List.len(list: read values) {
        Log.write(message: read Float.to_string(value: read List.get(list: read values, index: index)))
        index = index + 1
    }
    return Ok(Unit)
}

fn main() -> Result<Unit, TensorError> {
    let a = Tensor.from_f32_slice(data: read [1.0, 2.0, 3.0, 4.0, 5.0, 6.0], shape: read [2, 3])?
    let row = Tensor.from_f32_slice(data: read [10.0, 20.0, 30.0], shape: read [3])?
    let bcast = Tensor.add(a: read a, b: read row)?
    dump(t: read bcast)?
    // equal-shape add still matches (parity with the old path).
    let b = Tensor.from_f32_slice(data: read [6.0, 5.0, 4.0, 3.0, 2.0, 1.0], shape: read [2, 3])?
    let eq = Tensor.add(a: read a, b: read b)?
    dump(t: read eq)?
    // an incompatible pair surfaces an error identically.
    let bad = Tensor.from_f32_slice(data: read [1.0, 2.0], shape: read [2])?
    local fail = Tensor.add(a: read a, b: read bad)
    match fail {
        Ok(_) => {
            Log.write(message: read "ok")
        }
        Err(error) => {
            Log.write(message: read TensorError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-broadcast-binary.rss",
        "rsscript_parity_tensor_broadcast_binary",
        source,
    );
}

#[test]
fn parity_tensor_comparisons() {
    let source = r#"
features: native, local

fn dump(t: read Tensor) -> Result<Unit, TensorError> {
    Log.write(message: read String.from_int(value: Tensor.dtype_code(t: read t)))
    let values = Tensor.to_f32_slice(tensor: read t)?
    let mut index = 0
    while index < List.len(list: read values) {
        Log.write(message: read Float.to_string(value: read List.get(list: read values, index: index)))
        index = index + 1
    }
    return Ok(Unit)
}

fn main() -> Result<Unit, TensorError> {
    let a = Tensor.from_f32_slice(data: read [1.0, 5.0, 3.0, 2.0], shape: read [4])?
    let b = Tensor.from_f32_slice(data: read [2.0, 5.0, 1.0, 2.0], shape: read [4])?
    let lt = Tensor.cmplt(a: read a, b: read b)?
    dump(t: read lt)?
    let ne = Tensor.cmpne(a: read a, b: read b)?
    dump(t: read ne)?
    let eq = Tensor.cmpeq(a: read a, b: read b)?
    dump(t: read eq)?
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-comparisons.rss",
        "rsscript_parity_tensor_comparisons",
        source,
    );
}

#[test]
fn parity_tensor_select() {
    let source = r#"
features: native, local

fn dump(t: read Tensor) -> Result<Unit, TensorError> {
    Log.write(message: read String.from_int(value: Tensor.dtype_code(t: read t)))
    let values = Tensor.to_f32_slice(tensor: read t)?
    let mut index = 0
    while index < List.len(list: read values) {
        Log.write(message: read Float.to_string(value: read List.get(list: read values, index: index)))
        index = index + 1
    }
    return Ok(Unit)
}

fn main() -> Result<Unit, TensorError> {
    let a = Tensor.from_f32_slice(data: read [1.0, 5.0, 3.0], shape: read [3])?
    let b = Tensor.from_f32_slice(data: read [2.0, 5.0, 1.0], shape: read [3])?
    let cond = Tensor.cmplt(a: read a, b: read b)?
    let sel = Tensor.select(cond: read cond, a: read a, b: read b)?
    dump(t: read sel)?
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-select.rss",
        "rsscript_parity_tensor_select",
        source,
    );
}

#[test]
fn parity_tensor_maximum_minimum() {
    let source = r#"
features: native, local

fn dump(t: read Tensor) -> Result<Unit, TensorError> {
    let values = Tensor.to_f32_slice(tensor: read t)?
    let mut index = 0
    while index < List.len(list: read values) {
        Log.write(message: read Float.to_string(value: read List.get(list: read values, index: index)))
        index = index + 1
    }
    return Ok(Unit)
}

fn main() -> Result<Unit, TensorError> {
    let a = Tensor.from_f32_slice(data: read [1.0, 5.0, 3.0, 2.0], shape: read [2, 2])?
    let b = Tensor.from_f32_slice(data: read [2.0, 5.0, 1.0, 9.0], shape: read [2, 2])?
    let mx = Tensor.maximum(a: read a, b: read b)?
    dump(t: read mx)?
    let mn = Tensor.minimum(a: read a, b: read b)?
    dump(t: read mn)?
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-maximum-minimum.rss",
        "rsscript_parity_tensor_maximum_minimum",
        source,
    );
}

#[test]
fn parity_tensor_casts() {
    let source = r#"
features: native, local

fn dump(t: read Tensor) -> Result<Unit, TensorError> {
    Log.write(message: read String.from_int(value: Tensor.dtype_code(t: read t)))
    let values = Tensor.to_f32_slice(tensor: read t)?
    let mut index = 0
    while index < List.len(list: read values) {
        Log.write(message: read Float.to_string(value: read List.get(list: read values, index: index)))
        index = index + 1
    }
    return Ok(Unit)
}

fn main() -> Result<Unit, TensorError> {
    let t = Tensor.from_f32_slice(data: read [1.7, 0.0, 0.0, 2.9], shape: read [2, 2])?
    let neg = Tensor.from_f32_slice(data: read [0.0, 0.0, 0.0, 0.0], shape: read [2, 2])?
    let mixed = Tensor.sub(a: read neg, b: read t)?
    // dtype_code of a fresh F32 tensor.
    Log.write(message: read String.from_int(value: Tensor.dtype_code(t: read t)))
    let as_i = Tensor.cast_i32(t: read mixed)
    dump(t: read as_i)?
    let as_b = Tensor.cast_bool(t: read t)
    dump(t: read as_b)?
    let back_f = Tensor.cast_f32(t: read as_i)
    dump(t: read back_f)?
    // I32 + I32 -> I32 (dtype promotion check).
    let sum_i = Tensor.add(a: read as_i, b: read as_i)?
    Log.write(message: read String.from_int(value: Tensor.dtype_code(t: read sum_i)))
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-casts.rss",
        "rsscript_parity_tensor_casts",
        source,
    );
}

// movement+gather (ops B)

#[test]
fn parity_tensor_pad() {
    let source = r#"
features: native, local

fn dump(t: read Tensor) -> Result<Unit, TensorError> {
    let dims = Tensor.shape(tensor: read t)?
    let mut d = 0
    while d < List.len(list: read dims) {
        Log.write(message: read String.from_int(value: List.get(list: read dims, index: d)))
        d = d + 1
    }
    let values = Tensor.to_f32_slice(tensor: read t)?
    let mut index = 0
    while index < List.len(list: read values) {
        Log.write(message: read Float.to_string(value: read List.get(list: read values, index: index)))
        index = index + 1
    }
    return Ok(Unit)
}

fn main() -> Result<Unit, TensorError> {
    let t = Tensor.from_f32_slice(data: read [1.0, 2.0, 3.0, 4.0], shape: read [2, 2])?
    local pads = List<Int>.new()
    List.push<Int>(list: mut pads, value: read 1)
    List.push<Int>(list: mut pads, value: read 0)
    List.push<Int>(list: mut pads, value: read 0)
    List.push<Int>(list: mut pads, value: read 2)
    let p = Tensor.pad(t: read t, pads: read pads)?
    dump(t: read p)?
    // wrong-length pads surfaces an error identically.
    local short = List<Int>.new()
    List.push<Int>(list: mut short, value: read 1)
    local bad = Tensor.pad(t: read t, pads: read short)
    match bad {
        Ok(_) => {
            Log.write(message: read "ok")
        }
        Err(error) => {
            Log.write(message: read TensorError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-pad.rss",
        "rsscript_parity_tensor_pad",
        source,
    );
}

#[test]
fn parity_tensor_shrink() {
    let source = r#"
features: native, local

fn dump(t: read Tensor) -> Result<Unit, TensorError> {
    let dims = Tensor.shape(tensor: read t)?
    let mut d = 0
    while d < List.len(list: read dims) {
        Log.write(message: read String.from_int(value: List.get(list: read dims, index: d)))
        d = d + 1
    }
    let values = Tensor.to_f32_slice(tensor: read t)?
    let mut index = 0
    while index < List.len(list: read values) {
        Log.write(message: read Float.to_string(value: read List.get(list: read values, index: index)))
        index = index + 1
    }
    return Ok(Unit)
}

fn main() -> Result<Unit, TensorError> {
    let t = Tensor.from_f32_slice(data: read [1.0, 2.0, 3.0, 4.0, 5.0, 6.0], shape: read [2, 3])?
    local bounds = List<Int>.new()
    List.push<Int>(list: mut bounds, value: read 0)
    List.push<Int>(list: mut bounds, value: read 1)
    List.push<Int>(list: mut bounds, value: read 1)
    List.push<Int>(list: mut bounds, value: read 3)
    let s = Tensor.shrink(t: read t, bounds: read bounds)?
    dump(t: read s)?
    // out-of-range bounds surfaces an error identically.
    local bad_bounds = List<Int>.new()
    List.push<Int>(list: mut bad_bounds, value: read 0)
    List.push<Int>(list: mut bad_bounds, value: read 5)
    List.push<Int>(list: mut bad_bounds, value: read 0)
    List.push<Int>(list: mut bad_bounds, value: read 3)
    local bad = Tensor.shrink(t: read t, bounds: read bad_bounds)
    match bad {
        Ok(_) => {
            Log.write(message: read "ok")
        }
        Err(error) => {
            Log.write(message: read TensorError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-shrink.rss",
        "rsscript_parity_tensor_shrink",
        source,
    );
}

#[test]
fn parity_tensor_flip() {
    let source = r#"
features: native, local

fn dump(t: read Tensor) -> Result<Unit, TensorError> {
    let values = Tensor.to_f32_slice(tensor: read t)?
    let mut index = 0
    while index < List.len(list: read values) {
        Log.write(message: read Float.to_string(value: read List.get(list: read values, index: index)))
        index = index + 1
    }
    return Ok(Unit)
}

fn main() -> Result<Unit, TensorError> {
    let t = Tensor.from_f32_slice(data: read [1.0, 2.0, 3.0, 4.0, 5.0, 6.0], shape: read [2, 3])?
    local axes = List<Int>.new()
    List.push<Int>(list: mut axes, value: read 1)
    let f1 = Tensor.flip(t: read t, axes: read axes)?
    dump(t: read f1)?
    local both = List<Int>.new()
    List.push<Int>(list: mut both, value: read 0)
    List.push<Int>(list: mut both, value: read 1)
    let fb = Tensor.flip(t: read t, axes: read both)?
    dump(t: read fb)?
    // duplicate axis surfaces an error identically.
    local dup = List<Int>.new()
    List.push<Int>(list: mut dup, value: read 0)
    List.push<Int>(list: mut dup, value: read 0)
    local bad = Tensor.flip(t: read t, axes: read dup)
    match bad {
        Ok(_) => {
            Log.write(message: read "ok")
        }
        Err(error) => {
            Log.write(message: read TensorError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-flip.rss",
        "rsscript_parity_tensor_flip",
        source,
    );
}

#[test]
fn parity_tensor_gather() {
    let source = r#"
features: native, local

fn dump(t: read Tensor) -> Result<Unit, TensorError> {
    Log.write(message: read String.from_int(value: Tensor.dtype_code(t: read t)))
    let dims = Tensor.shape(tensor: read t)?
    let mut d = 0
    while d < List.len(list: read dims) {
        Log.write(message: read String.from_int(value: List.get(list: read dims, index: d)))
        d = d + 1
    }
    let values = Tensor.to_f32_slice(tensor: read t)?
    let mut index = 0
    while index < List.len(list: read values) {
        Log.write(message: read Float.to_string(value: read List.get(list: read values, index: index)))
        index = index + 1
    }
    return Ok(Unit)
}

fn main() -> Result<Unit, TensorError> {
    let t = Tensor.from_f32_slice(data: read [1.0, 2.0, 3.0, 4.0, 5.0, 6.0], shape: read [2, 3])?
    let idx_f = Tensor.from_f32_slice(data: read [1.0, 0.0, 1.0], shape: read [3])?
    let idx = Tensor.cast_i32(t: read idx_f)
    let g0 = Tensor.gather(data: read t, axis: 0, indices: read idx)?
    dump(t: read g0)?
    let cidx_f = Tensor.from_f32_slice(data: read [2.0, 0.0], shape: read [2])?
    let cidx = Tensor.cast_i32(t: read cidx_f)
    let g1 = Tensor.gather(data: read t, axis: 1, indices: read cidx)?
    dump(t: read g1)?
    // out-of-bounds index surfaces an error identically.
    let bidx_f = Tensor.from_f32_slice(data: read [9.0], shape: read [1])?
    let bidx = Tensor.cast_i32(t: read bidx_f)
    local bad = Tensor.gather(data: read t, axis: 0, indices: read bidx)
    match bad {
        Ok(_) => {
            Log.write(message: read "ok")
        }
        Err(error) => {
            Log.write(message: read TensorError.message(error: read error))
        }
    }
    return Ok(Unit)
}
"#;
    common::assert_vm_eval_matches_backend(
        "parity-tensor-gather.rss",
        "rsscript_parity_tensor_gather",
        source,
    );
}
