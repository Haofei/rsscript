use std::env;
use std::fs;
use std::path::PathBuf;

use rsscript_provider_bindgen::{
    GeneratedBlocking, GeneratedCancellation, GeneratedCleanup, ProviderInterface,
    RustProviderOptions,
};

fn main() {
    println!("cargo:rerun-if-changed=interface/lib.rssi");
    let interface =
        ProviderInterface::parse("interface/lib.rssi", include_str!("interface/lib.rssi"))
            .expect("valid environment Provider interface");
    let generated = interface.render_rust(&RustProviderOptions {
        provider_id: "rsscript.env",
        blocking: GeneratedBlocking::NonBlocking,
        cancellation: GeneratedCancellation::NotApplicable,
        thread_safe: true,
        reentrant: true,
        cleanup: GeneratedCleanup::None,
    });
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("provider_contract.rs");
    fs::write(output, generated).expect("write generated Provider contract");
}
