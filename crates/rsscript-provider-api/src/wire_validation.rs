use crate::{
    DataEffect, FunctionSignature, ProviderError, WireCallTypeTable, WireMutationResult,
    WireRecordLayout, WireValue, WireVariantLayout,
};

fn linked_wire_type_table(
    signature: &FunctionSignature,
    record_layouts: &[WireRecordLayout],
    variant_layouts: &[WireVariantLayout],
) -> Result<WireCallTypeTable, ProviderError> {
    WireCallTypeTable::for_signature(signature)
        .and_then(|table| table.with_record_layouts(record_layouts.to_vec()))
        .and_then(|table| table.with_variant_layouts(variant_layouts.to_vec()))
        .map_err(|error| {
            ProviderError::internal(format!(
                "linked Provider signature cannot form a wire type table: {error}"
            ))
        })
}

/// Validate canonical Provider arguments against a descriptor-linked call.
///
/// This is the shared fail-closed boundary used by the VM and Provider
/// conformance kit. Implementations never need to reproduce record, variant,
/// collection, or resource identity checks in handwritten adapter code.
pub fn validate_wire_arguments(
    signature: &FunctionSignature,
    record_layouts: &[WireRecordLayout],
    variant_layouts: &[WireVariantLayout],
    arguments: &[WireValue],
) -> Result<(), ProviderError> {
    if arguments.len() != signature.parameters.len() {
        return Err(ProviderError::invalid_argument(format!(
            "expected exactly {} Provider arguments, received {}",
            signature.parameters.len(),
            arguments.len()
        )));
    }
    let types = linked_wire_type_table(signature, record_layouts, variant_layouts)?;
    for (index, (parameter, argument)) in signature.parameters.iter().zip(arguments).enumerate() {
        types
            .validate_value(&parameter.ty, argument)
            .map_err(|error| {
                ProviderError::invalid_argument(format!(
                    "Provider argument `{}` at position {index} is invalid: {error}",
                    parameter.name
                ))
            })?;
    }
    Ok(())
}

/// Validate a Provider's canonical return value. A mismatch is a Provider
/// implementation fault, not a script argument error.
pub fn validate_wire_result(
    signature: &FunctionSignature,
    record_layouts: &[WireRecordLayout],
    variant_layouts: &[WireVariantLayout],
    result: &WireValue,
) -> Result<(), ProviderError> {
    linked_wire_type_table(signature, record_layouts, variant_layouts)?
        .validate_value(&signature.result, result)
        .map_err(|error| {
            ProviderError::internal(format!("Provider returned an invalid value: {error}"))
        })
}

/// Validate the explicit result and every declaration-ordered `mut`
/// write-back produced by a canonical mutation Provider.
pub fn validate_wire_mutation_result(
    signature: &FunctionSignature,
    record_layouts: &[WireRecordLayout],
    variant_layouts: &[WireVariantLayout],
    result: &WireMutationResult,
) -> Result<(), ProviderError> {
    validate_wire_result(signature, record_layouts, variant_layouts, &result.result)?;
    let mut_parameters = signature
        .parameters
        .iter()
        .filter(|parameter| parameter.effect == DataEffect::Mut)
        .collect::<Vec<_>>();
    if result.mutated.len() != mut_parameters.len() {
        return Err(ProviderError::internal(format!(
            "Provider returned {} mutation values for {} mut parameters",
            result.mutated.len(),
            mut_parameters.len()
        )));
    }
    let types = linked_wire_type_table(signature, record_layouts, variant_layouts)?;
    for (index, (parameter, value)) in mut_parameters.into_iter().zip(&result.mutated).enumerate() {
        types
            .validate_value(&parameter.ty, value)
            .map_err(|error| {
                ProviderError::internal(format!(
                    "Provider mutation `{}` at position {index} is invalid: {error}",
                    parameter.name
                ))
            })?;
    }
    Ok(())
}
