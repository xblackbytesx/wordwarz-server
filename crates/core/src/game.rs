//! The game state machine. A game is fully determined by its config, its
//! seed and its action log (decision 6): the server stores those and may
//! cache the state; `Game::replay` rebuilds it without a dictionary.

use serde::{Deserialize, Serialize};

use crate::board::{Board, Layout, Pos};
use crate::config::{ConfigError, GameConfig};
use crate::letters::Tile;
use crate::lexicon::Dictionary;
use crate::movegen::{self, Candidate};
use crate::moves::{self, FormedWord, MoveError, Placement};
use crate::rng::{Rng, purpose};
use crate::tiles::TileSet;
use crate::tilesets;

/// Every scoreless turn in a row counts; the game ends after this many
/// rounds of them (every active player passed, swapped or timed out twice).
pub const SCORELESS_ROUNDS: u32 = 2;
/// Points a hidden bounty can hold, drawn per bounty.
pub const BOUNTY_POINTS: [u32; 4] = [10, 15, 20, 25];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Action {
    Play {
        tiles: Vec<Placement>,
    },
    Swap {
        tiles: Vec<Tile>,
    },
    Pass,
    /// The server's turn timer ran out; scores like a pass.
    Timeout,
    /// Allowed at any time, not only on your turn.
    Resign,
    /// Spends a hint token on your turn; not a turn itself.
    Hint,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "error", rename_all = "camelCase")]
pub enum ActionError {
    #[error("the game is over")]
    GameOver,
    #[error("it is not your turn")]
    NotYourTurn,
    #[error("no such seat")]
    NoSuchSeat,
    #[error("you resigned from this game")]
    Resigned,
    #[error("the bag has too few tiles to swap")]
    BagTooSmall,
    #[error("no hints left")]
    NoHints,
    #[error("{0}")]
    Move(#[from] MoveError),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreBreakdown {
    /// Words plus bingo.
    pub base: u32,
    pub bingo: u32,
    pub bounty: u32,
    /// From the seat's score percent (negative below 100 %).
    pub handicap: i32,
    pub comeback: u32,
    pub total: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TurnKind {
    Play { tiles: Vec<Placement>, words: Vec<FormedWord>, score: ScoreBreakdown, bounties: Vec<Pos> },
    Swap { count: usize },
    Pass,
    Timeout,
    Resign,
}

/// One entry in the public history (what the move list and replays show).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Turn {
    pub seat: usize,
    #[serde(flatten)]
    pub kind: TurnKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "camelCase")]
