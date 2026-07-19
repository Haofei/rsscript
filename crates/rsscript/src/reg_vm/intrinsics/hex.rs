use super::super::*;
use crate::reg_vm::runtime_values::*;
use crate::reg_vm::value_access::*;
use crate::reg_vm::value_convert::*;
use crate::reg_vm::value_ops::*;

impl RegVm {
    pub(super) fn exec_hex_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = unit;
        let _ = next_base;
        match intrinsic {
            RegIntrinsic::HexDecode => {
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = json_result(
                    hex::decode(text)
                        .map(|bytes| VmValue::Bytes(Rc::new(bytes)))
                        .map_err(|error| decode_error_value(error.to_string())),
                );
                self.account_fresh_value_storage(&result)?;
                Ok(result)
            }
            RegIntrinsic::HexEncode => {
                let value = expect_bytes_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.fresh_string(hex::encode(value))
            }
            RegIntrinsic::HexEncodeString => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.fresh_string(hex::encode(value.as_bytes()))
            }
            other => unreachable!("exec_hex_intrinsics called with non-hex intrinsic: {other:?}"),
        }
    }
}
