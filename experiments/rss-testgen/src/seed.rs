//! A total byte-seed decoder.
//!
//! [`SeedReader`] turns an arbitrary `&[u8]` into a stream of bounded generator
//! decisions. It is **total**: reads wrap around the seed (and an empty seed
//! reads as all zeroes), so *every* byte string decodes to a valid sequence of
//! choices and therefore a valid program. That totality is exactly what a
//! coverage-guided fuzzer (libFuzzer / cargo-fuzz) needs, and it lets the same
//! generator be driven by proptest's random byte vectors, by a persistent fuzz
//! corpus, and by a fixed seed for deterministic reproduction.

/// A cursor over a fuzz seed handing out bounded choices.
pub struct SeedReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> SeedReader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        SeedReader { bytes, pos: 0 }
    }

    /// The next raw byte (wrapping; `0` for an empty seed).
    pub fn next_byte(&mut self) -> u8 {
        if self.bytes.is_empty() {
            return 0;
        }
        let byte = self.bytes[self.pos % self.bytes.len()];
        self.pos = self.pos.wrapping_add(1);
        byte
    }

    /// A value in `0..n` (`0` when `n == 0`). Uses up to 4 bytes so the range can
    /// exceed 256 without bias toward small values.
    pub fn choice(&mut self, n: usize) -> usize {
        if n <= 1 {
            return 0;
        }
        let mut acc = 0usize;
        // Enough bytes to cover the modulus; capped so a huge `n` doesn't drain
        // the seed. `n` here is always small (variant counts, list lengths).
        let width = if n <= 256 { 1 } else { 4 };
        for _ in 0..width {
            acc = (acc << 8) | usize::from(self.next_byte());
        }
        acc % n
    }

    /// An `i64` in `lo..=hi` (inclusive). `lo > hi` yields `lo`.
    pub fn range_i64(&mut self, lo: i64, hi: i64) -> i64 {
        if lo >= hi {
            return lo;
        }
        let span = (hi as i128 - lo as i128 + 1) as u128;
        let mut acc = 0u128;
        for _ in 0..8 {
            acc = (acc << 8) | u128::from(self.next_byte());
        }
        lo + (acc % span) as i64
    }

    /// A coin flip.
    pub fn bool(&mut self) -> bool {
        self.next_byte() & 1 == 1
    }

    /// Pick an index in `0..weights.len()` proportional to the weights. An
    /// all-zero (or empty) weight slice falls back to a uniform choice.
    pub fn weighted(&mut self, weights: &[u32]) -> usize {
        let total: u64 = weights.iter().map(|w| u64::from(*w)).sum();
        if total == 0 {
            return self.choice(weights.len().max(1));
        }
        let mut pick = self.range_i64(0, (total - 1) as i64) as u64;
        for (index, &weight) in weights.iter().enumerate() {
            let weight = u64::from(weight);
            if pick < weight {
                return index;
            }
            pick -= weight;
        }
        weights.len() - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_seed_is_total() {
        let mut reader = SeedReader::new(&[]);
        assert_eq!(reader.choice(5), 0);
        assert_eq!(reader.range_i64(-3, 3), -3);
        assert!(!reader.bool());
    }

    #[test]
    fn choice_is_bounded() {
        let mut reader = SeedReader::new(&[1, 2, 3, 4, 5, 250, 99, 7]);
        for _ in 0..50 {
            assert!(reader.choice(7) < 7);
        }
    }

    #[test]
    fn range_is_inclusive_and_bounded() {
        let mut reader = SeedReader::new(&[0xff, 0x00, 0x7f, 0x10, 0xab]);
        for _ in 0..50 {
            let value = reader.range_i64(-10, 10);
            assert!((-10..=10).contains(&value));
        }
    }

    #[test]
    fn weighted_respects_zero_weights() {
        // Index 1 has all the weight, so it must always be chosen.
        let mut reader = SeedReader::new(&[3, 9, 200, 1, 77]);
        for _ in 0..20 {
            assert_eq!(reader.weighted(&[0, 5, 0]), 1);
        }
    }
}
