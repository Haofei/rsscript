use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

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
    let left = left.as_bytes();
    let right = right.as_bytes();
    let mut diff = left.len() ^ right.len();
    let max_len = left.len().max(right.len());
    for index in 0..max_len {
        let left_byte = left.get(index).copied().unwrap_or(0);
        let right_byte = right.get(index).copied().unwrap_or(0);
        diff |= (left_byte ^ right_byte) as usize;
    }
    diff == 0
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
    }
}
