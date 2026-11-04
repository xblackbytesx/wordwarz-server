//! Demo data for looking at the website (screenshots, design work): the
//! Young Sheldon family with devices, open invites and games played by the
//! bot through the real API, finished and running. Never part of `make
//! test`: run on purpose; the database is dropped, created anew and kept,
//!
//!   WORDWARZ_DEMO_DATABASE_URL=postgres://.../wordwarz_demo \
//!     cargo test -p wordwarz-server --test demo_seed -- --ignored
//!
//! then serve that database and log in as `george` / `correct horse battery`.

mod common;

use common::{Device, PASSPHRASE};
use serde_json::{Value, json};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;
use wordwarz_core::config::{GameConfig, Preset};
use wordwarz_core::game::{Action, GameView, Status};
use wordwarz_core::lexicon::Language;
use wordwarz_core::{movegen, tilesets};
use wordwarz_server::auth::Role;
use wordwarz_server::lexicons::dictionary;
use wordwarz_server::users::{self, InviteKind};
use wordwarz_server::{AppState, MIGRATOR, router};

/// One move by whoever is to play: the best move, else a swap or a pass.
async fn step(id: &str, by: &Device, devices: &[&Device]) -> bool {
    // Seen by the game's creator: only players may look at a running game.
    let any: Value = by.get(&format!("/api/v1/games/{id}")).await;
    let v: GameView = serde_json::from_value(any["view"].clone()).unwrap();
    if v.status != Status::Active {
        return false;
    }
    let user: Uuid = serde_json::from_value(any["players"][v.turn]["userId"].clone()).unwrap();
    let dev = devices.iter().find(|d| d.user == user).unwrap();
    let mine: Value = dev.get(&format!("/api/v1/games/{id}")).await;
    let mv: GameView = serde_json::from_value(mine["view"].clone()).unwrap();
    let rack = mv.seats[mv.turn].rack.clone().unwrap();
    let lang = mv.config.language;
    let best = movegen::best_moves(
        &mv.board,
        &mv.layout,
        tilesets::for_language(lang),
        &rack,
        dictionary(lang),
        mv.config.bingo_bonus,
        4,
    );
    // Not always the very best move, so scores look like people played.
    let action = match best.get(mv.history.len() % best.len().max(1)) {
        Some(c) => Action::Play { tiles: c.tiles.clone() },
        None if mv.bag_count >= 7 => Action::Swap { tiles: rack },
        None => Action::Pass,
    };
    let _: Value = dev.json("POST", &format!("/api/v1/games/{id}/actions"), &action).await;
    true
}

async fn game(by: &Device, config: GameConfig, seats: Value, devices: &[&Device], moves: Option<usize>) {
    let g: Value =
        by.json("POST", "/api/v1/games", &json!({ "config": config, "seats": seats, "turnTimerHours": 72 })).await;
    let id = g["id"].as_str().unwrap().to_string();
    for _ in 0..moves.unwrap_or(400) {
        if !step(&id, by, devices).await {
            break;
        }
    }
}

#[tokio::test]
#[ignore]
async fn seed_demo() {
    let Ok(url) = std::env::var("WORDWARZ_DEMO_DATABASE_URL") else {
        eprintln!("SKIP: WORDWARZ_DEMO_DATABASE_URL unset");
        return;
    };
    // A fresh database each run: dropped and created from the server's `postgres` one.
    let (base, name) = url.rsplit_once('/').unwrap();
    let admin = PgPoolOptions::new().max_connections(1).connect(&format!("{base}/postgres")).await.unwrap();
    sqlx::query(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)")).execute(&admin).await.unwrap();
    sqlx::query(&format!("CREATE DATABASE {name}")).execute(&admin).await.unwrap();
    let pool = PgPoolOptions::new().max_connections(4).connect(&url).await.unwrap();
    MIGRATOR.run(&pool).await.unwrap();
    let app = router(AppState { cookie_secure: false, ..AppState::new(pool.clone()) });

    let cast = [
        ("george", "George", Role::Admin, "Pixel 9"),
        ("mary", "Mary", Role::User, "Pixel 8a"),
        ("sheldon", "Sheldon", Role::User, "Tablet"),
        ("missy", "Missy", Role::User, "Moto G"),
        ("georgie", "Georgie", Role::User, "Galaxy A55"),
    ];
    let mut devices = Vec::new();
    for (username, name, role, phone) in cast {
        let id = users::create(&pool, username, role).await.unwrap();
        users::set_password(&pool, username, PASSPHRASE).await.unwrap();
        sqlx::query("UPDATE users SET display_name = $2 WHERE id = $1")
            .bind(id)
            .bind(name)
            .execute(&pool)
            .await
            .unwrap();
        let token = users::new_token(&pool, id, phone).await.unwrap();
        devices.push(Device { app: app.clone(), token, user: id });
    }
    let [george, mary, sheldon, missy, georgie] = [0, 1, 2, 3, 4].map(|i| &devices[i]);
    users::new_token(&pool, george.user, "Laptop").await.unwrap();
    users::create_invite(&pool, george.user, Role::User, "Meemaw", InviteKind::Link).await.unwrap();
    users::create_invite(&pool, george.user, Role::User, "Pastor Jeff", InviteKind::InPerson).await.unwrap();

    let seat = |d: &Device| json!({ "userId": d.user });
    let all = [george, mary, sheldon, missy, georgie];
    // Finished: a duel, an English game with a handicap, a team game.
    game(george, GameConfig::classic(Language::Dutch, 2), json!([seat(george), seat(sheldon)]), &all, None).await;
    let mut en = GameConfig::classic(Language::English, 2);
    en.players[0].rack_size = 8;
    game(mary, en, json!([{ "userId": missy.user, "rackSize": 8 }, seat(mary)]), &all, None).await;
    game(
        george,
        GameConfig::preset(Preset::Family, Language::Bilingual, 4),
        json!([
            { "userId": george.user, "team": 0 }, { "userId": missy.user, "team": 0 },
            { "userId": mary.user, "team": 1 }, { "userId": sheldon.user, "team": 1 },
        ]),
        &all,
        None,
    )
    .await;
    game(sheldon, GameConfig::classic(Language::Dutch, 2), json!([seat(sheldon), seat(georgie)]), &all, None).await;
    // Running: part-way, so the board and the move list have something to show.
    game(george, GameConfig::classic(Language::Dutch, 2), json!([seat(george), seat(mary)]), &all, Some(9)).await;
    game(
        sheldon,
        GameConfig::preset(Preset::Chaos, Language::English, 3),
        json!([seat(sheldon), seat(georgie), seat(george)]),
        &all,
        Some(7),
    )
    .await;
    eprintln!("seeded: log in as george / {PASSPHRASE}");
}
