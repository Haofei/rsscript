pub(crate) const TEST_INTERFACES: &[(&str, &str)] = &[
    (
        "test/log.rssi",
        include_str!("../../../stdlib/log/log.rssi"),
    ),
    ("test/args.rssi", include_str!("../../../stdlib/os/os.rssi")),
];
