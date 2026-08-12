//! Canonical semantic properties of ordinary value types.

/// Whether a type has Copy value semantics in the current language contract.
pub fn is_copy_type_name(type_name: &str) -> bool {
    let type_name = type_name.trim();
    !type_name.contains('<')
        && matches!(
            type_name.strip_prefix("fresh ").unwrap_or(type_name),
            "Bool"
                | "Byte"
                | "Char"
                | "Float"
                | "Float32"
                | "Float64"
                | "Int"
                | "Int8"
                | "Int16"
                | "Int32"
                | "Int64"
                | "UInt"
                | "UInt8"
                | "UInt16"
                | "UInt32"
                | "UInt64"
                | "Unit"
        )
}

/// Whether a type may cross an isolate boundary as a self-contained message.
/// This is deliberately conservative: Copy scalars and immutable owned data
/// cross; managed/container/generic values do not until a future semantic
/// contract explicitly broadens the set.
pub fn is_cross_isolate_transferable(type_name: &str) -> bool {
    let type_name = type_name.trim();
    is_copy_type_name(type_name)
        || matches!(
            type_name.strip_prefix("fresh ").unwrap_or(type_name),
            "String" | "Bytes"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_and_message_contracts_remain_conservative() {
        assert!(is_copy_type_name("fresh Int"));
        assert!(!is_copy_type_name("List<Int>"));
        assert!(is_cross_isolate_transferable("Bytes"));
        assert!(!is_cross_isolate_transferable("Map<String, Bytes>"));
    }
}
