//! Games and the scoreboard on the website. Read-only: people play in the
//! app. A game page can step through the moves (`?move=N`), rebuilt from the
//! log with the core's replay.

use std::collections::HashMap;

use askama::Template;
use axum::extract::{Path, Query, State};
use axum::response::Response;
use serde::Deserialize;
use uuid::Uuid;
use wordwarz_core::board::{Pos, Premium};
use wordwarz_core::game::{Action, Game, Status, TurnKind};
use wordwarz_core::lexicon::Language;
use wordwarz_core::tilesets;

use super::session::WebUser;
use super::{Nav, html, local_day, local_time};
use crate::AppState;
use crate::error::{ApiError, WebError};
use crate::games::{self, Player};
use crate::stats;

type Page = Result<Response, WebError>;

fn language_label(l: Language) -> &'static str {
    match l {
        Language::Dutch => "Nederlands",
        Language::English => "English",
        Language::Bilingual => "NL + EN",
    }
}

fn name_of(p: &Player) -> String {
    if p.display_name.is_empty() { p.username.clone() } else { p.display_name.clone() }
}

struct GameRow {
    id: Uuid,
    players: String,
    language: &'static str,
    state: String,
    mine: bool,
    updated: String,
}

#[derive(Template)]
#[template(path = "games.html")]
struct GamesPage {
    nav: Nav,
    active: Vec<GameRow>,
    finished: Vec<GameRow>,
}

pub async fn list(State(st): State<AppState>, user: WebUser) -> Page {
    let mut active = Vec::new();
    let mut finished = Vec::new();
    for g in games::list(&st.pool, user.id).await? {
        let players =
            g.players.iter().map(|p| format!("{} {}", name_of(p), g.scores[p.seat])).collect::<Vec<_>>().join(" · ");
        let state = if g.finished {
            let w: Vec<String> = g.winners.iter().map(|&s| name_of(&g.players[s])).collect();
            if w.is_empty() { "ended".into() } else { format!("won by {}", w.join(" and ")) }
        } else if g.my_turn {
            "your turn".into()
        } else {
            format!("{}'s turn", name_of(&g.players[g.turn]))
        };
        let row = GameRow {
            id: g.id,
            players,
            language: language_label(g.language),
            mine: g.my_turn,
            state,
            updated: local_time(g.updated_at, user.tz),
        };
        if g.finished { finished.push(row) } else { active.push(row) }
    }
    html(GamesPage { nav: Nav::new(&user, "games"), active, finished })
}

struct Cell {
    text: String,
    value: String,
    class: String,
}

struct SeatLine {
    name: String,
    score: i32,
    turn: bool,
    winner: bool,
    resigned: bool,
}

struct MoveLine {
    n: usize,
    who: String,
    what: String,
    score: String,
    current: bool,
}

#[derive(Template)]
#[template(path = "game.html")]
struct GamePage {
    nav: Nav,
    id: Uuid,
    size: u8,
    rows: Vec<Vec<Cell>>,
    seats: Vec<SeatLine>,
    moves: Vec<MoveLine>,
    step: usize,
    total: usize,
    prev: Option<usize>,
    next: Option<usize>,
    language: &'static str,
    status: String,
    bag: usize,
}

#[derive(Deserialize)]
pub struct StepQuery {
    #[serde(rename = "move")]
    step: Option<usize>,
}

