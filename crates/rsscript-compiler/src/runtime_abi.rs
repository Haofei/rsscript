pub(crate) struct RuntimeIntrinsic {
    namespace: &'static str,
    name: &'static str,
}

const fn runtime_intrinsic(namespace: &'static str, name: &'static str) -> RuntimeIntrinsic {
    RuntimeIntrinsic {
        namespace,
        name,
    }
}

pub(crate) fn lookup_runtime_intrinsic(
    namespace: &str,
    name: &str,
) -> Option<&'static RuntimeIntrinsic> {
    RUNTIME_INTRINSICS
        .iter()
        .find(|intrinsic| intrinsic.namespace == namespace && intrinsic.name == name)
}

include!(concat!(env!("OUT_DIR"), "/rss-runtime-intrinsics.rs"));

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::RUNTIME_INTRINSICS;

    #[test]
    fn runtime_intrinsic_keys_are_unique() {
        let mut seen = HashSet::new();
        for intrinsic in RUNTIME_INTRINSICS {
            assert!(
                seen.insert((intrinsic.namespace, intrinsic.name)),
                "duplicate runtime intrinsic {}.{}",
                intrinsic.namespace,
                intrinsic.name
            );
        }
    }
}
