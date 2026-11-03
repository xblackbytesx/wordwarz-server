//! Shared harness: a fresh database per test (dropped at the end), the real
//! router, a tiny browser that keeps the session cookie, a device that talks
//! `/api/v1`, and a stand-in UnifiedPush server that records what it gets.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{Request, StatusCode, header};
use axum::routing::post;
use http_body_util::BodyExt as _;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt as _;
use uuid::Uuid;
use wordwarz_server::auth::Role;
use wordwarz_server::{AppState, MIGRATOR, router};

pub const PASSPHRASE: &str = "correct horse battery";

/// One push the stand-in received.
#[derive(Clone, Debug)]
pub struct Got {
    pub id: String,
    pub body: Vec<u8>,
    /// `Content-Encoding` (`aes128gcm` when encrypted).
    pub encoding: Option<String>,
}

impl Got {
    /// The body as JSON (plaintext pushes only).
    pub fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

type Inbox = Arc<Mutex<Vec<Got>>>;

pub struct Db {
    pub pool: PgPool,
    admin: PgPool,
    name: String,
    /// Shared by every state of this test, like the one server process.
    pub changes: Arc<wordwarz_server::changes::Changes>,
    pub push: FakePush,
}

impl Db {
    /// None (and a SKIP line) when `WORDWARZ_TEST_DATABASE_URL` is unset.
    pub async fn new() -> Option<Db> {
        let Ok(base) = std::env::var("WORDWARZ_TEST_DATABASE_URL") else {
            eprintln!("SKIP: WORDWARZ_TEST_DATABASE_URL unset");
            return None;
        };
        let admin = PgPoolOptions::new().max_connections(1).connect(&base).await.unwrap();
        let name = format!("wordwarz_test_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE DATABASE {name}")).execute(&admin).await.unwrap();
        let url = format!("{}/{name}", base.rsplit_once('/').unwrap().0);
        let pool = PgPoolOptions::new().max_connections(8).connect(&url).await.unwrap();
        MIGRATOR.run(&pool).await.unwrap();
        Some(Db { pool, admin, name, changes: Default::default(), push: FakePush::start().await })
    }

    pub fn state(&self) -> AppState {
        AppState {
            cookie_secure: false,
            push_hosts: vec![self.push.host.clone()],
            push_allow_http: true,
            changes: self.changes.clone(),
            ..AppState::new(self.pool.clone())
        }
    }

    pub fn app(&self) -> Router {
        router(self.state())
    }

    pub async fn drop(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP DATABASE {} WITH (FORCE)", self.name)).execute(&self.admin).await.unwrap();
    }

    /// A person with a passphrase; returns their id.
    pub async fn person(&self, username: &str, role: Role) -> Uuid {
        let id = wordwarz_server::users::create(&self.pool, username, role).await.unwrap();
        wordwarz_server::users::set_password(&self.pool, username, PASSPHRASE).await.unwrap();
        id
    }

    /// A device of `user` with its own token.
    pub async fn device(&self, user: Uuid, name: &str) -> Device {
        let token = wordwarz_server::users::new_token(&self.pool, user, name).await.unwrap();
        Device { app: self.app(), token, user }
    }
}

pub struct Resp {
    pub status: StatusCode,
    pub headers: axum::http::HeaderMap,
    pub body: String,
}

/// A browser: keeps the session cookie, sends same-origin Origin on writes.
pub struct Browser {
    pub app: Router,
    pub cookie: Option<String>,
}

impl Browser {
    pub fn new(app: Router) -> Self {
        Browser { app, cookie: None }
    }

    pub async fn logged_in(app: Router, username: &str) -> Self {
        let mut b = Browser::new(app);
        let r = b.post("/login", &[("username", username), ("passphrase", PASSPHRASE)]).await;
        assert_eq!(r.status, StatusCode::SEE_OTHER, "{}", r.body);
        b
    }

    pub async fn get(&mut self, path: &str) -> Resp {
        self.send(Request::get(path), Body::empty(), None).await
    }

    pub async fn post(&mut self, path: &str, form: &[(&str, &str)]) -> Resp {
        self.send(
            Request::post(path).header(header::CONTENT_TYPE, "application/x-www-form-urlencoded"),
            Body::from(form_body(form)),
            Some("http://test.local"),
        )
        .await
    }

