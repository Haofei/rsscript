use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use flate2::read::GzDecoder;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use std::io::Read;

#[derive(Debug, Clone)]
pub struct DecodeError {
    pub message: String,
}

// Base64

pub fn base64_encode(value: &str) -> String {
    STANDARD.encode(value.as_bytes())
}

pub fn base64_encode_bytes(value: &[u8]) -> String {
    STANDARD.encode(value)
}

pub fn base64_decode(text: &str) -> Result<Vec<u8>, DecodeError> {
    STANDARD.decode(text).map_err(|e| DecodeError {
        message: e.to_string(),
    })
}

pub fn base64_decode_string(text: &str) -> Result<String, DecodeError> {
    let bytes = base64_decode(text)?;
    String::from_utf8(bytes).map_err(|e| DecodeError {
        message: e.to_string(),
    })
}

// Hex

pub fn hex_encode(value: &[u8]) -> String {
    hex::encode(value)
}

pub fn hex_encode_string(value: &str) -> String {
    hex::encode(value.as_bytes())
}

pub fn hex_decode(text: &str) -> Result<Vec<u8>, DecodeError> {
    hex::decode(text).map_err(|e| DecodeError {
        message: e.to_string(),
    })
}

// Gzip

pub fn gzip_decompress_bytes(value: &[u8]) -> Result<Vec<u8>, DecodeError> {
    let mut decoder = GzDecoder::new(value);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).map_err(|e| DecodeError {
        message: e.to_string(),
    })?;
    Ok(out)
}

// URL encoding

const COMPONENT_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

pub fn url_encode_component(value: &str) -> String {
    utf8_percent_encode(value, COMPONENT_SET).to_string()
}

pub fn url_decode_component(value: &str) -> Result<String, DecodeError> {
    percent_decode_str(value)
        .decode_utf8()
        .map(|s| s.to_string())
        .map_err(|e| DecodeError {
            message: e.to_string(),
        })
}

pub fn decode_error_message(error: &DecodeError) -> String {
    error.message.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gzip_decompress_bytes_decodes_gzip_payload() {
        let gzipped = hex::decode("1f8b08000000000002ff4b4c4a0600c241243503000000").unwrap();
        let decoded = gzip_decompress_bytes(&gzipped).unwrap();
        assert_eq!(decoded, b"abc");
    }

    #[test]
    fn gzip_decompress_bytes_reports_decode_errors() {
        let err = gzip_decompress_bytes(b"not gzip").unwrap_err();
        assert!(!err.message.is_empty());
    }
}
