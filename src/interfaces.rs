pub(crate) const CORE_INTERFACES: &[(&str, &str)] = &[
    (
        "core/collections/map.rssi",
        include_str!("../core/collections/map.rssi"),
    ),
    ("core/fs/file.rssi", include_str!("../core/fs/file.rssi")),
    (
        "core/image/image.rssi",
        include_str!("../core/image/image.rssi"),
    ),
    (
        "core/json/json.rssi",
        include_str!("../core/json/json.rssi"),
    ),
    ("core/log/log.rssi", include_str!("../core/log/log.rssi")),
    (
        "core/resource/resource_pool.rssi",
        include_str!("../core/resource/resource_pool.rssi"),
    ),
    (
        "core/string/string.rssi",
        include_str!("../core/string/string.rssi"),
    ),
    (
        "core/test/assert.rssi",
        include_str!("../core/test/assert.rssi"),
    ),
];

pub(crate) const PROTOTYPE_INTERFACES: &[(&str, &str)] = &[(
    "core/prototype/builtins.rssi",
    include_str!("../core/prototype/builtins.rssi"),
)];

pub(crate) fn builtin_interfaces() -> impl Iterator<Item = (&'static str, &'static str)> {
    CORE_INTERFACES
        .iter()
        .chain(PROTOTYPE_INTERFACES.iter())
        .copied()
}
