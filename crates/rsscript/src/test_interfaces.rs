pub(crate) const TEST_INTERFACES: &[(&str, &str)] = &[
    (
        "test/output.rssi",
        include_str!("../../../stdlib/output/output.rssi"),
    ),
    ("test/args.rssi", include_str!("../../../stdlib/os/os.rssi")),
];
