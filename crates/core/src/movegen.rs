//! Finds every legal move for a rack: hints, "no moves left" checks, a
//! future practice bot and the daily puzzle all use it. Line by line, with
//! cross-checks per empty square, walking the dictionary trie as tiles go
//! down (the anchor method of Appel and Jacobson, without the left-part
//! special case: every start square is tried and the trie prunes).

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::board::{Board, Dir, Layout, Pos};
use crate::letters::{ALPHABET, Letter, PlacedTile, Tile};
use crate::lexicon::{Dictionary, Node};
use crate::moves::{self, Placement, Scored};
use crate::tiles::TileSet;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub tiles: Vec<Placement>,
    pub scored: Scored,
}

/// Up to `limit` legal moves, highest base score first (ties in a stable
/// order, so hints are deterministic).
pub fn best_moves(
    board: &Board,
    layout: &Layout,
    tiles: &TileSet,
    rack: &[Tile],
    dict: &Dictionary,
    bingo_bonus: u32,
    limit: usize,
) -> Vec<Candidate> {
    let mut found: HashSet<Vec<Placement>> = HashSet::new();
    let mut counts = [0u8; ALPHABET];
    let mut blanks = 0u8;
    for t in rack {
        match t {
            Tile::Letter(l) => counts[l.index()] += 1,
            Tile::Blank => blanks += 1,
        }
    }
    let empty_board = board.is_empty();
    for dir in [Dir::Across, Dir::Down] {
        for line in 0..board.size {
            let g = Line::new(board, layout, dict, dir, line, empty_board);
            for start in 0..g.len {
                if start > 0 && g.cells[start - 1].is_some() {
                    continue; // a word starts after an empty square or the edge
                }
                if !g.anchor_reachable(start, rack.len()) {
                    continue;
                }
                let mut walk = Walk { g: &g, dict, counts, blanks, placed: Vec::new(), found: &mut found };
                walk.extend(start, start, dict.root(), false);
            }
        }
    }
    let mut out: Vec<Candidate> = found
        .into_iter()
        .filter_map(|tiles_| {
            moves::evaluate(board, layout, tiles, &tiles_, bingo_bonus, None)
                .ok()
                .map(|scored| Candidate { tiles: tiles_, scored })
        })
        .collect();
    out.sort_by(|a, b| b.scored.base.cmp(&a.scored.base).then_with(|| a.tiles.cmp(&b.tiles)));
    out.truncate(limit);
    out
}

/// One row (across) or column (down) with what may go on each empty square.
struct Line {
    len: usize,
    dir: Dir,
    line: u8,
    cells: Vec<Option<PlacedTile>>,
    /// Per empty square: letters that make a valid cross word (all when
    /// there is no cross word).
    allowed: Vec<[bool; ALPHABET]>,
    /// Empty squares a move must touch: next to a tile, or the start square
    /// on an empty board.
    anchor: Vec<bool>,
}

impl Line {
    fn new(board: &Board, layout: &Layout, dict: &Dictionary, dir: Dir, line: u8, empty_board: bool) -> Line {
        let len = board.size as usize;
        let pos = |i: usize| match dir {
            Dir::Across => Pos::new(line, i as u8),
            Dir::Down => Pos::new(i as u8, line),
        };
        let cells: Vec<Option<PlacedTile>> = (0..len).map(|i| board.get(pos(i))).collect();
        let mut allowed = vec![[true; ALPHABET]; len];
        let mut anchor = vec![false; len];
        for i in 0..len {
            if cells[i].is_some() {
                continue;
            }
            let p = pos(i);
            anchor[i] = if empty_board { p == layout.start() } else { board.has_neighbour(p) };
            let cross = dir.other();
            let mut before = Vec::new();
            let mut q = p;
            while let Some(prev) = board.step(q, cross, -1).filter(|&x| board.get(x).is_some()) {
                before.push(board.get(prev).unwrap().letter);
                q = prev;
            }
            before.reverse();
            let mut after = Vec::new();
            let mut q = p;
            while let Some(next) = board.step(q, cross, 1).filter(|&x| board.get(x).is_some()) {
                after.push(board.get(next).unwrap().letter);
                q = next;
            }
            if before.is_empty() && after.is_empty() {
                continue;
            }
            let prefix = dict.walk(&before);
            for l in Letter::all() {
                allowed[i][l.index()] = prefix
                    .and_then(|n| dict.step(n, l))
                    .and_then(|n| after.iter().try_fold(n, |n, &a| dict.step(n, a)))
                    .is_some_and(|n| dict.is_word(n));
            }
        }
        Line { len, dir, line, cells, allowed, anchor }
    }

    fn pos(&self, i: usize) -> Pos {
        match self.dir {
            Dir::Across => Pos::new(self.line, i as u8),
            Dir::Down => Pos::new(i as u8, self.line),
        }
    }

    /// Whether some anchor at or after `start` is within `tiles` empty squares.
    fn anchor_reachable(&self, start: usize, tiles: usize) -> bool {
        let mut empties = 0;
        for i in start..self.len {
            if self.cells[i].is_none() {
                empties += 1;
                if empties > tiles {
                    return false;
                }
                if self.anchor[i] {
                    return true;
                }
            }
        }
        false
    }
}

struct Walk<'a> {
    g: &'a Line,
    dict: &'a Dictionary,
    counts: [u8; ALPHABET],
    blanks: u8,
    placed: Vec<Placement>,
    found: &'a mut HashSet<Vec<Placement>>,
}

impl Walk<'_> {
    fn extend(&mut self, start: usize, i: usize, node: Node, anchored: bool) {
        let g = self.g;
        let at_end = i == g.len || g.cells[i].is_none();
        if at_end && anchored && !self.placed.is_empty() && i - start >= 2 && self.dict.is_word(node) {
            let mut key = self.placed.clone();
            key.sort();
            self.found.insert(key);
        }
        if i == g.len {
            return;
        }
        if let Some(t) = g.cells[i] {
            if let Some(n) = self.dict.step(node, t.letter) {
                self.extend(start, i + 1, n, anchored);
            }
            return;
        }
        let anchored = anchored || g.anchor[i];
        for l in Letter::all() {
            if !g.allowed[i][l.index()] {
                continue;
            }
            let Some(n) = self.dict.step(node, l) else { continue };
            if self.counts[l.index()] > 0 {
                self.counts[l.index()] -= 1;
                self.placed.push(Placement { pos: g.pos(i), tile: PlacedTile::letter(l) });
                self.extend(start, i + 1, n, anchored);
                self.placed.pop();
                self.counts[l.index()] += 1;
            }
            if self.blanks > 0 {
                self.blanks -= 1;
                self.placed.push(Placement { pos: g.pos(i), tile: PlacedTile::blank(l) });
                self.extend(start, i + 1, n, anchored);
                self.placed.pop();
                self.blanks += 1;
            }
        }
    }
}
