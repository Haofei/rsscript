use super::*;

/// Validate the optional v1 source-map side table. It is debug-only data: it
/// cannot alter control flow, but it must still point at a real decoded
/// instruction so inspection tools never attribute evidence to arbitrary code.
pub(super) fn verify_source_map(
    unit: &serde_json::Map<String, serde_json::Value>,
    functions: &[serde_json::Value],
    total_instructions: usize,
) -> Result<(), BytecodeError> {
    let Some(entries) = unit.get("source_map") else {
        return Ok(());
    };
    let entries = entries
        .as_array()
        .ok_or_else(|| invalid_payload("source_map is not an array"))?;
    if entries.len() > total_instructions {
        return Err(BytecodeError::LimitExceeded("source map entries"));
    }
    let mut mapped = BTreeSet::new();
    for (index, entry) in entries.iter().enumerate() {
        let entry = entry
            .as_object()
            .ok_or_else(|| invalid_payload(format!("source_map entry {index} is not an object")))?;
        require_object_fields(
            entry,
            &[
                "function",
                "instruction",
                "file",
                "line",
                "column",
                "length",
            ],
            &format!("source_map entry {index}"),
        )?;
        let function = json_usize(&entry["function"], "source map function")?;
        let instruction = json_usize(&entry["instruction"], "source map instruction")?;
        let code = functions
            .get(function)
            .and_then(|function| function.get("code"))
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                invalid_payload(format!(
                    "source_map entry {index} references missing function {function}"
                ))
            })?;
        if instruction >= code.len() {
            return Err(invalid_payload(format!(
                "source_map entry {index} references missing instruction {instruction}"
            )));
        }
        if !mapped.insert((function, instruction)) {
            return Err(invalid_payload(format!(
                "source_map entry {index} duplicates function {function} instruction {instruction}"
            )));
        }
        if entry["file"].as_str().is_none_or(str::is_empty) {
            return Err(invalid_payload(format!(
                "source_map entry {index} has invalid file"
            )));
        }
        for field in ["line", "column", "length"] {
            let _ = json_usize(&entry[field], field)?;
        }
    }
    Ok(())
}

pub(super) fn resource_drop_inputs(
    unit: &serde_json::Map<String, serde_json::Value>,
    functions: &[serde_json::Value],
) -> Result<BTreeMap<usize, BTreeSet<usize>>, BytecodeError> {
    let drops = unit["resource_drop_functions"]
        .as_object()
        .ok_or_else(|| invalid_payload("resource_drop_functions is not an object"))?;
    let types = unit["types"]
        .as_object()
        .ok_or_else(|| invalid_payload("types is not an object"))?;
    let mut inputs = BTreeMap::new();
    for (type_name, function_id) in drops {
        let function_id = json_usize(function_id, "resource drop function")?;
        let function = functions
            .get(function_id)
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| {
                invalid_payload(format!(
                    "resource `{type_name}` references missing drop function {function_id}"
                ))
            })?;
        let ty = types
            .get(type_name)
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| invalid_payload(format!("resource type `{type_name}` is missing")))?;
        let fields = ty
            .get("fields")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| invalid_payload(format!("resource type `{type_name}` has no fields")))?;
        let locals = function
            .get("local_regs")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| {
                invalid_payload(format!(
                    "resource drop `{type_name}` has no local register map"
                ))
            })?;
        let mut registers = BTreeSet::new();
        for field in fields {
            let name = field
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    invalid_payload(format!("resource type `{type_name}` has invalid field"))
                })?;
            let register = locals.get(name).ok_or_else(|| {
                invalid_payload(format!(
                    "resource drop `{type_name}` is missing field register `{name}`"
                ))
            })?;
            registers.insert(json_usize(register, "resource field register")?);
        }
        if inputs.insert(function_id, registers).is_some() {
            return Err(invalid_payload(format!(
                "drop function {function_id} is shared by multiple resource types"
            )));
        }
    }
    Ok(inputs)
}

