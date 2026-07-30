// JSON decoding and normalization into typed adapter inputs.

pub fn review_map_input_from_json(
    package_name: &str,
    json: &str,
) -> Result<RsScriptReviewMapInput, serde_json::Error> {
    let value = serde_json::from_str(json)?;
    Ok(review_map_input_from_value(package_name, value))
}

pub fn package_review_input_from_json(
    json: &str,
) -> Result<RsScriptPackageReviewInput, serde_json::Error> {
    let value: Value = serde_json::from_str(json)?;
    Ok(package_review_input_from_value(&value))
}

pub fn package_lock_input_from_json(
    json: &str,
) -> Result<RsScriptPackageLockInput, serde_json::Error> {
    serde_json::from_str(json)
}

pub fn package_check_input_from_json(
    json: &str,
) -> Result<RsScriptPackageCheckInput, serde_json::Error> {
    let value: Value = serde_json::from_str(json)?;
    let mut input: RsScriptPackageCheckInput = serde_json::from_value(value.clone())?;
    input.diagnostics = package_diagnostics_from_json(&value);
    Ok(input)
}

pub fn package_lock_diff_input_from_json(
    json: &str,
) -> Result<RsScriptPackageLockDiffInput, serde_json::Error> {
    serde_json::from_str(json)
}

pub fn package_tree_input_from_json(
    json: &str,
) -> Result<RsScriptPackageTreeInput, serde_json::Error> {
    serde_json::from_str(json)
}

pub fn package_publish_input_from_json(
    json: &str,
) -> Result<RsScriptPackagePublishInput, serde_json::Error> {
    serde_json::from_str(json)
}

pub fn package_metadata_input_from_json(
    json: &str,
) -> Result<RsScriptPackageMetadataInput, serde_json::Error> {
    serde_json::from_str(json)
}

pub fn package_vendor_input_from_json(
    json: &str,
) -> Result<RsScriptPackageVendorInput, serde_json::Error> {
    serde_json::from_str(json)
}

impl RsScriptPackageRisk {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Elevated => "elevated",
            Self::High => "high",
            Self::Unknown => "unknown",
        }
    }
}

fn embedded_review_map_value(json: Option<&str>) -> Result<Option<Value>, serde_json::Error> {
    let Some(json) = json else {
        return Ok(None);
    };
    let value = serde_json::from_str::<Value>(json)?;
    Ok(value.get("review_map").cloned())
}

