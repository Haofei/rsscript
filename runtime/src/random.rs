use rand::Rng;

pub fn uuid_new_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

pub fn random_int(min: i64, max: i64) -> i64 {
    let mut rng = rand::thread_rng();
    rng.gen_range(min..=max)
}

pub fn random_bool() -> bool {
    let mut rng = rand::thread_rng();
    rng.r#gen()
}

pub fn random_float() -> f64 {
    let mut rng = rand::thread_rng();
    rng.r#gen()
}

pub fn random_bytes(len: i64) -> Vec<u8> {
    let mut rng = rand::thread_rng();
    let mut bytes = vec![0u8; len.max(0) as usize];
    rng.fill(bytes.as_mut_slice());
    bytes
}

pub fn random_string(len: i64) -> String {
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    (0..len.max(0))
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}
