fn main() {
    rsscript_build_support::write_aot_runtime_intrinsics()
        .expect("AOT runtime intrinsic table should generate");
}
