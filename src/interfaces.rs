pub(crate) const CORE_INTERFACES: &[(&str, &str)] = &[
    (
        "core/cache/image_cache.rssi",
        include_str!("../core/cache/image_cache.rssi"),
    ),
    (
        "core/collections/buffer.rssi",
        include_str!("../core/collections/buffer.rssi"),
    ),
    (
        "core/collections/list.rssi",
        include_str!("../core/collections/list.rssi"),
    ),
    (
        "core/collections/map.rssi",
        include_str!("../core/collections/map.rssi"),
    ),
    (
        "core/config/config.rssi",
        include_str!("../core/config/config.rssi"),
    ),
    (
        "core/counter/counter.rssi",
        include_str!("../core/counter/counter.rssi"),
    ),
    ("core/csv/csv.rssi", include_str!("../core/csv/csv.rssi")),
    ("core/db/db.rssi", include_str!("../core/db/db.rssi")),
    ("core/fs/file.rssi", include_str!("../core/fs/file.rssi")),
    (
        "core/http/http.rssi",
        include_str!("../core/http/http.rssi"),
    ),
    (
        "core/interpreter/interpreter.rssi",
        include_str!("../core/interpreter/interpreter.rssi"),
    ),
    (
        "core/image/image.rssi",
        include_str!("../core/image/image.rssi"),
    ),
    (
        "core/json/json.rssi",
        include_str!("../core/json/json.rssi"),
    ),
    ("core/log/log.rssi", include_str!("../core/log/log.rssi")),
    ("core/os/os.rssi", include_str!("../core/os/os.rssi")),
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