pub(super) fn verify_type_metadata(
    unit: &serde_json::Map<String, serde_json::Value>,
    limits: BytecodeLimits,
) -> Result<(), BytecodeError> {
    let types = unit["types"]
        .as_object()
        .ok_or_else(|| invalid_payload("types is not an object"))?;
    if types.len() > limits.max_functions {
        return Err(BytecodeError::LimitExceeded("type count"));
    }
    for (key, value) in types {
        let ty = value
            .as_object()
            .ok_or_else(|| invalid_payload(format!("type `{key}` is not an object")))?;
        require_object_fields(ty, &["name", "fields"], &format!("type `{key}`"))?;
        let name = ty["name"]
            .as_str()
            .ok_or_else(|| invalid_payload(format!("type `{key}` name is not a string")))?;
        if name != key {
            return Err(invalid_payload(format!(
                "type table key `{key}` does not match metadata name `{name}`"
            )));
        }
        let fields = ty["fields"]
            .as_array()
            .ok_or_else(|| invalid_payload(format!("type `{key}` fields is not an array")))?;
        if fields.len() > limits.max_registers_per_function {
            return Err(BytecodeError::LimitExceeded("fields per type"));
        }
        let mut field_names = BTreeSet::new();
        for field in fields {
            let field = field
                .as_object()
                .ok_or_else(|| invalid_payload(format!("type `{key}` field is not an object")))?;
            require_object_fields(
                field,
                &["name", "type_name"],
                &format!("type `{key}` field"),
            )?;
            let field_name = field["name"].as_str().ok_or_else(|| {
                invalid_payload(format!("type `{key}` field name is not a string"))
            })?;
            let type_name = field["type_name"].as_str().ok_or_else(|| {
                invalid_payload(format!("type `{key}` field type is not a string"))
            })?;
            if field_name.is_empty() || type_name.is_empty() || !field_names.insert(field_name) {
                return Err(invalid_payload(format!(
                    "type `{key}` has an empty or duplicate field `{field_name}`"
                )));
            }
        }
    }
    Ok(())
}

/// Validate the optional v1 named-sum side table. It is a compatibility
/// projection of typed MIR layout evidence: the legacy VM still executes
/// string-named variants, but report conversion can only assign canonical
/// numeric identities after this complete declaration table has been checked.
pub(super) fn verify_variant_layout_metadata(
    unit: &serde_json::Map<String, serde_json::Value>,
    limits: BytecodeLimits,
) -> Result<Option<BTreeMap<String, BTreeSet<String>>>, BytecodeError> {
    let Some(layouts) = unit.get("variant_layouts") else {
        return Ok(None);
    };
    let layouts = layouts
        .as_object()
        .ok_or_else(|| invalid_payload("variant_layouts is not an object"))?;
    if layouts.len() > limits.max_functions {
        return Err(BytecodeError::LimitExceeded("variant type count"));
    }
    let mut cases = BTreeMap::new();
    for (key, value) in layouts {
        let layout = value
            .as_object()
            .ok_or_else(|| invalid_payload(format!("variant layout `{key}` is not an object")))?;
        require_object_fields(
            layout,
            &["name", "variants"],
            &format!("variant layout `{key}`"),
        )?;
        let name = layout["name"].as_str().ok_or_else(|| {
            invalid_payload(format!("variant layout `{key}` name is not a string"))
        })?;
        if name != key || name.is_empty() {
            return Err(invalid_payload(format!(
                "variant layout key `{key}` does not match metadata name `{name}`"
            )));
        }
        let variants = layout["variants"].as_array().ok_or_else(|| {
            invalid_payload(format!("variant layout `{key}` variants is not an array"))
        })?;
        if variants.is_empty() || variants.len() > limits.max_registers_per_function {
            return Err(BytecodeError::LimitExceeded("variants per type"));
        }
        let mut variant_names = BTreeSet::new();
        for variant in variants {
            let variant = variant.as_object().ok_or_else(|| {
                invalid_payload(format!("variant layout `{key}` case is not an object"))
            })?;
            require_object_fields(
                variant,
                &["name", "fields"],
                &format!("variant layout `{key}` case"),
            )?;
            let variant_name = variant["name"].as_str().ok_or_else(|| {
                invalid_payload(format!("variant layout `{key}` case name is not a string"))
            })?;
            if variant_name.is_empty() || !variant_names.insert(variant_name) {
                return Err(invalid_payload(format!(
                    "variant layout `{key}` has an empty or duplicate case `{variant_name}`"
                )));
            }
            let fields = variant["fields"].as_array().ok_or_else(|| {
                invalid_payload(format!(
                    "variant layout `{key}` case fields is not an array"
                ))
            })?;
            if fields.len() > limits.max_registers_per_function {
                return Err(BytecodeError::LimitExceeded("fields per variant"));
            }
            let mut field_names = BTreeSet::new();
            for field in fields {
                let field = field.as_object().ok_or_else(|| {
                    invalid_payload(format!("variant layout `{key}` field is not an object"))
                })?;
                require_object_fields(
                    field,
                    &["name", "type_name"],
                    &format!("variant layout `{key}` field"),
                )?;
                let field_name = field["name"].as_str().ok_or_else(|| {
                    invalid_payload(format!("variant layout `{key}` field name is not a string"))
                })?;
                let type_name = field["type_name"].as_str().ok_or_else(|| {
                    invalid_payload(format!("variant layout `{key}` field type is not a string"))
                })?;
                if field_name.is_empty()
                    || type_name.is_empty()
                    || !field_names.insert(field_name.to_owned())
                {
                    return Err(invalid_payload(format!(
                        "variant layout `{key}` has an empty or duplicate field `{field_name}`"
                    )));
                }
            }
            if cases.insert(variant_name.to_owned(), field_names).is_some() {
                return Err(invalid_payload(format!(
                    "variant case `{variant_name}` occurs in multiple layouts"
                )));
            }
        }
    }
    Ok(Some(cases))
}

