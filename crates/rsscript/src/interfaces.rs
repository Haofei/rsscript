//! Compatibility façade over the platform-neutral interface catalog.

pub(crate) use rsscript_interface_catalog::{
    CORE_INTERFACES, STANDARD_PACKAGE_INTERFACES, builtin_interfaces,
};

#[cfg(not(test))]
pub(crate) use rsscript_interface_catalog::{default_interfaces, standard_package_interfaces};

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
