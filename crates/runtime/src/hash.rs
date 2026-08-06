use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use sha3::{
    Sha3_224, Sha3_256, Shake128,
    digest::{ExtendableOutput, Update, XofReader},
};
#[cfg(feature = "legacy-host")]
use std::io::Read;

#[cfg(feature = "legacy-host")]
use crate::fs::RuntimePath;

type HmacSha256 = Hmac<Sha256>;
const MAX_HASH_OUTPUT_BYTES: usize = 64 * 1024 * 1024;

pub fn hash_sha256_string(value: &str) -> String {
    let mut hasher = Sha256::new();
    Digest::update(&mut hasher, value.as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn hash_sha256_bytes(value: &[u8]) -> String {
    let mut hasher = Sha256::new();
    Digest::update(&mut hasher, value);
    format!("{:x}", hasher.finalize())
}

pub fn hash_sha3_224_bytes(value: &[u8]) -> Vec<u8> {
    let mut hasher = Sha3_224::new();
    Update::update(&mut hasher, value);
    hasher.finalize().to_vec()
}

pub fn hash_sha3_256_bytes(value: &[u8]) -> Vec<u8> {
    let mut hasher = Sha3_256::new();
    Update::update(&mut hasher, value);
    hasher.finalize().to_vec()
}

pub fn hash_shake128_bytes(value: &[u8], out_len: i64) -> Vec<u8> {
    let out_len = usize::try_from(out_len).unwrap_or_else(|_| {
        crate::error::panic_runtime_error(crate::error::invalid_argument_error(format!(
            "Hash.shake128_bytes output length must be non-negative, got {out_len}"
        )))
    });
    if out_len > MAX_HASH_OUTPUT_BYTES {
        crate::error::panic_runtime_error(crate::error::invalid_argument_error(format!(
            "Hash.shake128_bytes output exceeds the {MAX_HASH_OUTPUT_BYTES} byte limit"
        )));
    }
    let mut hasher = Shake128::default();
    Update::update(&mut hasher, value);
    let mut reader = hasher.finalize_xof();
    let mut out = vec![0u8; out_len];
    XofReader::read(&mut reader, &mut out);
    out
}

#[cfg(feature = "legacy-host")]
pub fn hash_sha256_file<P: RuntimePath + ?Sized>(path: &P) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path.as_path())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];
    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        Digest::update(&mut hasher, &buffer[..bytes_read]);
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
    Mac::update(&mut mac, value);
    format!("{:x}", mac.finalize().into_bytes())
}
