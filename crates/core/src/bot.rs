//! A practice opponent: picks one of the move generator's candidates by
//! strength. Deterministic for a given seed, so a computer seat in a saved
//! local game behaves the same after a restart.

use serde::{Deserialize, Serialize};

use crate::movegen::Candidate;
use crate::rng::Rng;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    /// A modest move from the lower half: fine company for a young child.
    Easy,
    /// Usually a good move, rarely the best.
    Medium,
    /// Always the highest-scoring move.
    Hard,
}

/// How many candidates the bot looks at; the generator should be asked for this many.
pub const POOL: usize = 60;

/// Picks the candidate to play, or None when there is none (swap or pass).
/// `candidates` must be best first, as `movegen::best_moves` returns them.
pub fn choose(candidates: &[Candidate], level: Level, seed: u64) -> Option<&Candidate> {
    if candidates.is_empty() {
        return None;
    }
    let n = candidates.len();
    let mut rng = Rng::new(seed);
    let i = match level {
        Level::Hard => 0,
        // Somewhere in the best quarter.
        Level::Medium => rng.below(n.div_ceil(4)),
        // The lower half, but never the very worst two-letter dregs if more exist.
        Level::Easy => {
            let start = n / 2;
            let end = if n > 4 { n - 1 } else { n };
            start + rng.below((end - start).max(1))
        }
    };
    candidates.get(i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::moves::Scored;

    fn cands(n: u32) -> Vec<Candidate> {
        (0..n)
            .rev()
            .map(|s| Candidate {
                tiles: Vec::new(),
                scored: Scored { words: Vec::new(), tiles_used: 1, bingo: 0, base: s },
            })
            .collect()
    }

    #[test]
    fn levels_pick_where_they_should() {
        let c = cands(40);
        assert_eq!(choose(&c, Level::Hard, 1).unwrap().scored.base, 39);
        for seed in 0..50 {
            assert!(choose(&c, Level::Medium, seed).unwrap().scored.base >= 29);
            let easy = choose(&c, Level::Easy, seed).unwrap().scored.base;
            assert!((1..=19).contains(&easy), "{easy}");
        }
        assert!(choose(&[], Level::Hard, 1).is_none());
        assert_eq!(choose(&cands(1), Level::Easy, 3).unwrap().scored.base, 0);
    }
}
