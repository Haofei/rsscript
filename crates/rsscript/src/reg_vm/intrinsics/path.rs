use super::super::*;
use crate::reg_vm::runtime_values::*;
use crate::reg_vm::value_access::*;
use crate::reg_vm::value_convert::*;
use crate::reg_vm::value_ops::*;

impl RegVm {
    #[allow(clippy::mutable_key_type)]
    pub(super) fn exec_path_intrinsics(
        &mut self,
        unit: &RegUnit,
        intrinsic: RegIntrinsic,
        args: &[Reg],
        base: usize,
        next_base: usize,
    ) -> Result<VmValue, EvalError> {
        let _ = next_base;
        let _ = unit;
        match intrinsic {
            RegIntrinsic::PathExists => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(Path::new(path).exists()))
            }
            RegIntrinsic::PathExtension => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = Path::new(path)
                    .extension()
                    .map(|extension| VmValue::some(VmValue::string(extension.to_string_lossy())))
                    .unwrap_or(VmValue::OptionNone);
                self.account_fresh_value_storage(&result)?;
                Ok(result)
            }
            RegIntrinsic::PathFileName => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = Path::new(path)
                    .file_name()
                    .map(|name| VmValue::some(VmValue::string(name.to_string_lossy())))
                    .unwrap_or(VmValue::OptionNone);
                self.account_fresh_value_storage(&result)?;
                Ok(result)
            }
            RegIntrinsic::PathFromString | RegIntrinsic::PathToString => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.fresh_string(value.to_string())
            }
            RegIntrinsic::PathIsAbsolute => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(Path::new(path).is_absolute()))
            }
            RegIntrinsic::PathIsDir => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(Path::new(path).is_dir()))
            }
            RegIntrinsic::PathIsFile => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                Ok(VmValue::Bool(Path::new(path).is_file()))
            }
            RegIntrinsic::PathJoin => {
                let base_path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let child = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                self.fresh_string(path_join_string(base_path, child))
            }
            RegIntrinsic::PathListFiles => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = json_result(
                    directory_list_files(Path::new(path))
                        .map(|files| {
                            VmValue::List(Rc::new(RefCell::new(
                                files.into_iter().map(VmValue::string).collect(),
                            )))
                        })
                        .map_err(|error| file_error_value(error.to_string())),
                );
                self.account_fresh_value_storage(&result)?;
                Ok(result)
            }
            RegIntrinsic::PathListPaths => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = json_result(
                    directory_list_paths(Path::new(path))
                        .map(|paths| {
                            VmValue::List(Rc::new(RefCell::new(
                                paths
                                    .into_iter()
                                    .map(|path| VmValue::string(path.to_string_lossy()))
                                    .collect(),
                            )))
                        })
                        .map_err(|error| file_error_value(error.to_string())),
                );
                self.account_fresh_value_storage(&result)?;
                Ok(result)
            }
            RegIntrinsic::PathNormalize => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                self.fresh_string(path_normalize_string(path))
            }
            RegIntrinsic::PathParent => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = Path::new(path)
                    .parent()
                    .map(|parent| VmValue::some(VmValue::string(parent.to_string_lossy())))
                    .unwrap_or(VmValue::OptionNone);
                self.account_fresh_value_storage(&result)?;
                Ok(result)
            }
            RegIntrinsic::PathReadString => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = json_result(
                    std::fs::read_to_string(path)
                        .map(VmValue::string)
                        .map_err(|error| file_error_value(error.to_string())),
                );
                self.account_fresh_value_storage(&result)?;
                Ok(result)
            }
            RegIntrinsic::PathResolveRelative => {
                let root = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let relative = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let result = json_result(
                    path_resolve_relative_string(root, relative)
                        .map(VmValue::string)
                        .map_err(VmValue::string),
                );
                self.account_fresh_value_storage(&result)?;
                Ok(result)
            }
            RegIntrinsic::PathSafeRelative => {
                let value = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let result = json_result(
                    path_safe_relative_string(value)
                        .map(VmValue::string)
                        .map_err(VmValue::string),
                );
                self.account_fresh_value_storage(&result)?;
                Ok(result)
            }
            RegIntrinsic::PathStartsWith => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let base_path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(VmValue::Bool(
                    Path::new(path).starts_with(Path::new(base_path)),
                ))
            }
            RegIntrinsic::PathWithExtension => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let extension = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                let mut path = PathBuf::from(path);
                path.set_extension(extension);
                self.fresh_string(path.to_string_lossy().into_owned())
            }
            RegIntrinsic::PathWriteString => {
                let path = expect_string_ref(intrinsic_arg(&self.stack, base, args, 0)?)?;
                let text = expect_string_ref(intrinsic_arg(&self.stack, base, args, 1)?)?;
                Ok(file_result_unit(std::fs::write(path, text)))
            }
            other => unreachable!("exec_path_intrinsics called with non-path intrinsic: {other:?}"),
        }
    }
}
