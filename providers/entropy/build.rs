use rsscript_provider_bindgen::{
    GeneratedBlocking, GeneratedCancellation, GeneratedCleanup, ProviderInterface,
    RustProviderOptions,
};
use rsscript_semantics::InterfaceDescriptorV1;
use std::{env, fs, path::PathBuf};
fn main() {
    println!("cargo:rerun-if-changed=interface/lib.rssi");
    let descriptor = InterfaceDescriptorV1::from_interface_source(
        "interface/lib.rssi",
        include_str!("interface/lib.rssi"),
    )
    .expect("valid entropy Provider interface");
    let interface =
        ProviderInterface::from_descriptor(descriptor).expect("supported entropy descriptor");
    let generated = interface.render_rust(&RustProviderOptions {
        provider_id: "rsscript.entropy",
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
