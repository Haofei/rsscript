//! Read-only runtime-intrinsic identity used for package async evidence.
//!
//! The table is generated from the shared intrinsic catalog. It deliberately
//! answers only whether a qualified name is a runtime intrinsic; it neither
//! links nor executes any runtime implementation.

struct RuntimeIntrinsic {
    namespace: &'static str,
    name: &'static str,
}

const fn runtime_intrinsic(namespace: &'static str, name: &'static str) -> RuntimeIntrinsic {
    RuntimeIntrinsic { namespace, name }
}

fn lookup_runtime_intrinsic(namespace: &str, name: &str) -> Option<&'static RuntimeIntrinsic> {
    RUNTIME_INTRINSICS
        .iter()
        .find(|intrinsic| intrinsic.namespace == namespace && intrinsic.name == name)
}

include!(concat!(env!("OUT_DIR"), "/rss-runtime-intrinsics.rs"));

pub fn is_runtime_intrinsic(callee: &str) -> bool {
    let Some((namespace, name)) = callee.rsplit_once('.') else {
        return false;
    };
    lookup_runtime_intrinsic(namespace, name).is_some()
}

#[cfg(test)]
mod tests {
    use super::is_runtime_intrinsic;

    #[test]
    fn generated_catalog_recognizes_runtime_intrinsic_keys() {
        assert!(is_runtime_intrinsic("Arguments.all"));
        assert!(!is_runtime_intrinsic("Host.unlinked"));
    }
}
