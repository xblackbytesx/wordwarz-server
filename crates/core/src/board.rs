//! Board geometry and premium layouts. The fixed layouts are our own
//! (decision 2), described by one octant and mirrored, so every layout is
//! symmetric across both axes and both diagonals by construction.

use serde::{Deserialize, Serialize};

use crate::letters::PlacedTile;
use crate::rng::Rng;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Premium {
    #[serde(rename = "dl")]
    DoubleLetter,
    #[serde(rename = "tl")]
    TripleLetter,
    #[serde(rename = "dw")]
    DoubleWord,
    #[serde(rename = "tw")]
    TripleWord,
}

impl Premium {
    pub fn letter_multiplier(self) -> u32 {
        match self {
            Premium::DoubleLetter => 2,
            Premium::TripleLetter => 3,
            _ => 1,
        }
    }

    pub fn word_multiplier(self) -> u32 {
        match self {
            Premium::DoubleWord => 2,
            Premium::TripleWord => 3,
            _ => 1,
        }
    }

    fn from_code(c: u8) -> Premium {
        match c {
            b'd' => Premium::DoubleLetter,
            b't' => Premium::TripleLetter,
            b'D' => Premium::DoubleWord,
            b'T' => Premium::TripleWord,
            _ => unreachable!("premium code {c}"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Pos {
    pub row: u8,
    pub col: u8,
}

impl Pos {
    pub const fn new(row: u8, col: u8) -> Self {
        Pos { row, col }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Dir {
    #[serde(rename = "across")]
    Across,
    #[serde(rename = "down")]
    Down,
}

impl Dir {
    pub fn other(self) -> Dir {
        match self {
            Dir::Across => Dir::Down,
            Dir::Down => Dir::Across,
        }
    }
}

/// Supported board sizes. Odd, so there is a centre square to start on.
pub const SIZES: [u8; 3] = [11, 15, 19];

/// Fixed layouts as octant entries `(row, col, code)` with `row <= col <= mid`
/// (codes: `d` DL, `t` TL, `D` DW, `T` TW). The centre is the plain start square.
const OCTANT_11: &[(u8, u8, u8)] =
    &[(0, 0, b't'), (0, 3, b'T'), (0, 5, b'd'), (1, 1, b'D'), (2, 2, b'd'), (2, 4, b't'), (3, 3, b'D'), (4, 4, b'd')];

const OCTANT_15: &[(u8, u8, u8)] = &[
    (0, 0, b't'),
    (0, 4, b'T'),
    (0, 7, b'd'),
    (1, 1, b'd'),
    (1, 5, b't'),
    (2, 2, b'D'),
    (2, 6, b'd'),
    (3, 3, b't'),
    (3, 7, b'D'),
    (4, 4, b'D'),
    (4, 6, b'd'),
    (5, 5, b't'),
];

const OCTANT_19: &[(u8, u8, u8)] = &[
    (0, 0, b't'),
    (0, 5, b'T'),
    (0, 9, b'd'),
    (1, 1, b'D'),
    (1, 7, b't'),
    (2, 2, b'd'),
    (2, 8, b'd'),
    (3, 3, b'D'),
    (3, 9, b'D'),
    (4, 4, b't'),
    (4, 7, b'd'),
    (5, 5, b'D'),
    (6, 6, b't'),
    (6, 8, b'd'),
    (7, 7, b'd'),
];

/// The premium squares of one board. Immutable for the whole game.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layout {
    pub size: u8,
    /// Row-major, `size * size` entries.
    pub cells: Vec<Option<Premium>>,
}

impl Layout {
    /// Our fixed layout for a supported size.
    pub fn classic(size: u8) -> Option<Layout> {
        let octant = match size {
            11 => OCTANT_11,
            15 => OCTANT_15,
            19 => OCTANT_19,
            _ => return None,
        };
        let mut l = Layout::empty(size);
        for &(r, c, code) in octant {
            l.set_mirrored(r, c, Some(Premium::from_code(code)));
        }
        Some(l)
    }

    /// A layout drawn from `seed` with the same number of each premium in
    /// the octant as the classic one, fully symmetric, with fairness rules:
    /// no two premiums share an edge, no word premium touches the start
    /// square, and triple words stay in the outer half. Falls back to the
    /// classic layout if no draw satisfies them (never seen in tests).
    pub fn random(size: u8, seed: u64) -> Option<Layout> {
        let classic = Layout::classic(size)?;
        let octant = match size {
            11 => OCTANT_11,
            15 => OCTANT_15,
            _ => OCTANT_19,
        };
        let mid = size / 2;
        let mut rng = Rng::derive(seed, crate::rng::purpose::BOARD);
        let mut candidates: Vec<(u8, u8)> = Vec::new();
        for r in 0..=mid {
            for c in r..=mid {
                if (r, c) != (mid, mid) {
                    candidates.push((r, c));
                }
            }
        }
        // Word multipliers first: they are the most constrained.
        let mut wanted: Vec<u8> = octant.iter().map(|e| e.2).collect();
        wanted.sort_by_key(|c| match c {
            b'T' => 0,
            b'D' => 1,
            b't' => 2,
            _ => 3,
        });
        'attempt: for _ in 0..500 {
            let mut l = Layout::empty(size);
            for &code in &wanted {
                let premium = Premium::from_code(code);
                let mut tries = 0;
                loop {
                    tries += 1;
                    if tries > 200 {
                        continue 'attempt;
                    }
                    let (r, c) = candidates[rng.below(candidates.len())];
                    if l.get(Pos::new(r, c)).is_some() {
                        continue;
                    }
                    let dist_from_centre = (mid - r).max(mid - c);
                    if premium == Premium::TripleWord && dist_from_centre < mid.div_ceil(2) {
                        continue;
                    }
                    if premium.word_multiplier() > 1 && dist_from_centre <= 1 {
                        continue;
                    }
                    let mut trial = l.clone();
                    trial.set_mirrored(r, c, Some(premium));
                    if trial.has_adjacent_premiums() {
                        continue;
                    }
                    l = trial;
                    break;
                }
            }
            return Some(l);
        }
        Some(classic)
    }

    fn empty(size: u8) -> Layout {
        Layout { size, cells: vec![None; size as usize * size as usize] }
    }

    fn set_mirrored(&mut self, r: u8, c: u8, p: Option<Premium>) {
        let last = self.size - 1;
        for (a, b) in [(r, c), (c, r)] {
            for (x, y) in [(a, b), (last - a, b), (a, last - b), (last - a, last - b)] {
                let i = self.index(Pos::new(x, y));
                self.cells[i] = p;
            }
        }
    }

    fn has_adjacent_premiums(&self) -> bool {
        let n = self.size;
        (0..n).any(|r| {
            (0..n).any(|c| {
                self.get(Pos::new(r, c)).is_some()
                    && ((c + 1 < n && self.get(Pos::new(r, c + 1)).is_some())
                        || (r + 1 < n && self.get(Pos::new(r + 1, c)).is_some()))
            })
        })
    }

    pub fn start(&self) -> Pos {
        Pos::new(self.size / 2, self.size / 2)
    }

    pub fn contains(&self, p: Pos) -> bool {
        p.row < self.size && p.col < self.size
    }

    pub fn index(&self, p: Pos) -> usize {
        p.row as usize * self.size as usize + p.col as usize
    }

    pub fn get(&self, p: Pos) -> Option<Premium> {
        self.cells[self.index(p)]
    }

    /// Counts of (DL, TL, DW, TW).
    pub fn census(&self) -> (usize, usize, usize, usize) {
        let count = |want| self.cells.iter().filter(|c| **c == Some(want)).count();
        (
            count(Premium::DoubleLetter),
            count(Premium::TripleLetter),
            count(Premium::DoubleWord),
            count(Premium::TripleWord),
        )
    }

    /// Text picture for tests and logs: `.` plain, `*` start, codes as above.
    pub fn picture(&self) -> String {
        let mut s = String::new();
        for r in 0..self.size {
            for c in 0..self.size {
                let p = Pos::new(r, c);
                s.push(match self.get(p) {
                    _ if p == self.start() => '*',
                    None => '.',
                    Some(Premium::DoubleLetter) => 'd',
                    Some(Premium::TripleLetter) => 't',
                    Some(Premium::DoubleWord) => 'D',
                    Some(Premium::TripleWord) => 'T',
                });
            }
            s.push('\n');
        }
        s
    }
}

/// The tiles on the board.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Board {
    pub size: u8,
    /// Row-major, `size * size` entries.
    pub cells: Vec<Option<PlacedTile>>,
}

impl Board {
    pub fn new(size: u8) -> Self {
        Board { size, cells: vec![None; size as usize * size as usize] }
    }

    pub fn contains(&self, p: Pos) -> bool {
        p.row < self.size && p.col < self.size
    }

    pub fn get(&self, p: Pos) -> Option<PlacedTile> {
        if self.contains(p) { self.cells[p.row as usize * self.size as usize + p.col as usize] } else { None }
    }

    pub fn set(&mut self, p: Pos, t: PlacedTile) {
        let i = p.row as usize * self.size as usize + p.col as usize;
        self.cells[i] = Some(t);
    }

    pub fn is_empty(&self) -> bool {
        self.cells.iter().all(Option::is_none)
    }

    /// The neighbour of `p` in `dir`, `delta` steps (+1 or -1), if on the board.
    pub fn step(&self, p: Pos, dir: Dir, delta: i32) -> Option<Pos> {
        let (r, c) = match dir {
            Dir::Across => (p.row as i32, p.col as i32 + delta),
            Dir::Down => (p.row as i32 + delta, p.col as i32),
        };
        (r >= 0 && c >= 0 && r < self.size as i32 && c < self.size as i32).then(|| Pos::new(r as u8, c as u8))
    }

    pub fn has_neighbour(&self, p: Pos) -> bool {
        [Dir::Across, Dir::Down]
            .into_iter()
            .flat_map(|d| [self.step(p, d, -1), self.step(p, d, 1)])
            .flatten()
            .any(|q| self.get(q).is_some())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symmetric(l: &Layout) -> bool {
        let n = l.size;
        (0..n).all(|r| {
            (0..n).all(|c| {
                let p = l.get(Pos::new(r, c));
                p == l.get(Pos::new(c, r)) && p == l.get(Pos::new(n - 1 - r, c)) && p == l.get(Pos::new(r, n - 1 - c))
            })
        })
    }

    #[test]
    fn classic_layouts_are_symmetric_and_fair() {
        for size in SIZES {
            let l = Layout::classic(size).unwrap();
            assert!(symmetric(&l), "{size}");
            assert!(!l.has_adjacent_premiums(), "{size}\n{}", l.picture());
            assert_eq!(l.get(l.start()), None);
        }
        assert_eq!(Layout::classic(15).unwrap().census(), (24, 20, 12, 8));
        assert_eq!(Layout::classic(13), None);
    }

    #[test]
    fn random_layouts_follow_the_rules() {
        for size in SIZES {
            for seed in 0..200 {
                let l = Layout::random(size, seed).unwrap();
                assert!(symmetric(&l));
                assert!(!l.has_adjacent_premiums());
                assert_eq!(l.get(l.start()), None);
            }
            assert_eq!(Layout::random(size, 9), Layout::random(size, 9));
            assert_ne!(Layout::random(size, 9), Layout::random(size, 10));
        }
    }
}
