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
