use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

const SHA256_BYTES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixedDigestError {
    InvalidHex,
    WrongLength,
}

pub fn sha256_hex_string(value: &str) -> String {
    sha256_hex_bytes(value.as_bytes())
}

pub fn sha256_hex_bytes(value: &[u8]) -> String {
    let digest = Sha256::digest(value);
    hex::encode(digest)
}

pub fn hmac_sha256_hex(key: &str, message: &str) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(key.as_bytes()).expect("HMAC accepts any key length");
    mac.update(message.as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// Compare arbitrary strings without data-dependent byte comparisons.
///
/// The length check remains observable. Authentication code should compare
/// fixed-size decoded digests with [`constant_time_equal_sha256_hex`] instead.
pub fn constant_time_equal(left: &str, right: &str) -> bool {
    bool::from(left.as_bytes().ct_eq(right.as_bytes()))
}

/// Decode and compare two SHA-256 hex digests as fixed-size values.
pub fn constant_time_equal_sha256_hex(left: &str, right: &str) -> Result<bool, FixedDigestError> {
    let left = decode_sha256_hex(left)?;
    let right = decode_sha256_hex(right)?;
    Ok(bool::from(left.ct_eq(&right)))
}

fn decode_sha256_hex(value: &str) -> Result<[u8; SHA256_BYTES], FixedDigestError> {
    if value.len() != SHA256_BYTES * 2 {
        return Err(FixedDigestError::WrongLength);
    }
    let decoded = hex::decode(value).map_err(|_| FixedDigestError::InvalidHex)?;
    decoded
        .try_into()
        .map_err(|_| FixedDigestError::WrongLength)
}

#[cfg(test)]
mod tests {
    #[test]
    fn hashes_and_compares() {
        assert_eq!(
            super::sha256_hex_string("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(super::constant_time_equal("same", "same"));
        assert!(!super::constant_time_equal("same", "different"));
        assert!(super::constant_time_equal("", ""));
        assert!(!super::constant_time_equal("", "x"));
    }

    #[test]
    fn fixed_digest_comparison_requires_valid_sha256_hex() {
        let digest = super::sha256_hex_string("abc");
        assert_eq!(
            super::constant_time_equal_sha256_hex(&digest, &digest),
            Ok(true)
        );
        assert_eq!(
            super::constant_time_equal_sha256_hex(
                &digest,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ae"
            ),
            Ok(false)
        );
        assert_eq!(
            super::constant_time_equal_sha256_hex("00", "00"),
            Err(super::FixedDigestError::WrongLength)
        );
        assert_eq!(
            super::constant_time_equal_sha256_hex(
                "zz7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
                &digest
            ),
            Err(super::FixedDigestError::InvalidHex)
        );
    }
}
