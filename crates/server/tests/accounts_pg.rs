//! Accounts: joining with an invite (no passphrase), signing in, device
//! links, roles, disabling and deletion. Needs `WORDWARZ_TEST_DATABASE_URL`;
//! skipped when unset.

mod common;

use axum::http::StatusCode;
use common::{Db, PASSPHRASE, anon};
use serde_json::{Value, json};
use wordwarz_server::auth::Role;

fn invite_token(url: &str) -> String {
    url.rsplit('/').next().unwrap().to_string()
}

#[tokio::test]
async fn joining_with_an_invite_needs_only_a_username() {
    let Some(db) = Db::new().await else { return };
    let mum = db.person("mum", Role::Admin).await;
    let dev = db.device(mum, "Pixel").await;
    let inv: Value = dev.json("POST", "/api/v1/invites", &json!({ "note": "for Mila" })).await;
    let url = inv["url"].as_str().unwrap();
    assert!(url.starts_with("http://test.local/invite/wwi_"), "{url}");
    let token = invite_token(url);
    let app = db.app();

    let (s, _) = anon(&app, "/api/v1/join", &json!({ "invite": token, "username": "x", "device": "Tablet" })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "bad username");
    let (s, _) = anon(&app, "/api/v1/join", &json!({ "invite": token, "username": "Mum", "device": "Tablet" })).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "taken (case-insensitive); the invite stays usable");

    let (s, joined) =
        anon(&app, "/api/v1/join", &json!({ "invite": token, "username": "Mila", "device": "Tablet" })).await;
    assert_eq!(s, StatusCode::OK, "{joined}");
    assert_eq!(joined["me"]["username"], "mila");
    assert_eq!(joined["me"]["hasPassphrase"], false);
    assert_eq!(joined["me"]["admin"], false);
    let mila = common::Device {
        app: db.app(),
        token: joined["token"].as_str().unwrap().into(),
        user: wordwarz_server::users::id_by_username(&db.pool, "mila").await.unwrap(),
    };
    let me: Value = mila.get("/api/v1/me").await;
    assert_eq!(me["username"], "mila");

    let (s, _) = anon(&app, "/api/v1/join", &json!({ "invite": token, "username": "mila2", "device": "Tablet" })).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "single use");

    // A player may invite players, not admins.
    let (s, _) = mila.call("POST", "/api/v1/invites", Some(&json!({ "admin": true }))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = mila.call("POST", "/api/v1/invites", Some(&json!({}))).await;
    assert_eq!(s, StatusCode::OK);

    // Without a passphrase there is nothing to log in with.
    let (s, _) = anon(&app, "/api/v1/login", &json!({ "username": "mila", "passphrase": "", "device": "x" })).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    db.drop().await;
}

