//! Hash utilities for Patina
//!
//! Provides hash implementations for general use across Patina components.
//!
//! ## License
//!
//! Copyright (c) Microsoft Corporation.
//!
//! SPDX-License-Identifier: Apache-2.0

use core::hash::Hasher;

/// A hasher that uses the Xorshift64* algorithm to generate a random number to xor with the input bytes.
///
/// **Note:** This hasher is not cryptographically secure. It is intended for fast, non-adversarial
/// hash-based change detection only.
///
/// [Xorshift64*](https://en.wikipedia.org/wiki/Xorshift#xorshift*)
pub struct Xorshift64starHasher {
    state: u64,
}

impl Xorshift64starHasher {
    /// Initialize the hasher with a seed.
    pub fn new(seed: u64) -> Self {
        Xorshift64starHasher { state: seed }
    }

    /// Generate a new random state.
    fn next_state(&mut self) -> u64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        self.state = self.state.wrapping_mul(0x2545F4914F6CDD1D);
        self.state
    }
}

/// The default seed is derived from [`compile_time::unix!`], so all `Default` instances within the
/// same binary share the same seed. This means two default-constructed hashers will produce
/// identical output for identical input.
impl Default for Xorshift64starHasher {
    fn default() -> Self {
        Xorshift64starHasher::new(compile_time::unix!())
    }
}

impl Hasher for Xorshift64starHasher {
    fn finish(&self) -> u64 {
        self.state
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.state ^= byte as u64;
            self.state = self.next_state();
        }
    }
}

#[cfg(test)]
#[coverage(off)]
mod tests {
    use super::*;

    #[test]
    fn test_different_seeds() {
        let seed1 = 12345;
        let seed2 = 54321;
        let mut hasher1 = Xorshift64starHasher::new(seed1);
        let mut hasher2 = Xorshift64starHasher::new(seed2);

        let num1 = hasher1.next_state();
        let num2 = hasher2.next_state();

        assert_ne!(num1, num2, "Random numbers should be different for different seeds");
    }

    #[test]
    fn test_same_seed() {
        let seed = 12345;
        let mut hasher1 = Xorshift64starHasher::new(seed);
        let mut hasher2 = Xorshift64starHasher::new(seed);

        let num1 = hasher1.next_state();
        let num2 = hasher2.next_state();

        assert_eq!(num1, num2, "Random numbers should be the same for the same seed");
    }

    #[test]
    fn test_default_hasher() {
        let hasher = Xorshift64starHasher::default();
        assert_ne!(hasher.state, 0, "Default hasher should have a non-zero seed");
    }

    #[test]
    fn test_write_and_finish() {
        let mut hasher1 = Xorshift64starHasher::new(42);
        let mut hasher2 = Xorshift64starHasher::new(42);

        hasher1.write(b"hello");
        hasher2.write(b"hello");
        assert_eq!(hasher1.finish(), hasher2.finish(), "Same input should produce same hash");

        let mut hasher3 = Xorshift64starHasher::new(42);
        hasher3.write(b"world");
        assert_ne!(hasher1.finish(), hasher3.finish(), "Different input should produce different hash");
    }

    #[test]
    fn test_detects_swapped_bytes() {
        // This is the key advantage over simple checksum: detecting byte swaps
        let mut hasher1 = Xorshift64starHasher::new(42);
        let mut hasher2 = Xorshift64starHasher::new(42);

        hasher1.write(&[1, 0]);
        hasher2.write(&[0, 1]);
        assert_ne!(hasher1.finish(), hasher2.finish(), "Swapped bytes should produce different hashes");
    }
}
