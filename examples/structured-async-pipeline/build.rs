use rsscript_provider_bindgen::{
    GeneratedBlocking, GeneratedCancellation, GeneratedCleanup, ProviderInterface,
    RustProviderOptions,
};
use rsscript_semantics::InterfaceDescriptorV1;
use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=interfaces/session.rssi");
    let descriptor = InterfaceDescriptorV1::from_interface_source(
        "interfaces/session.rssi",
        include_str!("interfaces/session.rssi"),
    )
    .expect("valid structured-async session interface");
    let interface = ProviderInterface::from_descriptor(descriptor)
        .expect("session interface must be bindgen-compatible");
    let generated = interface.render_rust(&RustProviderOptions {
        provider_id: "example.structured-async.session",
        blocking: GeneratedBlocking::NonBlocking,
        cancellation: GeneratedCancellation::Cooperative,
        thread_safe: true,
        reentrant: true,
        cleanup: GeneratedCleanup::RuntimeRegistered,
    });
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("provider_contract.rs"),
        generated,
    )
    .expect("write generated session Provider contract");
}