#[tokio::test]
async fn login_logout_and_device_links() {
    let Some(db) = Db::new().await else { return };
    let dad = db.person("dad", Role::User).await;
    let app = db.app();
    let (s, _) =
        anon(&app, "/api/v1/login", &json!({ "username": "dad", "passphrase": "wrong one!!", "device": "x" })).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) =
        anon(&app, "/api/v1/login", &json!({ "username": "nobody", "passphrase": PASSPHRASE, "device": "x" })).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "same answer for unknown names");
    let (s, v) =
        anon(&app, "/api/v1/login", &json!({ "username": "DAD", "passphrase": PASSPHRASE, "device": "Phone" })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let phone = common::Device { app: db.app(), token: v["token"].as_str().unwrap().into(), user: dad };

    // The phone shows a QR code; the tablet scans it.
    let link: Value = phone.json("POST", "/api/v1/link", &json!({})).await;
    let url = link["url"].as_str().unwrap();
    assert!(url.starts_with("http://test.local/link/wwl_"), "{url}");
    let code = url.rsplit('/').next().unwrap();
    let (s, v) = anon(&app, "/api/v1/link/claim", &json!({ "code": code, "device": "Tablet" })).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["me"]["username"], "dad");
    let (s, _) = anon(&app, "/api/v1/link/claim", &json!({ "code": code, "device": "Other" })).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "single use");
    let tablet = common::Device { app: db.app(), token: v["token"].as_str().unwrap().into(), user: dad };
    let _: Value = tablet.get("/api/v1/me").await;

    let (s, _) = phone.call::<()>("POST", "/api/v1/logout", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = phone.call::<()>("GET", "/api/v1/me", None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "signed out");
    let _: Value = tablet.get("/api/v1/me").await;

    // A disabled account's devices stop working.
    sqlx::query("UPDATE users SET disabled_at = now() WHERE id = $1").bind(dad).execute(&db.pool).await.unwrap();
    let (s, _) = tablet.call::<()>("GET", "/api/v1/me", None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    db.drop().await;
}

#[tokio::test]
async fn deleting_an_account_resigns_and_keeps_history() {
    let Some(db) = Db::new().await else { return };
    let mum = db.person("mum", Role::Admin).await;
    let kid = db.person("kid", Role::User).await;
    let mum_dev = db.device(mum, "Pixel").await;
    let kid_dev = db.device(kid, "Tablet").await;
    let game: Value = mum_dev
        .json(
            "POST",
            "/api/v1/games",
            &json!({
                "config": wordwarz_core::config::GameConfig::classic(wordwarz_core::lexicon::Language::Dutch, 2),
                "seats": [{ "userId": mum }, { "userId": kid }],
            }),
        )
        .await;
    let id = game["id"].as_str().unwrap().to_string();

    // The only admin cannot leave while others remain.
    let (s, _) = mum_dev.call::<()>("DELETE", "/api/v1/me", None).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);

    let export: Value = kid_dev.get("/api/v1/me/export").await;
    assert_eq!(export["account"]["username"], "kid");
    assert_eq!(export["games"].as_array().unwrap().len(), 1);

    let (s, _) = kid_dev.call::<()>("DELETE", "/api/v1/me", None).await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let g: Value = mum_dev.get(&format!("/api/v1/games/{id}")).await;
    assert_eq!(g["view"]["status"]["state"], "finished");
    assert_eq!(
        g["view"]["status"]["winners"],
        json!([g["players"].as_array().unwrap().iter().position(|p| p["username"] == "mum").unwrap()])
    );
    assert!(g["players"].as_array().unwrap().iter().any(|p| p["username"] == "deleted" && p["userId"].is_null()));
    let board: Value = mum_dev.get("/api/v1/scoreboard").await;
    assert_eq!(board["players"].as_array().unwrap().len(), 1);
    assert_eq!(board["recent"].as_array().unwrap().len(), 1, "the finished game stays");
    db.drop().await;
}

#[tokio::test]
async fn a_qr_account_can_set_a_passphrase_later() {
    let Some(db) = Db::new().await else { return };
    let mum = db.person("mum", Role::Admin).await;
    let dev = db.device(mum, "Pixel").await;
    let inv: Value = dev.json("POST", "/api/v1/invites", &json!({})).await;
    let app = db.app();
    let (_, joined) = anon(
        &app,
        "/api/v1/join",
        &json!({ "invite": invite_token(inv["url"].as_str().unwrap()), "username": "kid", "device": "Tablet" }),
    )
    .await;
    let kid = common::Device {
        app: db.app(),
        token: joined["token"].as_str().unwrap().into(),
        user: wordwarz_server::users::id_by_username(&db.pool, "kid").await.unwrap(),
    };

    let (s, _) = kid.call("PUT", "/api/v1/me/passphrase", Some(&json!({ "passphrase": "short" }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = kid.call("PUT", "/api/v1/me/passphrase", Some(&json!({ "passphrase": "a long passphrase" }))).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "the first one needs no current passphrase");
    let me: Value = kid.get("/api/v1/me").await;
    assert_eq!(me["hasPassphrase"], true);
    let (s, _) = anon(
        &app,
        "/api/v1/login",
        &json!({ "username": "kid", "passphrase": "a long passphrase", "device": "Laptop" }),
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (s, _) = kid.call("PUT", "/api/v1/me/passphrase", Some(&json!({ "passphrase": "another long one" }))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "changing needs the current one");
    let (s, _) = kid
        .call(
            "PUT",
            "/api/v1/me/passphrase",
            Some(&json!({ "current": "a long passphrase", "passphrase": "another long one" })),
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    db.drop().await;
}
