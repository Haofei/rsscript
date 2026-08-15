pub(crate) struct RuntimeIntrinsic {
    pub(crate) managed_handle_args: &'static [&'static str],
    namespace: &'static str,
    name: &'static str,
}

const fn runtime_intrinsic(namespace: &'static str, name: &'static str) -> RuntimeIntrinsic {
    RuntimeIntrinsic {
        namespace,
        name,
        managed_handle_args: &[],
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
    use crate::interfaces::default_interfaces;
    use crate::syntax::ast::Item;
    use crate::syntax::parse_source;
    use std::collections::{HashMap, HashSet};

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

    #[test]
    fn runtime_intrinsic_managed_handle_args_match_core_params() {
        let public_functions = core_public_function_params();
        for intrinsic in RUNTIME_INTRINSICS {
            if intrinsic.managed_handle_args.is_empty() {
                continue;
            }
            let signature = format!("{}.{}", intrinsic.namespace, intrinsic.name);
            let params = public_functions.get(&signature).unwrap_or_else(|| {
                panic!("runtime intrinsic `{signature}` should have core params")
            });
            for arg in intrinsic.managed_handle_args {
                assert!(
                    params.contains(*arg),
                    "runtime intrinsic `{signature}` declares managed handle arg `{arg}`, but core params are {params:?}"
                );
            }
        }
    }

    fn core_public_function_params() -> HashMap<String, HashSet<String>> {
        let mut public_functions = HashMap::new();
        for (path, source) in default_interfaces() {
            let program = parse_source(path, source);
            let protocol_names = program
                .protocols
                .iter()
                .map(|protocol| protocol.name.as_str())
                .collect::<HashSet<_>>();
            for item in program.items {
                if let Item::Function(function) = item
                    && (function.is_public
                        || function
                            .name
                            .rsplit_once('.')
                            .is_some_and(|(namespace, _)| protocol_names.contains(namespace)))
                {
                    public_functions.insert(
                        function.name,
                        function
                            .params
                            .into_iter()
                            .map(|param| param.name)
                            .collect(),
                    );
                }
            }
        }
        public_functions
    }
}
