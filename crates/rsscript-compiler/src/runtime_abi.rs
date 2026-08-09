pub(crate) struct RuntimeIntrinsic {
    pub(crate) rust_target: &'static str,
    pub(crate) managed_handle_args: &'static [&'static str],
    namespace: &'static str,
    name: &'static str,
}

const fn runtime_intrinsic(
    namespace: &'static str,
    name: &'static str,
    rust_target: &'static str,
) -> RuntimeIntrinsic {
    RuntimeIntrinsic {
        namespace,
        name,
        rust_target,
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

pub(crate) fn runtime_intrinsic_signatures() -> Vec<String> {
    RUNTIME_INTRINSICS
        .iter()
        .map(|intrinsic| format!("{}.{}", intrinsic.namespace, intrinsic.name))
        .collect()
}

pub(crate) fn runtime_intrinsic_supported_signatures() -> Vec<String> {
    let runtime_functions = runtime_public_function_names();
    RUNTIME_INTRINSICS
        .iter()
        .filter_map(|intrinsic| {
            let target = intrinsic.rust_target.strip_prefix("rsscript_runtime::")?;
            runtime_functions
                .contains(target)
                .then(|| format!("{}.{}", intrinsic.namespace, intrinsic.name))
        })
        .collect()
}

fn runtime_public_function_names() -> std::collections::HashSet<String> {
    let runtime_src =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../experiments/aot-runtime/src");
    let mut functions = std::collections::HashSet::new();
    for entry in std::fs::read_dir(&runtime_src).expect("runtime/src should be readable") {
        let path = entry.expect("runtime/src entry should be readable").path();
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).expect("runtime source should be readable");
        collect_runtime_public_functions(&source, &mut functions);
    }
    functions
}

fn collect_runtime_public_functions(
    source: &str,
    functions: &mut std::collections::HashSet<String>,
) {
    let bytes = source.as_bytes();
    let mut index = 0;
    while let Some(relative) = source[index..].find("pub ") {
        index += relative + "pub ".len();
        let mut cursor = skip_ascii_whitespace(bytes, index);
        if source[cursor..].starts_with("async ") {
            cursor += "async ".len();
            cursor = skip_ascii_whitespace(bytes, cursor);
        }
        if !source[cursor..].starts_with("fn ") {
            continue;
        }
        cursor += "fn ".len();
        cursor = skip_ascii_whitespace(bytes, cursor);
        let start = cursor;
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
        {
            cursor += 1;
        }
        if cursor > start {
            functions.insert(source[start..cursor].to_string());
        }
        index = cursor;
    }
}

fn skip_ascii_whitespace(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    index
}

include!(concat!(env!("OUT_DIR"), "/rss-runtime-intrinsics.rs"));

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::fs;
    use std::path::Path;

    use crate::interfaces::default_interfaces;
    use crate::syntax::ast::Item;
    use crate::syntax::parse_source;

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

    #[test]
    fn runtime_intrinsic_rust_targets_exist() {
        let runtime_functions = runtime_public_functions();
        for intrinsic in RUNTIME_INTRINSICS {
            let target = intrinsic
                .rust_target
                .strip_prefix("rsscript_runtime::")
                .expect("runtime intrinsic target should be in rsscript_runtime");
            assert!(
                runtime_functions.contains(target),
                "runtime intrinsic {}.{} points to missing Rust runtime target `{}`",
                intrinsic.namespace,
                intrinsic.name,
                intrinsic.rust_target
            );
        }
    }

    fn runtime_public_functions() -> HashSet<String> {
        let runtime_src =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../experiments/aot-runtime/src");
        let mut functions = HashSet::new();
        for entry in fs::read_dir(&runtime_src).expect("runtime/src should be readable") {
            let path = entry.expect("runtime/src entry should be readable").path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
                continue;
            }
            let source = fs::read_to_string(&path).expect("runtime source should be readable");
            collect_public_functions(&source, &mut functions);
        }
        functions
    }

    fn collect_public_functions(source: &str, functions: &mut HashSet<String>) {
        let bytes = source.as_bytes();
        let mut index = 0;
        while let Some(relative) = source[index..].find("pub ") {
            index += relative + "pub ".len();
            let mut cursor = skip_ascii_whitespace(bytes, index);
            if source[cursor..].starts_with("async ") {
                cursor += "async ".len();
                cursor = skip_ascii_whitespace(bytes, cursor);
            }
            if !source[cursor..].starts_with("fn ") {
                continue;
            }
            cursor += "fn ".len();
            cursor = skip_ascii_whitespace(bytes, cursor);
            let start = cursor;
            while cursor < bytes.len()
                && (bytes[cursor].is_ascii_alphanumeric() || bytes[cursor] == b'_')
            {
                cursor += 1;
            }
            if cursor > start {
                functions.insert(source[start..cursor].to_string());
            }
            index = cursor;
        }
    }

    fn skip_ascii_whitespace(bytes: &[u8], mut index: usize) -> usize {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        index
    }
}
