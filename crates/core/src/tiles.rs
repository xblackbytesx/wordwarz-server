//! Tile sets: how many of each letter go in the bag and what each is worth.
//! Derived from the lexicons' own letter frequencies (decision 3) by
//! `TileSet::derive`, then frozen in `tilesets.rs`: a stored game must keep
//! its bag when a word list update shifts the frequencies.

use serde::{Deserialize, Serialize};

use crate::letters::{ALPHABET, Letter, Tile};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileSet {
    pub counts: [u8; ALPHABET],
    pub values: [u8; ALPHABET],
    pub blanks: u8,
}

/// Letter tiles in a derived set (blanks come on top).
pub const LETTER_TILES: u32 = 100;
pub const BLANKS: u8 = 2;
const MAX_VALUE: f64 = 10.0;
/// No letter gets more tiles than this, however common.
const MAX_COUNT: u32 = 16;

impl TileSet {
    pub fn value(&self, letter: Letter) -> u32 {
        self.values[letter.index()] as u32
    }

    pub fn tile_value(&self, tile: Tile) -> u32 {
        match tile {
            Tile::Letter(l) => self.value(l),
            Tile::Blank => 0,
        }
    }

    pub fn total(&self) -> u32 {
        self.counts.iter().map(|&c| c as u32).sum::<u32>() + self.blanks as u32
    }

    /// Every tile, in alphabetical order (the game shuffles it).
    pub fn bag(&self) -> Vec<Tile> {
        let mut bag = Vec::with_capacity(self.total() as usize);
        for l in Letter::all() {
            bag.extend(std::iter::repeat_n(Tile::Letter(l), self.counts[l.index()] as usize));
        }
        bag.extend(std::iter::repeat_n(Tile::Blank, self.blanks as usize));
        bag
    }

    /// The derivation rule, from letter occurrences in playable words:
    ///
    /// - **Counts**: `LETTER_TILES` shared in proportion to frequency by
    ///   largest remainder, every letter at least 1 and at most `MAX_COUNT`.
    /// - **Values**: rarer is worth more on a log scale: the most common
    ///   letter is 1, the rarest `MAX_VALUE`, i.e.
    ///   `1 + round(9 * ln(f_max / f) / ln(f_max / f_min))`.
    pub fn derive(freq: &[f64; ALPHABET]) -> TileSet {
        let sum: f64 = freq.iter().sum();
        assert!(sum > 0.0, "empty frequency table");
        let share: Vec<f64> = freq.iter().map(|f| f / sum).collect();

        // Counts: floor of the ideal share with the clamps, then hand out the
        // rest by largest remainder among letters still below the cap.
        let ideal: Vec<f64> = share.iter().map(|s| s * LETTER_TILES as f64).collect();
        let mut counts: Vec<u32> = ideal.iter().map(|x| (x.floor() as u32).clamp(1, MAX_COUNT)).collect();
        let mut order: Vec<usize> = (0..ALPHABET).collect();
        order.sort_by(|&a, &b| {
            let ra = ideal[a] - ideal[a].floor();
            let rb = ideal[b] - ideal[b].floor();
            rb.total_cmp(&ra).then(a.cmp(&b))
        });
        let mut i = 0;
        while counts.iter().sum::<u32>() < LETTER_TILES {
            let l = order[i % ALPHABET];
            if counts[l] < MAX_COUNT {
                counts[l] += 1;
            }
            i += 1;
        }
        while counts.iter().sum::<u32>() > LETTER_TILES {
            // Only possible through the minimum of 1: take from the biggest.
            let big = (0..ALPHABET).max_by_key(|&l| (counts[l], std::cmp::Reverse(l))).unwrap();
            counts[big] -= 1;
        }

        let floor = 1e-6;
        let fmax = share.iter().cloned().fold(0.0, f64::max);
        let fmin = share.iter().cloned().map(|f| f.max(floor)).fold(f64::MAX, f64::min);
        let span = (fmax / fmin).ln().max(f64::EPSILON);
        let values: Vec<u8> = share
            .iter()
            .map(|&f| (1.0 + ((MAX_VALUE - 1.0) * (fmax / f.max(floor)).ln() / span).round()) as u8)
            .collect();

        TileSet {
            counts: counts.iter().map(|&c| c as u8).collect::<Vec<_>>().try_into().unwrap(),
            values: values.try_into().unwrap(),
            blanks: BLANKS,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_keeps_totals_and_order() {
        let mut freq = [0.0; ALPHABET];
        for (i, f) in freq.iter_mut().enumerate() {
            *f = (ALPHABET - i) as f64 * (ALPHABET - i) as f64;
        }
        let t = TileSet::derive(&freq);
        assert_eq!(t.counts.iter().map(|&c| c as u32).sum::<u32>(), LETTER_TILES);
        assert!(t.counts.iter().all(|&c| (1..=MAX_COUNT as u8).contains(&c)));
        assert_eq!(t.values[0], 1);
        assert_eq!(t.values[ALPHABET - 1], 10);
        assert!(t.values.windows(2).all(|w| w[0] <= w[1]));
        assert!(t.counts.windows(2).all(|w| w[0] >= w[1]));
        assert_eq!(t.total(), LETTER_TILES + BLANKS as u32);
        assert_eq!(t.bag().len() as u32, t.total());
    }
}
