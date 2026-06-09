use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use std::io::Read;

use crate::fs::RuntimePath;

type HmacSha256 = Hmac<Sha256>;

pub fn hash_sha256_string(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn hash_sha256_bytes(value: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value);
    format!("{:x}", hasher.finalize())
}

pub fn hash_sha256_file<P: RuntimePath + ?Sized>(path: &P) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path.as_path())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub fn hmac_sha256_string(key: &str, value: &str) -> String {
    hmac_sha256_digest(key.as_bytes(), value.as_bytes())
}

pub fn hmac_sha256_bytes(key: &[u8], value: &[u8]) -> String {
    hmac_sha256_digest(key, value)
}

fn hmac_sha256_digest(key: &[u8], value: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(value);
    format!("{:x}", mac.finalize().into_bytes())
}
