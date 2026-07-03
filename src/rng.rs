//! Deterministic RNG: splitmix64 seeded via FNV-1a over string parts.
//!
//! SAMPLE must be reproducible across machines and versions — "the same
//! seed draws the same world" is part of the query contract. No platform
//! RNG, no hash randomization: pure integer arithmetic.

pub struct Rng(u64);

impl Rng {
    pub fn from_parts(parts: &[&str]) -> Self {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for p in parts {
            for b in p.as_bytes() {
                h ^= u64::from(*b);
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
            // Separator so ["ab","c"] != ["a","bc"].
            h ^= 0xff;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        Rng(h)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn choice_index(&mut self, len: usize) -> usize {
        ((self.next_f64() * len as f64) as usize).min(len - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_and_part_sensitive() {
        let a: Vec<u64> = {
            let mut r = Rng::from_parts(&["e1", "42"]);
            (0..4).map(|_| r.next_u64()).collect()
        };
        let b: Vec<u64> = {
            let mut r = Rng::from_parts(&["e1", "42"]);
            (0..4).map(|_| r.next_u64()).collect()
        };
        assert_eq!(a, b);
        let mut c = Rng::from_parts(&["e14", "2"]);
        assert_ne!(a[0], c.next_u64());
    }

    #[test]
    fn f64_in_unit_interval() {
        let mut r = Rng::from_parts(&["x"]);
        for _ in 0..1000 {
            let v = r.next_f64();
            assert!((0.0..1.0).contains(&v));
        }
    }
}
