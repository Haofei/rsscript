fn main() {
    if let Err(error) = rsscript_build_support::write_mir_builtin_catalog() {
        panic!("{error}");
    }
}
