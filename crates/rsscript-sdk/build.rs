fn main() {
    rsscript_build_support::write_compiled_cache_fingerprint()
        .expect("failed to emit the SDK executable-cache fingerprint");
}
