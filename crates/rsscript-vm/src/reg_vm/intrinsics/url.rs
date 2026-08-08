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
                    percent_decode_str(value)
                        .decode_utf8()
                        .map(|value| VmValue::string(value.to_string()))
                        .map_err(|error| decode_error_value(error.to_string())),
                );
                self.account_fresh_value_storage(&result)?;
                Ok(result)
            }
            RegIntrinsic::UrlEncodeComponent => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.fresh_string(utf8_percent_encode(value, URL_COMPONENT_SET).to_string())
            }
            RegIntrinsic::UrlFromString | RegIntrinsic::UrlToString => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.fresh_string(value.to_string())
            }
            other => unreachable!("exec_url_intrinsics called with non-url intrinsic: {other:?}"),
        }
    }
}
