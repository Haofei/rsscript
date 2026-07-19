/// FNV-1a (64-bit) used only for deterministic local structural sub-hashes.
/// VM `Map`/`Set` tables deliberately use the standard randomized `HashMap`
/// hasher so attacker-controlled keys cannot rely on a fixed table seed.
#[derive(Clone, Copy)]
pub(crate) struct FnvHasher(u64);

impl Default for FnvHasher {
    fn default() -> Self {
        FnvHasher(0xcbf2_9ce4_8422_2325)
    }
}

impl std::hash::Hasher for FnvHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let mut hash = self.0;
        for &byte in bytes {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        self.0 = hash;
    }
}
