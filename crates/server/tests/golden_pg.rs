//! The `/api/v1` wire contract. Captures every response the app decodes into
//! `tests/golden/*.json`; the app's contract test parses these files with its
//! own models. Regenerate on purpose only: `UPDATE_GOLDEN=1 cargo test
//! --test golden_pg`. Without it, the live responses must keep the stored
//! files' shape (same keys at every level), so a server change that would
//! break the app fails here first. Needs `WORDWARZ_TEST_DATABASE_URL`.

mod common;

use std::path::PathBuf;

use axum::http::StatusCode;
use common::{Db, anon};
use serde_json::{Value, json};
use wordwarz_core::config::{GameConfig, Preset};
use wordwarz_core::game::{Action, GameView};
use wordwarz_core::lexicon::Language;
use wordwarz_core::{movegen, tilesets};
use wordwarz_server::auth::Role;
use wordwarz_server::lexicons::dictionary;

fn golden_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden")
}

/// Tags of the core's tagged unions: objects of different variants (a play
/// and a swap, `rackEmptied` and `scoreless`) legitimately differ in keys.
const TAGS: [&str; 5] = ["kind", "state", "reason", "error", "type"];

/// Keys the core leaves out when false (`PlacedTile.blank`).
const OPTIONAL: [&str; 1] = ["blank"];

/// Same keys at every level; for arrays, the first element's shape. Games
/// are random, so two variants of one union are not compared.
fn same_shape(path: &str, a: &Value, b: &Value) -> Result<(), String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) if TAGS.iter().any(|t| x.contains_key(*t) && x.get(*t) != y.get(*t)) => {
            Ok(())
        }
        (Value::Object(x), Value::Object(y)) => {
            let kx: Vec<_> = x.keys().filter(|k| !OPTIONAL.contains(&k.as_str())).collect();
            let ky: Vec<_> = y.keys().filter(|k| !OPTIONAL.contains(&k.as_str())).collect();
            if kx != ky {
                return Err(format!("{path}: keys {kx:?} != {ky:?}"));
            }
            for (k, v) in x {
                if let Some(w) = y.get(k) {
                    same_shape(&format!("{path}.{k}"), v, w)?;
                }
            }
            Ok(())
        }
        (Value::Array(x), Value::Array(y)) => match (x.first(), y.first()) {
            (Some(p), Some(q)) => same_shape(&format!("{path}[0]"), p, q),
            _ => Ok(()),
        },
        // Null against a value is fine: optional fields come and go.
        (Value::Null, _) | (_, Value::Null) => Ok(()),
        (x, y) if std::mem::discriminant(x) == std::mem::discriminant(y) => Ok(()),
        (x, y) => Err(format!("{path}: {x} vs {y}")),
    }
}

/// Tokens, invites and link codes from the throwaway database become
/// `<prefix>example000...`, so nothing in the repo looks like a credential.
fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = ["ww_", "wwi_", "wwl_"].iter().filter_map(|p| rest.find(p).map(|i| (i, p.len()))).min() {
        let (at, len) = i;
        out.push_str(&rest[..at + len]);
        // As long as a real secret (43 characters), so the app's parsers accept it.
        out.push_str("example0000000000000000000000000000000000000");
        rest = rest[at + len..].trim_start_matches(|c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    }
    out.push_str(rest);
    out
}

fn golden(name: &str, v: &Value) {
    let file = golden_dir().join(format!("{name}.json"));
    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::create_dir_all(golden_dir()).unwrap();
        std::fs::write(&file, redact(&serde_json::to_string_pretty(v).unwrap()) + "\n").unwrap();
        return;
    }
    let stored: Value = serde_json::from_str(
        &std::fs::read_to_string(&file)
            .unwrap_or_else(|_| panic!("{} missing: run UPDATE_GOLDEN=1 cargo test --test golden_pg", file.display())),
    )
    .unwrap();
    if let Err(e) = same_shape(name, &stored, v) {
        panic!("{name} changed shape ({e}); if on purpose, regenerate and update the app's models");
    }
}

#[tokio::test]
async fn the_api_wire_contract() {
    let Some(db) = Db::new().await else { return };
    let mum = db.person("mum", Role::Admin).await;
    let m = db.device(mum, "Pixel").await;
    let app = db.app();

    let inv: Value = m.json("POST", "/api/v1/invites", &json!({ "note": "Mila" })).await;
    golden("invite", &inv);
    let token = inv["url"].as_str().unwrap().rsplit('/').next().unwrap().to_string();
    let (_, joined) = anon(
        &app,
        "/api/v1/join",
        &json!({ "invite": token, "username": "mila", "displayName": "Mila", "device": "Tablet" }),
    )
    .await;
    golden("signed_in", &joined);
    let kid = wordwarz_server::users::id_by_username(&db.pool, "mila").await.unwrap();
    let k = common::Device { app: db.app(), token: joined["token"].as_str().unwrap().into(), user: kid };
    let link: Value = k.json("POST", "/api/v1/link", &json!({})).await;
    golden("link", &link);
    golden("me", &k.get::<Value>("/api/v1/me").await);
    golden("people", &k.get::<Value>("/api/v1/people").await);

    let mut config = GameConfig::preset(Preset::Family, Language::Dutch, 2);
    config.bounties = 2;
    let g: Value = m
        .json(
            "POST",
            "/api/v1/games",
            &json!({ "config": config, "seats": [{ "userId": mum }, { "userId": kid, "rackSize": 8 }], "turnTimerHours": 72 }),
        )
        .await;
    golden("game", &g);
    let id = g["id"].as_str().unwrap().to_string();

    // A refusal, a hint and a play by whoever is to move.
    let turn = g["view"]["turn"].as_u64().unwrap() as usize;
    let devices = [&m, &k];
    let me = |seat: usize| {
        let user = g["players"][seat]["userId"].as_str().unwrap().to_string();
        *devices.iter().find(|d| d.user.to_string() == user).unwrap()
    };
    let (s, refused) = me(1 - turn).call("POST", &format!("/api/v1/games/{id}/actions"), Some(&Action::Pass)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    golden("refused", &serde_json::from_str(&refused).unwrap());
    let hint: Value = me(turn).json("POST", &format!("/api/v1/games/{id}/actions"), &Action::Hint).await;
    golden("hint", &hint);

    let mut played = Value::Null;
    for _ in 0..300 {
        let any: Value = m.get(&format!("/api/v1/games/{id}")).await;
        let v: GameView = serde_json::from_value(any["view"].clone()).unwrap();
        if v.status != wordwarz_core::game::Status::Active {
            break;
        }
        let dev = me(v.turn);
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
            1,
        );
        let action = match best.into_iter().next() {
            Some(c) => Action::Play { tiles: c.tiles },
            None if mv.bag_count >= 7 => Action::Swap { tiles: rack },
            None => Action::Pass,
        };
        let r: Value = dev.json("POST", &format!("/api/v1/games/{id}/actions"), &action).await;
        if played.is_null() && r["turn"]["kind"] == "play" {
            played = r;
        }
    }
    golden("played", &played);
    golden("game_finished", &m.get::<Value>(&format!("/api/v1/games/{id}")).await);
    golden("games", &m.get::<Value>("/api/v1/games").await);
    golden("scoreboard", &m.get::<Value>("/api/v1/scoreboard").await);
    golden("changes", &m.get::<Value>("/api/v1/changes?since=999999").await);
    db.drop().await;
}