pub enum EndReason {
    /// Someone played their last tile with the bag empty.
    RackEmptied { seat: usize },
    /// Too many scoreless turns in a row.
    Scoreless,
    /// Resignations left one player or team.
    Resigned,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Status {
    Active,
    Finished {
        #[serde(flatten)]
        reason: EndReason,
        /// Seats that won (several on a tie; both partners in team games).
        winners: Vec<usize>,
        /// Rack penalties and bonuses applied at the end, per seat.
        adjustments: Vec<i32>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Seat {
    pub rack: Vec<Tile>,
    pub score: i32,
    pub resigned: bool,
    pub hints_left: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bounty {
    pub pos: Pos,
    pub points: u32,
    pub claimed_by: Option<usize>,
}

/// What a successful action did, for the caller to announce.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Turn(Turn),
    /// The hint (None when the rack has no legal move, or no dictionary was given).
    Hint(Option<Candidate>),
}

/// The authoritative state. Holds the bag order and every rack: only the
/// server ever has one; clients get a `GameView`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    pub config: GameConfig,
    pub seed: u64,
    pub layout: Layout,
    pub board: Board,
    pub seats: Vec<Seat>,
    /// Draws come off the end.
    pub bag: Vec<Tile>,
    pub turn: usize,
    pub scoreless: u32,
    pub bounties: Vec<Bounty>,
    pub status: Status,
    pub history: Vec<Turn>,
    rng: Rng,
}

impl Game {
    pub fn new(config: GameConfig, seed: u64) -> Result<Game, ConfigError> {
        config.validate()?;
        let size = config.board.size;
        let layout = if config.board.random { Layout::random(size, seed) } else { Layout::classic(size) }
            .expect("validated size");
        let mut rng = Rng::derive(seed, purpose::BAG);
        let mut bag = tilesets::for_language(config.language).bag();
        rng.shuffle(&mut bag);

        let bounties = place_bounties(&layout, config.bounties, seed);
        let mut seats: Vec<Seat> = config
            .players
            .iter()
            .map(|_| Seat { rack: Vec::new(), score: 0, resigned: false, hints_left: config.hint_tokens })
            .collect();
        for (seat, setup) in seats.iter_mut().zip(&config.players) {
            for _ in 0..setup.rack_size {
                seat.rack.push(bag.pop().expect("validate keeps racks under half the bag"));
            }
        }
        Ok(Game {
            board: Board::new(size),
            layout,
            seats,
            bag,
            turn: 0,
            scoreless: 0,
            bounties,
            status: Status::Active,
            history: Vec::new(),
            rng,
            config,
            seed,
        })
    }

    /// Rebuilds a game from its log without checking words (they were
    /// checked when played). Fails if the log does not fit the game, which
    /// means the log or the rules version is corrupt.
    pub fn replay(config: GameConfig, seed: u64, log: &[(usize, Action)]) -> Result<Game, ReplayError> {
        let mut g = Game::new(config, seed).map_err(ReplayError::Config)?;
        for (i, (seat, action)) in log.iter().enumerate() {
            g.apply(*seat, action.clone(), None).map_err(|error| ReplayError::Action { index: i, error })?;
        }
        Ok(g)
    }

    pub fn tiles(&self) -> &'static TileSet {
        tilesets::for_language(self.config.language)
    }

    pub fn is_over(&self) -> bool {
        matches!(self.status, Status::Finished { .. })
    }

    /// Applies one action by `seat`. `dict` is required to check words for a
    /// play and to produce a hint; replays pass `None`.
    pub fn apply(&mut self, seat: usize, action: Action, dict: Option<&Dictionary>) -> Result<Outcome, ActionError> {
        if self.is_over() {
            return Err(ActionError::GameOver);
        }
        if seat >= self.seats.len() {
            return Err(ActionError::NoSuchSeat);
        }
        if self.seats[seat].resigned {
            return Err(ActionError::Resigned);
        }
        if action == Action::Resign {
            return Ok(Outcome::Turn(self.resign(seat)));
        }
        if seat != self.turn {
            return Err(ActionError::NotYourTurn);
        }
        let kind = match action {
            Action::Hint => {
                if self.seats[seat].hints_left == 0 {
                    return Err(ActionError::NoHints);
                }
                self.seats[seat].hints_left -= 1;
                let best = dict.and_then(|d| self.candidates(seat, d, 1).into_iter().next());
                return Ok(Outcome::Hint(best));
            }
            Action::Play { tiles } => self.play(seat, tiles, dict)?,
            Action::Swap { tiles } => self.swap(seat, tiles)?,
            Action::Pass => TurnKind::Pass,
            Action::Timeout => TurnKind::Timeout,
            Action::Resign => unreachable!("handled above"),
        };
        if matches!(kind, TurnKind::Play { .. }) {
            self.scoreless = 0;
        } else {
            self.scoreless += 1;
        }
        let turn = Turn { seat, kind };
        self.history.push(turn.clone());

        if self.seats[seat].rack.is_empty() && self.bag.is_empty() {
            self.finish(EndReason::RackEmptied { seat });
        } else if self.scoreless >= SCORELESS_ROUNDS * self.active_seats() as u32 {
            self.finish(EndReason::Scoreless);
        } else {
            self.advance();
        }
        Ok(Outcome::Turn(turn))
    }

    /// Legal moves for `seat`'s rack, best base score first.
    pub fn candidates(&self, seat: usize, dict: &Dictionary, limit: usize) -> Vec<Candidate> {
        movegen::best_moves(
            &self.board,
            &self.layout,
            self.tiles(),
            &self.seats[seat].rack,
            dict,
            self.config.bingo_bonus,
            limit,
        )
    }

    fn play(&mut self, seat: usize, tiles: Vec<Placement>, dict: Option<&Dictionary>) -> Result<TurnKind, ActionError> {
        let rack = take_from_rack(&self.seats[seat].rack, tiles.iter().map(|p| p.tile.rack_tile()))
            .ok_or(MoveError::NotOnRack)?;
        let scored = moves::evaluate(&self.board, &self.layout, self.tiles(), &tiles, self.config.bingo_bonus, dict)?;

        let mut claimed = Vec::new();
        let mut bounty = 0;
        for b in &mut self.bounties {
            if b.claimed_by.is_none() && tiles.iter().any(|p| p.pos == b.pos) {
                b.claimed_by = Some(seat);
                bounty += b.points;
                claimed.push(b.pos);
            }
        }

        let scores: Vec<i32> = self.seats.iter().map(|s| s.score).collect();
        let score = breakdown(&self.config, seat, &scores, scored.base, scored.bingo, bounty);

        for p in &tiles {
            self.board.set(p.pos, p.tile);
        }
        self.seats[seat].rack = rack;
        self.seats[seat].score += score.total;
        self.refill(seat);
        Ok(TurnKind::Play { tiles, words: scored.words, score, bounties: claimed })
    }

    fn swap(&mut self, seat: usize, tiles: Vec<Tile>) -> Result<TurnKind, ActionError> {
        if tiles.is_empty() {
            return Err(MoveError::Empty.into());
        }
        let min = (self.config.swap_min_bag as usize).max(tiles.len());
        if self.bag.len() < min {
            return Err(ActionError::BagTooSmall);
        }
        let rack = take_from_rack(&self.seats[seat].rack, tiles.iter().copied()).ok_or(MoveError::NotOnRack)?;
        self.seats[seat].rack = rack;
        self.refill(seat);
        self.bag.extend(&tiles);
        self.rng.shuffle(&mut self.bag);
        Ok(TurnKind::Swap { count: tiles.len() })
    }

    fn resign(&mut self, seat: usize) -> Turn {
        self.seats[seat].resigned = true;
        let turn = Turn { seat, kind: TurnKind::Resign };
        self.history.push(turn.clone());
        if self.config.teams || self.active_seats() <= 1 {
            self.finish(EndReason::Resigned);
        } else {
            // The others play on; the tiles go back so nothing is lost.
            let rack = std::mem::take(&mut self.seats[seat].rack);
            self.bag.extend(rack);
            self.rng.shuffle(&mut self.bag);
            if self.turn == seat {
                self.advance();
            }
        }
        turn
    }

    fn refill(&mut self, seat: usize) {
        let want = self.config.players[seat].rack_size as usize;
        while self.seats[seat].rack.len() < want {
            match self.bag.pop() {
                Some(t) => self.seats[seat].rack.push(t),
                None => break,
            }
        }
    }

    fn advance(&mut self) {
        let n = self.seats.len();
        for step in 1..=n {
            let next = (self.turn + step) % n;
            if !self.seats[next].resigned {
                self.turn = next;
                return;
            }
        }
    }

    fn active_seats(&self) -> usize {
        self.seats.iter().filter(|s| !s.resigned).count()
    }

    /// Scores per side: per seat, or per team (two entries) in team games.
    pub fn standings(&self) -> Vec<i32> {
        let scores: Vec<i32> = self.seats.iter().map(|s| s.score).collect();
        standings(&self.config, &scores)
    }

    fn side_of(&self, seat: usize) -> usize {
        if self.config.teams { seat % 2 } else { seat }
    }

    fn finish(&mut self, reason: EndReason) {
        let mut adjustments = vec![0i32; self.seats.len()];
        let rack_value = |s: &Seat| s.rack.iter().map(|&t| self.tiles().tile_value(t) as i32).sum::<i32>();
        match reason {
            EndReason::RackEmptied { seat } => {
                for (i, s) in self.seats.iter().enumerate() {
                    if i != seat && !s.resigned {
                        let v = rack_value(s);
                        adjustments[i] -= v;
                        adjustments[seat] += v;
                    }
                }
            }
            EndReason::Scoreless => {
                for (i, s) in self.seats.iter().enumerate() {
                    if !s.resigned {
                        adjustments[i] -= rack_value(s);
                    }
                }
            }
            EndReason::Resigned => {}
        }
        for (s, a) in self.seats.iter_mut().zip(&adjustments) {
            s.score += a;
        }

        // A side is out if it resigned (in team games one partner is enough).
        let sides = self.standings();
        let out = |side: usize| self.seats.iter().enumerate().any(|(i, s)| s.resigned && self.side_of(i) == side);
        let best = (0..sides.len()).filter(|&s| !out(s)).map(|s| sides[s]).max();
        let winners = match best {
            Some(best) => {
                (0..self.seats.len()).filter(|&i| !out(self.side_of(i)) && sides[self.side_of(i)] == best).collect()
            }
            None => Vec::new(),
        };
        self.status = Status::Finished { reason, winners, adjustments };
    }

    /// What `viewer` may see (`None` for a spectator): no bag order, no
    /// hidden racks, no unclaimed bounties.
    pub fn view(&self, viewer: Option<usize>) -> GameView {
        let sees = |seat: usize| viewer.is_some_and(|v| self.config.sees_rack(v, seat));
        let mut unseen: Vec<Tile> = self.bag.clone();
        for (i, s) in self.seats.iter().enumerate() {
            if !sees(i) {
                unseen.extend(&s.rack);
            }
        }
        unseen.sort();
        let mut tally: Vec<TileCount> = Vec::new();
        for t in unseen {
            match tally.last_mut() {
                Some(last) if last.tile == t => last.count += 1,
                _ => tally.push(TileCount { tile: t, count: 1 }),
            }
        }
        GameView {
            config: self.config.clone(),
            layout: self.layout.clone(),
            board: self.board.clone(),
            viewer,
            seats: self
                .seats
                .iter()
                .enumerate()
                .map(|(i, s)| SeatView {
                    score: s.score,
                    rack_count: s.rack.len(),
                    rack: sees(i).then(|| s.rack.clone()),
                    resigned: s.resigned,
                    hints_left: s.hints_left,
                })
                .collect(),
            bag_count: self.bag.len(),
            unseen: tally,
            turn: self.turn,
            claimed_bounties: self.bounties.iter().filter(|b| b.claimed_by.is_some()).cloned().collect(),
            status: self.status.clone(),
            history: self.history.clone(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("config: {0}")]
    Config(ConfigError),
    #[error("action {index}: {error}")]
    Action { index: usize, error: ActionError },
}

/// Scores per side: per seat, or per team (two entries) in team games.
fn standings(config: &GameConfig, scores: &[i32]) -> Vec<i32> {
    if config.teams {
        let mut t = vec![0, 0];
        for (i, s) in scores.iter().enumerate() {
            t[i % 2] += s;
        }
        t
    } else {
        scores.to_vec()
    }
}

/// A move's score with the game's modifiers, from the scores before it.
/// Handicap and comeback both apply to words + bingo + bounty, rounded half up.
fn breakdown(config: &GameConfig, seat: usize, scores: &[i32], base: u32, bingo: u32, bounty: u32) -> ScoreBreakdown {
    let subtotal = base + bounty;
    let percent = config.players[seat].score_percent as i64;
    let handicap = ((subtotal as i64 * percent + 50) / 100 - subtotal as i64) as i32;
    let sides = standings(config, scores);
    let side = config.team_of(seat).unwrap_or(seat);
    let deficit = sides.iter().copied().max().unwrap_or(0) - sides[side];
    let comeback = match config.comeback {
        Some(c) if deficit >= c.behind_by as i32 => (subtotal * c.percent as u32 + 50) / 100,
        _ => 0,
    };
    let total = subtotal as i32 + handicap + comeback as i32;
    ScoreBreakdown { base, bingo, bounty, handicap, comeback, total }
}

/// The rack after removing `tiles`, or `None` if it does not hold them all.
fn take_from_rack(rack: &[Tile], tiles: impl IntoIterator<Item = Tile>) -> Option<Vec<Tile>> {
    let mut left = rack.to_vec();
    for t in tiles {
        let i = left.iter().position(|&r| r == t)?;
        left.swap_remove(i);
    }
    Some(left)
}

/// Bounties go on plain squares away from the start square.
fn place_bounties(layout: &Layout, count: u8, seed: u64) -> Vec<Bounty> {
    if count == 0 {
        return Vec::new();
    }
    let mut rng = Rng::derive(seed, purpose::BOUNTIES);
    let start = layout.start();
    let mut squares: Vec<Pos> = (0..layout.size)
        .flat_map(|r| (0..layout.size).map(move |c| Pos::new(r, c)))
        .filter(|&p| layout.get(p).is_none() && (p.row.abs_diff(start.row) + p.col.abs_diff(start.col)) >= 2)
        .collect();
    rng.shuffle(&mut squares);
    squares
        .into_iter()
        .take(count as usize)
        .map(|pos| Bounty { pos, points: BOUNTY_POINTS[rng.below(BOUNTY_POINTS.len())], claimed_by: None })
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileCount {
    pub tile: Tile,
    pub count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatView {
    pub score: i32,
    pub rack_count: usize,
    /// Only for the viewer and, in team games, their partner.
    pub rack: Option<Vec<Tile>>,
    pub resigned: bool,
    pub hints_left: u8,
}

/// A game as one player (or a spectator) sees it. This is what the API
/// sends and what the app previews moves against.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameView {
    pub config: GameConfig,
    pub layout: Layout,
    pub board: Board,
    pub viewer: Option<usize>,
    pub seats: Vec<SeatView>,
    pub bag_count: usize,
    /// Tiles the viewer cannot see (bag plus hidden racks), for tile tracking.
    pub unseen: Vec<TileCount>,
    pub turn: usize,
    pub claimed_bounties: Vec<Bounty>,
    pub status: Status,
    pub history: Vec<Turn>,
}

/// The live score while placing tiles. Never includes bounties: they stay
/// hidden until played (the server adds them).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub words: Vec<FormedWord>,
    pub score: ScoreBreakdown,
}

impl GameView {
    /// Checks and scores a placement for the viewer, as the server would
    /// (minus bounties). Pass the dictionary to flag invalid words.
    pub fn preview(&self, placements: &[Placement], dict: Option<&Dictionary>) -> Result<Preview, MoveError> {
        let seat = self.viewer.ok_or(MoveError::NotOnRack)?;
        let rack = self.seats[seat].rack.as_deref().unwrap_or(&[]);
        take_from_rack(rack, placements.iter().map(|p| p.tile.rack_tile())).ok_or(MoveError::NotOnRack)?;
        let tiles = tilesets::for_language(self.config.language);
        let scored = moves::evaluate(&self.board, &self.layout, tiles, placements, self.config.bingo_bonus, dict)?;
        let scores: Vec<i32> = self.seats.iter().map(|s| s.score).collect();
        let score = breakdown(&self.config, seat, &scores, scored.base, scored.bingo, 0);
        Ok(Preview { words: scored.words, score })
    }
}
