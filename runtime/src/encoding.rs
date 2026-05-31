use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};

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