fn review_map_input_from_value(package_name: &str, value: Value) -> RsScriptReviewMapInput {
    let modules = value
        .get("modules")
        .and_then(Value::as_array)
        .map(|modules| {
            modules
                .iter()
                .map(|module| RsScriptModuleInput {
                    file: string_field(module, "file").unwrap_or_default(),
                    module_path: string_field(module, "module_path").unwrap_or_default(),
                    line: usize_field(module, "line").unwrap_or(1),
                    uses: module
                        .get("uses")
                        .and_then(Value::as_array)
                        .map(|uses| {
                            uses.iter()
                                .map(|use_decl| RsScriptUseInput {
                                    path: string_field(use_decl, "path").unwrap_or_default(),
                                    line: usize_field(use_decl, "line").unwrap_or(1),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    let files = value
        .get("files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut regions = Vec::new();
    for file in files {
        let file_name = string_field(&file, "file").unwrap_or_default();
        let file_regions = file
            .get("regions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for region in file_regions {
            regions.push(RsScriptRegionInput {
                file: file_name.clone(),
                function_name: string_field(&region, "function").unwrap_or_default(),
                classification: classification_from_json(&region),
                line: usize_field(&region, "line").unwrap_or(1),
                reasons: string_array_field(&region, "reasons"),
            });
        }
    }
    RsScriptReviewMapInput {
        package_name: package_name.to_owned(),
        modules,
        regions,
    }
}

fn package_review_input_from_value(value: &Value) -> RsScriptPackageReviewInput {
    let package = value.get("package").unwrap_or(&Value::Null);
    let summary = value.get("summary").unwrap_or(&Value::Null);
    let package_name = string_field(package, "name").unwrap_or_else(|| "rsscript".to_owned());
    let version = string_field(package, "version").unwrap_or_else(|| "0.0.0".to_owned());
    let exports = value
        .get("exports")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    RsScriptPackageReviewInput {
        package_name: package_name.clone(),
        version,
        risk: package_risk_from_json(value),
        features: string_array_field(value, "features"),
        implements: package_implements_from_json(value),
        dependencies: package_dependencies_from_json(value),
        exports: package_exports_from_json(&exports),
        capabilities: package_capabilities_from_json(value),
        await_sites: package_await_sites_from_json(value),
        diagnostics: package_diagnostics_from_json(value),
        public_apis: usize_field(summary, "public_apis").unwrap_or(0),
        mutating_apis: usize_field(summary, "mutating_apis").unwrap_or(0),
        retaining_apis: usize_field(summary, "retaining_apis").unwrap_or(0),
        resource_apis: usize_field(summary, "resource_apis").unwrap_or(0),
        native_apis: usize_field(summary, "native_apis").unwrap_or(0),
        unsafe_apis: usize_field(summary, "unsafe_apis").unwrap_or(0),
        unknown_apis: usize_field(summary, "unknown_apis").unwrap_or(0),
        native_boundaries: native_boundaries_from_exports(&package_name, value, &exports),
        native_cargo_features: native_cargo_features_from_json(value),
        native_author_declaration: native_author_declaration_from_json(value),
        native_source_scan: native_source_scan_from_json(value),
    }
}

fn package_implements_from_json(value: &Value) -> Vec<RsScriptProviderImplementation> {
    value
        .get("implements")
        .and_then(Value::as_array)
        .map(|implements| {
            implements
                .iter()
                .map(|implementation| RsScriptProviderImplementation {
                    interface_package: string_field(implementation, "interface_package")
                        .unwrap_or_else(|| "unknown".to_owned()),
                    version: string_field(implementation, "version"),
                    interface_features: string_array_field(implementation, "interface_features"),
                    interface_effective_hash: string_field(
                        implementation,
                        "interface_effective_hash",
                    ),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn package_capabilities_from_json(value: &Value) -> Vec<RsScriptPackageCapability> {
    value
        .get("capabilities")
        .and_then(Value::as_array)
        .map(|capabilities| {
            capabilities
                .iter()
                .map(|capability| RsScriptPackageCapability {
                    function: string_field(capability, "function")
                        .unwrap_or_else(|| "unknown".to_owned()),
                    binding_symbol: string_field(capability, "binding_symbol")
                        .unwrap_or_else(|| "unknown".to_owned()),
                    category: string_field(capability, "category")
                        .unwrap_or_else(|| "unknown".to_owned())
                        .try_into()
                        .expect("capability category conversion is infallible"),
                    provider: string_field(capability, "provider"),
                    service: string_field(capability, "service"),
                    action: string_field(capability, "action"),
                    resource: string_field(capability, "resource"),
                    call_chain: string_array_field(capability, "call_chain"),
                    span: capability
                        .get("span")
                        .map(|span| diagnostic_span_from_json(span, String::new())),
                    unknown_reason: string_field(capability, "unknown_reason"),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn native_cargo_features_from_json(value: &Value) -> Vec<String> {
    value
        .get("native_rust")
        .map(|native| string_array_field(native, "cargo_features"))
        .unwrap_or_default()
}

fn native_author_declaration_from_json(value: &Value) -> Option<RsScriptNativeAuthorDeclaration> {
    let declaration = value
        .get("native_rust")?
        .get("semantic")?
        .get("author_declaration")?;
    Some(RsScriptNativeAuthorDeclaration {
        worker_thread_parallelism: bool_field(declaration, "worker_thread_parallelism")
            .unwrap_or(false),
        native_parallel_backend: string_field(declaration, "native_parallel_backend"),
        risk_reasons: string_array_field(declaration, "risk_reasons"),
    })
}

fn package_diagnostics_from_json(value: &Value) -> Vec<RsScriptDiagnosticInput> {
    value
        .get("diagnostics")
        .and_then(Value::as_array)
        .map(|diagnostics| {
            diagnostics
                .iter()
                .map(|diagnostic| RsScriptDiagnosticInput {
                    code: string_field(diagnostic, "code").unwrap_or_else(|| "unknown".to_owned()),
                    severity: string_field(diagnostic, "severity")
                        .unwrap_or_else(|| "error".to_owned()),
                    summary: string_field(diagnostic, "summary").unwrap_or_default(),
                    spans: diagnostic_spans_from_json(diagnostic),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn diagnostic_spans_from_json(value: &Value) -> Vec<RsScriptDiagnosticSpan> {
    if let Some(span) = value.get("span") {
        return vec![diagnostic_span_from_json(
            span,
            string_field(value, "label").unwrap_or_default(),
        )];
    }
    value
        .get("spans")
        .and_then(Value::as_array)
        .map(|spans| {
            spans
                .iter()
                .map(|span| {
                    diagnostic_span_from_json(span, string_field(span, "label").unwrap_or_default())
                })
                .collect()
        })
        .unwrap_or_default()
}

fn diagnostic_span_from_json(value: &Value, label: String) -> RsScriptDiagnosticSpan {
    RsScriptDiagnosticSpan {
        file: string_field(value, "file").unwrap_or_default(),
        line: usize_field(value, "line").unwrap_or(1),
        column: usize_field(value, "column").unwrap_or(1),
        length: usize_field(value, "length").unwrap_or(0),
        label,
    }
}

fn package_await_sites_from_json(value: &Value) -> Vec<RsScriptPackageAwaitSite> {
    value
        .get("await_sites")
        .and_then(Value::as_array)
        .map(|sites| {
            sites
                .iter()
                .map(|site| {
                    let span = site.get("span").unwrap_or(&Value::Null);
                    RsScriptPackageAwaitSite {
                        function: string_field(site, "function")
                            .unwrap_or_else(|| "unknown".to_owned()),
                        callee: string_field(site, "callee"),
                        boundary: string_field(site, "boundary")
                            .unwrap_or_else(|| "unknown".to_owned()),
                        live_across_await: string_array_field(site, "live_across_await"),
                        file: string_field(span, "file").unwrap_or_default(),
                        line: usize_field(span, "line").unwrap_or(1),
                        column: usize_field(span, "column").unwrap_or(1),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn native_source_scan_from_json(value: &Value) -> Option<RsScriptNativeSourceScan> {
    let scan = value
        .get("native_rust")?
        .get("semantic")?
        .get("source_scan_best_effort")?;
    Some(RsScriptNativeSourceScan {
        tool: string_field(scan, "tool").unwrap_or_else(|| "rss-native-source-scan".to_owned()),
        selected_graph: string_field(scan, "selected_graph")
            .unwrap_or_else(|| "package-native-rust".to_owned()),
        worker_thread_parallelism_detected: bool_field(scan, "worker_thread_parallelism_detected")
            .unwrap_or(false),
        unsafe_detected: bool_field(scan, "unsafe_detected").unwrap_or(false),
        ffi_detected: bool_field(scan, "ffi_detected").unwrap_or(false),
        filesystem_detected: bool_field(scan, "filesystem_detected").unwrap_or(false),
        network_detected: bool_field(scan, "network_detected").unwrap_or(false),
        build_script_present: bool_field(scan, "build_script_present").unwrap_or(false),
    })
}

fn package_exports_from_json(exports: &[Value]) -> Vec<RsScriptPackageExport> {
    exports
        .iter()
        .map(|export| RsScriptPackageExport {
            name: string_field(export, "name").unwrap_or_else(|| "unknown".to_owned()),
            kind: string_field(export, "kind").unwrap_or_else(|| "unknown".to_owned()),
            classification: string_field(export, "classification").unwrap_or_default(),
            reasons: string_array_field(export, "reasons"),
            normalized_effects: string_array_field(export, "normalized_effects"),
        })
        .collect()
}

fn package_dependencies_from_json(value: &Value) -> Vec<RsScriptPackageDependency> {
    value
        .get("dependencies")
        .and_then(Value::as_array)
        .map(|dependencies| {
            dependencies
                .iter()
                .map(|dependency| RsScriptPackageDependency {
                    name: string_field(dependency, "name").unwrap_or_else(|| "unknown".to_owned()),
                    requirement: string_field(dependency, "requirement"),
                    source: string_field(dependency, "source")
                        .unwrap_or_else(|| "registry".to_owned()),
                    features: string_array_field(dependency, "features"),
                    dependency_kind: string_field(dependency, "dependency_kind")
                        .unwrap_or_else(|| "normal".to_owned()),
                    compile_only: bool_field(dependency, "compile_only").unwrap_or(false),
                    test_only: bool_field(dependency, "test_only").unwrap_or(false),
                    platform_provided: bool_field(dependency, "platform_provided").unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn classification_from_json(value: &Value) -> RsScriptClassification {
    match string_field(value, "classification").as_deref() {
        Some("low_semantic_risk" | "foldable") => RsScriptClassification::Foldable,
        Some("unknown") => RsScriptClassification::Unknown,
        _ => RsScriptClassification::ReviewRequired,
    }
}

fn package_risk_from_json(value: &Value) -> RsScriptPackageRisk {
    match string_field(value, "risk").as_deref() {
        Some("low") => RsScriptPackageRisk::Low,
        Some("elevated") => RsScriptPackageRisk::Elevated,
        Some("unknown") => RsScriptPackageRisk::Unknown,
        _ => RsScriptPackageRisk::High,
    }
}

fn native_boundaries_from_exports(
    package_name: &str,
    value: &Value,
    exports: &[Value],
) -> Vec<RsScriptNativeBoundary> {
    let file = value
        .get("files")
        .and_then(Value::as_array)
        .and_then(|files| files.first())
        .and_then(|file| string_field(file, "path"))
        .unwrap_or_else(|| format!("{package_name}.rssi"));
    let mut boundaries = BTreeMap::<String, RsScriptNativeBoundary>::new();
    for export in exports.iter().filter(|export| {
        string_array_field(export, "normalized_effects")
            .iter()
            .any(|effect| effect == "native")
    }) {
        let function = string_field(export, "name").unwrap_or_default();
        let (file, line) =
            source_span_for_function(value, &function).unwrap_or_else(|| (file.clone(), 1));
        let module_name = function
            .rsplit_once('.')
            .map(|(module, _)| module.to_owned())
            .unwrap_or_else(|| "native".to_owned());
        let boundary =
            boundaries
                .entry(module_name.clone())
                .or_insert_with(|| RsScriptNativeBoundary {
                    module_name,
                    functions: Vec::new(),
                    file,
                    line,
                });
        if !function.is_empty() {
            boundary.functions.push(function);
        }
    }
    for boundary in boundaries.values_mut() {
        boundary.functions.sort();
        boundary.functions.dedup();
    }
    boundaries.into_values().collect()
}

fn source_span_for_function(value: &Value, function_name: &str) -> Option<(String, usize)> {
    value
        .get("review_map")
        .and_then(|review_map| review_map.get("files"))
        .and_then(Value::as_array)
        .and_then(|files| {
            files.iter().find_map(|file| {
                let file_name = string_field(file, "file")?;
                file.get("regions")
                    .and_then(Value::as_array)
                    .and_then(|regions| {
                        regions.iter().find_map(|region| {
                            let region_function = string_field(region, "function")?;
                            (region_function == function_name).then(|| {
                                (file_name.clone(), usize_field(region, "line").unwrap_or(1))
                            })
                        })
                    })
            })
        })
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_owned)
}

fn usize_field(value: &Value, field: &str) -> Option<usize> {
    value
        .get(field)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

fn string_array_field(value: &Value, field: &str) -> Vec<String> {
    value
        .get(field)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn bool_field(value: &Value, field: &str) -> Option<bool> {
    value.get(field).and_then(Value::as_bool)
}