/// Once a v1 Artifact opts into named-sum layouts, its executable instructions
/// must use that exact declared case shape. This is deliberately optional for
/// old Artifacts that predate the table, but fail-closed for new producers so
/// a report never assigns numeric wire identity to a different runtime shape.
pub(super) fn verify_variant_instruction_layouts(
    functions: &[serde_json::Value],
    cases: &Option<BTreeMap<String, BTreeSet<String>>>,
) -> Result<(), BytecodeError> {
    let Some(cases) = cases else {
        return Ok(());
    };
    for (function_id, function) in functions.iter().enumerate() {
        let code = function
            .get("code")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                invalid_payload(format!("function {function_id} code is not an array"))
            })?;
        for (ip, instruction) in code.iter().enumerate() {
            let Some(fields) = instruction
                .get("MakeVariant")
                .and_then(serde_json::Value::as_object)
            else {
                continue;
            };
            let layout = fields
                .get("layout")
                .and_then(serde_json::Value::as_object)
                .ok_or_else(|| invalid_payload(format!(
                    "function {function_id} instruction {ip} MakeVariant layout is not an object"
                )))?;
            let case = layout
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| invalid_payload(format!(
                    "function {function_id} instruction {ip} MakeVariant layout name is not a string"
                )))?;
            // `Result` remains a language primitive in v1 and uses the legacy
            // `MakeVariant` representation without a named sum layout.
            if matches!(case, "Ok" | "Err") && !cases.contains_key(case) {
                continue;
            }
            let expected = cases.get(case).ok_or_else(|| {
                invalid_payload(format!(
                    "function {function_id} instruction {ip} constructs undeclared variant `{case}`"
                ))
            })?;
            let names = layout
                .get("field_names")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| invalid_payload(format!(
                    "function {function_id} instruction {ip} MakeVariant field_names is not an array"
                )))?
                .iter()
                .map(|value| value.as_str().map(str::to_owned))
                .collect::<Option<BTreeSet<_>>>()
                .ok_or_else(|| invalid_payload(format!(
                    "function {function_id} instruction {ip} MakeVariant field name is not a string"
                )))?;
            if &names != expected {
                return Err(invalid_payload(format!(
                    "function {function_id} instruction {ip} variant `{case}` fields disagree with its declared layout"
                )));
            }
        }
    }
    Ok(())
}

pub(super) fn verify_native_signatures(
    unit: &serde_json::Map<String, serde_json::Value>,
    functions: &[serde_json::Value],
    limits: BytecodeLimits,
) -> Result<(), BytecodeError> {
    let signatures = unit["native_signatures"]
        .as_object()
        .ok_or_else(|| invalid_payload("native_signatures is not an object"))?;
    if signatures.len() > limits.max_functions {
        return Err(BytecodeError::LimitExceeded("native signature count"));
    }
    let function_ids = unit["function_ids"]
        .as_object()
        .ok_or_else(|| invalid_payload("function_ids is not an object"))?;
    if signatures.keys().collect::<BTreeSet<_>>() != function_ids.keys().collect::<BTreeSet<_>>() {
        return Err(invalid_payload(
            "native signature names differ from the public function map",
        ));
    }
    for (name, value) in signatures {
        let signature = value.as_object().ok_or_else(|| {
            invalid_payload(format!("native signature `{name}` is not an object"))
        })?;
        require_object_fields(
            signature,
            &["params", "return_type"],
            &format!("native signature `{name}`"),
        )?;
        let params = signature["params"].as_array().ok_or_else(|| {
            invalid_payload(format!("native signature `{name}` params is not an array"))
        })?;
        if params
            .iter()
            .any(|parameter| parameter.as_str().is_none_or(str::is_empty))
        {
            return Err(invalid_payload(format!(
                "native signature `{name}` has an invalid parameter type"
            )));
        }
        if !signature["return_type"].is_null()
            && signature["return_type"].as_str().is_none_or(str::is_empty)
        {
            return Err(invalid_payload(format!(
                "native signature `{name}` has an invalid return type"
            )));
        }
        let function_id = json_usize(&function_ids[name], "function id")?;
        let expected = functions[function_id]
            .get("params")
            .ok_or_else(|| invalid_payload(format!("function `{name}` is missing params")))?;
        require_arity(
            function_id,
            0,
            "native signature",
            json_usize(expected, "params")?,
            params.len(),
        )?;
    }
    Ok(())
}
