#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::sync::{Arc, Mutex};

use rsscript_compiler::provider_api::{
    ExternalSymbol, NativeInterpreterFn, NativeValue, ProviderError, ProviderErrorCode,
    ProviderFunction,
};
use rsscript_compiler::{
    analysis::SemanticDiffV1,
    artifact::ArtifactVerifier,
    compile::{Compiler, FrontendInputSnapshot},
    provider_api::ProviderRegistry,
    runtime::{ExecutionRequest, RunLimits, Runtime, TracePolicy},
};
use rsscript_semantics::InterfaceDescriptorV1;
use sha2::{Digest, Sha256};

const SOURCE: &str = include_str!("../script/main.rss");
const FS_INTERFACE: &str = include_str!("../interfaces/fs.rssi");
const LOG_INTERFACE: &str = include_str!("../interfaces/log.rssi");

type ProviderFunctions = BTreeMap<ExternalSymbol, ProviderFunction<NativeInterpreterFn>>;

fn memory_fs(files: Arc<Mutex<BTreeMap<String, String>>>) -> ProviderFunctions {
    let descriptor = rsscript_provider_fs::descriptor();
    descriptor
        .functions
        .iter()
        .map(|function| {
            let symbol = function.symbol.clone();
            let signature = function.signature.clone();
            let files = Arc::clone(&files);
            let callable = match symbol.as_str() {
                "host.fs.read_text" => NativeInterpreterFn::new(move |mut args| {
                    let NativeValue::String(path) = args.remove(0) else {
                        return Err(ProviderError::invalid_argument("path must be String"));
                    };
                    files
                        .lock()
                        .map_err(|_| ProviderError::internal("memory filesystem lock poisoned"))?
                        .get(&path)
                        .cloned()
                        .map(NativeValue::String)
                        .ok_or_else(|| {
                            ProviderError::new(
                                ProviderErrorCode::NotFound,
                                format!("missing memory file: {path}"),
                            )
                        })
                }),
                "host.fs.write_text" => NativeInterpreterFn::new(move |mut args| {
                    let NativeValue::String(path) = args.remove(0) else {
                        return Err(ProviderError::invalid_argument("path must be String"));
                    };
                    let NativeValue::String(text) = args.remove(0) else {
                        return Err(ProviderError::invalid_argument("text must be String"));
                    };
                    files
                        .lock()
                        .map_err(|_| ProviderError::internal("memory filesystem lock poisoned"))?
                        .insert(path, text);
                    Ok(NativeValue::Unit)
                }),
                unexpected => panic!("unexpected filesystem symbol: {unexpected}"),
            };
            (
                symbol,
                ProviderFunction {
                    signature,
                    callable,
                },
            )
        })
        .collect()
}

fn registry(fs_functions: ProviderFunctions, log_functions: ProviderFunctions) -> ProviderRegistry {
    let mut providers = ProviderRegistry::default();
    providers
        .register(&rsscript_provider_fs::descriptor(), fs_functions)
        .expect("filesystem provider should link");
    providers
        .register(&rsscript_provider_log::descriptor(), log_functions)
        .expect("log provider should link");
    providers
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let compiler = Compiler;
    let input = FrontendInputSnapshot::from_sources(
        [("main.rss", SOURCE)],
        [("fs.rssi", FS_INTERFACE), ("log.rssi", LOG_INTERFACE)],
    );
    let package = compiler.compile_snapshot(&input)?;
    let bundle_before_providers = package.bundle_bytes()?;
    let artifact_hash = format!("{:x}", Sha256::digest(&bundle_before_providers));
    let fs_descriptor = InterfaceDescriptorV1::from_interface_source("fs.rssi", FS_INTERFACE)
        .map_err(|error| std::io::Error::other(format!("invalid fs interface: {error:?}")))?;
    let log_descriptor = InterfaceDescriptorV1::from_interface_source("log.rssi", LOG_INTERFACE)
        .map_err(|error| std::io::Error::other(format!("invalid log interface: {error:?}")))?;
    let descriptor_hash = format!(
        "{:x}",
        Sha256::digest(
            [
                fs_descriptor.to_json_bytes()?,
                log_descriptor.to_json_bytes()?
            ]
            .concat()
        )
    );
    let unchanged = SemanticDiffV1::between(package.bundle(), package.bundle());
    assert!(
        unchanged.summary.is_empty()
            && unchanged.imports.added.is_empty()
            && unchanged.imports.removed.is_empty()
            && unchanged.imports.changed.is_empty()
            && unchanged.external_contracts.added.is_empty()
            && unchanged.external_contracts.removed.is_empty()
            && unchanged.external_contracts.changed.is_empty(),
        "an Artifact compared with itself must have no semantic evidence changes"
    );
    let verified = ArtifactVerifier.verify(package)?;

    let memory_files = Arc::new(Mutex::new(BTreeMap::from([(
        "input.csv".to_string(),
        "name,total\nalice,42\n".to_string(),
    )])));
    let memory_log = Arc::new(Mutex::new(Vec::<String>::new()));
    let captured_log = Arc::clone(&memory_log);
    let memory_runtime = Runtime::new(registry(
        memory_fs(Arc::clone(&memory_files)),
        rsscript_provider_log::functions(move |message| {
            captured_log
                .lock()
                .map_err(|_| ProviderError::internal("memory log lock poisoned"))?
                .push(message.to_string());
            Ok(())
        }),
    ));
    let memory_execution = memory_runtime.link(&verified)?.execute(
        ExecutionRequest::default()
            .limits(RunLimits::bounded().allow_blocking_provider_calls(true))
            .trace(TracePolicy::MetadataOnly),
    );
    if let Some(error) = memory_execution.failure {
        return Err(error.to_string().into());
    }

    let memory_report = memory_files
        .lock()
        .map_err(|_| "memory filesystem lock poisoned")?
        .get("report.txt")
        .cloned()
        .ok_or("memory provider did not create report.txt")?;

    let demo_dir = std::env::temp_dir().join(format!("rsscript-report-{}", std::process::id()));
    fs::create_dir_all(&demo_dir)?;
    fs::write(demo_dir.join("input.csv"), "name,total\nbob,7\n")?;
    let disk_provider = rsscript_provider_fs::RootedFsProvider::new(&demo_dir)?;
    let production_runtime = Runtime::new(registry(
        disk_provider.functions(),
        rsscript_provider_log::stderr_functions(),
    ));
    let production_report = production_runtime.link(&verified)?.execute(
        ExecutionRequest::default()
            .limits(RunLimits::bounded().allow_blocking_provider_calls(true)),
    );
    if let Some(error) = production_report.failure {
        return Err(error.to_string().into());
    }
    let disk_report = fs::read_to_string(demo_dir.join("report.txt"))?;
    fs::remove_dir_all(&demo_dir)?;

    assert_eq!(verified.bundle().to_bytes()?, bundle_before_providers);
    println!("artifact sha256: {artifact_hash}");
    println!("interface descriptor sha256: {descriptor_hash}");
    println!(
        "semantic diff schema: {} (self diff: empty)",
        unchanged.schema
    );
    println!("imports: {}", verified.external_imports().len());
    println!("memory provider report:\n{memory_report}");
    println!("filesystem provider report:\n{disk_report}");
    Ok(())
}
