use rsscript_provider_bindgen::{
    GeneratedBlocking, GeneratedCancellation, GeneratedCleanup, ProviderInterface,
    RustProviderOptions,
};
use std::{env, fs, path::PathBuf};
fn main() {
    println!("cargo:rerun-if-changed=interface/lib.rssi");
    let interface =
        ProviderInterface::parse("interface/lib.rssi", include_str!("interface/lib.rssi"))
            .expect("valid CLI Provider interface");
    let generated = interface.render_rust(&RustProviderOptions {
        provider_id: "rsscript.cli",
        blocking: GeneratedBlocking::NonBlocking,
        cancellation: GeneratedCancellation::NotApplicable,
        thread_safe: true,
        reentrant: true,
        cleanup: GeneratedCleanup::None,
    });
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("provider_contract.rs"),
        generated,
    )
    .expect("write generated Provider contract");
}
