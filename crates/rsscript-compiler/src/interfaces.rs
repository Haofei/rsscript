//! Compatibility façade over the platform-neutral interface catalog.

pub(crate) use rsscript_interface_catalog::{CORE_INTERFACES, STANDARD_PACKAGE_INTERFACES};

#[cfg(feature = "lowering")]
pub(crate) fn interface_catalog_digest() -> String {
    use sha2::{Digest, Sha256};

    let mut digest = Sha256::new();
    for (path, source) in CORE_INTERFACES
        .iter()
        .chain(STANDARD_PACKAGE_INTERFACES.iter())
    {
        digest.update((path.len() as u64).to_be_bytes());
        digest.update(path.as_bytes());
        digest.update((source.len() as u64).to_be_bytes());
        digest.update(source.as_bytes());
    }
    format!("sha256:{:x}", digest.finalize())
}
