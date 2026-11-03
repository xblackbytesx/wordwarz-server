//! Placing tiles: the geometry rules, the words a placement forms and their
//! base score. Modifiers that depend on the game (bounties, handicap,
//! comeback) are added by `game`, on top of this.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::board::{Board, Dir, Layout, Pos};
use crate::letters::{Letter, PlacedTile, spell};
use crate::lexicon::Dictionary;
use crate::tiles::TileSet;

/// Tiles used in one move for the bingo bonus, whatever the rack size.
pub const BINGO_TILES: usize = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Placement {
    pub pos: Pos,
    pub tile: PlacedTile,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FormedWord {
    pub word: String,
    pub start: Pos,
    pub dir: Dir,
    pub score: u32,
}

/// A placement that passed the rules, with its base score.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scored {
    /// The main word first, then cross words in placement order.
    pub words: Vec<FormedWord>,
    pub tiles_used: usize,
    pub bingo: u32,
    /// Words plus bingo.
    pub base: u32,
}

/// JSON: `{"reason": "notWords", "words": [...]}`. Its own tag, so it nests
/// inside `ActionError`'s `{"error": "move", ...}` without a duplicate key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "reason", rename_all = "camelCase")]
pub enum MoveError {
    #[error("place at least one tile")]
    Empty,
    #[error("a tile is off the board")]
    OffBoard,
    #[error("a square is already taken")]
    Occupied,
    #[error("two tiles on one square")]
    Duplicate,
    #[error("tiles must be in one row or column")]
    NotInLine,
    #[error("tiles must not leave gaps")]
    Gap,
    #[error("the first word must cover the centre square")]
    MissesStart,
    #[error("the word must touch tiles already on the board")]
    NotConnected,
    #[error("a word needs at least two letters")]
    TooShort,
    #[error("those tiles are not on your rack")]
    NotOnRack,
    #[error("not in the word list: {words:?}")]
    NotWords { words: Vec<String> },
}

/// Checks a placement and scores it. `dict = None` skips the word check:
/// replays trust words that were valid when played (decision 6).
pub fn evaluate(
    board: &Board,
    layout: &Layout,
    tiles: &TileSet,
    placements: &[Placement],
    bingo_bonus: u32,
    dict: Option<&Dictionary>,
) -> Result<Scored, MoveError> {
    if placements.is_empty() {
        return Err(MoveError::Empty);
    }
    let mut new: HashMap<Pos, PlacedTile> = HashMap::with_capacity(placements.len());
    for p in placements {
        if !board.contains(p.pos) {
            return Err(MoveError::OffBoard);
        }
        if board.get(p.pos).is_some() {
            return Err(MoveError::Occupied);
        }
        if new.insert(p.pos, p.tile).is_some() {
            return Err(MoveError::Duplicate);
        }
    }

    let first = placements[0].pos;
    let dir = if placements.len() == 1 {
        if span(board, &new, first, Dir::Across).len() >= 2 { Dir::Across } else { Dir::Down }
    } else if placements.iter().all(|p| p.pos.row == first.row) {
        Dir::Across
    } else if placements.iter().all(|p| p.pos.col == first.col) {
        Dir::Down
    } else {
        return Err(MoveError::NotInLine);
    };

    // No gaps: every square between the outermost new tiles is filled.
    let key = |p: Pos| if dir == Dir::Across { p.col } else { p.row };
    let lo = placements.iter().map(|p| p.pos).min_by_key(|&p| key(p)).unwrap();
    let hi_key = placements.iter().map(|p| key(p.pos)).max().unwrap();
    let mut cur = lo;
    while key(cur) < hi_key {
        cur = board.step(cur, dir, 1).expect("hi is on the board");
        if board.get(cur).is_none() && !new.contains_key(&cur) {
            return Err(MoveError::Gap);
        }
    }

    if board.is_empty() {
        if !new.contains_key(&layout.start()) {
            return Err(MoveError::MissesStart);
        }
    } else if !placements.iter().any(|p| board.has_neighbour(p.pos)) {
        return Err(MoveError::NotConnected);
    }

    let mut words = Vec::new();
    let main = span(board, &new, first, dir);
    if main.len() >= 2 {
        words.push(score_word(board, layout, tiles, &new, &main, dir));
    }
    for p in placements {
        let cross = span(board, &new, p.pos, dir.other());
        if cross.len() >= 2 {
            words.push(score_word(board, layout, tiles, &new, &cross, dir.other()));
        }
    }
    if words.is_empty() {
        return Err(MoveError::TooShort);
    }

    if let Some(dict) = dict {
        let bad: Vec<String> = words.iter().filter(|w| !dict.contains_str(&w.word)).map(|w| w.word.clone()).collect();
        if !bad.is_empty() {
            return Err(MoveError::NotWords { words: bad });
        }
    }

    let bingo = if placements.len() >= BINGO_TILES { bingo_bonus } else { 0 };
    let base = words.iter().map(|w| w.score).sum::<u32>() + bingo;
    Ok(Scored { words, tiles_used: placements.len(), bingo, base })
}

/// The run of filled squares through `at` along `dir`, old and new tiles alike.
fn span(board: &Board, new: &HashMap<Pos, PlacedTile>, at: Pos, dir: Dir) -> Vec<Pos> {
    let filled = |p: Pos| board.get(p).is_some() || new.contains_key(&p);
    let mut start = at;
    while let Some(prev) = board.step(start, dir, -1).filter(|&p| filled(p)) {
        start = prev;
    }
    let mut out = vec![start];
    let mut cur = start;
    while let Some(next) = board.step(cur, dir, 1).filter(|&p| filled(p)) {
        out.push(next);
        cur = next;
    }
    out
}

fn score_word(
    board: &Board,
    layout: &Layout,
    tiles: &TileSet,
    new: &HashMap<Pos, PlacedTile>,
    squares: &[Pos],
    dir: Dir,
) -> FormedWord {
    let mut letters: Vec<Letter> = Vec::with_capacity(squares.len());
    let mut sum = 0;
    let mut word_mult = 1;
    for &p in squares {
        let (tile, fresh) = match new.get(&p) {
            Some(t) => (*t, true),
            None => (board.get(p).expect("span only covers filled squares"), false),
        };
        letters.push(tile.letter);
        let value = if tile.blank { 0 } else { tiles.value(tile.letter) };
        let premium = if fresh { layout.get(p) } else { None };
        sum += value * premium.map_or(1, |p| p.letter_multiplier());
        word_mult *= premium.map_or(1, |p| p.word_multiplier());
    }
    FormedWord { word: spell(&letters), start: squares[0], dir, score: sum * word_mult }
}