    pub async fn htmx_post(&mut self, path: &str, form: &[(&str, &str)]) -> Resp {
        self.send(
            Request::post(path)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .header("hx-request", "true"),
            Body::from(form_body(form)),
            Some("http://test.local"),
        )
        .await
    }

    pub async fn send(&mut self, b: axum::http::request::Builder, body: Body, origin: Option<&str>) -> Resp {
        let mut b = b.header(header::HOST, "test.local");
        if let Some(o) = origin {
            b = b.header(header::ORIGIN, o);
        }
        if let Some(c) = &self.cookie {
            b = b.header(header::COOKIE, c);
        }
        let resp = self.app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        if let Some(set) = headers.get(header::SET_COOKIE).and_then(|v| v.to_str().ok()) {
            let kv = set.split(';').next().unwrap().to_string();
            self.cookie = if kv.ends_with('=') { None } else { Some(kv) };
        }
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        Resp { status, headers, body: String::from_utf8_lossy(&bytes).into_owned() }
    }
}

/// A device with its token.
pub struct Device {
    pub app: Router,
    pub token: String,
    pub user: Uuid,
}

impl Device {
    pub async fn call<T: Serialize>(&self, method: &str, path: &str, body: Option<&T>) -> (StatusCode, String) {
        let b = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, "test.local")
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
            .header(header::CONTENT_TYPE, "application/json");
        let body = body.map(|b| Body::from(serde_json::to_vec(b).unwrap())).unwrap_or_else(Body::empty);
        let resp = self.app.clone().oneshot(b.body(body).unwrap()).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&bytes).into_owned())
    }

    /// GET, expecting success.
    pub async fn get<R: DeserializeOwned>(&self, path: &str) -> R {
        let (status, text) = self.call::<()>("GET", path, None).await;
        assert!(status.is_success(), "{path}: {status}: {text}");
        serde_json::from_str(&text).unwrap()
    }

    /// POST (or another method) with a JSON body, expecting success.
    pub async fn json<T: Serialize, R: DeserializeOwned>(&self, method: &str, path: &str, body: &T) -> R {
        let (status, text) = self.call(method, path, Some(body)).await;
        assert!(status.is_success(), "{path}: {status}: {text}");
        serde_json::from_str(&text).unwrap()
    }
}

/// Unauthenticated JSON call (join, login, link claim).
pub async fn anon<T: Serialize>(app: &Router, path: &str, body: &T) -> (StatusCode, Value) {
    let b = Request::post(path).header(header::HOST, "test.local").header(header::CONTENT_TYPE, "application/json");
    let resp = app.clone().oneshot(b.body(Body::from(serde_json::to_vec(body).unwrap())).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

pub fn form_body(form: &[(&str, &str)]) -> String {
    form.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&")
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// A UnifiedPush server on localhost: every POST to `/up/{anything}` is
/// recorded; `/gone/...` answers 410 like an unsubscribed endpoint.
#[derive(Clone)]
pub struct FakePush {
    pub host: String,
    pub received: Inbox,
}

impl FakePush {
    async fn start() -> FakePush {
        let received: Inbox = Default::default();
        let app = Router::new()
            .route(
                "/up/{id}",
                post(
                    |State(r): State<Inbox>,
                     axum::extract::Path(id): axum::extract::Path<String>,
                     headers: axum::http::HeaderMap,
                     body: Bytes| async move {
                        let encoding =
                            headers.get("content-encoding").and_then(|v| v.to_str().ok()).map(str::to_string);
                        r.lock().unwrap().push(Got { id, body: body.to_vec(), encoding });
                        StatusCode::CREATED
                    },
                ),
            )
            .route("/gone/{id}", post(|| async { StatusCode::GONE }))
            .with_state(received.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let host = listener.local_addr().unwrap().to_string();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        FakePush { host, received }
    }

    pub fn url(&self, id: &str) -> String {
        format!("http://{}/up/{id}", self.host)
    }

    /// Waits up to 2 s for at least `n` messages (delivery is in the background).
    pub async fn wait_for(&self, n: usize) -> Vec<Got> {
        for _ in 0..100 {
            if self.received.lock().unwrap().len() >= n {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        self.received.lock().unwrap().clone()
    }
}
