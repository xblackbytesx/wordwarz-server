//! The game's only source of chance. Our own SplitMix64 and Fisher-Yates
//! instead of the `rand` crates, whose algorithms may change between
//! versions: a stored game must replay to the same bag forever (decision 5).

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng { state: seed }
    }

    /// A child stream for one purpose (board, bounties, bag), so adding a
    /// draw in one place never shifts another.
    pub fn derive(seed: u64, purpose: u64) -> Self {
        let mut r = Rng::new(seed ^ purpose.wrapping_mul(0xA076_1D64_78BD_642F));
        r.next_u64();
        r
    }

    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n` (Lemire's method with rejection, so no modulo bias).
    pub fn below(&mut self, n: usize) -> usize {
        assert!(n > 0, "below(0)");
        let n = n as u64;
        loop {
            let m = (self.next_u64() as u128) * (n as u128);
            let low = m as u64;
            if low >= n.wrapping_neg() % n {
                return (m >> 64) as usize;
            }
        }
    }

    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i + 1);
            items.swap(i, j);
        }
    }
}

pub(crate) mod purpose {
    pub const BOARD: u64 = 1;
    pub const BOUNTIES: u64 = 2;
    pub const BAG: u64 = 3;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pinned output: if this changes, every stored game replays differently.
    #[test]
    fn stream_is_stable() {
        let mut r = Rng::new(42);
        let got: Vec<u64> = (0..3).map(|_| r.next_u64()).collect();
        assert_eq!(got, vec![13679457532755275413, 2949826092126892291, 5139283748462763858]);
        let mut v: Vec<u32> = (0..10).collect();
        Rng::new(7).shuffle(&mut v);
        assert_eq!(v, vec![9, 5, 8, 6, 1, 2, 4, 7, 0, 3]);
    }

    #[test]
    fn below_stays_in_range() {
        let mut r = Rng::new(1);
        let mut seen = [0u32; 5];
        for _ in 0..5000 {
            seen[r.below(5)] += 1;
        }
        assert!(seen.iter().all(|&n| n > 800), "{seen:?}");
    }
}
