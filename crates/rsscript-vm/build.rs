fn main() {
    if let Err(error) = rsscript_build_support::write_reg_vm_runtime_intrinsics() {
        panic!("{error}");
    }
}
