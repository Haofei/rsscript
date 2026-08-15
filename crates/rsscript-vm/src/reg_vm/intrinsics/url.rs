use super::super::*;
use crate::reg_vm::runtime_values::*;
use crate::reg_vm::value_access::*;
use crate::reg_vm::value_convert::*;

impl RegVm {
    pub(super) fn exec_url_intrinsics(
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
            RegIntrinsic::UrlDecodeComponent => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = json_result(
                    url_decode_component(value)
                        .map(VmValue::string)
                        .map_err(decode_error_value),
                );
                self.account_fresh_value_storage(&result)?;
                Ok(result)
            }
            RegIntrinsic::UrlEncodeComponent => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.fresh_string(url_encode_component(value))
            }
            RegIntrinsic::UrlFromString | RegIntrinsic::UrlToString => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.fresh_string(value.to_string())
            }
            other => unreachable!("exec_url_intrinsics called with non-url intrinsic: {other:?}"),
        }
    }
}
