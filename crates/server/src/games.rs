//! Games: creating them, applying actions, turn timers, nudges and results.
//! Every action runs under a row lock on its game, appends to the log and
//! replaces the cached state in the same transaction.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;
use wordwarz_core::config::{GameConfig, PlayerSetup};
use wordwarz_core::game::{Action, Game, GameView, Outcome, Status, Turn, TurnKind};
use wordwarz_core::movegen::Candidate;

use crate::AppState;
use crate::error::ApiError;
use crate::lexicons::dictionary;
use crate::push::{self, Event};

/// Turn timers a game may choose (hours); `None` is off.
pub const TIMER_HOURS: [i32; 3] = [24, 72, 168];
/// One nudge per sender per game in this many hours.
pub const NUDGE_HOURS: i64 = 4;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatRequest {
    pub user_id: Uuid,
    #[serde(default)]
    pub rack_size: Option<u8>,
    #[serde(default)]
    pub score_percent: Option<u16>,
    /// 0 or 1, required in team games.
    #[serde(default)]
    pub team: Option<u8>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewGame {
    /// The rules; `players` is rebuilt from `seats` (handicaps travel with
    /// the person when seats are shuffled).
    pub config: GameConfig,
    /// Everyone in the game, the creator included.
    pub seats: Vec<SeatRequest>,
    #[serde(default)]
    pub turn_timer_hours: Option<i32>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Player {
    pub seat: usize,
    /// None once the account is deleted.
    pub user_id: Option<Uuid>,
    pub username: String,
    pub display_name: String,
}

/// One game as a player sees it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameEnvelope {
    pub id: Uuid,
    pub rev: i32,
    pub players: Vec<Player>,
    pub turn_timer_hours: Option<i32>,
    pub turn_started_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub view: GameView,
}

/// A line in the game list.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameSummary {
    pub id: Uuid,
    pub rev: i32,
    pub language: wordwarz_core::lexicon::Language,
    pub players: Vec<Player>,
    pub scores: Vec<i32>,
    pub my_seat: usize,
    pub turn: usize,
    pub my_turn: bool,
    pub finished: bool,
    pub winners: Vec<usize>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActResult {
    pub game: GameEnvelope,
    /// The turn just taken (absent for a hint).
    pub turn: Option<Turn>,
    /// The hint's move (absent unless the action was a hint; null when the
    /// rack has no legal move).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<Option<Candidate>>,
}

/// OS randomness for seeds and seat order (never the game's own RNG).
fn random_u64() -> u64 {
    let mut b = [0u8; 8];
    getrandom::fill(&mut b).expect("OS randomness");
    u64::from_le_bytes(b)
}

fn shuffle<T>(v: &mut [T]) {
    for i in (1..v.len()).rev() {
        let j = (random_u64() % (i as u64 + 1)) as usize;
        v.swap(i, j);
    }
}

/// Random seat order. Team games keep partners on alternate seats (0 and 2
/// against 1 and 3) with a random team starting.
fn seat_order(seats: &[SeatRequest], teams: bool) -> Result<Vec<SeatRequest>, ApiError> {
    if !teams {
        let mut v = seats.to_vec();
        shuffle(&mut v);
        return Ok(v);
    }
    let mut a: Vec<SeatRequest> = seats.iter().filter(|s| s.team == Some(0)).cloned().collect();
    let mut b: Vec<SeatRequest> = seats.iter().filter(|s| s.team == Some(1)).cloned().collect();
    if a.len() != 2 || b.len() != 2 {
        return Err(ApiError::bad("a team game needs two players in team 0 and two in team 1"));
    }
    shuffle(&mut a);
    shuffle(&mut b);
    if random_u64() % 2 == 1 {
        std::mem::swap(&mut a, &mut b);
    }
    Ok(vec![a[0].clone(), b[0].clone(), a[1].clone(), b[1].clone()])
}

pub async fn create(st: &AppState, creator: Uuid, req: NewGame) -> Result<Uuid, ApiError> {
    let mut seen = std::collections::HashSet::new();
    if !req.seats.iter().all(|s| seen.insert(s.user_id)) {
        return Err(ApiError::bad("each person takes one seat"));
    }
    if !seen.contains(&creator) {
        return Err(ApiError::bad("include yourself in the game"));
    }
    if let Some(h) = req.turn_timer_hours
        && !TIMER_HOURS.contains(&h)
    {
        return Err(ApiError::bad("the turn timer is 24, 72 or 168 hours, or off"));
    }
    let ids: Vec<Uuid> = req.seats.iter().map(|s| s.user_id).collect();
    let active: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE id = ANY($1) AND disabled_at IS NULL")
        .bind(&ids)
        .fetch_one(&st.pool)
        .await?;
    if active as usize != ids.len() {
        return Err(ApiError::bad("everyone in the game needs an active account"));
    }

    let seats = seat_order(&req.seats, req.config.teams)?;
    let mut config = req.config;
    config.players = seats
        .iter()
        .map(|s| PlayerSetup {
            rack_size: s.rack_size.unwrap_or(PlayerSetup::default().rack_size),
            score_percent: s.score_percent.unwrap_or(PlayerSetup::default().score_percent),
        })
        .collect();
    let seed = random_u64();
    let game = Game::new(config.clone(), seed).map_err(|e| ApiError::bad(e.to_string()))?;

    let id = Uuid::new_v4();
    let mut tx = st.pool.begin().await?;
    sqlx::query(
        "INSERT INTO games (id, created_by, config, seed, state, turn_seat, turn_timer_hours)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(id)
    .bind(creator)
    .bind(sqlx::types::Json(&config))
    .bind(seed as i64)
    .bind(sqlx::types::Json(&game))
    .bind(game.turn as i32)
    .bind(req.turn_timer_hours)
    .execute(&mut *tx)
    .await?;
    for (seat, s) in seats.iter().enumerate() {
        sqlx::query("INSERT INTO game_players (game_id, seat, user_id) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(seat as i32)
            .bind(s.user_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    st.changes.bump_all(&ids);
    for s in &seats {
        if s.user_id != creator {
            push::notify(st, s.user_id, Event::NewGame, id).await;
        }
    }
    Ok(id)
}

/// Who is acting: a person (the API and website) or the turn timer.
enum Actor {
    User(Uuid),
    Timer,
}

/// Applies `action` by `user` in `game`.
pub async fn act(st: &AppState, user: Uuid, game: Uuid, action: Action) -> Result<ActResult, ApiError> {
    apply(st, game, Actor::User(user), action).await
}

async fn apply(st: &AppState, game_id: Uuid, actor: Actor, action: Action) -> Result<ActResult, ApiError> {
    let mut tx = st.pool.begin().await?;
    let row: Option<(sqlx::types::Json<Game>, i32, Option<i32>)> =
        sqlx::query_as("SELECT state, rev, turn_timer_hours FROM games WHERE id = $1 FOR UPDATE")
            .bind(game_id)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((sqlx::types::Json(mut game), rev, timer)) = row else { return Err(ApiError::NotFound) };
    let players = players(&mut tx, game_id).await?;

    let (seat, action) = match actor {
        Actor::User(u) => {
            let seat = players.iter().find(|p| p.user_id == Some(u)).ok_or(ApiError::NotFound)?.seat;
            (seat, action)
        }
        // A second timeout in a row by the same seat resigns them.
        Actor::Timer => {
            let seat = game.turn;
            let last_own = game.history.iter().rev().find(|t| t.seat == seat);
            let again = last_own.is_some_and(|t| matches!(t.kind, TurnKind::Timeout));
            (seat, if again { Action::Resign } else { Action::Timeout })
        }
    };

    let turn_before = game.turn;
    let outcome = game.apply(seat, action.clone(), Some(dictionary(game.config.language)))?;
    let idx: i64 = sqlx::query_scalar("SELECT count(*) FROM game_actions WHERE game_id = $1")
        .bind(game_id)
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO game_actions (game_id, idx, seat, action) VALUES ($1, $2, $3, $4)")
        .bind(game_id)
        .bind(idx as i32)
        .bind(seat as i32)
        .bind(sqlx::types::Json(&action))
        .execute(&mut *tx)
        .await?;

    let finished = game.is_over();
    let took_turn = matches!(outcome, Outcome::Turn(_));
    let turn_started_at: DateTime<Utc> = sqlx::query_scalar(
        "UPDATE games SET state = $2, rev = rev + 1, turn_seat = $3, finished = $4, updated_at = now(),
                finished_at = CASE WHEN $4 THEN now() ELSE NULL END,
                turn_started_at = CASE WHEN $5 THEN now() ELSE turn_started_at END
          WHERE id = $1 RETURNING turn_started_at",
    )
    .bind(game_id)
    .bind(sqlx::types::Json(&game))
    .bind(game.turn as i32)
    .bind(finished)
    .bind(took_turn)
    .fetch_one(&mut *tx)
    .await?;
    if finished {
        write_results(&mut tx, game_id, &game, &players).await?;
    }
    tx.commit().await?;

    let everyone: Vec<Uuid> = players.iter().filter_map(|p| p.user_id).collect();
    st.changes.bump_all(&everyone);
    if finished {
        for u in &everyone {
            push::notify(st, *u, Event::Finished, game_id).await;
        }
    } else if took_turn
        && game.turn != turn_before
        && let Some(next) = players[game.turn].user_id
    {
        push::notify(st, next, Event::Turn, game_id).await;
    }

    let (turn, hint) = match outcome {
        Outcome::Turn(t) => (Some(t), None),
        Outcome::Hint(h) => (None, Some(h)),
    };
    let created_at: DateTime<Utc> =
        sqlx::query_scalar("SELECT created_at FROM games WHERE id = $1").bind(game_id).fetch_one(&st.pool).await?;
    let game = GameEnvelope {
        id: game_id,
        rev: rev + 1,
        players,
        turn_timer_hours: timer,
        turn_started_at,
        created_at,
        view: game.view(Some(seat)),
    };
    Ok(ActResult { game, turn, hint })
}

/// A seat as stored: seat, user (None once deleted), username, display name.
type SeatRow = (i32, Option<Uuid>, Option<String>, Option<String>);

async fn players(tx: &mut Transaction<'_, Postgres>, game: Uuid) -> Result<Vec<Player>, sqlx::Error> {
    players_of(&mut **tx, game).await
}

/// The seats of a game, in seat order.
pub async fn players_of<'e>(db: impl sqlx::PgExecutor<'e>, game: Uuid) -> Result<Vec<Player>, sqlx::Error> {
    let rows: Vec<SeatRow> = sqlx::query_as(
        "SELECT p.seat, p.user_id, u.username, u.display_name FROM game_players p
           LEFT JOIN users u ON u.id = p.user_id WHERE p.game_id = $1 ORDER BY p.seat",
    )
    .bind(game)
    .fetch_all(db)
    .await?;
    Ok(rows.into_iter().map(player).collect())
}

fn player((seat, user_id, username, display_name): SeatRow) -> Player {
    Player {
        seat: seat as usize,
        user_id,
        username: username.unwrap_or_else(|| "deleted".into()),
        display_name: display_name.unwrap_or_default(),
    }
}

/// One row per seat for the scoreboard.
async fn write_results(
    tx: &mut Transaction<'_, Postgres>,
    game_id: Uuid,
    game: &Game,
    players: &[Player],
) -> Result<(), sqlx::Error> {
    let Status::Finished { winners, .. } = &game.status else { return Ok(()) };
    let language = serde_json::to_value(game.config.language).unwrap_or_default();
    for p in players {
        let mut moves = 0;
        let mut bingos = 0;
        let mut best = (0, String::new());
        for t in game.history.iter().filter(|t| t.seat == p.seat) {
            if let TurnKind::Play { score, words, .. } = &t.kind {
                moves += 1;
                if score.bingo > 0 {
                    bingos += 1;
                }
                if score.total > best.0 {
                    best = (score.total, words.first().map(|w| w.word.clone()).unwrap_or_default());
                }
            }
        }
        sqlx::query(
            "INSERT INTO game_results (game_id, seat, user_id, team, score, won, resigned, moves, bingos,
                                       best_move, best_word, language, finished_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, now())",
        )
        .bind(game_id)
        .bind(p.seat as i32)
        .bind(p.user_id)
        .bind(game.config.team_of(p.seat).map(|t| t as i32))
        .bind(game.seats[p.seat].score)
        .bind(winners.contains(&p.seat))
        .bind(game.seats[p.seat].resigned)
        .bind(moves)
        .bind(bingos)
        .bind(best.0)
        .bind(best.1)
        .bind(language.as_str().unwrap_or_default())
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// The game as `user` sees it, if they play in it.
pub async fn get(pool: &PgPool, user: Uuid, game_id: Uuid) -> Result<GameEnvelope, ApiError> {
    let mut tx = pool.begin().await?;
    let players = players(&mut tx, game_id).await?;
    let seat = players.iter().find(|p| p.user_id == Some(user)).ok_or(ApiError::NotFound)?.seat;
    let (sqlx::types::Json(game), rev, timer, turn_started_at, created_at): (
        sqlx::types::Json<Game>,
        i32,
        Option<i32>,
        DateTime<Utc>,
        DateTime<Utc>,
    ) = sqlx::query_as("SELECT state, rev, turn_timer_hours, turn_started_at, created_at FROM games WHERE id = $1")
        .bind(game_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(GameEnvelope {
        id: game_id,
        rev,
        players,
        turn_timer_hours: timer,
        turn_started_at,
        created_at,
        view: game.view(Some(seat)),
    })
}

/// The full game (bag order included). Only for the replay page, which
/// reveals nothing until the game is over, and tests.
pub async fn load(pool: &PgPool, game_id: Uuid) -> Result<(Game, Vec<(usize, Action)>), ApiError> {
    let (sqlx::types::Json(game),): (sqlx::types::Json<Game>,) =
        sqlx::query_as("SELECT state FROM games WHERE id = $1")
            .bind(game_id)
            .fetch_optional(pool)
            .await?
            .ok_or(ApiError::NotFound)?;
    let rows: Vec<(i32, sqlx::types::Json<Action>)> =
        sqlx::query_as("SELECT seat, action FROM game_actions WHERE game_id = $1 ORDER BY idx")
            .bind(game_id)
            .fetch_all(pool)
            .await?;
    Ok((game, rows.into_iter().map(|(s, sqlx::types::Json(a))| (s as usize, a)).collect()))
}

/// Every game of `user`, newest activity first.
pub async fn list(pool: &PgPool, user: Uuid) -> Result<Vec<GameSummary>, ApiError> {
    #[derive(sqlx::FromRow)]
    struct Mine {
        id: Uuid,
        rev: i32,
        state: sqlx::types::Json<Game>,
        updated_at: DateTime<Utc>,
        seat: i32,
    }
    #[derive(sqlx::FromRow)]
    struct Seat {
        game_id: Uuid,
        seat: i32,
        user_id: Option<Uuid>,
        username: Option<String>,
        display_name: Option<String>,
    }
    let rows: Vec<Mine> = sqlx::query_as(
        "SELECT g.id, g.rev, g.state, g.updated_at, me.seat FROM games g
           JOIN game_players me ON me.game_id = g.id AND me.user_id = $1
          ORDER BY g.finished, g.updated_at DESC LIMIT 200",
    )
    .bind(user)
    .fetch_all(pool)
    .await?;
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let seats: Vec<Seat> = sqlx::query_as(
        "SELECT p.game_id, p.seat, p.user_id, u.username, u.display_name FROM game_players p
           LEFT JOIN users u ON u.id = p.user_id WHERE p.game_id = ANY($1) ORDER BY p.game_id, p.seat",
    )
    .bind(&ids)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|Mine { id, rev, state: sqlx::types::Json(g), updated_at, seat }| {
            let players: Vec<Player> = seats
                .iter()
                .filter(|s| s.game_id == id)
                .map(|s| player((s.seat, s.user_id, s.username.clone(), s.display_name.clone())))
                .collect();
            let my_seat = seat as usize;
            let winners = match &g.status {
                Status::Finished { winners, .. } => winners.clone(),
                Status::Active => Vec::new(),
            };
            GameSummary {
                id,
                rev,
                language: g.config.language,
                scores: g.seats.iter().map(|s| s.score).collect(),
                my_seat,
                turn: g.turn,
                my_turn: !g.is_over() && g.turn == my_seat,
                finished: g.is_over(),
                winners,
                players,
                updated_at,
            }
        })
        .collect())
}

/// Reminds the player whose turn it is. One per sender per game per
/// [`NUDGE_HOURS`].
pub async fn nudge(st: &AppState, from: Uuid, game_id: Uuid) -> Result<(), ApiError> {
    let mut tx = st.pool.begin().await?;
    let players = players(&mut tx, game_id).await?;
    if !players.iter().any(|p| p.user_id == Some(from)) {
        return Err(ApiError::NotFound);
    }
    let (turn, finished): (i32, bool) =
        sqlx::query_as("SELECT turn_seat, finished FROM games WHERE id = $1").bind(game_id).fetch_one(&mut *tx).await?;
    if finished {
        return Err(ApiError::bad("the game is over"));
    }
    let Some(to) = players[turn as usize].user_id.filter(|&u| u != from) else {
        return Err(ApiError::bad("it is your own turn"));
    };
    let recent: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM nudges WHERE game_id = $1 AND from_user = $2
                         AND at > now() - make_interval(hours => $3))",
    )
    .bind(game_id)
    .bind(from)
    .bind(NUDGE_HOURS as i32)
    .fetch_one(&mut *tx)
    .await?;
    if recent {
        return Err(ApiError::Busy(format!("you can nudge once every {NUDGE_HOURS} hours per game")));
    }
    sqlx::query("INSERT INTO nudges (game_id, from_user, to_user) VALUES ($1, $2, $3)")
        .bind(game_id)
        .bind(from)
        .bind(to)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM nudges WHERE at < now() - interval '7 days'").execute(&mut *tx).await?;
    tx.commit().await?;
    st.changes.bump(to);
    push::notify(st, to, Event::Nudge, game_id).await;
    Ok(())
}

/// Applies `Timeout` (or `Resign` on a second one in a row) to every game
/// whose turn timer ran out. Returns how many games it touched.
pub async fn expire_turns(st: &AppState) -> Result<usize, ApiError> {
    let due: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM games WHERE NOT finished AND turn_timer_hours IS NOT NULL
            AND turn_started_at + make_interval(hours => turn_timer_hours) < now()",
    )
    .fetch_all(&st.pool)
    .await?;
    for id in &due {
        if let Err(e) = apply(st, *id, Actor::Timer, Action::Timeout).await {
            tracing::warn!(game = %id, error = %e, "turn timer");
        }
    }
    Ok(due.len())
}

pub async fn run_timers(st: AppState) {
    loop {
        if let Err(e) = expire_turns(&st).await {
            tracing::warn!(error = %e, "turn timers");
        }
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    }
}

/// Resigns `user` from every running game, then deletes the account. Their
/// seats stay in finished games as "deleted".
pub async fn delete_account(st: &AppState, user: Uuid) -> Result<(), ApiError> {
    let admin: bool = sqlx::query_scalar("SELECT role = 'admin' FROM users WHERE id = $1")
        .bind(user)
        .fetch_optional(&st.pool)
        .await?
        .ok_or(ApiError::NotFound)?;
    let others: i64 =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE id <> $1").bind(user).fetch_one(&st.pool).await?;
    if admin && others > 0 && crate::users::other_admins(&st.pool, user).await? == 0 {
        return Err(ApiError::bad("you are the only admin; make someone else admin first"));
    }
    let running: Vec<Uuid> = sqlx::query_scalar(
        "SELECT g.id FROM games g JOIN game_players p ON p.game_id = g.id WHERE p.user_id = $1 AND NOT g.finished",
    )
    .bind(user)
    .fetch_all(&st.pool)
    .await?;
    for g in running {
        act(st, user, g, Action::Resign).await?;
    }
    sqlx::query("DELETE FROM users WHERE id = $1").bind(user).execute(&st.pool).await?;
    Ok(())
}