/// Players see their games; anyone signed in may look at a finished one
/// (the scoreboard links to them). Racks are never shown here.
pub async fn show(State(st): State<AppState>, user: WebUser, Path(id): Path<Uuid>, Query(q): Query<StepQuery>) -> Page {
    let (game, log) = games::load(&st.pool, id).await?;
    let players = games::players_of(&st.pool, id).await?;
    let plays_here = players.iter().any(|p| p.user_id == Some(user.id));
    if !plays_here && !game.is_over() {
        return Err(ApiError::NotFound.into());
    }
    let names: Vec<String> = players.iter().map(name_of).collect();

    // The log holds hints too; a step counts turns only.
    let total = game.history.len();
    let step = q.step.unwrap_or(total).min(total);
    let shown: Game = if step == total {
        game.clone()
    } else {
        let mut turns = 0;
        let prefix: Vec<(usize, Action)> = log
            .iter()
            .take_while(|(_, a)| {
                if *a == Action::Hint {
                    return true;
                }
                turns += 1;
                turns <= step
            })
            .cloned()
            .collect();
        Game::replay(game.config.clone(), game.seed, &prefix).map_err(|e| ApiError::Internal(e.into()))?
    };

    let last: Vec<Pos> = match shown.history.last().map(|t| &t.kind) {
        Some(TurnKind::Play { tiles, .. }) => tiles.iter().map(|p| p.pos).collect(),
        _ => Vec::new(),
    };
    let values = tilesets::for_language(game.config.language);
    let size = shown.board.size;
    let rows = (0..size)
        .map(|r| {
            (0..size)
                .map(|c| {
                    let p = Pos::new(r, c);
                    match shown.board.get(p) {
                        Some(t) => Cell {
                            text: t.letter.to_string(),
                            value: if t.blank { String::new() } else { values.value(t.letter).to_string() },
                            class: format!(
                                "cell tile{}{}",
                                if t.blank { " blank" } else { "" },
                                if last.contains(&p) { " last" } else { "" }
                            ),
                        },
                        None => {
                            let (text, class) = match shown.layout.get(p) {
                                _ if p == shown.layout.start() => ("★", "start"),
                                Some(Premium::DoubleLetter) => ("DL", "dl"),
                                Some(Premium::TripleLetter) => ("TL", "tl"),
                                Some(Premium::DoubleWord) => ("DW", "dw"),
                                Some(Premium::TripleWord) => ("TW", "tw"),
                                None => ("", "plain"),
                            };
                            Cell { text: text.into(), value: String::new(), class: format!("cell {class}") }
                        }
                    }
                })
                .collect()
        })
        .collect();

    let winners = match &shown.status {
        Status::Finished { winners, .. } => winners.clone(),
        Status::Active => Vec::new(),
    };
    let seats = shown
        .seats
        .iter()
        .enumerate()
        .map(|(i, s)| SeatLine {
            name: names.get(i).cloned().unwrap_or_default(),
            score: s.score,
            turn: !shown.is_over() && shown.turn == i,
            winner: winners.contains(&i),
            resigned: s.resigned,
        })
        .collect();
    let moves = game
        .history
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let (what, score) = match &t.kind {
                TurnKind::Play { words, score, .. } => {
                    (words.iter().map(|w| w.word.as_str()).collect::<Vec<_>>().join(", "), format!("+{}", score.total))
                }
                TurnKind::Swap { count } => (format!("swapped {count}"), String::new()),
                TurnKind::Pass => ("passed".into(), String::new()),
                TurnKind::Timeout => ("ran out of time".into(), String::new()),
                TurnKind::Resign => ("resigned".into(), String::new()),
            };
            MoveLine {
                n: i + 1,
                who: names.get(t.seat).cloned().unwrap_or_default(),
                what,
                score,
                current: i + 1 == step,
            }
        })
        .collect();
    let status = match &game.status {
        Status::Finished { winners, .. } if winners.is_empty() => "Ended".to_string(),
        Status::Finished { winners, .. } => {
            format!("Won by {}", winners.iter().map(|&w| names[w].clone()).collect::<Vec<_>>().join(" and "))
        }
        Status::Active => format!("{}'s turn", names[game.turn]),
    };

    html(GamePage {
        nav: Nav::new(&user, "games"),
        id,
        size,
        rows,
        seats,
        moves,
        step,
        total,
        prev: step.checked_sub(1),
        next: (step < total).then_some(step + 1),
        language: language_label(game.config.language),
        status,
        bag: shown.bag.len(),
    })
}

struct StatRow {
    name: String,
    games: i64,
    wins: i64,
    win_rate: i64,
    avg_score: i64,
    best: String,
    bingos: i64,
}

struct RivalRow {
    a: String,
    b: String,
    a_wins: i64,
    b_wins: i64,
    draws: i64,
}

struct MatchRow {
    id: Uuid,
    day: String,
    language: &'static str,
    seats: String,
}

#[derive(Template)]
#[template(path = "scoreboard.html")]
struct ScoreboardPage {
    nav: Nav,
    players: Vec<StatRow>,
    rivals: Vec<RivalRow>,
    recent: Vec<MatchRow>,
}

pub async fn scoreboard(State(st): State<AppState>, user: WebUser) -> Page {
    let board = stats::scoreboard(&st.pool).await?;
    let label = |u: &str, d: &str| if d.is_empty() { u.to_string() } else { d.to_string() };
    let names: HashMap<Uuid, String> =
        board.players.iter().map(|p| (p.user_id, label(&p.username, &p.display_name))).collect();
    let players = board
        .players
        .iter()
        .map(|p| StatRow {
            name: label(&p.username, &p.display_name),
            games: p.games,
            wins: p.wins,
            win_rate: p.win_rate,
            avg_score: p.avg_score,
            best: if p.best_move > 0 { format!("{} ({})", p.best_word, p.best_move) } else { String::new() },
            bingos: p.bingos,
        })
        .collect();
    let rivals = board
        .head_to_head
        .iter()
        .filter_map(|h| {
            Some(RivalRow {
                a: names.get(&h.a)?.clone(),
                b: names.get(&h.b)?.clone(),
                a_wins: h.a_wins,
                b_wins: h.b_wins,
                draws: h.draws,
            })
        })
        .collect();
    let recent = board
        .recent
        .iter()
        .map(|m| MatchRow {
            id: m.game_id,
            day: local_day(m.finished_at, user.tz),
            language: match m.language.as_str() {
                "nl" => "Nederlands",
                "en" => "English",
                _ => "NL + EN",
            },
            seats: m
                .seats
                .iter()
                .map(|s| {
                    format!("{}{} {}", if s.won { "🏆 " } else { "" }, label(&s.username, &s.display_name), s.score)
                })
                .collect::<Vec<_>>()
                .join(" · "),
        })
        .collect();
    html(ScoreboardPage { nav: Nav::new(&user, "scoreboard"), players, rivals, recent })
}
