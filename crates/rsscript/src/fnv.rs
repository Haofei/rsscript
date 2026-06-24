/// FNV-1a (64-bit) hasher. The standard library's `HashMap` uses a *randomly
/// seeded* SipHash, which is DoS-resistant but (a) slow for the short keys the VM
/// hashes constantly (struct field names, small map keys) and (b) gives a
/// run-to-run *random* iteration order. FNV is far faster and, being fixed-seed,
/// deterministic — and `Map.keys()`/`Map.values()` expose iteration order directly,
/// so a stable order is a correctness requirement, not just a nicety (the backend
/// differential and reproducible review output both depend on it).
///
/// The trade-off is that a fixed-seed hash is, by construction, vulnerable to
/// worst-case collision flooding: an adversary who controls map *keys* can force
/// O(n) lookups. That is accepted because RSScript's VM is a local execution and
/// review tool — the program author controls the workload, not a remote attacker —
/// and DoS-resistance is mutually exclusive with the deterministic iteration order
/// we require here. If a VM `Map` ever backs an adversary-facing surface (e.g. a
/// long-lived server keying a map on untrusted request data), that surface needs
/// its own bounded/ordered structure rather than relying on this hasher.
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

pub(crate) type FnvBuildHasher = std::hash::BuildHasherDefault<FnvHasher>;
