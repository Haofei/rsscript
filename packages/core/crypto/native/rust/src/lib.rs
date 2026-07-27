use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

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

pub fn constant_time_equal(left: &str, right: &str) -> bool {
    bool::from(left.as_bytes().ct_eq(right.as_bytes()))
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
}
