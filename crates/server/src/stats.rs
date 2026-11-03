//! The scoreboard: everyone's record, head-to-head standings and recent
//! matches, all from `game_results` (one row per seat of a finished game).
//! Family-sized: computed on request, no caches.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlayerStats {
    pub user_id: Uuid,
    pub username: String,
    pub display_name: String,
    pub games: i64,
    pub wins: i64,
    /// Whole percent.
    pub win_rate: i64,
    pub avg_score: i64,
    pub best_move: i64,
    pub best_word: String,
    pub bingos: i64,
}

/// `a` and `b` in username order.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadToHead {
    pub a: Uuid,
    pub b: Uuid,
    pub a_wins: i64,
    pub b_wins: i64,
    pub draws: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchSeat {
    pub user_id: Option<Uuid>,
    pub username: String,
    pub display_name: String,
    pub score: i32,
    pub won: bool,
    pub team: Option<i32>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentMatch {
    pub game_id: Uuid,
    pub finished_at: DateTime<Utc>,
    pub language: String,
    pub seats: Vec<MatchSeat>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Scoreboard {
    /// Most wins first.
    pub players: Vec<PlayerStats>,
    pub head_to_head: Vec<HeadToHead>,
    /// Newest first, at most [`RECENT`].
    pub recent: Vec<RecentMatch>,
}

pub const RECENT: usize = 30;

#[derive(sqlx::FromRow)]
struct Row {
    game_id: Uuid,
    user_id: Option<Uuid>,
    username: Option<String>,
    display_name: Option<String>,
    team: Option<i32>,
    score: i32,
    won: bool,
    resigned: bool,
    bingos: i32,
    best_move: i32,
    best_word: String,
    language: String,
    finished_at: DateTime<Utc>,
}

/// Who did better in one game: the winner flag, then not resigning, then score.
fn compare(a: &Row, b: &Row) -> std::cmp::Ordering {
    a.won.cmp(&b.won).then(b.resigned.cmp(&a.resigned)).then(a.score.cmp(&b.score))
}

pub async fn scoreboard(pool: &PgPool) -> Result<Scoreboard, sqlx::Error> {
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT r.game_id, r.user_id, u.username, u.display_name, r.team, r.score, r.won, r.resigned,
                r.bingos, r.best_move, r.best_word, r.language, r.finished_at
           FROM game_results r LEFT JOIN users u ON u.id = r.user_id
          ORDER BY r.finished_at DESC, r.game_id, r.seat",
    )
    .fetch_all(pool)
    .await?;
    let people: Vec<(Uuid, String, String)> =
        sqlx::query_as("SELECT id, username, display_name FROM users WHERE disabled_at IS NULL ORDER BY username")
            .fetch_all(pool)
            .await?;

    let mut players: Vec<PlayerStats> = people
        .iter()
        .map(|(id, username, display_name)| {
            let mine: Vec<&Row> = rows.iter().filter(|r| r.user_id == Some(*id)).collect();
            let games = mine.len() as i64;
            let wins = mine.iter().filter(|r| r.won).count() as i64;
            let best = mine.iter().max_by_key(|r| r.best_move);
            PlayerStats {
                user_id: *id,
                username: username.clone(),
                display_name: display_name.clone(),
                games,
                wins,
                win_rate: if games > 0 { (wins * 100 + games / 2) / games } else { 0 },
                avg_score: if games > 0 { mine.iter().map(|r| r.score as i64).sum::<i64>() / games } else { 0 },
                best_move: best.map_or(0, |r| r.best_move as i64),
                best_word: best.map(|r| r.best_word.clone()).unwrap_or_default(),
                bingos: mine.iter().map(|r| r.bingos as i64).sum(),
            }
        })
        .collect();
    players.sort_by(|a, b| b.wins.cmp(&a.wins).then(b.win_rate.cmp(&a.win_rate)).then(a.username.cmp(&b.username)));

    let names: HashMap<Uuid, &str> = people.iter().map(|(id, u, _)| (*id, u.as_str())).collect();
    let mut games: BTreeMap<(DateTime<Utc>, Uuid), Vec<&Row>> = BTreeMap::new();
    for r in &rows {
        games.entry((r.finished_at, r.game_id)).or_default().push(r);
    }
    let mut h2h: BTreeMap<(Uuid, Uuid), HeadToHead> = BTreeMap::new();
    for seats in games.values() {
        for (i, x) in seats.iter().enumerate() {
            for y in &seats[i + 1..] {
                let (Some(xu), Some(yu)) = (x.user_id, y.user_id) else { continue };
                if x.team.is_some() && x.team == y.team {
                    continue; // partners are not rivals
                }
                let (Some(xn), Some(yn)) = (names.get(&xu), names.get(&yu)) else { continue };
                let (a, b, ra, rb) = if xn <= yn { (xu, yu, *x, *y) } else { (yu, xu, *y, *x) };
                let e = h2h.entry((a, b)).or_insert(HeadToHead { a, b, a_wins: 0, b_wins: 0, draws: 0 });
                match compare(ra, rb) {
                    std::cmp::Ordering::Greater => e.a_wins += 1,
                    std::cmp::Ordering::Less => e.b_wins += 1,
                    std::cmp::Ordering::Equal => e.draws += 1,
                }
            }
        }
    }

    let recent = games
        .iter()
        .rev()
        .take(RECENT)
        .map(|((finished_at, game_id), seats)| RecentMatch {
            game_id: *game_id,
            finished_at: *finished_at,
            language: seats[0].language.clone(),
            seats: seats
                .iter()
                .map(|r| MatchSeat {
                    user_id: r.user_id,
                    username: r.username.clone().unwrap_or_else(|| "deleted".into()),
                    display_name: r.display_name.clone().unwrap_or_default(),
                    score: r.score,
                    won: r.won,
                    team: r.team,
                })
                .collect(),
        })
        .collect();

    Ok(Scoreboard { players, head_to_head: h2h.into_values().collect(), recent })
}
