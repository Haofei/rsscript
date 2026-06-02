pub fn core_package_index_json() -> &'static str {
    include_str!(concat!(env!("OUT_DIR"), "/rss-core-package-index.json"))
}
