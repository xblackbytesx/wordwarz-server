//! The website: login, the guard and security headers, every page (no
//! inline script or style), invites with QR codes, replay and export.
//! Needs `WORDWARZ_TEST_DATABASE_URL`; skipped when unset.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{Browser, Db, PASSPHRASE};
use serde_json::{Value, json};
use wordwarz_core::config::GameConfig;
use wordwarz_core::game::Action;
use wordwarz_core::lexicon::Language;
use wordwarz_server::auth::Role;

/// No inline handlers, inline scripts or style attributes (the CSP would block them).
fn assert_csp_clean(path: &str, body: &str) {
    assert!(!body.contains(" style="), "{path}: inline style");
    assert!(!body.contains(" onclick=") && !body.contains(" onsubmit="), "{path}: inline handler");
    for chunk in body.split("<script").skip(1) {
        assert!(chunk.trim_start().starts_with("src="), "{path}: inline script");
    }
}

#[tokio::test]
async fn login_headers_and_every_page() {
    let Some(db) = Db::new().await else { return };
    let mum = db.person("mum", Role::Admin).await;
    let kid = db.person("kid", Role::User).await;
    let m = db.device(mum, "Pixel").await;
    let k = db.device(kid, "Tablet").await;
    let g: Value = m
        .json(
            "POST",
            "/api/v1/games",
            &json!({ "config": GameConfig::classic(Language::Dutch, 2), "seats": [{ "userId": mum }, { "userId": kid }] }),
        )
        .await;
    let id = g["id"].as_str().unwrap().to_string();
    // One swap so the replay has a move to step through.
    let turn = g["view"]["turn"].as_u64().unwrap() as usize;
    let player = if g["players"][turn]["username"] == "mum" { &m } else { &k };
    let mine: Value = player.get(&format!("/api/v1/games/{id}")).await;
    let rack: Vec<wordwarz_core::letters::Tile> =
        serde_json::from_value(mine["view"]["seats"][turn]["rack"].clone()).unwrap();
    let _: Value =
        player.json("POST", &format!("/api/v1/games/{id}/actions"), &Action::Swap { tiles: rack[..2].to_vec() }).await;

    let mut b = Browser::logged_in(db.app(), "mum").await;
    for path in [
        "/games".to_string(),
        format!("/games/{id}"),
        format!("/games/{id}?move=0"),
        "/scoreboard".into(),
        "/people".into(),
        "/devices".into(),
        "/settings".into(),
    ] {
        let r = b.get(&path).await;
        assert_eq!(r.status, StatusCode::OK, "{path}: {}", r.body);
        assert_csp_clean(&path, &r.body);
        let csp = r.headers.get("content-security-policy").unwrap().to_str().unwrap();
        assert!(csp.contains("script-src 'self'") && !csp.contains("unsafe"), "{csp}");
        assert_eq!(r.headers.get("referrer-policy").unwrap(), "same-origin");
    }
    let r = b.get(&format!("/games/{id}")).await;
    assert!(r.body.contains("board s15") && r.body.contains("swapped 2"), "{}", r.body);

    // Someone else's running game is not visible.
    db.person("other", Role::User).await;
    let mut o = Browser::logged_in(db.app(), "other").await;
    assert_eq!(o.get(&format!("/games/{id}")).await.status, StatusCode::NOT_FOUND);

    let js = b.get("/static/htmx.min.js").await;
    let etag = js.headers.get("etag").unwrap().to_str().unwrap().to_string();
    let r304 = b.send(Request::get("/static/htmx.min.js").header("if-none-match", &etag), Body::empty(), None).await;
    assert_eq!(r304.status, StatusCode::NOT_MODIFIED);

    let export = b.get("/settings/export").await;
    assert_eq!(export.status, StatusCode::OK);
    assert_eq!(serde_json::from_str::<Value>(&export.body).unwrap()["account"]["username"], "mum");

    let r = b.post("/logout", &[]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    assert_eq!(b.get("/games").await.headers.get("location").unwrap(), "/login");
    let r = b.post("/login", &[("username", "mum"), ("passphrase", "wrong passphrase")]).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = b.post("/login", &[("username", "nobody"), ("passphrase", PASSPHRASE)]).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    db.drop().await;
}

#[tokio::test]
async fn cross_site_writes_are_refused() {
    let Some(db) = Db::new().await else { return };
    db.person("mum", Role::Admin).await;
    let mut b = Browser::logged_in(db.app(), "mum").await;
    for path in ["/people/invites", "/devices/link", "/settings/delete"] {
        let r = b
            .send(
                Request::post(path).header("content-type", "application/x-www-form-urlencoded"),
                Body::from("note=x&confirm=mum"),
                Some("https://evil.example"),
            )
            .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{path}");
    }
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM invites").fetch_one(&db.pool).await.unwrap();
    assert_eq!(n, 0);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users").fetch_one(&db.pool).await.unwrap();
    assert_eq!(n, 1);
    db.drop().await;
}

#[tokio::test]
async fn web_invites_show_a_qr_code_and_make_accounts() {
    let Some(db) = Db::new().await else { return };
    db.person("dad", Role::User).await;
    let mut dad = Browser::logged_in(db.app(), "dad").await;
    let r = dad.htmx_post("/people/invites", &[("note", "Oma"), ("role", "admin")]).await;
    assert_eq!(r.status, StatusCode::FORBIDDEN, "players cannot invite admins");
    let r = dad.htmx_post("/people/invites", &[("note", "Oma")]).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(r.body.contains("<svg"), "a QR code");
    assert_csp_clean("invite_created", &r.body);
    let link = r.body.split("<code id=\"invite-link\">").nth(1).unwrap().split('<').next().unwrap().to_string();
    let path = link.trim_start_matches("http://test.local");

    let mut oma = Browser::new(db.app());
    let page = oma.get(path).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("<svg"));
    let form = [
        ("username", "Oma_Riet"),
        ("name", "Oma"),
        ("passphrase", "a long passphrase"),
        ("passphrase2", "a long passphrase"),
    ];
    let r = oma.post(path, &form).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
    assert_eq!(oma.get("/settings").await.status, StatusCode::OK);
    let r = Browser::new(db.app()).post(path, &form).await;
    assert!(r.body.contains("used up"), "single use");

    // Device links render a QR code too.
    let r = oma.htmx_post("/devices/link", &[]).await;
    assert!(r.status.is_success() && r.body.contains("<svg"), "{}", r.body);

    // Deleting needs the username typed.
    let r = oma.htmx_post("/settings/delete", &[("confirm", "someone")]).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = oma.post("/settings/delete", &[("confirm", "oma_riet")]).await;
    assert_eq!(r.status, StatusCode::SEE_OTHER);
    let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users").fetch_one(&db.pool).await.unwrap();
    assert_eq!(n, 1);
    db.drop().await;
}
