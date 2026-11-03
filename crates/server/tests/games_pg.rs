//! Games over the API: whole games played by the move generator, seats and
//! handicaps, rule errors, nudges and pushes, turn timers, long-poll and the
//! scoreboard. Needs `WORDWARZ_TEST_DATABASE_URL`; skipped when unset.

mod common;

use axum::http::StatusCode;
use common::{Db, Device};
use serde_json::{Value, json};
use uuid::Uuid;
use wordwarz_core::config::{GameConfig, Preset};
use wordwarz_core::game::{Action, Game, GameView};
use wordwarz_core::lexicon::Language;
use wordwarz_core::{movegen, tilesets};
use wordwarz_server::auth::Role;
use wordwarz_server::games;
use wordwarz_server::lexicons::dictionary;

async fn new_game(by: &Device, config: GameConfig, seats: Value) -> Value {
    by.json("POST", "/api/v1/games", &json!({ "config": config, "seats": seats })).await
}

fn view(g: &Value) -> GameView {
    serde_json::from_value(g["view"].clone()).unwrap()
}

/// The device whose owner sits at `seat`.
fn at_seat<'a>(g: &Value, seat: usize, devices: &'a [&'a Device]) -> &'a Device {
    let user: Uuid = serde_json::from_value(g["players"][seat]["userId"].clone()).unwrap();
    devices.iter().find(|d| d.user == user).unwrap()
}

/// One move by whoever is to play: the best move, else swap or pass.
async fn bot_move(id: &str, devices: &[&Device]) -> Value {
    let g: Value = devices[0].get(&format!("/api/v1/games/{id}")).await;
    let turn = view(&g).turn;
    let dev = at_seat(&g, turn, devices);
    let mine: Value = dev.get(&format!("/api/v1/games/{id}")).await;
    let v = view(&mine);
    let rack = v.seats[turn].rack.clone().expect("own rack is visible");
    let lang = v.config.language;
    let best = movegen::best_moves(
        &v.board,
        &v.layout,
        tilesets::for_language(lang),
        &rack,
        dictionary(lang),
        v.config.bingo_bonus,
        1,
    );
    let action = match best.into_iter().next() {
        Some(c) => Action::Play { tiles: c.tiles },
        None if v.bag_count >= 7 => Action::Swap { tiles: rack },
        None => Action::Pass,
    };
    dev.json("POST", &format!("/api/v1/games/{id}/actions"), &action).await
}

#[tokio::test]
async fn a_whole_game_over_the_api_replays_and_lands_on_the_scoreboard() {
    let Some(db) = Db::new().await else { return };
    let mum = db.person("mum", Role::Admin).await;
    let kid = db.person("kid", Role::User).await;
    let (m, k) = (db.device(mum, "Pixel").await, db.device(kid, "Tablet").await);
    let g =
        new_game(&m, GameConfig::classic(Language::English, 2), json!([{ "userId": mum }, { "userId": kid }])).await;
    let id = g["id"].as_str().unwrap().to_string();

    // Each player sees only their own rack; nobody sees the seed or the bag.
    let v = view(&g);
    let my_seat = g["players"].as_array().unwrap().iter().position(|p| p["username"] == "mum").unwrap();
    assert!(v.seats[my_seat].rack.is_some() && v.seats[1 - my_seat].rack.is_none());
    let text = g.to_string();
    assert!(!text.contains("\"seed\"") && !text.contains("\"bag\""), "{text}");

    // Out of turn: a typed rule error.
    let both = [&m, &k];
    let wrong = at_seat(&g, 1 - v.turn, &both);
    let (s, body) = wrong.call("POST", &format!("/api/v1/games/{id}/actions"), Some(&Action::Pass)).await;
    assert_eq!(s, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["rule"]["error"], "notYourTurn");

    // Outsiders see nothing.
    let stranger = db.person("stranger", Role::User).await;
    let s_dev = db.device(stranger, "x").await;
    let (s, _) = s_dev.call::<()>("GET", &format!("/api/v1/games/{id}"), None).await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    let mut last = g;
    for _ in 0..300 {
        last = bot_move(&id, &[&m, &k]).await;
        if last["game"]["view"]["status"]["state"] == "finished" {
            break;
        }
    }
    assert_eq!(last["game"]["view"]["status"]["state"], "finished", "game did not end");

    // The log replays to the stored state.
    let (stored, log) = games::load(&db.pool, id.parse().unwrap()).await.unwrap();
    assert_eq!(Game::replay(stored.config.clone(), stored.seed, &log).unwrap(), stored);

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM game_results WHERE game_id = $1")
        .bind(Uuid::parse_str(&id).unwrap())
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(rows, 2);
    let board: Value = k.get("/api/v1/scoreboard").await;
    let played: i64 = board["players"].as_array().unwrap().iter().map(|p| p["games"].as_i64().unwrap()).sum();
    assert_eq!(played, 2);
    let wins: i64 = board["players"].as_array().unwrap().iter().map(|p| p["wins"].as_i64().unwrap()).sum();
    assert!(wins >= 1);
    assert_eq!(board["headToHead"].as_array().unwrap().len(), 1);
    assert_eq!(board["recent"][0]["gameId"], id.as_str());

    let list: Value = m.get("/api/v1/games").await;
    assert_eq!(list[0]["finished"], true);
    db.drop().await;
}

#[tokio::test]
async fn seats_shuffle_but_teams_and_handicaps_stay_with_their_people() {
    let Some(db) = Db::new().await else { return };
    let ids = [
        db.person("mum", Role::Admin).await,
        db.person("dad", Role::User).await,
        db.person("anna", Role::User).await,
        db.person("ben", Role::User).await,
    ];
    let mum = db.device(ids[0], "p").await;
    let seats = json!([
        { "userId": ids[0], "team": 0 },
        { "userId": ids[1], "team": 1 },
        { "userId": ids[2], "team": 0, "rackSize": 8 },
        { "userId": ids[3], "team": 1, "scorePercent": 150 },
    ]);
    for _ in 0..5 {
        let g = new_game(&mum, GameConfig::preset(Preset::Family, Language::Bilingual, 4), seats.clone()).await;
        let names: Vec<String> =
            g["players"].as_array().unwrap().iter().map(|p| p["username"].as_str().unwrap().to_string()).collect();
        let team_of = |n: &str| if n == "mum" || n == "anna" { 0 } else { 1 };
        assert_eq!(team_of(&names[0]), team_of(&names[2]), "{names:?}");
        assert_eq!(team_of(&names[1]), team_of(&names[3]), "{names:?}");
        assert_ne!(team_of(&names[0]), team_of(&names[1]), "{names:?}");
        let v = view(&g);
        let anna = names.iter().position(|n| n == "anna").unwrap();
        let ben = names.iter().position(|n| n == "ben").unwrap();
        assert_eq!(v.config.players[anna].rack_size, 8);
        assert_eq!(v.seats[anna].rack_count, 8);
        assert_eq!(v.config.players[ben].score_percent, 150);
    }

    let (s, _) = mum
        .call(
            "POST",
            "/api/v1/games",
            Some(&json!({ "config": GameConfig::classic(Language::Dutch, 2), "seats": [{ "userId": ids[1] }, { "userId": ids[2] }] })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "the creator must play");
    let (s, _) = mum
        .call(
            "POST",
            "/api/v1/games",
            Some(&json!({ "config": GameConfig::classic(Language::Dutch, 2), "seats": [{ "userId": ids[0] }, { "userId": ids[1] }], "turnTimerHours": 5 })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "timer choices only");
    db.drop().await;
}

#[tokio::test]
async fn nudges_push_only_an_event_and_are_rate_limited() {
    let Some(db) = Db::new().await else { return };
    let mum = db.person("mum", Role::Admin).await;
    let kid = db.person("kid", Role::User).await;
    let (m, k) = (db.device(mum, "Pixel").await, db.device(kid, "Tablet").await);
    let (s, _) = m.call("PUT", "/api/v1/push", Some(&json!({ "url": "https://evil.example/up/x" }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "only declared push hosts");
    for (d, tag) in [(&m, "mum"), (&k, "kid")] {
        let (s, body) = d.call("PUT", "/api/v1/push", Some(&json!({ "url": db.push.url(tag) }))).await;
        assert_eq!(s, StatusCode::NO_CONTENT, "{body}");
    }

    let g = new_game(&m, GameConfig::classic(Language::Dutch, 2), json!([{ "userId": mum }, { "userId": kid }])).await;
    let id = g["id"].as_str().unwrap();
    let got = db.push.wait_for(1).await;
    assert_eq!(got[0].id, "kid");
    assert_eq!(got[0].json(), json!({ "event": "newGame", "game": id }), "nothing else travels");

    let both = [&m, &k];
    let to_play = at_seat(&g, view(&g).turn, &both);
    let other = if to_play.user == mum { &k } else { &m };
    let (s, _) = to_play.call::<()>("POST", &format!("/api/v1/games/{id}/nudge"), None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "not on your own turn");
    let (s, _) = other.call::<()>("POST", &format!("/api/v1/games/{id}/nudge"), None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = other.call::<()>("POST", &format!("/api/v1/games/{id}/nudge"), None).await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS);
    let got = db.push.wait_for(2).await;
    let target = if to_play.user == mum { "mum" } else { "kid" };
    assert_eq!((got[1].id.as_str(), got[1].json()), (target, json!({ "event": "nudge", "game": id })));

    // A move tells the next player it is their turn.
    bot_move(id, &[&m, &k]).await;
    let got = db.push.wait_for(3).await;
    let next = if target == "mum" { "kid" } else { "mum" };
    assert_eq!((got[2].id.as_str(), got[2].json()), (next, json!({ "event": "turn", "game": id })));
    db.drop().await;
}

#[tokio::test]
async fn turn_timers_time_out_then_resign() {
    let Some(db) = Db::new().await else { return };
    let mum = db.person("mum", Role::Admin).await;
    let kid = db.person("kid", Role::User).await;
    let m = db.device(mum, "Pixel").await;
    let g: Value = m
        .json(
            "POST",
            "/api/v1/games",
            &json!({ "config": GameConfig::classic(Language::Dutch, 2), "seats": [{ "userId": mum }, { "userId": kid }], "turnTimerHours": 24 }),
        )
        .await;
    let id: Uuid = g["id"].as_str().unwrap().parse().unwrap();
    let st = db.state();
    assert_eq!(games::expire_turns(&st).await.unwrap(), 0, "not due yet");
    let age = || sqlx::query("UPDATE games SET turn_started_at = now() - interval '25 hours' WHERE id = $1").bind(id);
    let first = view(&g).turn;

    age().execute(&db.pool).await.unwrap();
    assert_eq!(games::expire_turns(&st).await.unwrap(), 1);
    age().execute(&db.pool).await.unwrap();
    assert_eq!(games::expire_turns(&st).await.unwrap(), 1);
    let (game, _) = games::load(&db.pool, id).await.unwrap();
    assert_eq!(game.history.len(), 2);
    assert!(!game.is_over());
    // The first player times out a second time in a row: resigned.
    age().execute(&db.pool).await.unwrap();
    games::expire_turns(&st).await.unwrap();
    let (game, _) = games::load(&db.pool, id).await.unwrap();
    assert!(game.is_over());
    assert!(game.seats[first].resigned);
    db.drop().await;
}

#[tokio::test]
async fn the_other_players_long_poll_wakes_on_a_move() {
    let Some(db) = Db::new().await else { return };
    let mum = db.person("mum", Role::Admin).await;
    let kid = db.person("kid", Role::User).await;
    let (m, k) = (db.device(mum, "Pixel").await, db.device(kid, "Tablet").await);
    let g =
        new_game(&m, GameConfig::classic(Language::English, 2), json!([{ "userId": mum }, { "userId": kid }])).await;
    let id = g["id"].as_str().unwrap().to_string();
    let now: Value = k.get("/api/v1/changes").await;
    let since = now["generation"].as_u64().unwrap();
    let waiting = {
        let app = k.app.clone();
        let token = k.token.clone();
        tokio::spawn(async move {
            let d = Device { app, token, user: kid };
            d.get::<Value>(&format!("/api/v1/changes?since={since}")).await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    bot_move(&id, &[&m, &k]).await;
    let woke = tokio::time::timeout(std::time::Duration::from_secs(5), waiting).await.unwrap().unwrap();
    assert_eq!(woke["changed"], true);
    db.drop().await;
}

/// RFC 8188 `aes128gcm` decryption with the device's private key: an
/// independent reader for what the server sends (the app's connector does
/// the same with Tink).
fn webpush_decrypt(ua: &p256::SecretKey, auth: &[u8; 16], body: &[u8]) -> Vec<u8> {
    use aes_gcm::aead::{Aead, KeyInit};
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    let salt = &body[..16];
    let idlen = body[20] as usize;
    let as_public = p256::PublicKey::from_sec1_bytes(&body[21..21 + idlen]).unwrap();
    let shared = p256::ecdh::diffie_hellman(ua.to_nonzero_scalar(), as_public.as_affine());
    let mut info = b"WebPush: info\0".to_vec();
    info.extend_from_slice(ua.public_key().to_encoded_point(false).as_bytes());
    info.extend_from_slice(&body[21..21 + idlen]);
    let mut ikm = [0u8; 32];
    hkdf::Hkdf::<sha2::Sha256>::new(Some(auth), shared.raw_secret_bytes()).expand(&info, &mut ikm).unwrap();
    let prk = hkdf::Hkdf::<sha2::Sha256>::new(Some(salt), &ikm);
    let (mut cek, mut nonce) = ([0u8; 16], [0u8; 12]);
    prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek).unwrap();
    prk.expand(b"Content-Encoding: nonce\0", &mut nonce).unwrap();
    let mut plain = aes_gcm::Aes128Gcm::new_from_slice(&cek)
        .unwrap()
        .decrypt(aes_gcm::Nonce::from_slice(&nonce), &body[21 + idlen..])
        .unwrap();
    assert_eq!(plain.pop(), Some(2), "last-record delimiter");
    plain
}

#[tokio::test]
async fn pushes_are_encrypted_to_the_device_keys() {
    use base64::Engine as _;
    use p256::elliptic_curve::sec1::ToEncodedPoint;
    let Some(db) = Db::new().await else { return };
    let mum = db.person("mum", Role::Admin).await;
    let kid = db.person("kid", Role::User).await;
    let (m, k) = (db.device(mum, "Pixel").await, db.device(kid, "Tablet").await);
    let ua = p256::SecretKey::from_slice(&[7u8; 32]).unwrap();
    let auth = [9u8; 16];
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let pub_key = b64.encode(ua.public_key().to_encoded_point(false).as_bytes());
    let (s, _) = k
        .call("PUT", "/api/v1/push", Some(&json!({ "url": db.push.url("kid"), "pubKey": pub_key, "auth": "short" })))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "a bad auth secret fails at registration");
    let (s, body) = k
        .call(
            "PUT",
            "/api/v1/push",
            Some(&json!({ "url": db.push.url("kid"), "pubKey": pub_key, "auth": b64.encode(auth) })),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{body}");

    let g = new_game(&m, GameConfig::classic(Language::Dutch, 2), json!([{ "userId": mum }, { "userId": kid }])).await;
    let id = g["id"].as_str().unwrap();
    let got = db.push.wait_for(1).await;
    assert_eq!(got[0].encoding.as_deref(), Some("aes128gcm"));
    assert!(!String::from_utf8_lossy(&got[0].body).contains(id), "the push server sees no game id");
    let plain = webpush_decrypt(&ua, &auth, &got[0].body);
    assert_eq!(serde_json::from_slice::<Value>(&plain).unwrap(), json!({ "event": "newGame", "game": id }));
    db.drop().await;
}
