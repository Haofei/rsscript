#![forbid(unsafe_code)]

//! Deterministic algorithms for RSScript's core library.
//!
//! This crate deliberately knows nothing about bytecode, VM values, Providers,
//! budgets, or host state. Execution backends adapt their value representation
//! at the boundary and retain responsibility for accounting.

use base64::Engine;
use percent_encoding::{NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

const URL_COMPONENT_SET: &percent_encoding::AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

pub mod encoding {
    use super::*;

    pub fn base64_decode(value: &str) -> Result<Vec<u8>, String> {
        base64::engine::general_purpose::STANDARD
            .decode(value)
            .map_err(|error| error.to_string())
    }

    pub fn base64_encode(value: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(value)
    }

    pub fn hex_decode(value: &str) -> Result<Vec<u8>, String> {
        hex::decode(value).map_err(|error| error.to_string())
    }

    pub fn hex_encode(value: &[u8]) -> String {
        hex::encode(value)
    }

    pub fn url_decode_component(value: &str) -> Result<String, String> {
        percent_decode_str(value)
            .decode_utf8()
            .map(|value| value.into_owned())
            .map_err(|error| error.to_string())
    }

    pub fn url_encode_component(value: &str) -> String {
        utf8_percent_encode(value, URL_COMPONENT_SET).to_string()
    }
}

/// Pure, representation-independent collection transformations. Callers own
/// allocation accounting and choose their concrete collection layout.
pub mod collections {
    pub fn dedup<T: PartialEq>(values: impl IntoIterator<Item = T>) -> Vec<T> {
        let mut result = Vec::new();
        for value in values {
            if !result.contains(&value) {
                result.push(value);
            }
        }
        result
    }

    pub fn reverse<T>(values: impl IntoIterator<Item = T>) -> Vec<T> {
        let mut result = values.into_iter().collect::<Vec<_>>();
        result.reverse();
        result
    }

    pub fn skip<T>(values: impl IntoIterator<Item = T>, count: usize) -> Vec<T> {
        values.into_iter().skip(count).collect()
    }

    pub fn take<T>(values: impl IntoIterator<Item = T>, count: usize) -> Vec<T> {
        values.into_iter().take(count).collect()
    }

    pub fn slice<T>(values: impl IntoIterator<Item = T>, start: usize, len: usize) -> Vec<T> {
        values.into_iter().skip(start).take(len).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{collections::*, encoding::*};

    #[test]
    fn encoding_algorithms_are_deterministic_and_round_trip() {
        let bytes = b"hello / RSScript";
        assert_eq!(base64_decode(&base64_encode(bytes)).unwrap(), bytes);
        assert_eq!(hex_decode(&hex_encode(bytes)).unwrap(), bytes);
        assert_eq!(
            url_decode_component(&url_encode_component("hello / RSScript")).unwrap(),
            "hello / RSScript"
        );
    }

    #[test]
    fn malformed_decoding_returns_data_errors_not_host_errors() {
        assert!(base64_decode("%%%not-base64%%%").is_err());
        assert!(hex_decode("xyz").is_err());
        assert!(url_decode_component("%FF").is_err());
    }

    #[test]
    fn collection_transforms_are_generic_and_preserve_order() {
        assert_eq!(dedup([1, 2, 1, 3, 2]), vec![1, 2, 3]);
        assert_eq!(reverse([1, 2, 3]), vec![3, 2, 1]);
        assert_eq!(skip([1, 2, 3], 1), vec![2, 3]);
        assert_eq!(take([1, 2, 3], 2), vec![1, 2]);
        assert_eq!(slice([1, 2, 3, 4], 1, 2), vec![2, 3]);
    }
}
