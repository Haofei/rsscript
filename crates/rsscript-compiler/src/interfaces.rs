//! Compatibility façade over the platform-neutral interface catalog.

pub(crate) use rsscript_interface_catalog::{CORE_INTERFACES, STANDARD_PACKAGE_INTERFACES};

#[cfg(feature = "execution")]
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

#[cfg(feature = "execution")]
pub(crate) use rsscript_interface_catalog::builtin_interfaces;

#[cfg(not(test))]
pub(crate) use rsscript_interface_catalog::default_interfaces;
#[cfg(all(not(test), feature = "execution"))]
pub(crate) use rsscript_interface_catalog::standard_package_interfaces;

#[cfg(test)]
pub(crate) fn default_interfaces() -> impl Iterator<Item = (&'static str, &'static str)> {
    rsscript_interface_catalog::CORE_INTERFACES
        .iter()
        .chain(rsscript_interface_catalog::STANDARD_PACKAGE_INTERFACES.iter())
        .chain(crate::test_interfaces::TEST_INTERFACES.iter())
        .copied()
}

#[cfg(test)]
pub(crate) fn standard_package_interfaces() -> impl Iterator<Item = (&'static str, &'static str)> {
    rsscript_interface_catalog::STANDARD_PACKAGE_INTERFACES
        .iter()
        .chain(crate::test_interfaces::TEST_INTERFACES.iter())
        .copied()
}
